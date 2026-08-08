//! Codex lane — persistent app-server integration via V2 JSON-RPC protocol.
//!
//! The main query flow uses `codex app-server` (persistent process, Unix socket,
//! NDJSON event stream). Code review uses one-shot `codex exec review`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use base64::Engine;
use chrono::Utc;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, Command};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::commands::error::CommandError;
use crate::providers::types::{StreamEvent, StreamEventType};
use crate::runtime::a2a::A2AMessage;
use crate::runtime::agent::{
    AgentSlot, AgentStatus, ContentBlock, ConversationMessage, ImageAttachment, MessageRole,
};
use crate::AppState;

use super::codex_events::{self, TypedCodexEvent};
use super::codex_server::{self, CodexEvent};
use super::codex_thread_ops;
use super::codex_types::*;
use super::connection::ConnectionConfig;

// =============================================================================
// Constants
// =============================================================================

pub const OPENAI_OAUTH_PROVIDER: &str = "openai";
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.5";
const CODEX_EXECUTABLE: &str = "codex";

// =============================================================================
// Public types
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexMode {
    Suggest,
    AutoEdit,
    FullAuto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexSessionControlAction {
    Continue,
    Review,
    Plan,
    Status,
}

pub fn default_credentials_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".codex/auth.json")
}

pub fn default_model() -> String {
    DEFAULT_CODEX_MODEL.to_string()
}

pub fn is_codex_provider(provider: &str) -> bool {
    provider.eq_ignore_ascii_case(OPENAI_OAUTH_PROVIDER)
}

pub fn is_codex_oauth(connection: &ConnectionConfig) -> bool {
    matches!(
        connection,
        ConnectionConfig::OAuth { provider, .. } if is_codex_provider(provider)
    )
}

// =============================================================================
// Error types
// =============================================================================

#[derive(Debug, thiserror::Error)]
pub enum CodexExecutorError {
    #[error("Agent not found: {0}")]
    AgentNotFound(String),
    #[error("Codex lane requires an OpenAI OAuth connection")]
    InvalidConfig,
    #[error("Codex is not installed or failed to start: {0}")]
    Spawn(String),
    #[error("Codex interrupted by user")]
    Interrupted,
    #[error("Codex network error: {0}")]
    Network(String),
    #[error("Codex execution failed: {0}")]
    Execution(String),
    #[error("Failed to read Codex output: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse Codex output: {0}")]
    Json(#[from] serde_json::Error),

    // --- New: app-server specific errors ---
    #[error("Codex app-server failed to start: {0}")]
    ServerSpawn(String),
    #[error("Codex app-server connection failed: {0}")]
    ServerConnection(String),
    #[error("Codex app-server died unexpectedly: {0}")]
    ServerDied(String),
    #[error("Codex rate limited (resets at {resets_at:?})")]
    RateLimited { resets_at: Option<i64> },
    #[error("Codex credits exhausted — no remaining balance")]
    CreditsExhausted,
    #[error("Thread fork failed: {0}")]
    ThreadForkFailed(String),
}

/// Decide whether a rate-limit state must block a new turn.
///
/// Only a true hard exhaustion (`limit_reached`) stops the turn. `has_credits ==
/// false` alone just means the account has no purchased *supplemental* credits —
/// the included plan usage may still be available, so it must NOT block. This was
/// the "Codex dies on the second turn" bug: the app-server reports
/// `credits.hasCredits = false` after the first turn, and the old preflight
/// wrongly treated that as account exhaustion (`CreditsExhausted`), so the second
/// prompt was rejected before it was ever sent.
fn classify_rate_limit_state(state: &codex_server::RateLimitState) -> Option<CodexExecutorError> {
    if state.limit_reached.is_some() {
        Some(CodexExecutorError::RateLimited {
            resets_at: state.resets_at,
        })
    } else {
        None
    }
}

// =============================================================================
// Main query flow (app-server)
// =============================================================================

/// Public entry point for Codex queries. Drains A2A messages and delegates
/// to `run_codex_agent`. Retries once on server connection failures (broken pipe,
/// server died) by killing the stale server and letting the next attempt respawn.
pub async fn codex_query(
    agent_id: &str,
    prompt: &str,
    images: Option<Vec<ImageAttachment>>,
    app_state: &Arc<AppState>,
    app_handle: Option<&AppHandle>,
    transient_user_message: Option<&str>,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<String, CommandError> {
    let a2a_messages = app_state.a2a_registry.drain_messages(agent_id).await;

    let result = run_codex_agent(
        &app_state.agents,
        agent_id,
        prompt,
        images.clone(),
        &a2a_messages,
        transient_user_message,
        app_handle,
        cancellation_token,
        Some(app_state),
        true,
    )
    .await;

    let is_retryable = matches!(
        &result,
        Err(CodexExecutorError::ServerConnection(_))
            | Err(CodexExecutorError::ServerDied(_))
            | Err(CodexExecutorError::ServerSpawn(_))
    );

    let final_result = if is_retryable {
        tracing::warn!(
            agent_id,
            error = %result.as_ref().unwrap_err(),
            "Codex server failed — shutting down and retrying once"
        );

        // Kill the stale server so the retry spawns fresh
        codex_server::shutdown_server(agent_id).await.ok();

        // Clear stale session ID since the thread is gone with the server
        {
            let mut agents = app_state.agents.write().await;
            if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
                agent.agent_session_id = None;
                agent.system_prompt_hash = None;
            }
        }

        // Retry once
        run_codex_agent(
            &app_state.agents,
            agent_id,
            prompt,
            images,
            &a2a_messages,
            transient_user_message,
            app_handle,
            cancellation_token,
            Some(app_state),
            true,
        )
        .await
    } else {
        result
    };

    // Error cleanup: `run_codex_agent` only flips status back to Idle on its
    // success path (step 13). Any earlier error — rate limit, thread/turn failure,
    // or an exhausted retry — leaves the agent stuck at `Working`, so the lane
    // looks permanently busy and the *next* turn can never be sent. Reset it here:
    // surface the error and free the lane so the user can retry. (Mirrors the
    // Claude runtime's error handling in agent_sdk.)
    if let Err(ref e) = final_result {
        let err_msg = e.to_string();
        {
            let mut agents = app_state.agents.write().await;
            if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
                agent.status = AgentStatus::Error(err_msg.clone());
                agent.is_dirty = true;
            }
        }
        emit_stream_event(
            app_handle,
            agent_id,
            StreamEventType::Error,
            serde_json::json!({ "message": err_msg }),
        );
    }

    final_result.map_err(CommandError::from)
}

