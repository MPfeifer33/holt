use chrono::Utc;
use serde_json::json;
use std::sync::Arc;

use crate::config::agent_config::AgentToolsBlock;
use crate::runtime::agent::{
    AgentColor, AgentMetadata, AgentSlot, AgentStatus, ConversationHistory,
};
use crate::runtime::connection::ConnectionConfig;
use crate::runtime::limits::AgentLimits;
use crate::runtime::subagent::{SubagentManager, SubagentSlot, SubagentStatus};
use crate::tools::registry::ToolRegistry;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct SpawnSubagentTool;

#[async_trait::async_trait]
impl Tool for SpawnSubagentTool {
    fn name(&self) -> &'static str {
        "spawn_subagent"
    }

    fn description(&self) -> &'static str {
        "Spawn a subagent to work on a subtask in parallel. Returns a job_id to check status later."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"task": "Research best practices for error handling in Rust"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "description": "Clear description of the subtask for the subagent to accomplish" },
                "name": { "type": "string", "description": "Optional friendly name for this subagent (default: auto-generated)" },
                "model_override": { "type": "string", "description": "Optional model to use instead of parent's model" }
            },
            "required": ["task"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let task = arguments
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: task".to_string(),
                retryable: false,
            })?;

        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                let id = uuid::Uuid::new_v4().to_string();
                format!("sub-{}", id.chars().take(8).collect::<String>())
            });

        let model_override = arguments
            .get("model_override")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Check per-parent concurrent limit
        let config = context.config.read().await;
        let max_concurrent = config
            .subagents
            .as_ref()
            .map(|s| s.max_concurrent)
            .unwrap_or(4);
        drop(config);

        let active = context
            .subagent_manager
            .active_count(&context.agent_id)
            .await;
        if active >= max_concurrent as usize {
            return Err(ToolError {
                code: ToolErrorCode::SubagentLimitReached,
                message: format!(
                    "Per-parent subagent limit reached ({}/{}). Wait for existing subagents to complete or cancel one.",
                    active, max_concurrent
                ),
                retryable: true,
            });
        }

        // Check global subagent limit (across all parents)
        let global_max = {
            let config = context.config.read().await;
            config.subagents.as_ref().map(|s| s.max_total).unwrap_or(16)
        };
        let total_active = context.subagent_manager.total_active_count().await;
        if total_active >= global_max as usize {
            return Err(ToolError {
                code: ToolErrorCode::SubagentLimitReached,
                message: format!(
                    "Global subagent limit reached ({}/{}). Wait for subagents to complete.",
                    total_active, global_max
                ),
                retryable: true,
            });
        }

        // Get parent's connection config
        let connection = {
            let agents = context.agents.read().await;
            agents
                .iter()
                .find(|a| a.id == context.agent_id)
                .map(|a| a.connection.clone())
                .ok_or_else(|| ToolError {
                    code: ToolErrorCode::InternalError,
                    message: "Parent agent not found".to_string(),
                    retryable: false,
                })?
        };

        let job_id = uuid::Uuid::new_v4().to_string();

        let slot = SubagentSlot {
            job_id: job_id.clone(),
            parent_id: context.agent_id.clone(),
            name: name.clone(),
            connection,
            status: SubagentStatus::Spawning,
            conversation: ConversationHistory::default(),
            task: task.to_string(),
            model_override,
            created_at: Utc::now(),
            result: None,
        };

        // Get parent's tool config from persisted TOML (so subagent inherits tool flags)
        let parent_tools_config = {
            let config_dir =
                crate::config::app_config::agent_dir(&context.agent_id).join("config.toml");
            if config_dir.exists() {
                crate::config::agent_config::AgentConfigFile::load(&config_dir)
                    .map(|cfg| cfg.tools)
                    .unwrap_or_default()
            } else {
                AgentToolsBlock::default()
            }
        };

        // Get app_state for running the agentic loop
        let app_state = context.app_state.clone().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "AppState not available for subagent spawning".to_string(),
            retryable: false,
        })?;

        // Apply model_override to connection config (clone before slot consumed it)
        let mut subagent_connection = slot.connection.clone();
        if let Some(ref model_name) = slot.model_override {
            match &mut subagent_connection {
                ConnectionConfig::Api { model, .. } => {
                    *model = model_name.clone();
                }
                ConnectionConfig::Local { model, .. } => {
                    *model = Some(model_name.clone());
                }
                ConnectionConfig::McpStdio { .. } => {
                    // MCP stdio doesn't have a model field — ignore override
                }
                ConnectionConfig::OAuth {
                    provider,
                    credentials_path,
                    ..
                } => {
                    subagent_connection = ConnectionConfig::OAuth {
                        provider: provider.clone(),
                        credentials_path: credentials_path.clone(),
                        model: model_name.clone(),
                    };
                }
                ConnectionConfig::AgentSdk => {
                    // ClaudeCode doesn't have a model field — ignore override
                }
            }
        }

        // Build the subagent's AgentSlot
        let subagent_id = job_id.clone();

        // Read parent's inheritable settings (not personality — subagents are neutral per D8)
        let (parent_anthropic_settings, parent_local_model_settings) = {
            let agents = context.agents.read().await;
            agents
                .iter()
                .find(|a| a.id == context.agent_id)
                .map(|a| (a.anthropic_settings.clone(), a.local_model_settings.clone()))
                .unwrap_or((None, None))
        };

        let subagent_slot = AgentSlot {
            id: subagent_id.clone(),
            name: name.clone(),
            color: AgentColor::default(),
            protected: false,
            connection: subagent_connection.clone(),
            protocol: crate::runtime::protocol::preferred_protocol(&subagent_connection),
            working_directory: context.working_directory.clone(),
            status: AgentStatus::Idle,
            subagents: vec![],
            conversation: ConversationHistory::default(),
            tools: vec![],
            system_prompt: {
                // Subagents get parent's AGENTS.md + TOOLS.md for operational context,
                // but NO personality injection (D8: subagent neutrality prevents
                // sycophancy inheritance from high-agreeableness parents).
                let persona_prefix = {
                    let parent_id = &context.agent_id;
                    let all_files = crate::runtime::persona::load_persona_files(parent_id, false);
                    let filtered = crate::runtime::persona::filter_for_subagent(&all_files);
                    crate::runtime::persona::format_as_system_prompt(&filtered)
                };
                format!(
                    "{}You are a subagent. Your task: {}. Complete this task using available tools and respond with a concise summary of results.",
                    persona_prefix,
                    task
                )
            },
            metadata: AgentMetadata::default(),
            limits: AgentLimits::for_connection(&subagent_connection),
            context_window_size: 125_000,
            context_tokens_override: None,
            context_probed: false,
            sandbox_config: crate::config::agent_config::BuildSandboxBlock::default(),
            sandbox: None,
            session_prompt_tokens: 0,
            session_completion_tokens: 0,
            current_context_tokens: 0,
            session_start: None,
            is_dirty: false,
            anthropic_settings: parent_anthropic_settings,
            skills: vec![],
            user_notes: vec![],
            agent_session_id: None,
            system_prompt_hash: None,
            coordinator_mode: false,
            autonomous: false,
            extraction_pending_done: false,
            provider: None,
            cached_params: None,
            multi_agent_model: false,
            archetype_base: None,
            archetype_specialization: None,
            session_memory: Default::default(),
            cognitive_intake: Default::default(),
            appearance: None,
            openai_responses_settings: None,
            xai_settings: None,
            gemini_settings: None,
            local_model_settings: parent_local_model_settings,
        };

        // Insert subagent into shared agents vec so run_agentic_loop can find it
        {
            let mut agents = context.agents.write().await;
            agents.push(subagent_slot);
        }

        // Spawn the subagent task using tauri::async_runtime (NOT tokio::spawn)
        let subagent_manager = Arc::clone(&context.subagent_manager);
        let shell_manager = Arc::clone(&context.shell_manager);
        let subagent_working_dir = context.working_directory.clone();
        let job_id_clone = job_id.clone();
        let task_clone = task.to_string();
        let agents_ref = Arc::clone(&context.agents);
        let subagent_id_clone = subagent_id.clone();
        let subagent_app_handle = context.app_handle.clone();
        let parent_agent_id = context.agent_id.clone();

        let handle = tauri::async_runtime::spawn(async move {
            // Abort-safe cleanup guard: if this task is killed via handle.abort(),
            // Drop fires and ensures the agents vec and cancellation tokens are
            // cleaned up. On normal completion, `completed` is set to true to
            // skip the Drop cleanup (since we do it explicitly).
            struct SubagentCleanupGuard {
                agents: Arc<tokio::sync::RwLock<Vec<AgentSlot>>>,
                app_state: Arc<crate::AppState>,
                subagent_id: String,
                subagent_manager: Arc<SubagentManager>,
                job_id: String,
                completed: bool,
            }

            impl Drop for SubagentCleanupGuard {
                fn drop(&mut self) {
                    if self.completed {
                        return;
                    }
                    let agents = Arc::clone(&self.agents);
                    let app_state = Arc::clone(&self.app_state);
                    let subagent_id = self.subagent_id.clone();
                    let subagent_manager = Arc::clone(&self.subagent_manager);
                    let job_id = self.job_id.clone();
                    tauri::async_runtime::spawn(async move {
                        agents.write().await.retain(|a| a.id != subagent_id);
                        app_state
                            .cancellation_tokens
                            .write()
                            .await
                            .remove(&subagent_id);
                        subagent_manager
                            .fail(&job_id, "Subagent was cancelled".to_string())
                            .await;
                    });
                }
            }

            let mut guard = SubagentCleanupGuard {
                agents: Arc::clone(&agents_ref),
                app_state: Arc::clone(&app_state),
                subagent_id: subagent_id_clone.clone(),
                subagent_manager: Arc::clone(&subagent_manager),
                job_id: job_id_clone.clone(),
                completed: false,
            };

            // Update status to Working
            subagent_manager
                .update_status(&job_id_clone, SubagentStatus::Working)
                .await;

            // Inherit parent's sandbox level for both PTY spawn and tool registry
            let subagent_sandbox_config = {
                let sm = shell_manager.read().await;
                sm.sandbox_configs
                    .get(&parent_agent_id)
                    .map(|(cfg, _)| cfg.clone())
                    .unwrap_or_default()
            };

            // Spawn PTY session for subagent
            {
                let mut sm = shell_manager.write().await;
                if let Err(e) = sm
                    .spawn_session(
                        subagent_id_clone.clone(),
                        subagent_working_dir.clone(),
                        &subagent_sandbox_config,
                        &subagent_working_dir,
                    )
                    .await
                {
                    eprintln!(
                        "Failed to spawn PTY for subagent {}: {}",
                        subagent_id_clone, e
                    );
                }
            }

            // Create cancellation token for subagent
            let cancel_token = tokio_util::sync::CancellationToken::new();
            app_state
                .cancellation_tokens
                .write()
                .await
                .insert(subagent_id_clone.clone(), cancel_token.clone());

            // Read config for network/limits
            let config = app_state.config.read().await;
            let network_config = config.network.clone();
            let limits_config = config.limits.clone();
            drop(config);

            // Build scoped tool registry — inherits parent's tool flags, no spawn_subagent (depth=1)
            let tool_registry = ToolRegistry::build_for_subagent(
                &parent_tools_config,
                Some(&app_state.plugin_host),
                Some(&subagent_sandbox_config.level),
            )
            .await;

            // Run the real agentic loop
            let engine_ctx = crate::runtime::engine::EngineContext {
                agents: &agents_ref,
                agent_id: &subagent_id_clone,
                trace_engine: &app_state.trace_engine,
                keychain: &app_state.keychain,
                rate_limiter: &app_state.rate_limiter,
                network_config: &network_config,
                limits_config: &limits_config,
                app_handle: subagent_app_handle.as_ref(),
                tool_registry: &tool_registry,
                shell_manager: &app_state.shell_manager,
                app_config: &app_state.config,
                approval_manager: &app_state.approval_manager,
                vfl_registry: &app_state.vfl_registry,
                subagent_manager: &app_state.subagent_manager,
                a2a_registry: &app_state.a2a_registry,
                cancellation_token: Some(&cancel_token),
                app_state: Some(Arc::clone(&app_state)),
                suppress_memory_pipeline: false,
                transient_user_message: None,
            };
            let result =
                crate::runtime::engine::run_agentic_loop(&engine_ctx, &task_clone, None, None)
                    .await;

            // Handle result
            match result {
                Ok(response) => {
                    subagent_manager.complete(&job_id_clone, response).await;
                }
                Err(e) => {
                    subagent_manager.fail(&job_id_clone, e.to_string()).await;
                }
            }

            // Cleanup: remove subagent from agents vec
            {
                let mut agents = agents_ref.write().await;
                agents.retain(|a| a.id != subagent_id_clone);
            }

            // Cleanup: remove cancellation token
            app_state
                .cancellation_tokens
                .write()
                .await
                .remove(&subagent_id_clone);

            // Cleanup: destroy subagent PTY session
            {
                let mut sm = shell_manager.write().await;
                sm.destroy_session(&subagent_id_clone).await;
            }

            // Set guard as completed so Drop doesn't double-clean
            guard.completed = true;
        });

        context.subagent_manager.register(slot, handle).await;

        Ok(ToolResult {
            content: json!({
                "job_id": job_id,
                "name": name,
                "status": "spawned",
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
