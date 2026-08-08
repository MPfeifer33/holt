use chrono::Utc;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use super::a2a::A2ARegistry;
use super::agent::{AgentSlot, AgentStatus, ContentBlock, ConversationMessage, MessageRole};
use super::approval::ApprovalManager;
use super::connection::ConnectionConfig;
use super::limits::LimitCheckResult;
use super::rate_limiter::RateLimiter;
use super::subagent::SubagentManager;
use crate::config::app_config::AppConfig;
use crate::providers::types::{StreamError, StreamEvent, StreamEventType};
use crate::security::keychain::KeychainManager;
use crate::tools::registry::ToolRegistry;
use crate::tools::shell::ShellManager;
use crate::tools::vfl::VirtualFileLockRegistry;
use crate::trace::engine::{TraceEngine, TraceInput};
use crate::trace::truncation::{TraceOutcome, TraceType};
use tracing;

pub mod compaction;
pub mod helpers;
pub mod payload;
pub mod tool_execution;
pub mod turn_injection;

/// Legacy helper that merged transient user prompts with A2A content.
/// No longer used -- A2A now goes to system prompt, not user message channel.
/// Kept temporarily for test coverage; will be removed in cleanup pass.
#[allow(dead_code)]
fn merge_transient_prompts(base_prompt: Option<&str>, a2a_prompt: Option<&str>) -> Option<String> {
    let mut parts = Vec::new();

    if let Some(prompt) = base_prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        parts.push(prompt.to_string());
    }

    if let Some(prompt) = a2a_prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        parts.push(prompt.to_string());
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// Errors from the agentic execution engine.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("Agent not found: {0}")]
    AgentNotFound(String),
    #[error("Streaming error: {0}")]
    Stream(#[from] StreamError),
    #[error("Agent is in error state: {0}")]
    AgentError(String),
    #[error("Session limit reached — waiting for HIL")]
    SessionLimitReached,
    #[error("Agent interrupted by user")]
    Interrupted,
    #[error("No model endpoint configured")]
    NoEndpoint,
    #[error("API key not found: {0}")]
    ApiKeyNotFound(String),
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("Configuration error: {0}")]
    ConfigError(String),
}

/// All shared state needed by the agentic execution loop.
pub struct EngineContext<'a> {
    pub agents: &'a Arc<RwLock<Vec<AgentSlot>>>,
    pub agent_id: &'a str,
    pub trace_engine: &'a Arc<TraceEngine>,
    pub keychain: &'a KeychainManager,
    pub rate_limiter: &'a RwLock<RateLimiter>,
    pub network_config: &'a crate::config::app_config::NetworkBlock,
    pub limits_config: &'a crate::config::app_config::LimitsBlock,
    pub app_handle: Option<&'a AppHandle>,
    pub tool_registry: &'a ToolRegistry,
    pub shell_manager: &'a Arc<RwLock<ShellManager>>,
    pub app_config: &'a RwLock<AppConfig>,
    pub approval_manager: &'a Arc<ApprovalManager>,
    pub vfl_registry: &'a Arc<VirtualFileLockRegistry>,
    pub subagent_manager: &'a Arc<SubagentManager>,
    pub a2a_registry: &'a Arc<A2ARegistry>,
    pub cancellation_token: Option<&'a tokio_util::sync::CancellationToken>,
    pub app_state: Option<Arc<crate::AppState>>,
    /// Suppress turn-boundary memory pipeline traces for internal helper loops.
    pub suppress_memory_pipeline: bool,
    /// Optional ephemeral prompt included in the request payload for this loop
    /// invocation without being persisted to conversation history.
    pub transient_user_message: Option<&'a str>,
}