/// Core Codex query flow using persistent app-server.
///
/// 1. Extract agent config + load persona
/// 2. Build developer instructions (persona + system_prompt + tool awareness)
/// 3. Compute persona hash (persona + system_prompt only, excludes tool awareness)
/// 4. Spawn or reuse app-server
/// 5. Check rate limits
/// 6. Thread management: fork on persona change, start fresh if no thread, reuse if unchanged
/// 7. Inject A2A messages as items
/// 8. Build user input and start turn
/// 9. Stream events until turn completes
/// 10. Update agent state
pub async fn run_codex_agent(
    agents: &Arc<tokio::sync::RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    prompt: &str,
    images: Option<Vec<ImageAttachment>>,
    a2a_messages: &[A2AMessage],
    transient_user_message: Option<&str>,
    app_handle: Option<&AppHandle>,
    cancellation_token: &tokio_util::sync::CancellationToken,
    app_state: Option<&Arc<AppState>>,
    enable_holt_tools: bool,
) -> Result<String, CodexExecutorError> {
    let effective_prompt = build_effective_prompt(prompt, transient_user_message, a2a_messages);

    // --- 1. Extract agent config ---
    let (
        working_directory,
        model,
        credentials_path,
        system_prompt,
        stored_system_prompt_hash,
        agent_session_id,
        inherited_context,
    ) = {
        let agents = agents.read().await;
        let agent = agents
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| CodexExecutorError::AgentNotFound(agent_id.to_string()))?;

        let (_provider, credentials_path, model) = match &agent.connection {
            ConnectionConfig::OAuth {
                provider,
                credentials_path,
                model,
            } if is_codex_provider(provider) => (provider, credentials_path.clone(), model.clone()),
            _ => return Err(CodexExecutorError::InvalidConfig),
        };

        (
            agent.working_directory.clone(),
            model,
            credentials_path,
            agent.system_prompt.clone(),
            agent.system_prompt_hash.clone(),
            agent.agent_session_id.clone(),
            build_inherited_context(agent),
        )
    };

    // --- 2. Load persona files ---
    // Static persona (identity files) and the dynamic memory profile are kept
    // separate: the profile's content is volatile (importance scores,
    // staleness labels, ranking order shift every turn as cognitive intake
    // writes new memories), so it must NOT feed the thread-reuse hash —
    // hashing it forced a brand-new codex thread on nearly every message
    // (full uncached prefill + MCP restart each time). Within a live thread,
    // memory freshness comes from the MCP memory tools (recall etc.).
    let memory_active = app_state.map(|s| s.has_memory_engine()).unwrap_or(false);
    let static_files = crate::runtime::persona::load_persona_files(agent_id, memory_active);
    let static_persona_prefix = crate::runtime::persona::format_as_system_prompt(&static_files);

    let persona_prefix = {
        let mut files = static_files;
        if memory_active {
            if let Some(state) = app_state {
                if let Some(engine) = state.get_memory_engine() {
                    if let Some(dynamic_profile) = crate::runtime::persona::fetch_dynamic_profile(
                        &engine,
                        agent_id,
                        crate::runtime::persona::DYNAMIC_PROFILE_TOKEN_BUDGET,
                    )
                    .await
                    {
                        tracing::info!(
                            agent_id,
                            "Dynamic memory profile injected into Codex persona"
                        );
                        files.push(dynamic_profile);
                    }
                }
            }
        }
        crate::runtime::persona::format_as_system_prompt(&files)
    };

    // --- 3. Build developer instructions ---
    // Base: persona prefix (incl. memory profile) + config system_prompt
    let base_instructions = if persona_prefix.is_empty() {
        system_prompt.clone()
    } else {
        format!("{}\n\n{}", persona_prefix, system_prompt)
    };

    // Hash only STATIC persona + system_prompt — the dynamic memory profile
    // and tool awareness are volatile and must not invalidate sessions.
    let hash_input = if static_persona_prefix.is_empty() {
        system_prompt.clone()
    } else {
        format!("{}\n\n{}", static_persona_prefix, system_prompt)
    };
    let persona_hash = system_prompt_hash(&hash_input);

    // Holt MCP mount for the app-server spawn. Resolved BEFORE tool
    // awareness so the two can never disagree: awareness text is only
    // injected when the mount is actually passed to the spawn — advertising
    // tools the server can't reach sends agents rooting through the
    // filesystem for fallbacks.
    let mcp_launch = if enable_holt_tools {
        match app_state {
            Some(state) => prepare_codex_mcp_launch(agent_id, state).await,
            None => None,
        }
    } else {
        None
    };

    // Full instructions include tool awareness (not hashed)
    let tool_awareness = match (&mcp_launch, app_state) {
        (Some(_), Some(state)) => build_codex_mcp_awareness(agent_id, state).await,
        _ => String::new(),
    };
    let developer_instructions = if tool_awareness.is_empty() {
        base_instructions
    } else {
        format!("{}\n\n{}", base_instructions, tool_awareness)
    };

    // --- 4. Determine thread state ---
    // Only consider persona "changed" if we had a stored hash AND it differs.
    // None means cold start (no prior hash stored) — not a persona change.
    let persona_hash_changed = stored_system_prompt_hash
        .as_deref()
        .map_or(false, |stored| stored != persona_hash);

    // Early return for empty prompts
    if effective_prompt.trim().is_empty() {
        let mut agents = agents.write().await;
        if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
            agent.status = AgentStatus::Idle;
            agent.metadata.last_active = Some(Utc::now());
            agent.is_dirty = true;
        }
        return Ok(String::new());
    }

    // --- 5. Update agent state: record user message ---
    {
        let mut agents = agents.write().await;
        if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
            agent.system_prompt_hash = Some(persona_hash.clone());
            agent.status = AgentStatus::Working;
            if agent.session_start.is_none() {
                agent.session_start = Some(Utc::now());
            }
            if !prompt.trim().is_empty() {
                let content_blocks = images.as_ref().map(|items| {
                    let mut blocks = vec![ContentBlock::Text {
                        text: prompt.to_string(),
                    }];
                    for image in items {
                        blocks.push(ContentBlock::Image {
                            data: image.data.clone(),
                            media_type: image.media_type.clone(),
                        });
                    }
                    blocks
                });
                agent.add_message(ConversationMessage {
                    role: MessageRole::User,
                    content: prompt.to_string(),
                    content_blocks,
                    timestamp: Utc::now(),
                    token_estimate: Some(prompt.len() as u32 / 4),
                    tool_calls: None,
                    tool_call_id: None,
                    outcome_tag: None,
                    thinking_content: None,
                    responses_phase: None,
                    reasoning_items: None,
                });
            } else {
                agent.is_dirty = true;
            }
        }
    }

    // --- 6. Spawn or reuse app-server ---
    let codex_home = credentials_path.parent().map(Path::to_path_buf);
    let mut events_rx = codex_server::get_or_spawn_server(
        agent_id,
        codex_home.as_deref(),
        &working_directory,
        mcp_launch.as_ref(),
    )
    .await
    .map_err(|e| CodexExecutorError::ServerSpawn(e.to_string()))?;

    // Emit bridge status
    emit_stream_event(
        app_handle,
        agent_id,
        StreamEventType::SandboxStatus,
        serde_json::json!({
            "bridge_kind": "codex_v2",
            "transport": "app_server_stdio",
        }),
    );

    // --- 7. Check rate limits ---
    // Only a hard `limit_reached` blocks the turn; `has_credits == false` alone
    // (no purchased supplemental credits) must NOT — see classify_rate_limit_state.
    if let Some(state) = codex_server::get_rate_limit_state(agent_id).await {
        if let Some(err) = classify_rate_limit_state(&state) {
            return Err(err);
        }
    }

    // --- 8. Thread management: fork / start / reuse ---
    //
    // Thread IDs are only valid for the lifetime of the app-server process.
    // On app restart, stale session IDs will fail — we fall back to fresh start.
    let thread_id = match (&agent_session_id, persona_hash_changed) {
        // Persona changed + existing thread → try fork, fall back to fresh start
        (Some(existing_tid), true) => {
            tracing::info!(
                agent_id,
                existing_thread = %existing_tid,
                "Persona hash changed — attempting thread fork to preserve history"
            );
            match codex_thread_ops::thread_fork(
                agent_id,
                ThreadForkParams {
                    thread_id: existing_tid.clone(),
                    developer_instructions: Some(developer_instructions.clone()),
                    approval_policy: Some(FULL_TRUST_APPROVAL_POLICY),
                    sandbox: Some(FULL_TRUST_SANDBOX),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(fork_resp) => fork_resp.thread.id,
                Err(e) => {
                    tracing::warn!(
                        agent_id,
                        error = %e,
                        "Thread fork failed (stale session?) — starting fresh thread"
                    );
                    start_fresh_thread(
                        agent_id,
                        &model,
                        &developer_instructions,
                        &working_directory,
                        prompt,
                        &inherited_context,
                    )
                    .await?
                }
            }
        }

        // No existing thread → start fresh
        (None, _) => {
            start_fresh_thread(
                agent_id,
                &model,
                &developer_instructions,
                &working_directory,
                prompt,
                &inherited_context,
            )
            .await?
        }

        // Existing thread, persona unchanged → try reuse, fall back to fresh start
        (Some(existing_tid), false) => {
            // Resume loads the thread into THIS app-server process. Metadata
            // reads (e.g. thread/goal/get) are NOT sufficient validation:
            // they pass for threads persisted on disk from a prior process,
            // but turn/start requires a loaded thread and rejects them with
            // "thread not found". Resume validates + loads in one call, is
            // idempotent on already-loaded threads (~1ms), and carries
            // conversation history across app restarts.
            match codex_thread_ops::thread_resume(
                agent_id,
                ThreadResumeParams {
                    thread_id: existing_tid.clone(),
                    approval_policy: Some(FULL_TRUST_APPROVAL_POLICY),
                    sandbox: Some(FULL_TRUST_SANDBOX),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => existing_tid.clone(),
                Err(e) => {
                    tracing::warn!(
                        agent_id,
                        thread_id = %existing_tid,
                        error = %e,
                        "Stale thread ID — starting fresh thread"
                    );
                    start_fresh_thread(
                        agent_id,
                        &model,
                        &developer_instructions,
                        &working_directory,
                        prompt,
                        &inherited_context,
                    )
                    .await?
                }
            }
        }
    };

    // --- 9. Inject A2A messages as items ---
    if !a2a_messages.is_empty() {
        let a2a_text = format_a2a_as_text(a2a_messages);
        let items = vec![serde_json::json!({
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": a2a_text,
            }]
        })];
        codex_thread_ops::thread_inject_items(agent_id, &thread_id, items)
            .await
            .ok();
    }

    // --- 10. Build user input and start turn ---
    let input = build_user_input(&effective_prompt, images.as_deref())?;

    let turn_resp = codex_thread_ops::turn_start(
        agent_id,
        TurnStartParams {
            thread_id: thread_id.clone(),
            input,
            model: None,
            effort: None,
            approval_policy: None,
            cwd: None,
            output_schema: None,
            personality: None,
            service_tier: None,
            summary: None,
        },
    )
    .await
    .map_err(|e| CodexExecutorError::Execution(e.to_string()))?;

    // Our turn's id, straight from the turn/start response. The event loop
    // must only react to THIS turn — other turns (e.g. compaction) also emit
    // turn/completed on the same thread.
    let started_turn_id = turn_resp
        .turn
        .get("id")
        .and_then(|v| v.as_str())
        .map(String::from);

    // --- 11. Stream events until turn completes ---
    let final_response = stream_codex_events(
        agent_id,
        &thread_id,
        started_turn_id,
        &mut events_rx,
        app_handle,
        cancellation_token,
    )
    .await?;

    // --- 12. Memory pipeline (background, non-blocking) ---
    if let Some(state) = app_state {
        // Cognitive intake — capture turn content to Hillock
        if let Some(mem_engine) = state.get_memory_engine() {
            let intake_state = {
                let agents_r = agents.read().await;
                agents_r
                    .iter()
                    .find(|a| a.id == agent_id)
                    .map(|a| a.cognitive_intake.clone())
            };
            if let Some(mut intake) = intake_state {
                let embedder = Arc::clone(&mem_engine.embedder);
                let engine = Arc::clone(&mem_engine);
                let agent_id_owned = agent_id.to_string();
                let prompt_owned = effective_prompt.clone();
                let response_owned = final_response.clone();
                let agents_ci = Arc::clone(agents);
                tauri::async_runtime::spawn(async move {
                    let result = crate::runtime::cognitive_intake::process_turn(
                        &mut intake,
                        &embedder,
                        &engine,
                        &agent_id_owned,
                        &prompt_owned,
                        &response_owned,
                    )
                    .await;

                    // Write back updated intake state
                    {
                        let mut agents_w = agents_ci.write().await;
                        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
                            agent.cognitive_intake = intake;
                        }
                    }

                    match result {
                        crate::runtime::cognitive_intake::IntakeResult::Forwarded { memory_id } => {
                            tracing::debug!(
                                agent_id = agent_id_owned,
                                memory_id = %memory_id,
                                "Codex cognitive intake forwarded"
                            );
                        }
                        crate::runtime::cognitive_intake::IntakeResult::HillockError(e) => {
                            tracing::warn!(
                                agent_id = agent_id_owned,
                                error = %e,
                                "Codex cognitive intake error"
                            );
                        }
                        _ => {} // Disabled, TooShort, NoveltyRejected — all fine
                    }
                });
            }
        }

        // Session memory extraction — check if token threshold crossed
        let should_extract = {
            let agents_r = agents.read().await;
            agents_r
                .iter()
                .find(|a| a.id == agent_id)
                .is_some_and(|agent| {
                    let sm_config = crate::runtime::session_memory::SessionMemoryConfig::default();
                    agent
                        .session_memory
                        .should_trigger(&sm_config, agent.current_context_tokens)
                        && agent.agent_session_id.is_some()
                        && !agent.session_memory.extraction_in_progress
                })
        };

        if should_extract {
            let sm_ids = {
                let mut agents_w = agents.write().await;
                agents_w
                    .iter_mut()
                    .find(|a| a.id == agent_id)
                    .and_then(|agent| {
                        agent.session_memory.extraction_in_progress = true;
                        let sid = agent.agent_session_id.clone()?;
                        Some((agent.id.clone(), sid, agent.current_context_tokens))
                    })
            };

            if let Some((sm_agent_id, sm_sid, sm_tokens)) = sm_ids {
                crate::runtime::session_memory::spawn_session_memory_fork(
                    state,
                    &sm_agent_id,
                    &sm_sid,
                    sm_tokens,
                    app_handle,
                );
            }
        }
    }

    // --- 13. Update agent state ---
    {
        let mut agents = agents.write().await;
        if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
            agent.agent_session_id = Some(thread_id.clone());
            agent.status = AgentStatus::Idle;
            agent.metadata.total_api_calls += 1;
            agent.metadata.last_active = Some(Utc::now());
            if !final_response.is_empty() {
                agent.add_message(ConversationMessage {
                    role: MessageRole::Assistant,
                    content: final_response.clone(),
                    content_blocks: None,
                    timestamp: Utc::now(),
                    token_estimate: Some(final_response.len() as u32 / 4),
                    tool_calls: None,
                    tool_call_id: None,
                    outcome_tag: None,
                    thinking_content: None,
                    responses_phase: None,
                    reasoning_items: None,
                });
            } else {
                agent.is_dirty = true;
            }
        }
    }

    emit_stream_event(
        app_handle,
        agent_id,
        StreamEventType::ConversationUpdated,
        serde_json::json!({}),
    );
    emit_stream_event(
        app_handle,
        agent_id,
        StreamEventType::Done,
        // Include the final text so the frontend has a fallback if any
        // streamed deltas were missed.
        serde_json::json!({ "session_id": thread_id, "text": final_response }),
    );

    Ok(final_response)
}

