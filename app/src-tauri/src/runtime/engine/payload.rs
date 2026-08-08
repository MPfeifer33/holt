//! Request payload construction for the agentic loop.
//!
//! Resolves connection details, augments the system prompt (persona, tools,
//! coordinator preamble, memory, skills), and assembles the context payload.

use std::borrow::Cow;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::app_config::AppConfig;
use crate::runtime::agent::{AgentSlot, ConversationMessage, MessageRole};
use crate::runtime::connection::ConnectionConfig;
use crate::security::keychain::KeychainManager;
use crate::tools::registry::ToolRegistry;

use super::helpers::COORDINATOR_PREAMBLE;
use super::EngineError;

/// Everything the main loop needs from the context-building phase.
pub struct LoopPayload {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub model: String,
    pub messages: Vec<serde_json::Value>,
    pub needs_compaction: bool,
    pub context_tokens: u32,
    pub context_window_size: u32,
    pub context_config: crate::config::app_config::ContextBlock,
    pub working_dir: String,
    /// Augmented system prompt for CacheSafeParams capture.
    pub augmented_system_prompt: String,
    /// Whether the model supports tool/function calling. When false, the engine
    /// omits tool definitions from the API request. Derived from
    /// `local_model_settings.supports_tools` (defaults to true for cloud models).
    pub supports_tools: bool,
}