/// The core agentic execution loop (PRD Section 4.6).
///
/// This is the main message processing cycle. When a user sends a message,
/// this loop:
/// 1. Builds context payload
/// 2. Sends to model via SSE
/// 3. Parses response (text-only or tool_calls)
/// 4. Checks execution limits
/// 5. Executes tool calls via ToolRegistry
/// 6. Feeds results back and loops
/// 7. Delivers final response
pub async fn run_agentic_loop(
    ctx: &EngineContext<'_>,
    user_message: &str,
    content_blocks: Option<Vec<ContentBlock>>,
    max_turns: Option<usize>,
) -> Result<String, EngineError> {
    // Bind context fields for ergonomic access within the loop body
    let agents = ctx.agents;
    let agent_id = ctx.agent_id;
    let trace_engine = ctx.trace_engine;
    let keychain = ctx.keychain;
    let rate_limiter = ctx.rate_limiter;
    let network_config = ctx.network_config;
    let limits_config = ctx.limits_config;
    let app_handle = ctx.app_handle;
    let tool_registry = ctx.tool_registry;
    let shell_manager = ctx.shell_manager;
    let app_config = ctx.app_config;
    let approval_manager = ctx.approval_manager;
    let vfl_registry = ctx.vfl_registry;
    let subagent_manager = ctx.subagent_manager;
    let a2a_registry = ctx.a2a_registry;
    let cancellation_token = ctx.cancellation_token;
    let app_state = &ctx.app_state;
    let transient_user_message = ctx.transient_user_message;

    static SHARED_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .pool_max_idle_per_host(10)
            .build()
            .expect("Failed to build shared reqwest::Client")
    });
    let client = SHARED_CLIENT.clone();

    // Cloud API context probing: fire-and-forget background task to query model context size.
    // Runs once per session, skipped if user set an explicit context_tokens override.
    // Spawned as a background task so it never blocks the engine startup.
    {
        let (should_probe, connection_clone, api_key_opt) = {
            let agents_read = agents.read().await;
            if let Some(agent) = agents_read.iter().find(|a| a.id == agent_id) {
                let should = !agent.context_probed && agent.context_tokens_override.is_none();
                if !should && agent.context_tokens_override.is_some() {
                    tracing::debug!(
                        agent_id,
                        override_tokens = ?agent.context_tokens_override,
                        "Skipping cloud API probe: explicit context_tokens override set"
                    );
                }
                let conn = if should {
                    Some(agent.connection.clone())
                } else {
                    None
                };
                let key = if should {
                    match &agent.connection {
                        super::connection::ConnectionConfig::Api { api_key_ref, .. } => {
                            keychain.get_api_key(api_key_ref).ok()
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                (should, conn, key)
            } else {
                (false, None, None)
            }
        }; // agents_read dropped here

        if should_probe {
            if let (Some(connection), Some(api_key)) = (connection_clone, api_key_opt) {
                let agents_bg = agents.clone();
                let agent_id_bg = agent_id.to_string();
                // Record as probed immediately so concurrent messages don't re-probe
                {
                    let mut agents_w = agents.write().await;
                    if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                        agent.context_probed = true;
                    }
                }
                let probe_client = client.clone();
                tauri::async_runtime::spawn(async move {
                    match super::connection::probe_cloud_context_size(
                        &connection,
                        &api_key,
                        &probe_client,
                    )
                    .await
                    {
                        Some(probed_size) if probed_size >= 1024 && probed_size <= 10_000_000 => {
                            let mut agents_w = agents_bg.write().await;
                            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_bg) {
                                tracing::info!(
                                    agent_id = %agent_id_bg,
                                    old = agent.context_window_size,
                                    new = probed_size,
                                    "Updated context window from cloud API probe"
                                );
                                agent.context_window_size = probed_size;
                            }
                        }
                        Some(probed_size) => {
                            tracing::warn!(
                                agent_id = %agent_id_bg,
                                probed_size,
                                "Cloud API probe returned suspicious context size, ignoring"
                            );
                        }
                        None => {
                            // Probe failed — allow re-probing next session
                            tracing::debug!(
                                agent_id = %agent_id_bg,
                                "Cloud API probe returned no result, resetting probed flag"
                            );
                            let mut agents_w = agents_bg.write().await;
                            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_bg) {
                                agent.context_probed = false;
                            }
                        }
                    }
                });
            }
        }
    }

    // Get session ID (create one if needed)
    let session_id = trace_engine
        .create_session(agent_id)
        .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());

    tracing::info!(agent_id, session_id, "starting agentic loop");

    // Ensure agent has a session ID for session memory.
    // SDK agents get their session ID from Claude Code; API agents generate one here.
    {
        let mut agents_w = agents.write().await;
        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
            if agent.agent_session_id.is_none() {
                if !matches!(
                    agent.connection,
                    super::connection::ConnectionConfig::AgentSdk
                ) {
                    let new_sid = uuid::Uuid::new_v4().to_string();
                    tracing::debug!(agent_id, agent_session_id = %new_sid, "Generated agent_session_id for session memory");
                    agent.agent_session_id = Some(new_sid);
                    agent.is_dirty = true;
                }
            }
        }
    }

    // Inject memory context from previous sessions (only at conversation start)
    {
        let agents_read = agents.read().await;
        if let Some(agent) = agents_read.iter().find(|a| a.id == agent_id) {
            if agent.conversation.messages.is_empty() {
                if let Some(memory_context) = super::memory::inject_memory_context(agent_id) {
                    drop(agents_read);
                    let mut agents_w = agents.write().await;
                    if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                        agent.add_message(ConversationMessage {
                            role: MessageRole::System,
                            content: memory_context,
                            content_blocks: None,
                            timestamp: Utc::now(),
                            token_estimate: None,
                            tool_calls: None,
                            tool_call_id: None,
                            outcome_tag: None,
                            thinking_content: None,
                            responses_phase: None,
                            reasoning_items: None,
                        });
                    }
                }
            }
        }
    }

    // Setup turn state and optionally add user message.
    //
    // For normal user-initiated turns, user_message is non-empty and gets
    // added to the conversation as a persisted User message.
    //
    // Some internal call sites (notably non-SDK A2A wake handling) use
    // `transient_user_message` instead. That overlay is included in the
    // payload for this loop invocation but intentionally NOT written into
    // conversation history, avoiding synthetic user turns that can confuse
    // later iterations.
    //
    // The status transition to Working and the per-turn counter reset
    // happen unconditionally so the loop runs correctly in either case.
    {
        let mut agents = agents.write().await;
        let agent = agents
            .iter_mut()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| EngineError::AgentNotFound(agent_id.to_string()))?;

        agent.status = AgentStatus::Working;
        agent.reset_turn_limits();

        if !user_message.is_empty() {
            agent.add_message(ConversationMessage {
                role: MessageRole::User,
                content: user_message.to_string(),
                content_blocks,
                timestamp: Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            });
        }
    }

    // Log user message trace. Some runs use a transient prompt overlay that is
    // intentionally excluded from conversation history (for example non-SDK
    // A2A wake handling), but the request should still be traceable.
    let traced_message = if !user_message.is_empty() {
        Some((user_message, false))
    } else {
        transient_user_message.map(|msg| (msg, true))
    };
    if let Some((message, transient)) = traced_message {
        if let Err(e) = trace_engine.write_trace(TraceInput {
            agent_id: agent_id.to_string(),
            trace_type: TraceType::HilMessage,
            content: serde_json::json!({
                "message": message,
                "transient": transient,
            }),
            outcome: Some(TraceOutcome::Success),
            duration_ms: None,
            token_count: None,
            parent_trace_id: None,
            session_id: session_id.clone(),
            prompt_tokens: None,
            completion_tokens: None,
            cost_usd_cents: None,
        }) {
            tracing::warn!(agent_id, "trace write failed: {e}");
        }
    }

    // Main agentic loop
    let mut final_response = String::new();
    let max_iterations = max_turns.unwrap_or(50); // 50 is the default safety cap; forks pass Some(10)
    let mut has_compacted = false; // Prevent repeated compaction within one loop invocation
    let mut consecutive_turn_limit_warnings: u32 = 0;
    let mut has_retried_auth = false;
    let mut refreshed_api_key: Option<String> = None;
    let a2a_send_guard = Arc::new(RwLock::new(HashSet::new()));
    // Internal wake paths (notably non-SDK A2A) can provide a transient user
    // prompt that should anchor only the FIRST successful model response of
    // this loop. If it persists across later tool-result iterations, the model
    // keeps seeing the same synthetic "latest user" prompt and may repeat the
    // same tool call indefinitely.
    let mut pending_transient_user_message = transient_user_message;

    // Circuit breaker state: detect repeated identical tool calls (1A) and
    // repeated tool failures (1B). When triggered, tools are stripped from
    // the next request and a system message forces text-only response.
    let mut circuit_breaker_window: Vec<(String, u64)> = Vec::with_capacity(3); // (tool_name, args_hash)
    let mut circuit_breaker_tripped = false;
    let mut consecutive_tool_failures: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();

    // Snapshot config once per turn — config only changes on user settings edits,
    // never mid-turn. Eliminates ~50 read-lock acquisitions + clones per turn.
    let iter_config = app_config.read().await.clone();

    'agentic: for _iteration in 0..max_iterations {
        // Check for cancellation at top of each iteration
        if let Some(token) = cancellation_token {
            if token.is_cancelled() {
                return Err(EngineError::Interrupted);
            }
        }

        // TURN-BOUNDARY INJECTION: Drain subagent/A2A messages, retrieve memories
        let injection = turn_injection::inject_turn_messages(
            agents,
            agent_id,
            subagent_manager,
            a2a_registry,
            app_state,
            app_handle,
            trace_engine,
            &session_id,
            ctx.suppress_memory_pipeline,
        )
        .await?;
        let memory_section = injection.memory_section;
        let a2a_section = &injection.a2a_section;

        // SESSION MEMORY: Check trigger conditions at turn boundary (cheap — integers only)
        {
            let should_extract = {
                let agents_read = agents.read().await;
                agents_read
                    .iter()
                    .find(|a| a.id == agent_id)
                    .is_some_and(|agent| {
                        if matches!(
                            agent.connection,
                            super::connection::ConnectionConfig::AgentSdk
                        ) {
                            return false;
                        }
                        let sm_config =
                            crate::runtime::session_memory::SessionMemoryConfig::default();
                        agent
                            .session_memory
                            .should_trigger(&sm_config, agent.current_context_tokens)
                            && agent.agent_session_id.is_some()
                    })
            };

            if should_extract {
                // Set flag synchronously — prevents double-spawn
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
                    if let Some(ref app_state_arc) = app_state {
                        crate::runtime::session_memory::spawn_session_memory_fork(
                            app_state_arc,
                            &sm_agent_id,
                            &sm_sid,
                            sm_tokens,
                            app_handle,
                        );
                    }
                }
            }
        }

        // STEP 1: Build context payload
        let mut loop_payload = payload::build_loop_payload(
            agents,
            agent_id,
            keychain,
            &iter_config,
            tool_registry,
            app_state,
            &memory_section,
            a2a_section,
            pending_transient_user_message,
        )
        .await?;

        // Override api_key with refreshed OAuth token if we retried auth
        if let Some(ref token) = refreshed_api_key {
            loop_payload.api_key = Some(token.clone());
        }

        // Get or build the completion provider for this agent.
        // Provider may be pre-built (from create_agent) or needs construction
        // here (OAuth agents whose token was resolved above, or token refresh).
        let provider = {
            let agents_read = agents.read().await;
            let agent = agents_read.iter().find(|a| a.id == agent_id);
            agent.and_then(|a| a.provider.clone())
        };
        let provider = if let Some(p) = provider {
            // If OAuth token was refreshed, rebuild provider with new token
            if refreshed_api_key.is_some() {
                let agents_read = agents.read().await;
                let agent = agents_read.iter().find(|a| a.id == agent_id);
                if let Some(agent) = agent {
                    crate::providers::build_provider(
                        &agent.connection,
                        &agent.protocol,
                        loop_payload.api_key.as_deref(),
                        agent.anthropic_settings.as_ref(),
                        agent.openai_responses_settings.as_ref(),
                        agent.xai_settings.as_ref(),
                    )
                    .map_err(|e| EngineError::ConfigError(format!("Provider rebuild failed: {e}")))?
                    .unwrap_or(p)
                } else {
                    p
                }
            } else {
                p
            }
        } else {
            // Build provider on first use (OAuth agents, or provider wasn't set at creation)
            let agents_read = agents.read().await;
            let agent = agents_read
                .iter()
                .find(|a| a.id == agent_id)
                .ok_or_else(|| EngineError::AgentNotFound(agent_id.to_string()))?;
            crate::providers::build_provider(
                &agent.connection,
                &agent.protocol,
                loop_payload.api_key.as_deref(),
                agent.anthropic_settings.as_ref(),
                agent.openai_responses_settings.as_ref(),
                agent.xai_settings.as_ref(),
            )
            .map_err(|e| EngineError::ConfigError(format!("Provider build failed: {e}")))?
            .ok_or_else(|| EngineError::ConfigError("No provider for this agent".to_string()))?
        };

        // PRE-COMPACTION EXTRACTION: At lower threshold, extract context before eviction
        if !has_compacted {
            let pre_threshold = loop_payload.context_config.pre_compaction_threshold_percent;
            if pre_threshold > 0 {
                let extraction_threshold =
                    (loop_payload.context_window_size as u64 * pre_threshold as u64) / 100;
                if let Some(ref app_state_arc) = app_state {
                    if let Some(mem_engine) = app_state_arc.get_memory_engine() {
                        let extraction_params = crate::memory::compaction::ExtractionParams {
                            client: &client,
                            endpoint: &loop_payload.endpoint,
                            api_key: loop_payload.api_key.as_deref(),
                            model: &loop_payload.model,
                            summary_tokens: loop_payload.context_config.compaction_summary_tokens,
                            keep_recent: loop_payload.context_config.compaction_keep_recent,
                        };
                        match crate::memory::compaction::check_and_extract(
                            &mem_engine,
                            agents,
                            agent_id,
                            loop_payload.context_tokens as u64,
                            extraction_threshold,
                            &extraction_params,
                        )
                        .await
                        {
                            Ok(crate::memory::compaction::ExtractionOutcome::StoreFailed(
                                reason,
                            )) => {
                                tracing::warn!(
                                    agent_id,
                                    "Pre-compaction extraction store failed (will retry): {reason}"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(agent_id, "Pre-compaction extraction failed: {e}");
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // COMPACTION: If context is nearing the threshold, auto-compact
        if !has_compacted {
            if compaction::try_compact_if_needed(
                agents,
                agent_id,
                &client,
                &loop_payload,
                app_handle,
                app_state,
                trace_engine,
                &session_id,
            )
            .await?
            {
                has_compacted = true;
                continue; // Re-loop to rebuild payload with compacted messages
            }
        } else if loop_payload.needs_compaction {
            // Already compacted this invocation — still warn frontend about context pressure
            if let Some(handle) = app_handle {
                if let Err(e) = handle.emit(
                    "agent-stream",
                    StreamEvent {
                        agent_id: agent_id.to_string(),
                        event_type: StreamEventType::ContextWarning,
                        data: serde_json::json!({
                            "estimated_tokens": loop_payload.context_tokens,
                            "needs_compaction": true,
                        }),
                    },
                ) {
                    tracing::warn!(agent_id, "event emission failed: {e}");
                }
            }
        }

        // Destructure payload into local variables for the rest of the loop
        let augmented_system_prompt = loop_payload.augmented_system_prompt;
        let endpoint = loop_payload.endpoint;
        let model = loop_payload.model;
        let messages = loop_payload.messages;
        let working_dir = loop_payload.working_dir;

        // Check rate limiter
        {
            let rl = rate_limiter.read().await;
            if let Some(wait_duration) = rl.check_wait(&endpoint) {
                tokio::time::sleep(wait_duration).await;
            }
        }

        // Check cancellation before the potentially long LLM request
        if let Some(token) = cancellation_token {
            if token.is_cancelled() {
                return Err(EngineError::Interrupted);
            }
        }

        // STEP 2: Send to model
        let start = std::time::Instant::now();

        // Stream coalescer batches Token/Thinking events to reduce frontend event rate.
        // Significant events (ToolCall, Done, Error) flush immediately.
        let coalescer = if let Some(handle) = app_handle {
            Some(crate::runtime::stream_coalescer::StreamCoalescer::with_app_handle(handle.clone()))
        } else {
            None
        };
        let flush_task = coalescer.as_ref().map(|c| c.start_flush_task());

        let token_callback = |event: StreamEvent| {
            if let Some(ref c) = coalescer {
                c.push(event);
            }
        };

        // Build tool definitions for the model.
        // If the model doesn't support tool calling (local_model_settings.supports_tools == false),
        // omit tool schemas from the request entirely to avoid confusing the model.
        // If the circuit breaker tripped, omit tools to force a text-only response.
        let tool_defs = tool_registry.core_definitions();
        let tools_param =
            if tool_defs.is_empty() || !loop_payload.supports_tools || circuit_breaker_tripped {
                None
            } else {
                Some(tool_defs)
            };

        let result = crate::providers::send_provider_completion(
            provider.as_ref(),
            &client,
            &messages,
            tools_param.as_deref(),
            limits_config.max_response_tokens,
            &token_callback,
            agent_id,
        )
        .await;

        // Flush any remaining buffered tokens and stop the flush task
        if let Some(ref c) = coalescer {
            c.flush_all();
        }
        if let Some(task) = flush_task {
            task.abort();
        }

        let duration_ms = start.elapsed().as_millis() as u64;
        tracing::info!(agent_id, duration_ms, "LLM request completed");

        // Handle errors
        let response = match result {
            Ok(r) => {
                // Clear rate limiter on success
                let mut rl = rate_limiter.write().await;
                rl.clear(&endpoint);
                r
            }
            Err(StreamError::RateLimited { retry_after }) => {
                let mut rl = rate_limiter.write().await;
                rl.record_rate_limit(&endpoint, retry_after);
                let attempt = rl.retry_count(&endpoint);
                let max_retries = network_config.rate_limit_max_retries;

                tracing::warn!(
                    agent_id,
                    ?retry_after,
                    attempt,
                    max_retries,
                    endpoint,
                    "rate limited"
                );

                // Check if we've exceeded max retries
                if attempt > max_retries {
                    // Emit error to frontend so the chat UI shows the problem
                    if let Some(handle) = app_handle {
                        if let Err(e) = handle.emit("agent-stream", StreamEvent {
                            agent_id: agent_id.to_string(),
                            event_type: StreamEventType::Error,
                            data: serde_json::json!({
                                "message": format!("Rate limited by API after {} retries. Try again in a minute.", max_retries),
                            }),
                        }) { tracing::warn!(agent_id, "event emission failed: {e}"); }
                    }
                    let mut agents = agents.write().await;
                    if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
                        agent.status = AgentStatus::Error("Rate limit exceeded".to_string());
                    }
                    return Err(EngineError::Stream(StreamError::RateLimited {
                        retry_after,
                    }));
                }

                // Wait before retrying — use retry_after if provided, else exponential backoff with jitter
                let base_backoff = retry_after
                    .map(|s| s as u64 * 1000)
                    .unwrap_or(network_config.retry_backoff_ms * (1 << attempt.min(5)));
                // Add jitter: 50-100% of base backoff to prevent thundering herd
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos();
                let jitter_factor = 0.5 + (nanos as f64 / u32::MAX as f64) * 0.5;
                let jitter = (base_backoff as f64 * jitter_factor) as u64;
                tracing::info!(
                    agent_id,
                    backoff_ms = jitter,
                    attempt,
                    "retrying after rate limit backoff"
                );
                tokio::time::sleep(Duration::from_millis(jitter)).await;
                continue;
            }
            Err(StreamError::Network(e)) => {
                tracing::error!(agent_id, error = %e, "network error during streaming");
                // Retry once on network error
                if network_config.retry_on_network_error > 0 {
                    tokio::time::sleep(Duration::from_millis(network_config.retry_backoff_ms))
                        .await;
                    // One more attempt using the same provider
                    let retry_result = crate::providers::send_provider_completion(
                        provider.as_ref(),
                        &client,
                        &messages,
                        tools_param.as_deref(),
                        limits_config.max_response_tokens,
                        &token_callback,
                        agent_id,
                    )
                    .await;
                    match retry_result {
                        Ok(r) => r,
                        Err(e) => {
                            let mut agents = agents.write().await;
                            if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
                                agent.status = AgentStatus::Error(format!("Network error: {}", e));
                            }
                            return Err(EngineError::Stream(e));
                        }
                    }
                } else {
                    let mut agents = agents.write().await;
                    if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
                        agent.status = AgentStatus::Error(format!("Network error: {}", e));
                    }
                    return Err(EngineError::Stream(StreamError::Network(e)));
                }
            }
            Err(StreamError::ApiError(ref msg))
                if msg.contains("401 Unauthorized") && !has_retried_auth =>
            {
                // Attempt OAuth token refresh
                let refreshed = {
                    let agents_read = agents.read().await;
                    if let Some(agent) = agents_read.iter().find(|a| a.id == agent_id) {
                        if let ConnectionConfig::OAuth {
                            credentials_path, ..
                        } = &agent.connection
                        {
                            crate::security::oauth::resolve_bearer_token(credentials_path, &client)
                                .await
                                .ok()
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                if let Some(new_token) = refreshed {
                    refreshed_api_key = Some(new_token);
                    has_retried_auth = true;
                    continue; // retry the agentic loop
                }
                let mut agents = agents.write().await;
                if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
                    agent.status = AgentStatus::Error("Authentication failed".to_string());
                }
                return Err(EngineError::Stream(StreamError::ApiError(msg.clone())));
            }
            Err(e) => {
                let mut agents = agents.write().await;
                if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
                    agent.status = AgentStatus::Error(format!("Error: {}", e));
                }
                return Err(EngineError::Stream(e));
            }
        };

        // The transient prompt has now served its purpose. Clear it after the
        // first successful model response so subsequent iterations in the same
        // loop reason over the assistant/tool history rather than replaying the
        // synthetic wake prompt.
        pending_transient_user_message = None;

        // Compute cost for trace record (if pricing configured)
        let trace_cost_usd_cents = if let Some(ref usage) = response.usage {
            iter_config.pricing.lookup(&model).map(|rates| {
                let prompt = usage.prompt_tokens.unwrap_or(0) as f64;
                let completion = usage.completion_tokens.unwrap_or(0) as f64;
                let cost = (prompt * rates.input_per_million
                    + completion * rates.output_per_million)
                    / 1_000_000.0;
                (cost * 100.0) as i64
            })
        } else {
            None
        };

        // Log response trace
        if let Err(e) = trace_engine.write_trace(TraceInput {
            agent_id: agent_id.to_string(),
            trace_type: TraceType::AgentResponse,
            content: serde_json::json!({
                "content": &response.content,
                "tool_call_count": response.tool_calls.len(),
                "finish_reason": &response.finish_reason,
            }),
            outcome: Some(TraceOutcome::Success),
            duration_ms: Some(duration_ms),
            token_count: response.usage.as_ref().and_then(|u| u.total_tokens),
            parent_trace_id: None,
            session_id: session_id.clone(),
            prompt_tokens: response.usage.as_ref().and_then(|u| u.prompt_tokens),
            completion_tokens: response.usage.as_ref().and_then(|u| u.completion_tokens),
            cost_usd_cents: trace_cost_usd_cents,
        }) {
            tracing::warn!(agent_id, "trace write failed: {e}");
        }

        // Capture actual token usage from API response
        if let Some(ref usage) = response.usage {
            let mut agents_w = agents.write().await;
            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                if let Some(pt) = usage.prompt_tokens {
                    agent.session_prompt_tokens += pt as u64;
                    agent.current_context_tokens = pt as u64;
                }
                if let Some(ct) = usage.completion_tokens {
                    agent.session_completion_tokens += ct as u64;
                }
                if agent.session_start.is_none() {
                    agent.session_start = Some(Utc::now());
                }
                // Record dirty state so persistence picks up token changes.
                // NOTE: cached_params update moved to final-response path (STEP 7)
                // to eliminate clone churn — previously cloned entire conversation
                // on every tool-call iteration (5-50x per turn).
                agent.is_dirty = true;
            }
        }

        // Detect truncated stream (connection dropped without finish_reason)
        if response.finish_reason.is_none() && !response.content.is_empty() {
            tracing::warn!(
                agent_id,
                "stream ended without finish_reason — possible mid-stream disconnect"
            );
        }

        // Reset circuit breaker flag — it's been consumed for this iteration's
        // tools_param decision. It will be re-evaluated after tool execution.
        if circuit_breaker_tripped {
            circuit_breaker_tripped = false;
        }

        // STEP 3: Parse response
        if response.tool_calls.is_empty() {
            // Text-only response — reset circuit breaker state
            circuit_breaker_window.clear();
            consecutive_tool_failures.clear();
            // STEP 7: deliver final response
            let mut agents = agents.write().await;
            if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
                agent.add_message(ConversationMessage {
                    role: MessageRole::Assistant,
                    content: response.content.clone(),
                    content_blocks: None,
                    timestamp: Utc::now(),
                    token_estimate: response.usage.as_ref().and_then(|u| u.completion_tokens),
                    tool_calls: None,
                    tool_call_id: None,
                    outcome_tag: None,
                    thinking_content: response.thinking_content.clone(),
                    responses_phase: None,
                    reasoning_items: None,
                });

                // Reconcile token usage
                if let Some(usage) = &response.usage {
                    if let Some(total) = usage.total_tokens {
                        agent.metadata.total_tokens_used += total as u64;
                    }
                }
                agent.metadata.total_api_calls += 1;
                agent.metadata.last_active = Some(Utc::now());

                // Capture cache-safe params for fork spawning — only on final
                // response, not on every tool-call iteration. This eliminates
                // the largest source of allocation churn: previously cloned the
                // entire conversation Vec 5-50x per turn.
                agent.cached_params = Some(crate::runtime::agent::CacheSafeParams {
                    system_prompt: augmented_system_prompt.clone(),
                    tool_definitions: tool_registry.core_definitions(),
                    context_messages: std::sync::Arc::new(agent.conversation.messages.clone()),
                });

                // Reset turn counter and set idle
                agent.status = AgentStatus::Idle;
            }

            // Notify frontend that conversation state is now consistent
            if let Some(handle) = app_handle {
                if let Err(e) = handle.emit(
                    "agent-stream",
                    StreamEvent {
                        agent_id: agent_id.to_string(),
                        event_type: StreamEventType::ConversationUpdated,
                        data: serde_json::json!({}),
                    },
                ) {
                    tracing::warn!(agent_id, "event emission failed: {e}");
                }
            }

            final_response = response.content.clone();

            // Auto-capture removed — replaced by LLM extraction at compaction (Phase 3)

            break;
        }

        // STEP 4: Check execution limits
        let mut hit_turn_limit = false;
        let mut limit_exceeded_fatally = false;
        {
            let mut agents = agents.write().await;
            let agent = agents
                .iter_mut()
                .find(|a| a.id == agent_id)
                .ok_or_else(|| EngineError::AgentNotFound(agent_id.to_string()))?;

            // Add assistant message with tool calls
            agent.add_message(ConversationMessage {
                role: MessageRole::Assistant,
                content: response.content.clone(),
                content_blocks: None,
                timestamp: Utc::now(),
                token_estimate: None,
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: response.thinking_content.clone(),
                responses_phase: None,
                reasoning_items: None,
            });

            // SESSION MEMORY: Count assistant messages
            if !response.content.is_empty() {
                agent.session_memory.assistant_messages_since_extraction += 1;
            }

            // Check turn limits using per-agent limits.
            //
            // Strict-provider contract: if the limit fires mid-batch, we must
            // NOT insert any non-Tool message between the assistant-with-
            // tool_calls message (added above) and the dummy Tool responses.
            // OpenAI's GPT-5/o-series models reject requests where a System
            // or User message is interposed. So the "limit reached" System
            // message and the final "execution stopped" System message are
            // deferred until STEP 5, which first adds all the dummy Tool
            // responses and THEN the System message — preserving adjacency.
            for _tc in &response.tool_calls {
                agent.metadata.total_api_calls += 1;

                match agent.limits_check_and_increment() {
                    LimitCheckResult::TurnLimitReached => {
                        hit_turn_limit = true;
                        consecutive_turn_limit_warnings += 1;

                        if consecutive_turn_limit_warnings >= 2 {
                            tracing::warn!(
                                agent_id,
                                "model ignored turn limit twice, forcing stop"
                            );
                            limit_exceeded_fatally = true;
                        }
                        // Skip remaining tool_calls. Messages are added below
                        // in STEP 5 to preserve assistant→tool adjacency.
                        break;
                    }
                    LimitCheckResult::SessionLimitReached => {
                        agent.status = AgentStatus::WaitingForHil;
                        return Err(EngineError::SessionLimitReached);
                    }
                    LimitCheckResult::Ok => {}
                }
            }

            // Reset consecutive warning counter if tools executed without hitting limit
            if !hit_turn_limit {
                consecutive_turn_limit_warnings = 0;
            }
        }

        // STEP 5: Execute tool calls via ToolRegistry
        // Skip tool execution if turn limit was hit — but add dummy results for all tool calls
        // so OpenAI, Anthropic, and any other strict provider see matching
        // tool_result for every tool_use. The dummy Tool messages MUST come
        // directly after the assistant-with-tool_calls message; any System
        // or User message inserted between them will be rejected by
        // OpenAI's GPT-5/o-series models with "tool_call_ids did not have
        // response messages". System messages related to the limit are
        // appended AFTER the dummy tool responses.
        if hit_turn_limit {
            let mut agents_write = agents.write().await;
            if let Some(agent) = agents_write.iter_mut().find(|a| a.id == agent_id) {
                // (1) Dummy Tool responses — must be adjacent to the assistant.
                for tc in &response.tool_calls {
                    agent.add_message(ConversationMessage {
                        role: MessageRole::Tool,
                        content: serde_json::json!({
                            "error": "Tool call limit reached for this turn. Skipped.",
                            "code": "TurnLimitReached",
                            "retryable": false,
                        })
                        .to_string(),
                        content_blocks: None,
                        timestamp: Utc::now(),
                        token_estimate: None,
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        outcome_tag: None,
                        thinking_content: None,
                        responses_phase: None,
                        reasoning_items: None,
                    });
                }

                // (2) System message — appended AFTER dummy tool responses so
                //     the assistant→tool adjacency required by strict providers
                //     (OpenAI GPT-5/o-series, Anthropic) is preserved.
                if limit_exceeded_fatally {
                    agent.status = AgentStatus::Idle;
                    agent.add_message(ConversationMessage {
                        role: MessageRole::System,
                        content: "[Turn limit exceeded — execution stopped]".to_string(),
                        content_blocks: None,
                        timestamp: Utc::now(),
                        token_estimate: None,
                        tool_calls: None,
                        tool_call_id: None,
                        outcome_tag: None,
                        thinking_content: None,
                        responses_phase: None,
                        reasoning_items: None,
                    });
                    if !response.content.is_empty() {
                        final_response = response.content.clone();
                    }
                    drop(agents_write);
                    break 'agentic;
                }

                // Non-fatal: inject stronger system message forcing text response next turn.
                agent.add_message(ConversationMessage {
                    role: MessageRole::System,
                    content: format!(
                        "[System: Tool call limit reached for this turn ({}). You MUST respond to the user with text now. Do NOT call any more tools.]",
                        agent.limits.max_tool_calls_per_turn
                    ),
                    content_blocks: None,
                    timestamp: Utc::now(),
                    token_estimate: None,
                    tool_calls: None,
                    tool_call_id: None,
                    outcome_tag: None,
                    thinking_content: None,
                    responses_phase: None,
                    reasoning_items: None,
                });
            }
            drop(agents_write);
            continue;
        }

        let tool_outcome = tool_execution::execute_tool_calls(
            agents,
            agent_id,
            &response,
            tool_registry,
            &iter_config.approval,
            approval_manager,
            app_handle,
            app_config,
            shell_manager,
            trace_engine,
            &session_id,
            vfl_registry,
            subagent_manager,
            a2a_registry,
            &a2a_send_guard,
            app_state,
            &working_dir,
        )
        .await?;

        if tool_outcome == tool_execution::ToolExecutionOutcome::EndTurn {
            let mut agents_w = agents.write().await;
            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                agent.status = AgentStatus::Idle;
            }
            drop(agents_w);

            if let Some(handle) = app_handle {
                if let Err(e) = handle.emit(
                    "agent-stream",
                    StreamEvent {
                        agent_id: agent_id.to_string(),
                        event_type: StreamEventType::ConversationUpdated,
                        data: serde_json::json!({}),
                    },
                ) {
                    tracing::warn!(agent_id, "event emission failed: {e}");
                }

                if let Err(e) = handle.emit(
                    "agent-stream",
                    StreamEvent {
                        agent_id: agent_id.to_string(),
                        event_type: StreamEventType::Done,
                        data: serde_json::json!({}),
                    },
                ) {
                    tracing::warn!(agent_id, "event emission failed: {e}");
                }
            }
            break;
        }

        // CIRCUIT BREAKER: Check for repeated identical tool calls (1A) and
        // repeated tool failures (1B) after tool execution completes.
        for tc in &response.tool_calls {
            // Hash the arguments for compact comparison
            let mut hasher = DefaultHasher::new();
            tc.arguments.to_string().hash(&mut hasher);
            let args_hash = hasher.finish();
            let entry = (tc.name.clone(), args_hash);

            circuit_breaker_window.push(entry);
            // Keep only last 3 entries
            if circuit_breaker_window.len() > 3 {
                circuit_breaker_window.remove(0);
            }
        }

        // 1A: Check if last 3 tool calls are identical (same tool, same args)
        if circuit_breaker_window.len() == 3
            && circuit_breaker_window[0] == circuit_breaker_window[1]
            && circuit_breaker_window[1] == circuit_breaker_window[2]
        {
            let tool_name = &circuit_breaker_window[0].0;
            tracing::warn!(
                agent_id,
                tool_name,
                "circuit breaker: identical tool call repeated 3 times"
            );
            circuit_breaker_tripped = true;

            // Inject system message forcing text response
            let mut agents_w = agents.write().await;
            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                agent.add_message(ConversationMessage {
                    role: MessageRole::System,
                    content: format!(
                        "[System: You have called '{}' with the same arguments 3 times. The result has not changed. Please respond to the user based on the results you have.]",
                        tool_name
                    ),
                    content_blocks: None,
                    timestamp: Utc::now(),
                    token_estimate: None,
                    tool_calls: None,
                    tool_call_id: None,
                    outcome_tag: None,
                    thinking_content: None,
                    responses_phase: None,
                    reasoning_items: None,
                });
            }
            drop(agents_w);
            circuit_breaker_window.clear();
        }

        // 1B: Check for repeated tool failures (retryable errors on same tool)
        // Parse tool results from conversation to detect failures
        if !circuit_breaker_tripped {
            let agents_r = agents.read().await;
            if let Some(agent) = agents_r.iter().find(|a| a.id == agent_id) {
                // Check the most recent tool result messages for failures
                for tc in &response.tool_calls {
                    let last_msgs: Vec<_> = agent
                        .conversation
                        .messages
                        .iter()
                        .rev()
                        .take(response.tool_calls.len())
                        .collect();

                    let failed = last_msgs.iter().any(|m| {
                        m.role == MessageRole::Tool
                            && m.tool_call_id.as_deref() == Some(&tc.id)
                            && m.content.contains("\"tool_result_status\":\"failed\"")
                    });

                    if failed {
                        let count = consecutive_tool_failures
                            .entry(tc.name.clone())
                            .or_insert(0);
                        *count += 1;

                        if *count >= 3 {
                            tracing::warn!(
                                agent_id,
                                tool_name = tc.name,
                                consecutive_failures = *count,
                                "circuit breaker: tool failed 3 consecutive times"
                            );
                            circuit_breaker_tripped = true;

                            drop(agents_r);
                            let mut agents_w = agents.write().await;
                            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                                agent.add_message(ConversationMessage {
                                    role: MessageRole::System,
                                    content: format!(
                                        "[System: Tool '{}' has failed {} consecutive times and appears unavailable. Do not retry it. Respond to the user based on what you have.]",
                                        tc.name, count
                                    ),
                                    content_blocks: None,
                                    timestamp: Utc::now(),
                                    token_estimate: None,
                                    tool_calls: None,
                                    tool_call_id: None,
                                    outcome_tag: None,
                                    thinking_content: None,
                                    responses_phase: None,
                                    reasoning_items: None,
                                });
                            }
                            drop(agents_w);
                            consecutive_tool_failures.remove(&tc.name);
                            break;
                        }
                    } else {
                        // Tool succeeded — reset its failure counter
                        consecutive_tool_failures.remove(&tc.name);
                    }
                }
            }
        }

        // SESSION MEMORY: Count tool calls for trigger conditions
        if !response.tool_calls.is_empty() {
            let mut agents_w = agents.write().await;
            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                agent.session_memory.tool_calls_since_extraction +=
                    response.tool_calls.len() as u32;
            }
        }

        // Loop back to STEP 1 — model will see tool results and continue
    }

    // NOTE: auto-wake-on-Done was removed here. An earlier version checked
    // has_pending_messages and re-emitted `agent-a2a-wake` after the loop
    // ended, mirroring the SDK auto-wake pattern at agent_sdk.rs:592. But
    // for non-SDK agents this created infinite conversation loops: Agent A
    // replies to Agent B, B replies back (fast), auto-wake catches B's
    // reply and re-triggers A, A replies again, ad infinitum. The send-time
    // wake from each agent's send_message_to_agent tool call is sufficient
    // for the common case — it fires when the message is sent and the
    // frontend's triggerAgentTurn handles delivery. The narrow window where
    // a message arrives after the last inject_turn_messages drain but before
    // the loop exits is an acceptable limitation: the message stays in the
    // queue and gets drained on the agent's next interaction (user message,
    // another A2A wake, etc.). This is a better trade-off than infinite
    // conversation loops between agents.

    // COGNITIVE INTAKE: Fire-and-forget extraction after turn completes.
    // Extracts conversational content, novelty-checks, and forwards to Hillock.
    if !final_response.is_empty() {
        if let Some(ref app_state_arc) = app_state {
            if let Some(mem_engine) = app_state_arc.get_memory_engine() {
                let agents_clone = agents.clone();
                let agent_id_owned = agent_id.to_string();
                let user_msg = user_message.to_string();
                let assistant_msg = final_response.clone();
                let embedder = mem_engine.embedder.clone();

                tokio::spawn(async move {
                    // Get agent namespace and cognitive intake state
                    let agent_ns = {
                        let agents_r = agents_clone.read().await;
                        agents_r
                            .iter()
                            .find(|a| a.id == agent_id_owned)
                            .map(|a| crate::memory::MemoryEngine::agent_namespace(&a.id))
                    };

                    let Some(agent_ns) = agent_ns else { return };

                    // Clone state, process, then write back
                    let mut intake_state = {
                        let agents_r = agents_clone.read().await;
                        match agents_r.iter().find(|a| a.id == agent_id_owned) {
                            Some(agent) => agent.cognitive_intake.clone(),
                            None => return,
                        }
                    };

                    let result = crate::runtime::cognitive_intake::process_turn(
                        &mut intake_state,
                        &embedder,
                        &mem_engine,
                        &agent_ns,
                        &user_msg,
                        &assistant_msg,
                    )
                    .await;

                    // Write updated state back
                    {
                        let mut agents_w = agents_clone.write().await;
                        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
                            agent.cognitive_intake = intake_state;
                        }
                    }

                    match result {
                        crate::runtime::cognitive_intake::IntakeResult::Forwarded { memory_id } => {
                            tracing::debug!(
                                agent_id = agent_id_owned,
                                memory_id = %memory_id,
                                "Cognitive intake forwarded"
                            );
                        }
                        crate::runtime::cognitive_intake::IntakeResult::NoveltyRejected {
                            max_similarity,
                        } => {
                            tracing::debug!(
                                agent_id = agent_id_owned,
                                max_similarity,
                                "Cognitive intake: novelty rejected"
                            );
                        }
                        crate::runtime::cognitive_intake::IntakeResult::HillockError(e) => {
                            tracing::warn!(
                                agent_id = agent_id_owned,
                                error = %e,
                                "Cognitive intake bridge error"
                            );
                        }
                        _ => {} // TooShort, Disabled — silent
                    }
                });
            }
        }
    }

    // End trace session
    tracing::info!(
        agent_id,
        session_id,
        response_len = final_response.len(),
        "agentic loop completed"
    );
    if let Err(e) = trace_engine.end_session(&session_id, "success") {
        tracing::warn!(agent_id, "trace session end failed: {e}");
    }

    Ok(final_response)
}