/// Start a fresh thread with all the standard initialization steps:
/// memory mode disabled, name set, inherited context injected.
async fn start_fresh_thread(
    agent_id: &str,
    model: &str,
    developer_instructions: &str,
    working_directory: &Path,
    prompt: &str,
    inherited_context: &Option<String>,
) -> Result<String, CodexExecutorError> {
    let start_resp = codex_thread_ops::thread_start(
        agent_id,
        ThreadStartParams {
            model: Some(model.to_string()),
            developer_instructions: Some(developer_instructions.to_string()),
            approval_policy: Some(FULL_TRUST_APPROVAL_POLICY),
            sandbox: Some(FULL_TRUST_SANDBOX),
            cwd: Some(working_directory.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| CodexExecutorError::ServerConnection(e.to_string()))?;

    let new_tid = start_resp.thread.id.clone();

    // Disable Codex native memory — Hillock manages ours
    codex_thread_ops::thread_memory_mode_set(agent_id, &new_tid, ThreadMemoryMode::Disabled)
        .await
        .ok();

    // Set thread name from prompt preview
    let name = truncate_for_name(prompt);
    codex_thread_ops::thread_set_name(agent_id, &new_tid, &name)
        .await
        .ok();

    // Inject inherited context for new threads replacing an invalidated session
    if let Some(context) = inherited_context {
        let items = vec![serde_json::json!({
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": format!(
                    "[Inherited Holt context]\n\
                    Use the following context as background for this fresh session.\n\n\
                    {context}"
                ),
            }]
        })];
        codex_thread_ops::thread_inject_items(agent_id, &new_tid, items)
            .await
            .ok();
    }

    Ok(new_tid)
}

