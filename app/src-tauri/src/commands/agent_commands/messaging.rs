use super::*;
use crate::runtime::turn_manager::{
    TurnPriority, TurnProtocol, TurnRequest, TurnResult, TurnSource,
};

/// Serializable conversation message for the frontend chat UI.
#[derive(serde::Serialize)]
pub struct ConversationMessageSummary {
    pub role: String,
    pub content: String,
    pub timestamp: String,
    pub tool_calls: Option<Vec<crate::runtime::agent::ToolCallRecord>>,
    pub tool_call_id: Option<String>,
    pub content_blocks: Option<Vec<crate::runtime::agent::ContentBlock>>,
}

#[tauri::command]
pub async fn get_conversation(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<Vec<ConversationMessageSummary>, CommandError> {
    let agents = state.agents.read().await;
    let agent = agents
        .iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    Ok(agent
        .conversation
        .messages
        .iter()
        .map(|m| ConversationMessageSummary {
            role: match m.role {
                crate::runtime::agent::MessageRole::System => "system".to_string(),
                crate::runtime::agent::MessageRole::User => "user".to_string(),
                crate::runtime::agent::MessageRole::Assistant => "assistant".to_string(),
                crate::runtime::agent::MessageRole::Tool => "tool".to_string(),
            },
            content: m.content.clone(),
            timestamp: m.timestamp.to_rfc3339(),
            tool_calls: m.tool_calls.clone(),
            tool_call_id: m.tool_call_id.clone(),
            content_blocks: m.content_blocks.clone(),
        })
        .collect())
}

/// Core turn execution logic — protocol dispatch, engine loop, status transitions.
///
/// Accepts an externally-owned `CancellationToken` so callers (TurnManager worker
/// or legacy path) control the token lifecycle. This is the single place where
/// turns actually execute.
pub(crate) async fn execute_turn_inner(
    state: &Arc<AppState>,
    app_handle: &AppHandle,
    agent_id: &str,
    message: &str,
    images: Option<Vec<crate::runtime::agent::ImageAttachment>>,
    transient_user_message: Option<&str>,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> Result<String, CommandError> {
    let config = state.config.read().await;
    let network_config = config.network.clone();
    let limits_config = config.limits.clone();
    drop(config);

    let protocol = {
        let agents = state.agents.read().await;
        agents
            .iter()
            .find(|a| a.id == agent_id)
            .map(|a| a.protocol.clone())
            .unwrap_or_default()
    };

    if protocol == crate::runtime::connection::AgentProtocol::AgentSdk {
        let result =
            crate::runtime::agent_sdk::agent_sdk_query(agent_id, message, state, Some(app_handle))
                .await;

        return result
            .map(|()| String::new())
            .map_err(CommandError::internal);
    }

    if protocol == crate::runtime::connection::AgentProtocol::Codex {
        let result = crate::runtime::codex::codex_query(
            agent_id,
            message,
            images,
            state,
            Some(app_handle),
            transient_user_message,
            cancel_token,
        )
        .await;

        return result;
    }

    let (tools_config, sandbox_level) = {
        let config_path = crate::config::app_config::agent_dir(agent_id).join("config.toml");
        if config_path.exists() {
            crate::config::agent_config::AgentConfigFile::load(&config_path)
                .map(|cfg| {
                    let es = cfg.resolved_execution_sandbox();
                    (cfg.tools, es.level)
                })
                .unwrap_or_else(|_| (AgentToolsBlock::default(), "unrestricted".to_string()))
        } else {
            (AgentToolsBlock::default(), "unrestricted".to_string())
        }
    };
    let coordinator_mode = {
        let agents = state.agents.read().await;
        agents
            .iter()
            .find(|a| a.id == agent_id)
            .map(|a| a.coordinator_mode)
            .unwrap_or(false)
    };
    let tool_registry = crate::tools::registry::ToolRegistry::build_for_agent(
        &tools_config,
        Some(&state.plugin_host),
        coordinator_mode,
        Some(&sandbox_level),
    )
    .await;

    let app_state: Arc<AppState> = state.clone();

    let engine_ctx = crate::runtime::engine::EngineContext {
        agents: &state.agents,
        agent_id,
        trace_engine: &state.trace_engine,
        keychain: &state.keychain,
        rate_limiter: &state.rate_limiter,
        network_config: &network_config,
        limits_config: &limits_config,
        app_handle: Some(app_handle),
        tool_registry: &tool_registry,
        shell_manager: &state.shell_manager,
        app_config: &state.config,
        approval_manager: &state.approval_manager,
        vfl_registry: &state.vfl_registry,
        subagent_manager: &state.subagent_manager,
        a2a_registry: &state.a2a_registry,
        cancellation_token: Some(cancel_token),
        app_state: Some(app_state),
        suppress_memory_pipeline: false,
        transient_user_message,
    };
    let content_blocks = images.map(|imgs| {
        use crate::runtime::agent::ContentBlock;
        let mut blocks = vec![ContentBlock::Text {
            text: message.to_string(),
        }];
        for img in imgs {
            blocks.push(ContentBlock::Image {
                data: img.data,
                media_type: img.media_type,
            });
        }
        blocks
    });

    let result =
        crate::runtime::engine::run_agentic_loop(&engine_ctx, message, content_blocks, None).await;

    {
        let mut agents = state.agents.write().await;
        if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
            if matches!(agent.status, crate::runtime::agent::AgentStatus::Working) {
                agent.status = if result.is_ok() {
                    crate::runtime::agent::AgentStatus::Idle
                } else {
                    crate::runtime::agent::AgentStatus::Error(
                        result
                            .as_ref()
                            .err()
                            .map(|e| e.to_string())
                            .unwrap_or_default(),
                    )
                };
                agent.is_dirty = true;
            }
        }
    }

    result.map_err(CommandError::from)
}

// ── TurnManager Integration ────────────────────────────────────────────

/// Register a TurnManager worker for an agent that dispatches to execute_turn_inner.
///
/// Idempotent — safe to call multiple times for the same agent.
/// The worker captures clones of AppState and AppHandle, dual-writes the
/// cancellation token to `state.cancellation_tokens` for backward compat
/// with `interrupt_agent` during migration.
pub(crate) async fn register_turn_worker(
    state: &Arc<AppState>,
    app_handle: &AppHandle,
    agent_id: &str,
) {
    let st = state.clone();
    let ah = app_handle.clone();

    state
        .turn_manager
        .ensure_worker(agent_id.to_string(), move |aid, req, token| {
            let st = st.clone();
            let ah = ah.clone();
            async move {
                let (message, images, transient) = match req.protocol {
                    TurnProtocol::ApiDirect {
                        message,
                        images,
                        transient_prompt,
                        ..
                    } => (message, images, transient_prompt),
                    TurnProtocol::AgentSdk { message } => (message, None, None),
                    TurnProtocol::Codex {
                        message,
                        images,
                        transient_prompt,
                    } => (message, images, transient_prompt),
                };

                // Dual-write token for backward compat with interrupt_agent
                st.cancellation_tokens
                    .write()
                    .await
                    .insert(aid.clone(), token.clone());

                let result = execute_turn_inner(
                    &st,
                    &ah,
                    &aid,
                    &message,
                    images,
                    transient.as_deref(),
                    &token,
                )
                .await;

                st.cancellation_tokens.write().await.remove(&aid);

                match result {
                    Ok(response) => TurnResult::Completed(response),
                    Err(e) => TurnResult::Error(e.to_string()),
                }
            }
        })
        .await;
}

/// Submit a turn through the TurnManager and await its result.
async fn submit_and_await(
    state: &Arc<AppState>,
    agent_id: &str,
    request: TurnRequest,
) -> Result<String, CommandError> {
    match state.turn_manager.submit_turn(agent_id, request).await {
        Some(handle) => match handle.result().await {
            TurnResult::Completed(response) => Ok(response),
            TurnResult::Error(e) => Err(CommandError::internal(e)),
            TurnResult::Cancelled => Err(CommandError::internal("Turn was cancelled")),
            TurnResult::Dropped => Err(CommandError::internal("Turn was dropped")),
        },
        None => Err(CommandError::internal(
            "Turn queue full or agent not registered",
        )),
    }
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, Arc<AppState>>,
    app_handle: AppHandle,
    agent_id: String,
    message: String,
    images: Option<Vec<crate::runtime::agent::ImageAttachment>>,
) -> Result<String, CommandError> {
    let app_state: Arc<AppState> = state.inner().clone();

    // Ensure worker is registered (idempotent)
    register_turn_worker(&app_state, &app_handle, &agent_id).await;

    let protocol = {
        let agents = app_state.agents.read().await;
        agents
            .iter()
            .find(|a| a.id == agent_id)
            .map(|a| a.protocol.clone())
            .unwrap_or_default()
    };

    let turn_protocol = TurnProtocol::from_message(&protocol, message, images, None);
    let request = TurnRequest::new(turn_protocol, TurnSource::SendMessage, TurnPriority::User);

    submit_and_await(&app_state, &agent_id, request).await
}

#[tauri::command]
pub async fn run_codex_review(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    instructions: Option<String>,
) -> Result<String, CommandError> {
    let protocol = {
        let agents = state.agents.read().await;
        agents
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?
            .protocol
            .clone()
    };

    if protocol != crate::runtime::connection::AgentProtocol::Codex {
        return Err(CommandError::internal(
            "/review is only available for Codex agents",
        ));
    }

    let cancellation_token = tokio_util::sync::CancellationToken::new();
    crate::runtime::codex::codex_review(
        &state.agents,
        &agent_id,
        instructions.as_deref(),
        &cancellation_token,
    )
    .await
    .map_err(CommandError::from)
}

/// Core A2A wake logic — callable from both the Tauri command and the A2A send tool.
///
/// Wakes an idle agent that has pending A2A messages by submitting a turn with a
/// transient wake prompt. No-ops if the agent is already active or has no pending
/// messages. SDK agents get a simple empty-message turn (their own auto-wake loop
/// handles the rest).
pub(crate) async fn a2a_wake_agent(
    app_state: &Arc<AppState>,
    app_handle: &AppHandle,
    agent_id: &str,
) -> Result<(), String> {
    // Ensure worker is registered (idempotent)
    register_turn_worker(app_state, app_handle, agent_id).await;

    // Check if already active via TurnManager
    if app_state.turn_manager.is_active(agent_id).await {
        tracing::debug!(agent_id, "a2a_wake_agent: turn already active, no-op");
        return Ok(());
    }

    let (protocol, externally_driven) = {
        let agents = app_state.agents.read().await;
        match agents.iter().find(|a| a.id == agent_id) {
            Some(a) => (a.protocol.clone(), a.is_externally_driven()),
            None => return Err(format!("Agent not found: {}", agent_id)),
        }
    };

    // Externally-driven slots (e.g. the TUI lane's placeholder connection) have
    // their turns submitted by an external process. Submitting an engine turn here
    // would drain the A2A queue into a turn with no model behind it, silently
    // destroying the messages. This guard covers EVERY wake caller (the frontend's
    // trigger_agent_turn command, the send tool's server-side auto-wake, and any
    // future path) at the single choke point they all funnel through.
    if externally_driven {
        tracing::info!(
            agent_id,
            "a2a_wake_agent: target is externally driven; message stays queued for external drain"
        );
        return Ok(());
    }

    // SDK agents get a simple empty-message turn
    if protocol == crate::runtime::connection::AgentProtocol::AgentSdk {
        let turn_protocol = TurnProtocol::from_message(&protocol, String::new(), None, None);
        let request = TurnRequest::new(
            turn_protocol,
            TurnSource::TriggerAgentTurn,
            TurnPriority::AgentInitiated,
        );
        let _ = submit_and_await(app_state, agent_id, request)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    // Check for pending A2A messages without draining -- the actual drain
    // happens inside inject_turn_messages during the engine loop, where A2A
    // content is injected into the system prompt (not as a user message).
    if !app_state.a2a_registry.has_pending_messages(agent_id).await {
        tracing::debug!(agent_id, "a2a_wake_agent: no pending A2A messages, no-op");
        return Ok(());
    }

    // Record wake for cooldown tracking
    app_state.a2a_registry.record_wake(agent_id).await;

    // Start turn with a synthetic wake prompt so the model knows to check system context.
    // A2A content is injected into the system prompt by inject_turn_messages (API agents)
    // or drained by codex_query (Codex agents). The wake prompt is transient -- it anchors
    // the first model response but is NOT persisted to conversation history.
    let wake_prompt = "[You have received direct messages from other agents. Check your system context and respond via send_message_to_agent.]".to_string();
    let turn_protocol =
        TurnProtocol::from_message(&protocol, String::new(), None, Some(wake_prompt));
    let request = TurnRequest::new(
        turn_protocol,
        TurnSource::TriggerAgentTurn,
        TurnPriority::AgentInitiated,
    );
    let _ = submit_and_await(app_state, agent_id, request)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn trigger_agent_turn(
    state: State<'_, Arc<AppState>>,
    app_handle: AppHandle,
    agent_id: String,
) -> Result<(), CommandError> {
    let app_state: Arc<AppState> = state.inner().clone();
    a2a_wake_agent(&app_state, &app_handle, &agent_id)
        .await
        .map_err(CommandError::internal)
}

#[tauri::command]
pub async fn check_vision_support(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<bool, CommandError> {
    let agents = state.agents.read().await;
    let agent = agents
        .iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;
    let model = match &agent.connection {
        crate::runtime::connection::ConnectionConfig::Api { model, .. } => model.clone(),
        crate::runtime::connection::ConnectionConfig::Local { model, .. } => {
            model.clone().unwrap_or_default()
        }
        crate::runtime::connection::ConnectionConfig::OAuth { model, .. } => model.clone(),
        crate::runtime::connection::ConnectionConfig::AgentSdk => "claude-sonnet".to_string(),
        crate::runtime::connection::ConnectionConfig::McpStdio { .. } => String::new(),
    };
    Ok(crate::runtime::agent::model_supports_vision(&model))
}

#[tauri::command]
pub async fn interrupt_agent(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<(), CommandError> {
    let is_sdk = {
        let agents = state.agents.read().await;
        agents
            .iter()
            .find(|a| a.id == agent_id)
            .map(|a| a.protocol == crate::runtime::connection::AgentProtocol::AgentSdk)
            .unwrap_or(false)
    };
    if is_sdk {
        if let Err(e) = crate::runtime::agent_sdk::abort_bridge(&agent_id).await {
            tracing::warn!(agent_id, "Failed to abort SDK sidecar: {e}");
        }
    }

    crate::runtime::interrupt::interrupt_agent(
        &agent_id,
        &state.cancellation_tokens,
        &state.vfl_registry,
        &state.subagent_manager,
        &state.trace_engine,
        &state.agents,
        Some(&state.approval_manager),
        Some(&state.turn_manager),
    )
    .await;

    Ok(())
}

#[tauri::command]
pub async fn respond_to_approval(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    tool_call_id: String,
    approved: bool,
) -> Result<(), CommandError> {
    let key = format!("{}:{}", agent_id, tool_call_id);
    if state.approval_manager.resolve(&key, approved).await {
        Ok(())
    } else {
        Err(CommandError::not_found(format!(
            "No pending approval for agent '{}' tool call '{}'",
            agent_id, tool_call_id
        )))
    }
}

#[tauri::command]
pub async fn respond_to_hil(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    response: String,
    selected_option: Option<u32>,
) -> Result<(), CommandError> {
    let mut manager = state.shell_manager.write().await;
    if let Some(sender) = manager.hil_senders.remove(&agent_id) {
        let _ = sender.send(serde_json::json!({
            "response": response,
            "selected_option": selected_option,
        }));
        Ok(())
    } else {
        Err(CommandError::not_found(format!(
            "No pending HIL request for agent '{}'",
            agent_id
        )))
    }
}

#[tauri::command]
pub async fn tag_message_outcome(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    message_index: usize,
    outcome: Option<String>,
) -> Result<(), CommandError> {
    use crate::runtime::agent::OutcomeTag;

    let tag = match outcome.as_deref() {
        Some("success") => Some(OutcomeTag::Success),
        Some("failure") => Some(OutcomeTag::Failure),
        Some("partial") => Some(OutcomeTag::Partial),
        None | Some("clear") => None,
        Some(other) => {
            return Err(CommandError::validation(format!(
                "Unknown outcome tag: '{}'. Use success, failure, partial, or clear.",
                other
            )))
        }
    };

    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    let msg = agent
        .conversation
        .messages
        .get_mut(message_index)
        .ok_or_else(|| {
            CommandError::not_found(format!("Message index {} out of range", message_index))
        })?;

    msg.outcome_tag = tag;
    agent.is_dirty = true;

    Ok(())
}

#[tauri::command]
pub async fn list_subagents(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<Vec<crate::runtime::subagent::SubagentInfo>, CommandError> {
    Ok(state.subagent_manager.list(&agent_id).await)
}

#[tauri::command]
pub async fn cancel_subagent(
    state: State<'_, Arc<AppState>>,
    job_id: String,
) -> Result<(), CommandError> {
    state.subagent_manager.cancel(&job_id).await;
    Ok(())
}

#[tauri::command]
pub async fn broadcast_message(
    state: State<'_, Arc<AppState>>,
    app_handle: AppHandle,
    message: String,
) -> Result<Vec<String>, CommandError> {
    let app_state: Arc<AppState> = state.inner().clone();

    let agents_snapshot: Vec<(String, crate::runtime::connection::AgentProtocol)> = {
        let agents = app_state.agents.read().await;
        agents
            .iter()
            .map(|a| (a.id.clone(), a.protocol.clone()))
            .collect()
    };

    if agents_snapshot.is_empty() {
        return Err(CommandError::not_found("No agents configured"));
    }

    let mut dispatched = Vec::new();

    for (agent_id, protocol) in &agents_snapshot {
        // Skip agents that already have a turn in progress
        if app_state.turn_manager.is_active(agent_id).await {
            tracing::debug!(agent_id, "broadcast_message: turn already active, skipping");
            continue;
        }

        // Ensure worker is registered
        register_turn_worker(&app_state, &app_handle, agent_id).await;

        let turn_protocol = TurnProtocol::from_message(protocol, message.clone(), None, None);
        let request = TurnRequest::new(
            turn_protocol,
            TurnSource::BroadcastMessage,
            TurnPriority::User,
        );

        // Fire-and-forget: submit the turn, spawn a task to log errors
        if let Some(handle) = app_state.turn_manager.submit_turn(agent_id, request).await {
            let aid = agent_id.clone();
            let ah = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                if let TurnResult::Error(e) = handle.result().await {
                    tracing::warn!("Broadcast failed for agent {}: {}", aid, e);
                    let _ = ah.emit(
                        "agent-stream",
                        crate::providers::types::StreamEvent {
                            agent_id: aid,
                            event_type: crate::providers::types::StreamEventType::Error,
                            data: serde_json::json!({
                                "error": format!("Broadcast execution failed: {}", e)
                            }),
                        },
                    );
                }
            });
            dispatched.push(agent_id.clone());
        }
    }

    Ok(dispatched)
}