#[cfg(test)]
pub(crate) mod test_utils {
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::runtime::agent::{
        AgentColor, AgentMetadata, AgentSlot, AgentStatus, ConversationHistory,
    };
    use crate::runtime::connection::ConnectionConfig;
    use crate::runtime::limits::AgentLimits;

    /// Create a minimal test agent with sensible defaults.
    pub fn test_agent() -> AgentSlot {
        AgentSlot {
            id: "test-agent".to_string(),
            name: "Test Agent".to_string(),
            color: AgentColor {
                r: 100,
                g: 149,
                b: 237,
            },
            protected: false,
            connection: ConnectionConfig::Api {
                endpoint: "https://test.example.com".to_string(),
                api_key_ref: "test-key".to_string(),
                model: "test-model".to_string(),
            },
            protocol: crate::runtime::connection::AgentProtocol::default(),
            working_directory: PathBuf::from("/tmp/test-agent"),
            status: AgentStatus::Idle,
            subagents: vec![],
            conversation: ConversationHistory::default(),
            tools: vec![],
            system_prompt: "You are a test agent.".to_string(),
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

    /// Wrap an agent in the Arc<RwLock<Vec>> structure used by the engine.
    pub fn wrap_agents(agent: AgentSlot) -> Arc<RwLock<Vec<AgentSlot>>> {
        Arc::new(RwLock::new(vec![agent]))
    }
}

#[cfg(test)]
mod tests {
    use super::merge_transient_prompts;

    #[test]
    fn merge_transient_prompts_handles_empty_inputs() {
        assert!(merge_transient_prompts(None, None).is_none());
        assert_eq!(
            merge_transient_prompts(Some("user prompt"), None).as_deref(),
            Some("user prompt")
        );
        assert_eq!(
            merge_transient_prompts(None, Some("a2a prompt")).as_deref(),
            Some("a2a prompt")
        );
    }

    #[test]
    fn merge_transient_prompts_preserves_order() {
        let merged = merge_transient_prompts(
            Some("wake prompt"),
            Some("[Message from agent 'Agent A': hi]"),
        )
        .expect("merged prompt");
        assert_eq!(merged, "wake prompt\n\n[Message from agent 'Agent A': hi]");
    }
}