/// Stream events from the app-server until OUR turn completes or the server
/// disconnects.
///
/// `expected_turn_id` comes from the turn/start response. Compaction (and any
/// other concurrent turn) emits its own turn/started + turn/completed on the
/// same thread — without id filtering, a compaction finishing would end our
/// turn with whatever text had accumulated (usually nothing).
async fn stream_codex_events(
    agent_id: &str,
    thread_id: &str,
    expected_turn_id: Option<String>,
    events_rx: &mut tokio::sync::broadcast::Receiver<CodexEvent>,
    app_handle: Option<&AppHandle>,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<String, CodexExecutorError> {
    let mut final_response = String::new();
    let mut current_turn_id: Option<String> = expected_turn_id;

    loop {
        tokio::select! {
            _ = cancellation_token.cancelled() => {
                if let Some(turn_id) = &current_turn_id {
                    codex_thread_ops::turn_interrupt(agent_id, thread_id, turn_id)
                        .await
                        .ok();
                }
                return Err(CodexExecutorError::Interrupted);
            }
            event = events_rx.recv() => {
                match event {
                    Ok(CodexEvent::Notification { method, params }) => {
                        let typed = codex_events::parse_notification(&method, params);
                        match handle_typed_event(
                            typed,
                            agent_id,
                            thread_id,
                            &mut final_response,
                            &mut current_turn_id,
                            app_handle,
                        ).await? {
                            EventAction::Continue => {}
                            EventAction::TurnComplete => break,
                        }
                    }
                    Ok(CodexEvent::ServerRequest { method, .. }) => {
                        // Transport layer already answered (approvals declined —
                        // we run with ApprovalPolicy::Never). Informational only.
                        tracing::warn!(agent_id, method, "Codex server request auto-answered");
                    }
                    Ok(CodexEvent::ParseError { line, error }) => {
                        tracing::warn!(
                            agent_id,
                            line,
                            error,
                            "Unparseable NDJSON line from app-server"
                        );
                    }
                    Ok(CodexEvent::Disconnected) => {
                        return Err(CodexExecutorError::ServerDied(
                            "app-server disconnected during turn".to_string(),
                        ));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Missed some events due to slow consumption — log but continue
                        tracing::warn!(agent_id, missed = n, "Event receiver lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err(CodexExecutorError::ServerDied(
                            "app-server event channel closed".to_string(),
                        ));
                    }
                }
            }
        }
    }

    Ok(final_response)
}

enum EventAction {
    Continue,
    TurnComplete,
}