/// Build the request payload for one iteration of the agentic loop.
///
/// Resolves connection endpoint/key/model, augments the system prompt with
/// persona files, tool awareness, coordinator preamble, skills, user notes,
/// and memory context, then assembles the context payload via the three-zone
/// layout.
pub async fn build_loop_payload(
    agents: &Arc<RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    keychain: &KeychainManager,
    iter_config: &AppConfig,
    tool_registry: &ToolRegistry,
    app_state: &Option<Arc<crate::AppState>>,
    memory_section: &str,
    a2a_section: &str,
    transient_user_message: Option<&str>,
) -> Result<LoopPayload, EngineError> {
    // Clone the agent and release the read lock immediately.
    // This trades one clone for 100-200ms less lock contention during the
    // expensive payload assembly (persona I/O, skill resolution, context building).
    let agent = {
        let agents = agents.read().await;
        agents
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| EngineError::AgentNotFound(agent_id.to_string()))?
            .clone()
    };

    let (endpoint, api_key, model) = match &agent.connection {
        ConnectionConfig::Api {
            endpoint,
            api_key_ref,
            model,
        } => {
            let key = keychain.get_api_key(api_key_ref).ok();
            (endpoint.clone(), key, model.clone())
        }
        ConnectionConfig::Local { host, port, model } => {
            let endpoint = format!("http://{}:{}/v1", host, port);
            let model = model.clone().unwrap_or_else(|| "local".to_string());
            (endpoint, None, model)
        }
        ConnectionConfig::McpStdio { .. } => {
            return Err(EngineError::NoEndpoint);
        }
        ConnectionConfig::OAuth {
            provider,
            credentials_path,
            model,
            ..
        } => {
            if crate::runtime::codex::is_codex_provider(provider) {
                return Err(EngineError::ConfigError(
                    "Codex lane exists, but its execution path is not wired yet".to_string(),
                ));
            }
            let oauth_client = app_state
                .as_ref()
                .map(|s| s.http_client.clone())
                .unwrap_or_else(reqwest::Client::new);
            let token =
                crate::security::oauth::resolve_bearer_token(credentials_path, &oauth_client)
                    .await
                    .map_err(|e| {
                        EngineError::NetworkError(format!("OAuth credential error: {e}"))
                    })?;
            (
                "https://api.anthropic.com/v1/messages".to_string(),
                Some(token),
                model.clone(),
            )
        }
        ConnectionConfig::AgentSdk => {
            // Claude Code SDK agents use a separate execution path (claude_sdk.rs)
            return Err(EngineError::NoEndpoint);
        }
    };

    // Build context payload using three-zone layout with eviction. Some call
    // sites provide a transient user prompt that should steer the current loop
    // without mutating persisted conversation history.
    let payload_messages: Cow<'_, [ConversationMessage]> =
        conversation_for_payload(&agent, transient_user_message);

    let context_config = iter_config.context.clone();
    let tool_defs = tool_registry.core_definitions();

    // Augment system prompt with tool catalog.
    // If supports_tools is true, structured tool definitions will be sent in the
    // request body — skip the text catalog to avoid sending tools twice.
    // If supports_tools is false (model can't do native function calling), include
    // a neutral text catalog without imperative "proactively" instructions.
    let supports_tools_early = agent
        .local_model_settings
        .as_ref()
        .map(|s| s.supports_tools)
        .unwrap_or(true);

    let tool_awareness = if !tool_defs.is_empty() && !supports_tools_early {
        // Model doesn't support structured tools — give it a text catalog
        let tool_list: Vec<String> = tool_defs
            .iter()
            .filter_map(|t| {
                let func = t.get("function")?;
                let name = func.get("name")?.as_str()?;
                let desc = func.get("description")?.as_str().unwrap_or("");
                Some(format!("- {}: {}", name, desc))
            })
            .collect();
        format!(
            "\n\nThe following tools are available in this environment:\n{}",
            tool_list.join("\n")
        )
    } else {
        // Model supports tools natively — structured definitions are sent separately
        String::new()
    };
    // Load persona files and prepend to system prompt.
    // When native memory engine is active, skip MEMORY.md and daily logs
    // (superseded by Hillock-backed memory injection above).
    let memory_active = app_state
        .as_ref()
        .map(|s| s.has_memory_engine())
        .unwrap_or(false);
    let persona_prefix = {
        let mut persona_files =
            crate::runtime::persona::load_persona_files(&agent.id, memory_active);

        // Living Persona: inject dynamic memory profile after static files
        if memory_active {
            if let Some(ref state) = app_state {
                if let Some(engine) = state.get_memory_engine() {
                    let agent_ns = &agent.id;
                    if let Some(dynamic_profile) = crate::runtime::persona::fetch_dynamic_profile(
                        &engine,
                        agent_ns,
                        crate::runtime::persona::DYNAMIC_PROFILE_TOKEN_BUDGET,
                    )
                    .await
                    {
                        tracing::info!(
                            agent_id = %agent.id,
                            "Dynamic memory profile injected into persona"
                        );
                        persona_files.push(dynamic_profile);
                    }
                }
            }
        }

        crate::runtime::persona::format_as_system_prompt(&persona_files)
    };

    // Build skill context from recent conversation
    let skills_section = if let Some(ref state) = app_state {
        let (recent_tools, recent_files, last_user_msg) = {
            let mut tools = Vec::new();
            let mut files = Vec::new();
            let mut last_user = String::new();

            for msg in payload_messages.iter().rev().take(6) {
                match msg.role {
                    MessageRole::User => {
                        if last_user.is_empty() {
                            last_user = msg.content.clone();
                        }
                    }
                    MessageRole::Assistant => {
                        if let Some(ref tcs) = msg.tool_calls {
                            for tc in tcs {
                                tools.push(tc.name.clone());
                                if let Some(path) =
                                    tc.arguments.get("path").and_then(|p| p.as_str())
                                {
                                    files.push(path.to_string());
                                }
                                if let Some(path) =
                                    tc.arguments.get("file_path").and_then(|p| p.as_str())
                                {
                                    files.push(path.to_string());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            (tools, files, last_user)
        };

        let skill_ctx = crate::runtime::skills::SkillContext {
            agent_skill_names: agent.skills.clone(),
            recent_tool_names: recent_tools,
            recent_file_paths: recent_files,
            last_user_message: last_user_msg,
            token_budget: 8000,
        };

        let resolved_skills =
            crate::runtime::skills::resolve_skills(&state.skill_registry, &skill_ctx);

        if !resolved_skills.is_empty() {
            use crate::runtime::skills::DeliveryMode;

            let inject_skills: Vec<_> = resolved_skills
                .iter()
                .filter(|s| s.delivery == DeliveryMode::Inject)
                .collect();
            let reference_skills: Vec<_> = resolved_skills
                .iter()
                .filter(|s| s.delivery == DeliveryMode::Reference)
                .collect();

            let all_names: Vec<&str> = resolved_skills
                .iter()
                .map(|s| s.definition.frontmatter.name.as_str())
                .collect();
            tracing::info!(
                agent_id,
                inject = inject_skills.len(),
                reference = reference_skills.len(),
                "Active skills: {:?}",
                all_names
            );

            let mut sections = String::new();

            // Inject skills: full content in system prompt
            if !inject_skills.is_empty() {
                let blocks: Vec<String> = inject_skills
                    .iter()
                    .map(|s| {
                        format!(
                            "<skill name=\"{}\">\n{}\n</skill>",
                            s.definition.frontmatter.name, s.definition.body
                        )
                    })
                    .collect();
                sections.push_str(&format!("\n\n# Active Skills\n\n{}", blocks.join("\n\n")));
            }

            // Reference skills: compact manifest (agent can pull full content via read_skill tool)
            if !reference_skills.is_empty() {
                let manifest_lines: Vec<String> = reference_skills
                    .iter()
                    .map(|s| {
                        format!(
                            "- [{}] {} ({} tokens, use read_skill to load)",
                            s.definition.frontmatter.name,
                            s.definition.frontmatter.description,
                            s.definition.frontmatter.max_tokens
                        )
                    })
                    .collect();
                sections.push_str(&format!(
                    "\n\n# Available Skills (read_skill to load full content)\n\n{}",
                    manifest_lines.join("\n")
                ));
            }

            sections
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let user_notes_section = if !agent.user_notes.is_empty() {
        let notes = agent
            .user_notes
            .iter()
            .enumerate()
            .map(|(i, n)| format!("{}. {}", i + 1, n))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n\n# User Notes (via /btw)\n\n{}", notes)
    } else {
        String::new()
    };

    let coordinator_prefix = if agent.coordinator_mode {
        COORDINATOR_PREAMBLE
    } else {
        ""
    };

    // Prompt ordering optimized for provider-side prefix caching.
    // Stable content first (cacheable), dynamic content last (invalidates nothing after it).
    // See CACHING_SYSTEM_PRD.md Section 2 for rationale.
    //
    // Sentinel markers between stability zones enable provider adapters to insert
    // cache breakpoints. The 1HR sentinel follows session-stable content (system prompt,
    // tools, persona). The 5M sentinel follows semi-stable content (user notes, skills).
    // Everything after (memory) is dynamic and not cached.
    use crate::runtime::cache::sentinels::{CACHE_SENTINEL_1HR, CACHE_SENTINEL_5M};

    let augmented_system_prompt = format!(
        "{}{}{}{}{}{}{}{}{}{}",
        coordinator_prefix,
        agent.system_prompt,
        tool_awareness,
        persona_prefix,
        CACHE_SENTINEL_1HR,
        user_notes_section,
        skills_section,
        CACHE_SENTINEL_5M,
        memory_section,
        a2a_section,
    );

    let budget = crate::runtime::context::ContextBudget::from_config(
        &context_config,
        agent.context_window_size,
    );
    let enable_pinned_bottom =
        agent.protocol == crate::runtime::connection::AgentProtocol::ChatCompletions;
    let context_payload = crate::runtime::context::build_context_payload(
        &augmented_system_prompt,
        &tool_defs,
        payload_messages.as_ref(),
        &budget,
        context_config.max_tool_result_tokens,
        &context_config.truncation_strategy,
        context_config.compaction_threshold_percent,
        enable_pinned_bottom,
    );

    // --- Strict system position rewrite ---
    // For local models with strict Jinja chat templates (e.g., Qwen/Ornith),
    // mid-conversation system messages cause HTTP 500. Rewrite them to
    // role: user with a structural wrapper so the model still gets the context
    // but the template doesn't reject it.
    let strict_system_position = agent
        .local_model_settings
        .as_ref()
        .map(|s| s.strict_system_position)
        .unwrap_or(false);

    let mut final_messages = context_payload.messages;
    if strict_system_position {
        for (i, msg) in final_messages.iter_mut().enumerate() {
            if i > 0 && msg.get("role").and_then(|r| r.as_str()) == Some("system") {
                msg["role"] = serde_json::json!("user");
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    msg["content"] = serde_json::json!(format!(
                        "[SYSTEM CONTEXT — NOT FROM HUMAN]\n{}\n[END SYSTEM CONTEXT]",
                        content
                    ));
                }
            }
        }
    }

    let working_dir = agent.working_directory.to_string_lossy().to_string();
    let context_window_size = agent.context_window_size;

    let supports_tools = agent
        .local_model_settings
        .as_ref()
        .map(|s| s.supports_tools)
        .unwrap_or(true); // cloud models always support tools

    Ok(LoopPayload {
        endpoint,
        api_key,
        model,
        messages: final_messages,
        needs_compaction: context_payload.needs_compaction,
        context_tokens: context_payload.total_estimated_tokens,
        context_window_size,
        context_config,
        working_dir,
        augmented_system_prompt,
        supports_tools,
    })
}

fn conversation_for_payload<'a>(
    agent: &'a AgentSlot,
    transient_user_message: Option<&str>,
) -> Cow<'a, [ConversationMessage]> {
    if let Some(message) = transient_user_message {
        let mut messages = agent.conversation.messages.clone();
        messages.push(ConversationMessage {
            role: MessageRole::User,
            content: message.to_string(),
            content_blocks: None,
            timestamp: chrono::Utc::now(),
            token_estimate: None,
            tool_calls: None,
            tool_call_id: None,
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        });
        Cow::Owned(messages)
    } else {
        Cow::Borrowed(&agent.conversation.messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::agent::{AgentColor, AgentMetadata, AgentStatus, ConversationHistory};
    use crate::runtime::connection::{AgentProtocol, ConnectionConfig};
    use crate::runtime::limits::AgentLimits;
    use chrono::Utc;
    use std::path::PathBuf;

    fn test_agent() -> AgentSlot {
        AgentSlot {
            id: "agent-1".to_string(),
            name: "Agent".to_string(),
            color: AgentColor::default(),
            protected: false,
            connection: ConnectionConfig::Local {
                host: "localhost".to_string(),
                port: 11434,
                model: Some("test-model".to_string()),
            },
            protocol: AgentProtocol::ChatCompletions,
            working_directory: PathBuf::from("/tmp"),
            status: AgentStatus::Idle,
            subagents: vec![],
            conversation: ConversationHistory {
                messages: vec![ConversationMessage {
                    role: MessageRole::User,
                    content: "real user".to_string(),
                    content_blocks: None,
                    timestamp: Utc::now(),
                    token_estimate: None,
                    tool_calls: None,
                    tool_call_id: None,
                    outcome_tag: None,
                    thinking_content: None,
                    responses_phase: None,
                    reasoning_items: None,
                }],
                total_token_estimate: 0,
            },
            tools: vec![],
            system_prompt: "prompt".to_string(),
            metadata: AgentMetadata::default(),
            limits: AgentLimits::default(),
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
            anthropic_settings: None,
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
            local_model_settings: None,
        }
    }

    #[test]
    fn conversation_for_payload_borrows_when_no_transient_message() {
        let agent = test_agent();
        let payload = conversation_for_payload(&agent, None);
        assert_eq!(payload.len(), 1);
        assert_eq!(
            payload.last().map(|m| m.content.as_str()),
            Some("real user")
        );
    }

    #[test]
    fn conversation_for_payload_appends_transient_user_message() {
        let agent = test_agent();
        let payload = conversation_for_payload(&agent, Some("[Message from agent 'Agent A': hi]"));
        assert_eq!(agent.conversation.messages.len(), 1);
        assert_eq!(payload.len(), 2);
        assert_eq!(
            payload.last().map(|m| (m.role.clone(), m.content.as_str())),
            Some((MessageRole::User, "[Message from agent 'Agent A': hi]"))
        );
    }
}