/// Handle a single typed event from the app-server.
///
/// Turn-scoped events (deltas, item completions, turn/completed) are filtered
/// against `current_turn_id` — concurrent turns on the same thread (e.g.
/// compaction) must not contribute text or end our turn.
async fn handle_typed_event(
    event: TypedCodexEvent,
    agent_id: &str,
    thread_id: &str,
    final_response: &mut String,
    current_turn_id: &mut Option<String>,
    app_handle: Option<&AppHandle>,
) -> Result<EventAction, CodexExecutorError> {
    // A turn-scoped event belongs to us if we don't know our turn id yet
    // (defensive: turn/start response lacked one) or the ids match.
    let is_our_turn = |event_turn_id: Option<&str>| -> bool {
        match (current_turn_id.as_deref(), event_turn_id) {
            (Some(ours), Some(theirs)) => ours == theirs,
            _ => true,
        }
    };

    match &event {
        TypedCodexEvent::TurnStarted(n) => {
            // Only adopt the id if we don't have one from the turn/start
            // response — a compaction turn starting must not hijack it.
            if current_turn_id.is_none() {
                if let Some(turn_id) = n.turn.get("id").and_then(|v| v.as_str()) {
                    *current_turn_id = Some(turn_id.to_string());
                }
            }
        }

        TypedCodexEvent::AgentMessageDelta(n) => {
            if is_our_turn(Some(&n.turn_id)) {
                final_response.push_str(&n.delta);
            }
        }

        TypedCodexEvent::TurnCompleted(n) => {
            let completed_id = n.turn.get("id").and_then(|v| v.as_str());
            if is_our_turn(completed_id) {
                return Ok(EventAction::TurnComplete);
            }
            tracing::debug!(
                agent_id,
                completed = completed_id.unwrap_or("?"),
                "Ignoring turn/completed from a different turn (e.g. compaction)"
            );
        }

        TypedCodexEvent::TokenUsageUpdated(n) => {
            // Compaction is managed entirely by Codex — we only observe.
            // (Triggering thread/compact from here started a concurrent turn
            // whose turn/completed ended ours with an empty reply.)
            tracing::debug!(
                agent_id,
                thread_id,
                last_input = n.token_usage.last.input_tokens,
                window = ?n.token_usage.model_context_window,
                "Token usage updated"
            );
        }

        TypedCodexEvent::GoalUpdated(n) => {
            let status_str = match &n.goal.status {
                ThreadGoalStatus::Active => "active",
                ThreadGoalStatus::Complete => "complete",
                ThreadGoalStatus::BudgetLimited => "budget_limited",
                _ => "unknown",
            };
            emit_stream_event(
                app_handle,
                agent_id,
                StreamEventType::GoalUpdate,
                serde_json::json!({
                    "status": status_str,
                    "objective": n.goal.objective,
                    "token_budget": n.goal.token_budget,
                    "tokens_used": n.goal.tokens_used,
                }),
            );
            if matches!(
                &n.goal.status,
                ThreadGoalStatus::BudgetLimited | ThreadGoalStatus::Complete
            ) {
                tracing::info!(
                    agent_id,
                    status = status_str,
                    tokens_used = n.goal.tokens_used,
                    "Goal status changed"
                );
            }
        }

        TypedCodexEvent::RateLimitUpdated(n) => {
            // Rate limit state is tracked by transport layer, but also emit to frontend
            let used_pct = n
                .rate_limits
                .primary
                .as_ref()
                .map(|w| w.used_percent)
                .unwrap_or(0);
            let resets_at = n.rate_limits.primary.as_ref().and_then(|w| w.resets_at);
            let has_credits = n
                .rate_limits
                .credits
                .as_ref()
                .map(|c| c.has_credits)
                .unwrap_or(true);
            let limit_reached = n.rate_limits.rate_limit_reached_type.is_some();

            emit_stream_event(
                app_handle,
                agent_id,
                StreamEventType::RateLimitUpdate,
                serde_json::json!({
                    "used_pct": used_pct,
                    "resets_at": resets_at,
                    "has_credits": has_credits,
                    "limit_reached": limit_reached,
                    "limit_type": n.rate_limits.rate_limit_reached_type,
                    "plan_type": n.rate_limits.plan_type,
                }),
            );
        }

        TypedCodexEvent::ApprovalRequested(n) => {
            // Policy is "never", but handle edge cases by auto-approving
            tracing::debug!(
                agent_id,
                review_id = %n.review_id,
                "Auto-approving guardian action"
            );
            codex_thread_ops::thread_approve_action(
                agent_id,
                thread_id,
                serde_json::json!({
                    "reviewId": n.review_id,
                    "decision": "approved",
                }),
            )
            .await
            .ok();
        }

        TypedCodexEvent::ItemCompleted(n) => {
            // The agentMessage item carries the authoritative full reply —
            // turn/completed ships an empty items list, and streamed deltas
            // can be dropped under broadcast lag. Prefer the full text.
            if is_our_turn(n.turn_id.as_deref()) {
                if let ThreadItem::AgentMessage { text, .. } = &n.item {
                    *final_response = text.clone();
                }
            }
        }

        TypedCodexEvent::Error(n) => {
            let message = &n.error.message;
            if n.will_retry {
                tracing::warn!(agent_id, message, "Codex error (will retry)");
                // Don't break — let the server retry
            } else {
                return Err(classify_codex_event_error(message));
            }
        }

        _ => {
            // All other events: handled below via to_stream_event
        }
    }

    // Forward displayable events to frontend
    if let Some(stream_event) = codex_events::to_stream_event(&event, agent_id) {
        if let Some(handle) = app_handle {
            let _ = handle.emit("agent-stream", stream_event);
        }
    }

    Ok(EventAction::Continue)
}

// =============================================================================
// Code review (one-shot `codex exec review`)
// =============================================================================

/// One-shot code review via `codex exec review`. Not part of the app-server flow.
pub async fn codex_review(
    agents: &Arc<RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    instructions: Option<&str>,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<String, CodexExecutorError> {
    let (working_directory, model, credentials_path, developer_instructions) = {
        let agents = agents.read().await;
        let agent = agents
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| CodexExecutorError::AgentNotFound(agent_id.to_string()))?;

        let (_, credentials_path, model) = match &agent.connection {
            ConnectionConfig::OAuth {
                provider,
                credentials_path,
                model,
            } if is_codex_provider(provider) => (provider, credentials_path.clone(), model.clone()),
            _ => return Err(CodexExecutorError::InvalidConfig),
        };

        (
            agent.working_directory.clone(),
            model,
            credentials_path,
            agent.system_prompt.clone(),
        )
    };

    let temp_dir = tempfile::tempdir()?;
    let output_path = temp_dir.path().join("codex-review.txt");
    let spec = build_review_command_spec(
        &model,
        &credentials_path,
        &developer_instructions,
        instructions,
        &output_path,
    );
    let result =
        run_codex_command(&working_directory, &spec, &output_path, cancellation_token).await?;
    Ok(result.final_response)
}

// =============================================================================
// One-shot command infrastructure (kept for codex_review)
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReviewCommandSpec {
    args: Vec<String>,
    codex_home: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ParsedCodexOutput {
    thread_id: Option<String>,
    error_messages: Vec<String>,
}

#[derive(Debug)]
struct CodexProcessResult {
    final_response: String,
    #[allow(dead_code)]
    thread_id: Option<String>,
}

fn build_review_command_spec(
    model: &str,
    credentials_path: &Path,
    developer_instructions: &str,
    instructions: Option<&str>,
    output_path: &Path,
) -> ReviewCommandSpec {
    let mut args = vec![
        "exec".to_string(),
        "review".to_string(),
        "--json".to_string(),
        "--uncommitted".to_string(),
        "--skip-git-repo-check".to_string(),
        "-m".to_string(),
        model.to_string(),
        "-o".to_string(),
        output_path.to_string_lossy().into_owned(),
        "-c".to_string(),
        format!(
            "developer_instructions={}",
            toml::Value::String(developer_instructions.to_string())
        ),
    ];

    if let Some(instructions) = instructions {
        if !instructions.trim().is_empty() {
            args.push(instructions.trim().to_string());
        }
    }

    let codex_home = credentials_path.parent().map(Path::to_path_buf);

    ReviewCommandSpec { args, codex_home }
}

async fn run_codex_command(
    working_directory: &Path,
    spec: &ReviewCommandSpec,
    output_path: &Path,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<CodexProcessResult, CodexExecutorError> {
    let mut command = Command::new(CODEX_EXECUTABLE);
    command
        .args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(working_directory)
        .kill_on_drop(true);

    if let Some(codex_home) = &spec.codex_home {
        command.env("CODEX_HOME", codex_home);
    }

    let mut child = command.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            CodexExecutorError::Spawn("`codex` was not found on PATH".to_string())
        } else {
            CodexExecutorError::Spawn(err.to_string())
        }
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CodexExecutorError::Execution("failed to capture Codex stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| CodexExecutorError::Execution("failed to capture Codex stderr".into()))?;

    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut parsed = ParsedCodexOutput::default();
        loop {
            let mut line = String::new();
            let bytes_read = reader.read_line(&mut line).await?;
            if bytes_read == 0 {
                break;
            }
            parse_codex_stdout_line(&line, &mut parsed)?;
        }
        Result::<ParsedCodexOutput, CodexExecutorError>::Ok(parsed)
    });
    let stderr_task = spawn_codex_stderr_task(stderr);

    let status = tokio::select! {
        _ = cancellation_token.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(CodexExecutorError::Interrupted);
        }
        status = child.wait() => status?,
    };

    let parsed = join_codex_task(stdout_task).await?;
    let stderr_lines = join_codex_task(stderr_task).await?;
    let final_response = match std::fs::read_to_string(output_path) {
        Ok(content) => content.trim().to_string(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(CodexExecutorError::Io(err)),
    };

    if !status.success() {
        let mut details = parsed.error_messages;
        details.extend(stderr_lines);
        let message = details
            .into_iter()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(classify_codex_failure(if message.is_empty() {
            format!("Codex exited with status {status}")
        } else {
            message
        }));
    }

    Ok(CodexProcessResult {
        final_response,
        thread_id: parsed.thread_id,
    })
}

fn parse_codex_stdout_line(
    line: &str,
    parsed: &mut ParsedCodexOutput,
) -> Result<(), CodexExecutorError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return Ok(());
    };
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("thread.started") => {
            parsed.thread_id = value
                .get("thread_id")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
        }
        Some("error") => {
            if let Some(message) = value.get("message").and_then(serde_json::Value::as_str) {
                parsed.error_messages.push(message.to_string());
            }
        }
        _ => {}
    }

    Ok(())
}

fn spawn_codex_stderr_task(
    stderr: ChildStderr,
) -> JoinHandle<Result<Vec<String>, CodexExecutorError>> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            let bytes_read = reader.read_line(&mut line).await?;
            if bytes_read == 0 {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
            }
        }
        Ok(lines)
    })
}

async fn join_codex_task<T>(
    handle: JoinHandle<Result<T, CodexExecutorError>>,
) -> Result<T, CodexExecutorError> {
    handle
        .await
        .map_err(|err| CodexExecutorError::Execution(format!("Codex task join failed: {err}")))?
}

// =============================================================================
// Helper functions
// =============================================================================

/// Build user input for a turn: text prompt + optional images.
fn build_user_input(
    prompt: &str,
    images: Option<&[ImageAttachment]>,
) -> Result<Vec<UserInput>, CodexExecutorError> {
    let mut input = vec![UserInput::Text {
        text: prompt.to_string(),
        text_elements: Vec::new(),
    }];

    if let Some(images) = images {
        // Write images to temp files and pass as local image paths.
        // The temp directory must outlive the turn_start call.
        let temp_dir = tempfile::tempdir()?;
        let paths = write_image_attachments(temp_dir.path(), images)?;
        for path in paths {
            input.push(UserInput::LocalImage {
                path: path.to_string_lossy().into_owned(),
                detail: None,
            });
        }
        // Leak the temp dir intentionally — the app-server needs the files
        // to persist until it reads them during turn processing.
        // They'll be cleaned up on next turn or process exit.
        std::mem::forget(temp_dir);
    }

    Ok(input)
}

/// Format A2A messages as injectable text (for thread_inject_items).
fn format_a2a_as_text(a2a_messages: &[A2AMessage]) -> String {
    let a2a_section =
        crate::runtime::engine::turn_injection::format_a2a_system_section(a2a_messages);
    format!(
        "[SYSTEM CONTEXT -- NOT FROM HUMAN]\n{}\n[END SYSTEM CONTEXT]",
        a2a_section
    )
}

/// Build the effective prompt from user input, transient messages, and A2A.
///
/// For the app-server flow, A2A messages are injected as items (not in the prompt),
/// so this only combines the human prompt with transient messages.
fn build_effective_prompt(
    prompt: &str,
    transient_user_message: Option<&str>,
    _a2a_messages: &[A2AMessage],
) -> String {
    let mut parts = Vec::new();

    if let Some(transient) = transient_user_message {
        if !transient.trim().is_empty() {
            parts.push(transient.trim().to_string());
        }
    }

    if !prompt.trim().is_empty() {
        parts.push(prompt.to_string());
    }

    parts.join("\n\n")
}

/// Truncate a prompt to a reasonable thread name length.
fn truncate_for_name(prompt: &str) -> String {
    let trimmed = prompt.trim();
    if trimmed.len() <= 80 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..77])
    }
}

/// Build the Holt MCP mount for a codex app-server spawn.
///
/// Returns `None` when the Holt MCP server hasn't published its URL yet
/// (it binds at app startup, so this is effectively never during normal
/// operation). The tool list mirrors what `build_codex_mcp_awareness`
/// advertises — every advertised tool gets `approval_mode = "approve"`.
async fn prepare_codex_mcp_launch(
    agent_id: &str,
    app_state: &Arc<AppState>,
) -> Option<codex_server::CodexMcpLaunch> {
    let base_url = app_state.codex_mcp_url.read().await.clone()?;
    let separator = if base_url.contains('?') { '&' } else { '?' };
    let tools_config = load_agent_tools_config(agent_id);
    let registry = crate::tools::registry::ToolRegistry::build_for_codex_agent_with_tools(
        &tools_config,
        Some(&app_state.plugin_host),
    )
    .await;
    Some(codex_server::CodexMcpLaunch {
        holt_url: format!(
            "{}{}agent_id={}",
            base_url,
            separator,
            urlencoding::encode(agent_id)
        ),
        auto_approve_tools: registry
            .inventory()
            .into_iter()
            .map(|tool| tool.name)
            .collect(),
    })
}

/// Build tool awareness for MCP-connected Codex agents.
async fn build_codex_mcp_awareness(agent_id: &str, app_state: &Arc<AppState>) -> String {
    let tools_config = load_agent_tools_config(agent_id);
    let registry = crate::tools::registry::ToolRegistry::build_for_codex_agent_with_tools(
        &tools_config,
        Some(&app_state.plugin_host),
    )
    .await;
    let holt_tools = registry.awareness_block(
        "You have Holt-native tools mounted through Codex MCP. Use the Holt MCP tools as they appear in your active tool schema for Holt collaboration, persona, drafts, watches, orchestration, and agent introspection:",
    );
    format!(
        r#"{holt_tools}

Hillock is also mounted through Codex MCP for direct long-term memory operations.
Use Codex built-in tools for code, shell, filesystem, git, and patch work. Use MCP tools only for host-managed Holt or memory capabilities."#
    )
}

fn load_agent_tools_config(agent_id: &str) -> crate::config::agent_config::AgentToolsBlock {
    let config_path = crate::config::app_config::agent_dir(agent_id).join("config.toml");
    if config_path.exists() {
        crate::config::agent_config::AgentConfigFile::load(&config_path)
            .map(|cfg| cfg.tools)
            .unwrap_or_default()
    } else {
        crate::config::agent_config::AgentToolsBlock::default()
    }
}

fn build_inherited_context(agent: &AgentSlot) -> Option<String> {
    if agent.conversation.messages.is_empty() {
        return None;
    }

    let summary = crate::runtime::context::rule_based_summary(
        &agent.conversation.messages,
        &agent.working_directory.to_string_lossy(),
    );
    let recent_transcript = render_recent_transcript(&agent.conversation.messages, 8, 400);

    Some(format!(
        "{summary}\n\n### Recent Transcript Excerpts\n{recent_transcript}"
    ))
}

fn render_recent_transcript(
    messages: &[ConversationMessage],
    limit: usize,
    max_chars_per_message: usize,
) -> String {
    let start = messages.len().saturating_sub(limit);
    messages[start..]
        .iter()
        .map(|message| {
            let role = match message.role {
                MessageRole::System => "System",
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
                MessageRole::Tool => "Tool",
            };
            let content = if message.content.chars().count() > max_chars_per_message {
                format!(
                    "{}...",
                    message
                        .content
                        .chars()
                        .take(max_chars_per_message)
                        .collect::<String>()
                )
            } else {
                message.content.clone()
            };
            format!("- {role}: {content}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_image_attachments(
    temp_dir: &Path,
    images: &[ImageAttachment],
) -> Result<Vec<PathBuf>, CodexExecutorError> {
    let mut paths = Vec::with_capacity(images.len());

    for (index, image) in images.iter().enumerate() {
        let extension = image_extension(&image.media_type);
        let path = temp_dir.join(format!("image-{index}.{extension}"));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .map_err(|err| {
                CodexExecutorError::Execution(format!("invalid image payload: {err}"))
            })?;
        std::fs::write(&path, bytes)?;
        paths.push(path);
    }

    Ok(paths)
}

fn image_extension(media_type: &str) -> &'static str {
    match media_type {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    }
}

// =============================================================================
// Error classification
// =============================================================================

fn classify_codex_failure(message: String) -> CodexExecutorError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("api.openai.com")
        || lower.contains("responses")
        || lower.contains("websocket")
        || lower.contains("stream disconnected")
        || lower.contains("error sending request")
        || lower.contains("timed out")
    {
        CodexExecutorError::Network(message)
    } else {
        CodexExecutorError::Execution(message)
    }
}

fn classify_codex_event_error(message: &str) -> CodexExecutorError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("rate limit") || lower.contains("429") {
        CodexExecutorError::RateLimited { resets_at: None }
    } else if lower.contains("api.openai.com")
        || lower.contains("network")
        || lower.contains("timed out")
    {
        CodexExecutorError::Network(message.to_string())
    } else {
        CodexExecutorError::Execution(message.to_string())
    }
}

// =============================================================================
// Hashing & event emission
// =============================================================================

/// Deterministic hash for session dedup. Uses FNV-1a which is stable across
/// Rust versions and process restarts (unlike DefaultHasher which uses a
/// random per-process seed). Not cryptographic — only used for cache keys.
///
/// IMPORTANT: Hash only persona + system_prompt, NOT tool awareness. This prevents
/// unnecessary thread invalidation when plugins change but persona is unchanged.
fn system_prompt_hash(system_prompt: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for byte in system_prompt.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV-1a prime
    }
    format!("{:016x}", hash)
}

fn emit_stream_event(
    app_handle: Option<&AppHandle>,
    agent_id: &str,
    event_type: StreamEventType,
    data: serde_json::Value,
) {
    if let Some(handle) = app_handle {
        let _ = handle.emit(
            "agent-stream",
            StreamEvent {
                agent_id: agent_id.to_string(),
                event_type,
                data,
            },
        );
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Regression: "Codex dies on the second turn." After the first turn the
    // app-server reports `has_credits == false` (no purchased supplemental
    // credits) but NO hard `limit_reached`. The preflight MUST continue —
    // included plan usage may still be available.
    #[test]
    fn rate_limit_no_credits_without_reached_type_continues() {
        let state = codex_server::RateLimitState {
            has_credits: false,
            limit_reached: None,
            ..Default::default()
        };
        assert!(
            classify_rate_limit_state(&state).is_none(),
            "has_credits==false without limit_reached must NOT block the turn"
        );
    }

    // A true hard exhaustion (`limit_reached`) must still stop the turn, carrying
    // the reset time through.
    #[test]
    fn rate_limit_reached_stops_with_resets_at() {
        let state = codex_server::RateLimitState {
            has_credits: false,
            limit_reached: Some(RateLimitReachedType::RateLimitReached),
            resets_at: Some(1_234_567),
            ..Default::default()
        };
        assert!(matches!(
            classify_rate_limit_state(&state),
            Some(CodexExecutorError::RateLimited {
                resets_at: Some(1_234_567)
            })
        ));
    }

    #[test]
    fn test_default_credentials_path_points_to_codex_auth() {
        let path = default_credentials_path();
        assert!(path.to_string_lossy().contains(".codex/auth.json"));
    }

    #[test]
    fn test_is_codex_provider_is_openai_specific() {
        assert!(is_codex_provider("openai"));
        assert!(is_codex_provider("OpenAI"));
        assert!(!is_codex_provider("anthropic"));
    }

    #[test]
    fn test_build_review_command_spec_uses_codex_review_subcommand() {
        let spec = build_review_command_spec(
            "gpt-5.5",
            Path::new("/home/test/.codex/auth.json"),
            "system prompt",
            Some("focus on regressions"),
            Path::new("/tmp/review.txt"),
        );

        assert_eq!(spec.args[0], "exec");
        assert_eq!(spec.args[1], "review");
        assert!(spec.args.contains(&"--uncommitted".to_string()));
        assert!(spec.args.contains(&"--json".to_string()));
        assert!(spec.args.contains(&"focus on regressions".to_string()));
        assert_eq!(
            spec.args.last().map(String::as_str),
            Some("focus on regressions")
        );
        assert_eq!(spec.codex_home, Some(PathBuf::from("/home/test/.codex")));
    }

    #[test]
    fn test_build_effective_prompt_combines_transient_and_prompt() {
        let prompt = build_effective_prompt("real prompt", Some("[Wake notice]"), &[]);
        assert!(prompt.contains("[Wake notice]"));
        assert!(prompt.contains("real prompt"));
    }

    #[test]
    fn test_build_effective_prompt_skips_empty_transient() {
        let prompt = build_effective_prompt("real prompt", Some("   "), &[]);
        assert_eq!(prompt, "real prompt");
    }

    #[test]
    fn test_parse_codex_stdout_line_captures_thread_and_error() {
        let mut parsed = ParsedCodexOutput::default();
        parse_codex_stdout_line(
            r#"{"type":"thread.started","thread_id":"thread-abc"}"#,
            &mut parsed,
        )
        .unwrap();
        parse_codex_stdout_line(r#"{"type":"error","message":"network down"}"#, &mut parsed)
            .unwrap();

        assert_eq!(parsed.thread_id.as_deref(), Some("thread-abc"));
        assert_eq!(parsed.error_messages, vec!["network down".to_string()]);
    }

    #[test]
    fn test_classify_codex_failure_detects_network() {
        let err = classify_codex_failure(
            "failed to connect to websocket: url: wss://api.openai.com/v1/responses".into(),
        );
        assert!(matches!(err, CodexExecutorError::Network(_)));
    }

    #[test]
    fn test_classify_codex_event_error_detects_rate_limit() {
        let err = classify_codex_event_error("rate limit exceeded (429)");
        assert!(matches!(err, CodexExecutorError::RateLimited { .. }));
    }

    #[test]
    fn session_hash_is_deterministic() {
        let a = system_prompt_hash("You are a helpful assistant");
        let b = system_prompt_hash("You are a helpful assistant");
        assert_eq!(a, b, "Same input must produce same hash");
    }

    #[test]
    fn session_hash_differs_for_different_prompts() {
        let a = system_prompt_hash("You are a helpful assistant");
        let b = system_prompt_hash("You are a coding assistant");
        assert_ne!(a, b, "Different inputs must produce different hashes");
    }

    #[test]
    fn session_hash_is_16_hex_chars() {
        let h = system_prompt_hash("test");
        assert_eq!(h.len(), 16, "Hash should be 16 hex characters");
        assert!(
            h.chars().all(|c| c.is_ascii_hexdigit()),
            "Should be valid hex"
        );
    }

    #[test]
    fn test_truncate_for_name_short() {
        assert_eq!(truncate_for_name("hello"), "hello");
    }

    #[test]
    fn test_truncate_for_name_long() {
        let long = "a".repeat(100);
        let name = truncate_for_name(&long);
        assert!(name.len() <= 80);
        assert!(name.ends_with("..."));
    }

    #[tokio::test]
    async fn item_completed_agent_message_sets_final_response() {
        // The agentMessage item/completed is authoritative — it must replace
        // whatever deltas accumulated (deltas can be dropped under broadcast
        // lag, leaving a silently truncated or empty reply).
        let event = TypedCodexEvent::ItemCompleted(ItemNotification {
            thread_id: "t_1".to_string(),
            turn_id: Some("turn_1".to_string()),
            item: ThreadItem::AgentMessage {
                id: "i_2".to_string(),
                text: "the full reply".to_string(),
            },
        });
        let mut final_response = String::from("partial del");
        let mut current_turn_id = None;
        let action = handle_typed_event(
            event,
            "agent-1",
            "t_1",
            &mut final_response,
            &mut current_turn_id,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(action, EventAction::Continue));
        assert_eq!(final_response, "the full reply");
    }

    #[tokio::test]
    async fn foreign_turn_completed_does_not_end_our_turn() {
        // Compaction (or any concurrent turn) emits turn/completed on the
        // same thread. Without id filtering it ended our turn with an empty
        // reply while the model was still generating — the "silent agent"
        // bug observed live on 2026-07-04.
        let mut final_response = String::new();
        let mut current_turn_id = Some("our-turn".to_string());

        let foreign = TypedCodexEvent::TurnCompleted(TurnCompletedNotification {
            thread_id: "t_1".to_string(),
            turn: serde_json::json!({ "id": "compaction-turn", "status": "completed" }),
        });
        let action = handle_typed_event(
            foreign,
            "agent-1",
            "t_1",
            &mut final_response,
            &mut current_turn_id,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(action, EventAction::Continue));

        let ours = TypedCodexEvent::TurnCompleted(TurnCompletedNotification {
            thread_id: "t_1".to_string(),
            turn: serde_json::json!({ "id": "our-turn", "status": "completed" }),
        });
        let action = handle_typed_event(
            ours,
            "agent-1",
            "t_1",
            &mut final_response,
            &mut current_turn_id,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(action, EventAction::TurnComplete));
    }

    #[tokio::test]
    async fn foreign_turn_deltas_and_items_are_ignored() {
        let mut final_response = String::new();
        let mut current_turn_id = Some("our-turn".to_string());

        // Delta from another turn must not accumulate
        let foreign_delta = TypedCodexEvent::AgentMessageDelta(AgentMessageDeltaNotification {
            thread_id: "t_1".to_string(),
            turn_id: "other-turn".to_string(),
            item_id: "i_1".to_string(),
            delta: "noise".to_string(),
        });
        handle_typed_event(
            foreign_delta,
            "agent-1",
            "t_1",
            &mut final_response,
            &mut current_turn_id,
            None,
        )
        .await
        .unwrap();
        assert_eq!(final_response, "");

        // agentMessage item/completed from another turn must not overwrite
        let foreign_item = TypedCodexEvent::ItemCompleted(ItemNotification {
            thread_id: "t_1".to_string(),
            turn_id: Some("other-turn".to_string()),
            item: ThreadItem::AgentMessage {
                id: "i_2".to_string(),
                text: "wrong text".to_string(),
            },
        });
        final_response = "the real reply".to_string();
        handle_typed_event(
            foreign_item,
            "agent-1",
            "t_1",
            &mut final_response,
            &mut current_turn_id,
            None,
        )
        .await
        .unwrap();
        assert_eq!(final_response, "the real reply");

        // A turn/started from another turn must not hijack our id
        let foreign_started = TypedCodexEvent::TurnStarted(TurnStartedNotification {
            thread_id: "t_1".to_string(),
            turn: serde_json::json!({ "id": "other-turn" }),
        });
        handle_typed_event(
            foreign_started,
            "agent-1",
            "t_1",
            &mut final_response,
            &mut current_turn_id,
            None,
        )
        .await
        .unwrap();
        assert_eq!(current_turn_id.as_deref(), Some("our-turn"));
    }

    #[test]
    fn test_format_a2a_as_text_wraps_in_system_context() {
        let messages = vec![A2AMessage {
            from_id: "agent-a".into(),
            from_name: "Agent A".into(),
            to_id: "orbital".into(),
            content: "Need review".into(),
            timestamp: Utc::now(),
        }];
        let text = format_a2a_as_text(&messages);
        assert!(text.contains("SYSTEM CONTEXT -- NOT FROM HUMAN"));
        assert!(text.contains("Need review"));
    }
}
