//! Agent SDK integration — NDJSON-based query orchestration and event mapping.
//!
//! This module provides:
//! 1. **Pure functions** (testable): `map_ndjson_event()` and `event_to_conversation_message()`
//! 2. **Async orchestration**: `agent_sdk_query()`, bridge lifecycle, MCP tool routing
//! 3. **NDJSON transport**: `NdjsonProcess` replaces the old JSON-RPC SidecarProcess

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, LazyLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStderr, ChildStdin, Command};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::Duration;

use crate::config::agent_sdk_config::AgentSdkBlock;
use crate::runtime::agent::{AgentSlot, AgentStatus, ConversationMessage, MessageRole};
use crate::runtime::memory_pipeline::{write_memory_pipeline_trace, MemoryPipelineReport};
use crate::tools::registry::ToolRegistry;
use crate::tools::types::ToolContext;
use crate::AppState;

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

async fn build_sdk_tool_awareness(agent_id: &str, app_state: &Arc<AppState>) -> String {
    let tools_config = load_agent_tools_config(agent_id);
    let registry =
        ToolRegistry::build_for_sdk_agent_with_tools(&tools_config, Some(&app_state.plugin_host))
            .await;
    registry.awareness_block(
        "You also have Holt-native tools in this lane (distinct from the SDK's provider-native file/code tools). Treat these as first-class capabilities and prefer them over repo or shell guesswork when you need Holt state, collaboration, memory, persona, watches, drafts, or agent introspection:",
    )
}

// ── Tauri event payloads ────────────────────────────────────────────────

/// Payload emitted to the Tauri event bus for frontend consumption.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentSdkEvent {
    Token {
        agent_id: String,
        text: String,
    },
    Thinking {
        agent_id: String,
        text: String,
    },
    ToolCall {
        agent_id: String,
        name: String,
        args: Value,
        tool_use_id: Option<String>,
    },
    ToolResult {
        agent_id: String,
        name: String,
        output: String,
        tool_use_id: Option<String>,
    },
    ToolInputDelta {
        agent_id: String,
        partial_json: String,
    },
    Session {
        agent_id: String,
        session_id: String,
    },
    Result {
        agent_id: String,
        cost_usd: Option<f64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    Done {
        agent_id: String,
        session_id: Option<String>,
        cost_usd: Option<f64>,
    },
    Error {
        agent_id: String,
        message: String,
    },
    BridgeStatus {
        agent_id: String,
        bridge_kind: String,
        tool_surface: String,
        holt_mcp_mounted: bool,
        tool_count: u32,
        mcp_url: Option<String>,
    },
}

// ── NDJSON event types ────────────────────────────────────────────────

/// Parsed NDJSON event from the bridge's stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NdjsonEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(flatten)]
    pub fields: HashMap<String, Value>,
}

/// A tool request from the bridge that needs a response from Rust.
#[derive(Debug, Clone)]
pub struct ToolRequest {
    pub name: String,
    pub args: Value,
    pub request_id: String,
}

/// A permission request from the bridge's canUseTool callback.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub tool: String,
    pub input: Value,
    pub request_id: String,
}

// ── Pure mapping functions ──────────────────────────────────────────────

/// Map an NDJSON event into a typed Tauri event payload.
///
/// Returns `None` for unrecognized event types (silently dropped).
pub fn map_ndjson_event(agent_id: &str, event: &NdjsonEvent) -> Option<AgentSdkEvent> {
    let aid = agent_id.to_string();
    match event.event_type.as_str() {
        "token" => {
            let text = event.fields.get("text")?.as_str()?.to_string();
            Some(AgentSdkEvent::Token {
                agent_id: aid,
                text,
            })
        }
        "thinking" => {
            let text = event.fields.get("text")?.as_str()?.to_string();
            Some(AgentSdkEvent::Thinking {
                agent_id: aid,
                text,
            })
        }
        "tool_call" => {
            let name = event.fields.get("name")?.as_str()?.to_string();
            let args = event
                .fields
                .get("args")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            let tool_use_id = event
                .fields
                .get("toolUseId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Some(AgentSdkEvent::ToolCall {
                agent_id: aid,
                name,
                args,
                tool_use_id,
            })
        }
        "tool_result" => {
            let name = event
                .fields
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let output = event
                .fields
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tool_use_id = event
                .fields
                .get("toolUseId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Some(AgentSdkEvent::ToolResult {
                agent_id: aid,
                name,
                output,
                tool_use_id,
            })
        }
        "tool_input_delta" => {
            let partial_json = event
                .fields
                .get("partialJson")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(AgentSdkEvent::ToolInputDelta {
                agent_id: aid,
                partial_json,
            })
        }
        "session" => {
            let session_id = event.fields.get("sessionId")?.as_str()?.to_string();
            Some(AgentSdkEvent::Session {
                agent_id: aid,
                session_id,
            })
        }
        "result" => {
            let cost_usd = event.fields.get("costUsd").and_then(|v| v.as_f64());
            let input_tokens = event.fields.get("inputTokens").and_then(|v| v.as_u64());
            let output_tokens = event.fields.get("outputTokens").and_then(|v| v.as_u64());
            Some(AgentSdkEvent::Result {
                agent_id: aid,
                cost_usd,
                input_tokens,
                output_tokens,
            })
        }
        "done" => {
            let session_id = event
                .fields
                .get("sessionId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let cost_usd = event.fields.get("costUsd").and_then(|v| v.as_f64());
            Some(AgentSdkEvent::Done {
                agent_id: aid,
                session_id,
                cost_usd,
            })
        }
        "error" => {
            let message = event
                .fields
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            Some(AgentSdkEvent::Error {
                agent_id: aid,
                message,
            })
        }
        "debug" => {
            // Phase 1 diagnostic: log bridge debug events (info level to bypass holt_lib=info filter)
            let source = event
                .fields
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            tracing::info!(
                agent_id = agent_id,
                source = source,
                fields = ?event.fields,
                "SDK bridge diagnostic"
            );
            None
        }
        "bridge_status" => {
            let bridge_kind = event
                .fields
                .get("bridgeKind")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let tool_surface = event
                .fields
                .get("toolSurface")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let holt_mcp_mounted = event
                .fields
                .get("holtMcpMounted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let tool_count = event
                .fields
                .get("toolCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let mcp_url = event
                .fields
                .get("mcpUrl")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Some(AgentSdkEvent::BridgeStatus {
                agent_id: aid,
                bridge_kind,
                tool_surface,
                holt_mcp_mounted,
                tool_count,
                mcp_url,
            })
        }
        _ => None,
    }
}

/// Convert an `AgentSdkEvent` into a `ConversationMessage` for history reconstruction.
pub fn event_to_conversation_message(event: &AgentSdkEvent) -> Option<ConversationMessage> {
    match event {
        AgentSdkEvent::ToolResult {
            name,
            output,
            tool_use_id,
            ..
        } => Some(ConversationMessage {
            role: MessageRole::Tool,
            content: output.clone(),
            content_blocks: None,
            timestamp: Utc::now(),
            token_estimate: Some(output.len() as u32 / 4),
            tool_calls: None,
            tool_call_id: tool_use_id.clone().or_else(|| Some(name.clone())),
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        }),
        AgentSdkEvent::Error { message, .. } => Some(ConversationMessage {
            role: MessageRole::Assistant,
            content: format!("[error] {}", message),
            content_blocks: None,
            timestamp: Utc::now(),
            token_estimate: Some(message.len() as u32 / 4),
            tool_calls: None,
            tool_call_id: None,
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        }),
        _ => None,
    }
}

/// Flush accumulated token buffer into an Assistant conversation message.
pub fn flush_token_buffer(tokens: &str, thinking: Option<&str>) -> Option<ConversationMessage> {
    if tokens.is_empty() && thinking.is_none() {
        return None;
    }
    Some(ConversationMessage {
        role: MessageRole::Assistant,
        content: tokens.to_string(),
        content_blocks: None,
        timestamp: Utc::now(),
        token_estimate: Some(tokens.len() as u32 / 4),
        tool_calls: None,
        tool_call_id: None,
        outcome_tag: None,
        thinking_content: thinking.map(|s| s.to_string()),
        responses_phase: None,
        reasoning_items: None,
    })
}

// ── NDJSON transport ──────────────────────────────────────────────────

/// Channel capacity for bridge event and tool request streams.
/// Bounded to provide backpressure when the event processing loop is slow.
const BRIDGE_CHANNEL_CAPACITY: usize = 256;

/// Spawn result — the bridge process handle + separated channel receivers.
/// Receivers are extracted at spawn time so background tasks own them directly
/// without going through the BRIDGES registry lock.
pub struct BridgeSpawnResult {
    pub handle: BridgeHandle,
    pub events_rx: mpsc::Receiver<NdjsonEvent>,
    pub tool_requests_rx: mpsc::Receiver<ToolRequest>,
}

/// The part of the bridge stored in the BRIDGES registry.
/// Only contains what's needed for sending queries and shutdown — NOT receivers.
pub struct BridgeHandle {
    stdin: Arc<Mutex<BufWriter<ChildStdin>>>,
    child: Arc<Mutex<Child>>,
    reader_task: Option<tauri::async_runtime::JoinHandle<()>>,
    stderr_task: Option<tauri::async_runtime::JoinHandle<()>>,
    event_task: Option<tauri::async_runtime::JoinHandle<()>>,
    tool_task: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl BridgeHandle {
    pub async fn write_line(&self, event: &Value) -> Result<(), String> {
        let json =
            serde_json::to_string(event).map_err(|e| format!("Failed to serialize: {}", e))?;
        let mut writer = self.stdin.lock().await;
        writer
            .write_all(json.as_bytes())
            .await
            .map_err(|e| format!("Failed to write: {}", e))?;
        writer
            .write_all(b"\n")
            .await
            .map_err(|e| format!("Failed to write newline: {}", e))?;
        writer
            .flush()
            .await
            .map_err(|e| format!("Failed to flush: {}", e))?;
        Ok(())
    }

    pub async fn is_alive(&self) -> bool {
        let mut child = self.child.lock().await;
        matches!(child.try_wait(), Ok(None))
    }

    pub async fn kill(&self) -> Result<(), String> {
        if let Some(task) = &self.reader_task {
            task.abort();
        }
        if let Some(task) = &self.stderr_task {
            task.abort();
        }
        if let Some(task) = &self.event_task {
            task.abort();
        }
        if let Some(task) = &self.tool_task {
            task.abort();
        }
        let mut child = self.child.lock().await;
        child
            .kill()
            .await
            .map_err(|e| format!("Failed to kill bridge: {}", e))
    }
}

fn spawn_stderr_drain_task(
    stderr: ChildStderr,
    process_label: String,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        tracing::debug!(process = %process_label, stderr = %trimmed, "Bridge stderr");
                    }
                }
                Err(e) => {
                    tracing::warn!(process = %process_label, "Bridge stderr read error: {}", e);
                    break;
                }
            }
        }
    })
}

/// Spawn a bridge process. Returns the handle (for BRIDGES registry) and
/// channel receivers (for background tasks to own directly).
async fn spawn_bridge_process(
    command: &str,
    args: &[&str],
    env: &HashMap<String, String>,
) -> Result<BridgeSpawnResult, String> {
    let mut cmd = Command::new(command);
    cmd.args(args)
        .envs(env.iter())
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Ensure bridge child dies when the parent (Holt) exits, even on
    // hard exits (SIGKILL, crash, etc). kill_on_drop only works if the
    // tokio Child is dropped cleanly, which doesn't happen for statics.
    // PR_SET_PDEATHSIG asks the kernel to send SIGKILL to the child when
    // its parent process terminates — no userspace cleanup needed.
    #[cfg(target_os = "linux")]
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn bridge: {}", e))?;

    let stderr = child.stderr.take().ok_or("No stderr")?;
    let stdout = child.stdout.take().ok_or("No stdout")?;
    let stdin = child.stdin.take().ok_or("No stdin")?;

    let writer = Arc::new(Mutex::new(BufWriter::new(stdin)));
    let (events_tx, events_rx) = mpsc::channel(BRIDGE_CHANNEL_CAPACITY);
    let (tool_tx, tool_requests_rx) = mpsc::channel(BRIDGE_CHANNEL_CAPACITY);
    let stderr_task = spawn_stderr_drain_task(stderr, command.to_string());

    // Background reader task — parses each line, routes by type
    let reader_task = tauri::async_runtime::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let event: NdjsonEvent = match serde_json::from_str(trimmed) {
                        Ok(e) => e,
                        Err(e) => {
                            tracing::warn!("Bridge sent invalid NDJSON: {} — line: {}", e, trimmed);
                            continue;
                        }
                    };

                    match event.event_type.as_str() {
                        "tool_request" => {
                            let name = event
                                .fields
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args = event
                                .fields
                                .get("args")
                                .cloned()
                                .unwrap_or(Value::Object(Default::default()));
                            let request_id = event
                                .fields
                                .get("requestId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let _ = tool_tx
                                .send(ToolRequest {
                                    name,
                                    args,
                                    request_id,
                                })
                                .await;
                        }
                        // permission_request events are also routed to tool channel
                        // (Rust handles routing to approval system)
                        "permission_request" => {
                            let name = format!(
                                "__permission__{}",
                                event
                                    .fields
                                    .get("tool")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown")
                            );
                            let args = event
                                .fields
                                .get("input")
                                .cloned()
                                .unwrap_or(Value::Object(Default::default()));
                            let request_id = event
                                .fields
                                .get("requestId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let _ = tool_tx
                                .send(ToolRequest {
                                    name,
                                    args,
                                    request_id,
                                })
                                .await;
                        }
                        _ => {
                            let _ = events_tx.send(event).await;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Bridge stdout read error: {}", e);
                    break;
                }
            }
        }
    });

    Ok(BridgeSpawnResult {
        handle: BridgeHandle {
            stdin: writer,
            child: Arc::new(Mutex::new(child)),
            reader_task: Some(reader_task),
            stderr_task: Some(stderr_task),
            event_task: None,
            tool_task: None,
        },
        events_rx,
        tool_requests_rx,
    })
}

// ── Bridge lifecycle ──────────────────────────────────────────────────

/// Global bridge registry keyed by agent ID.
/// Only holds BridgeHandle (stdin writer + child) — NOT channel receivers.
static BRIDGES: LazyLock<RwLock<HashMap<String, BridgeHandle>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Get or spawn an Agent SDK bridge for the given agent.
pub async fn get_or_spawn_bridge(
    agent_id: &str,
    agent: &AgentSlot,
    app_state: &Arc<AppState>,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<(), String> {
    // Check if already alive
    {
        let bridges = BRIDGES.read().await;
        if let Some(entry) = bridges.get(agent_id) {
            if entry.is_alive().await {
                return Ok(());
            }
        }
    }

    // Kill old bridge (and abort its spawned tasks) before spawning a new one.
    // This prevents zombie event/tool processing tasks from lingering.
    {
        let mut bridges = BRIDGES.write().await;
        if let Some(old) = bridges.remove(agent_id) {
            tracing::info!(agent_id, "Cleaning up dead bridge and its spawned tasks");
            let _ = old.kill().await;
        }
    }

    let sdk_config = app_state.config.read().await.agent_sdk.clone();
    let node_path = sdk_config.node_path.clone();
    let bridge_path = resolve_bridge_path(agent, app_handle)?;

    let mut env = HashMap::new();
    env.insert("NODE_NO_WARNINGS".to_string(), "1".to_string());

    // Spawn bridge — get handle + channel receivers
    let spawn_result = spawn_bridge_process(&node_path, &[&bridge_path], &env).await?;
    let BridgeSpawnResult {
        handle,
        mut events_rx,
        mut tool_requests_rx,
    } = spawn_result;

    // Build system prompt with persona prefix
    let model = resolve_sdk_model(agent, &sdk_config);
    let memory_active = app_state.has_memory_engine();
    let persona_prefix = {
        let mut files = crate::runtime::persona::load_persona_files(agent_id, memory_active);

        // Living Persona: inject dynamic memory profile after static files
        if memory_active {
            if let Some(engine) = app_state.get_memory_engine() {
                if let Some(dynamic_profile) = crate::runtime::persona::fetch_dynamic_profile(
                    &engine,
                    agent_id,
                    crate::runtime::persona::DYNAMIC_PROFILE_TOKEN_BUDGET,
                )
                .await
                {
                    tracing::info!(agent_id, "Dynamic memory profile injected into SDK persona");
                    files.push(dynamic_profile);
                }
            }
        }

        crate::runtime::persona::format_as_system_prompt(&files)
    };
    let sdk_tool_awareness = build_sdk_tool_awareness(agent_id, app_state).await;
    let full_system_prompt = if persona_prefix.is_empty() {
        if sdk_tool_awareness.is_empty() {
            agent.system_prompt.clone()
        } else {
            format!("{}\n\n{}", agent.system_prompt, sdk_tool_awareness)
        }
    } else {
        let base = format!("{}\n\n{}", persona_prefix, agent.system_prompt);
        if sdk_tool_awareness.is_empty() {
            base
        } else {
            format!("{}\n\n{}", base, sdk_tool_awareness)
        }
    };

    // Compute hash of system prompt — invalidate session if persona changed
    use std::hash::{Hash, Hasher};
    let prompt_hash = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        full_system_prompt.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    };

    let session_id = if agent.agent_session_id.is_some()
        && agent.system_prompt_hash.as_deref() != Some(&prompt_hash)
    {
        tracing::info!(
            agent_id = agent_id,
            "System prompt changed — invalidating stale session for fresh persona injection"
        );
        // Clear stale session on the agent slot
        let mut agents_w = app_state.agents.write().await;
        if let Some(a) = agents_w.iter_mut().find(|a| a.id == agent_id) {
            a.agent_session_id = None;
            a.system_prompt_hash = Some(prompt_hash.clone());
            a.is_dirty = true;
        }
        None
    } else {
        // Hash matches or no session — store hash for future comparisons
        if agent.system_prompt_hash.as_deref() != Some(&prompt_hash) {
            let mut agents_w = app_state.agents.write().await;
            if let Some(a) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                a.system_prompt_hash = Some(prompt_hash.clone());
                a.is_dirty = true;
            }
        }
        agent.agent_session_id.clone()
    };

    tracing::info!(
        agent_id = agent_id,
        persona_chars = full_system_prompt.len(),
        memory_active = memory_active,
        has_session_id = session_id.is_some(),
        prompt_hash = %prompt_hash,
        "SDK bridge init — systemPrompt {} chars",
        full_system_prompt.len()
    );

    // Send init
    let init_msg = json!({
        "type": "init",
        "agentId": agent_id,
        "cwd": agent.working_directory.to_string_lossy(),
        "systemPrompt": full_system_prompt,
        "model": model,
        "autonomous": agent.autonomous,
        "sessionId": session_id,
    });

    handle.write_line(&init_msg).await?;

    // Store handle in registry (only the stdin writer + child — NOT receivers)
    {
        let mut bridges = BRIDGES.write().await;
        bridges.insert(agent_id.to_string(), handle);
    }

    // Spawn event processing task — owns events_rx directly, no BRIDGES lock needed
    let agents = Arc::clone(&app_state.agents);
    let app_state_events = Arc::clone(app_state);
    let app_handle_clone = app_handle.cloned();
    let agent_id_owned = agent_id.to_string();
    let token_buf = Arc::new(RwLock::new(String::new()));
    let thinking_buf = Arc::new(RwLock::new(String::new()));
    let token_buf_clone = Arc::clone(&token_buf);
    let thinking_buf_clone = Arc::clone(&thinking_buf);

    // Clone agents handle for the post-loop status update
    let agents_cleanup = Arc::clone(&app_state.agents);
    let agent_id_cleanup = agent_id.to_string();

    // ── StreamCoalescer integration ─────────────────────────────────
    // Route ALL frontend emissions through the StreamCoalescer. Unifies
    // event path with engine loop: Token/Thinking coalesced at 50ms,
    // ToolResult batched (deferred), ToolCall/Done/Error immediate.
    // Internal token accumulation for conversation building is decoupled.

    let event_task = tauri::async_runtime::spawn(async move {
        // Create coalescer wired to the app handle (or no-op if headless)
        let coalescer = app_handle_clone.as_ref().map(|handle| {
            crate::runtime::stream_coalescer::StreamCoalescer::with_app_handle(handle.clone())
        });
        let _flush_task = coalescer.as_ref().map(|c| c.start_flush_task());

        loop {
            let event = events_rx.recv().await;
            let Some(event) = event else {
                break;
            };
            let Some(sdk_event) = map_ndjson_event(&agent_id_owned, &event) else {
                continue;
            };

            match &sdk_event {
                // ── Tokens: accumulate internally + push to coalescer ──
                AgentSdkEvent::Token { text, .. } => {
                    let mut buf = token_buf_clone.write().await;
                    if buf.len() < 1_048_576 {
                        buf.push_str(text);
                    }
                    drop(buf);
                    if let Some(ref c) = coalescer {
                        use crate::providers::types::{StreamEvent, StreamEventType};
                        c.push(StreamEvent {
                            agent_id: agent_id_owned.clone(),
                            event_type: StreamEventType::Token,
                            data: json!({ "text": text }),
                        });
                    }
                }
                AgentSdkEvent::Thinking { text, .. } => {
                    let mut buf = thinking_buf_clone.write().await;
                    if buf.len() < 1_048_576 {
                        buf.push_str(text);
                    }
                    drop(buf);
                    if let Some(ref c) = coalescer {
                        use crate::providers::types::{StreamEvent, StreamEventType};
                        c.push(StreamEvent {
                            agent_id: agent_id_owned.clone(),
                            event_type: StreamEventType::Thinking,
                            data: json!({ "text": text }),
                        });
                    }
                }

                // ── Significant: coalescer handles flush-before-emit ──
                AgentSdkEvent::ToolCall {
                    agent_id,
                    name,
                    args,
                    tool_use_id,
                } => {
                    if let Some(ref c) = coalescer {
                        use crate::providers::types::{StreamEvent, StreamEventType};
                        c.push(StreamEvent {
                            agent_id: agent_id.clone(),
                            event_type: StreamEventType::ToolCall,
                            data: json!({
                                "tool": name,
                                "arguments": args,
                                "result": "",
                                "tool_call_id": tool_use_id,
                            }),
                        });
                    }
                }
                AgentSdkEvent::ToolResult {
                    agent_id,
                    name,
                    output,
                    tool_use_id,
                } => {
                    if let Some(ref c) = coalescer {
                        use crate::providers::types::{StreamEvent, StreamEventType};
                        c.push(StreamEvent {
                            agent_id: agent_id.clone(),
                            event_type: StreamEventType::ToolResult,
                            data: json!({
                                "tool": name,
                                "result": output,
                                "tool_call_id": tool_use_id,
                            }),
                        });
                    }
                    // Single lock acquisition for conversation + counter
                    if let Some(msg) = event_to_conversation_message(&sdk_event) {
                        let mut agents_w = agents.write().await;
                        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
                            agent.add_message(msg);
                            agent.session_memory.tool_calls_since_extraction += 1;
                            agent.is_dirty = true;
                        }
                    }
                }
                AgentSdkEvent::Session { session_id, .. } => {
                    let sid = session_id.clone();
                    let mut agents_w = agents.write().await;
                    if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
                        agent.agent_session_id = Some(sid);
                        agent.is_dirty = true;
                    }
                }
                AgentSdkEvent::Result {
                    input_tokens,
                    output_tokens,
                    ..
                } => {
                    let input = *input_tokens;
                    let output = *output_tokens;
                    let mut agents_w = agents.write().await;
                    if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
                        if let Some(i) = input {
                            agent.session_prompt_tokens += i;
                            agent.current_context_tokens = i;
                        }
                        if let Some(o) = output {
                            agent.session_completion_tokens += o;
                        }
                        agent.metadata.total_api_calls += 1;
                        agent.is_dirty = true;
                    }
                }
                AgentSdkEvent::Error { agent_id, message } => {
                    if let Some(ref c) = coalescer {
                        use crate::providers::types::{StreamEvent, StreamEventType};
                        c.push(StreamEvent {
                            agent_id: agent_id.clone(),
                            event_type: StreamEventType::Error,
                            data: json!({ "message": message }),
                        });
                    }
                    let error_msg = message.clone();
                    if let Some(msg) = event_to_conversation_message(&sdk_event) {
                        let mut agents_w = agents.write().await;
                        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
                            agent.add_message(msg);
                            agent.status = AgentStatus::Error(error_msg);
                        }
                    }
                }
                AgentSdkEvent::BridgeStatus {
                    bridge_kind,
                    tool_surface,
                    holt_mcp_mounted,
                    tool_count,
                    mcp_url,
                    ..
                } => {
                    tracing::info!(
                        agent_id = agent_id_owned,
                        bridge_kind = %bridge_kind,
                        tool_surface = %tool_surface,
                        holt_mcp_mounted,
                        tool_count,
                        mcp_url = mcp_url.as_deref().unwrap_or("none"),
                        "SDK bridge status"
                    );
                }
                AgentSdkEvent::Done { session_id, .. } => {
                    // Done is immediate in coalescer: flushes all
                    // pending text + deferred, then emits Done
                    if let Some(ref c) = coalescer {
                        use crate::providers::types::{StreamEvent, StreamEventType};
                        c.push(StreamEvent {
                            agent_id: agent_id_owned.clone(),
                            event_type: StreamEventType::Done,
                            data: json!({ "session_id": session_id }),
                        });
                    }

                    // Drain token and thinking accumulators
                    let tokens = {
                        let mut buf = token_buf_clone.write().await;
                        std::mem::take(&mut *buf)
                    };
                    let thinking = {
                        let mut buf = thinking_buf_clone.write().await;
                        let t = std::mem::take(&mut *buf);
                        if t.is_empty() {
                            None
                        } else {
                            Some(t)
                        }
                    };

                    // Single lock acquisition for all Done state updates
                    let sid_clone = session_id.clone();
                    let mut sdk_session_memory_capture: Option<(String, u64)> = None;
                    {
                        let mut agents_w = agents.write().await;
                        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
                            let flushed_assistant = if let Some(msg) =
                                flush_token_buffer(&tokens, thinking.as_deref())
                            {
                                agent.add_message(msg);
                                true
                            } else {
                                false
                            };
                            if flushed_assistant {
                                agent.session_memory.assistant_messages_since_extraction += 1;
                            }
                            if let Some(sid) = sid_clone {
                                agent.agent_session_id = Some(sid);
                            }
                            agent.status = AgentStatus::Idle;
                            agent.metadata.last_active = Some(Utc::now());
                            agent.is_dirty = true;

                            let sm_config =
                                crate::runtime::session_memory::SessionMemoryConfig::default();
                            if agent.agent_session_id.is_some()
                                && agent
                                    .session_memory
                                    .should_trigger(&sm_config, agent.current_context_tokens)
                            {
                                agent.session_memory.extraction_in_progress = true;
                                sdk_session_memory_capture = agent
                                    .agent_session_id
                                    .clone()
                                    .map(|sid| (sid, agent.current_context_tokens));
                            }
                        }
                    } // agents_w dropped here before any async calls

                    if let Some((sid, current_tokens)) = sdk_session_memory_capture {
                        crate::runtime::session_memory::spawn_sdk_session_memory_capture(
                            &app_state_events,
                            &agent_id_owned,
                            &sid,
                            current_tokens,
                            app_handle_clone.as_ref(),
                        );
                    }

                    // COGNITIVE INTAKE: Fire-and-forget extraction for SDK agents.
                    if !tokens.is_empty() {
                        if let Some(mem_engine) = app_state_events.get_memory_engine() {
                            let agents_ci = Arc::clone(&agents);
                            let agent_id_ci = agent_id_owned.clone();
                            let assistant_response = tokens.clone();
                            let embedder = mem_engine.embedder.clone();

                            tokio::spawn(async move {
                                let (agent_ns, user_msg) = {
                                    let agents_r = agents_ci.read().await;
                                    match agents_r.iter().find(|a| a.id == agent_id_ci) {
                                        Some(agent) => (
                                            crate::memory::MemoryEngine::agent_namespace(&agent.id),
                                            agent
                                                .cognitive_intake
                                                .last_user_message
                                                .clone()
                                                .unwrap_or_default(),
                                        ),
                                        None => return,
                                    }
                                };

                                let mut intake_state = {
                                    let agents_r = agents_ci.read().await;
                                    match agents_r.iter().find(|a| a.id == agent_id_ci) {
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
                                    &assistant_response,
                                )
                                .await;

                                // Write updated state back
                                {
                                    let mut agents_w = agents_ci.write().await;
                                    if let Some(agent) =
                                        agents_w.iter_mut().find(|a| a.id == agent_id_ci)
                                    {
                                        agent.cognitive_intake = intake_state;
                                    }
                                }

                                match result {
                                    crate::runtime::cognitive_intake::IntakeResult::Forwarded { memory_id } => {
                                        tracing::debug!(
                                            agent_id = agent_id_ci,
                                            memory_id = %memory_id,
                                            "SDK cognitive intake forwarded"
                                        );
                                    }
                                    crate::runtime::cognitive_intake::IntakeResult::HillockError(e) => {
                                        tracing::warn!(
                                            agent_id = agent_id_ci,
                                            error = %e,
                                            "SDK cognitive intake bridge error"
                                        );
                                    }
                                    _ => {}
                                }
                            });
                        }
                    }

                    // Auto-wake: pending A2A messages trigger a new turn
                    let has_pending = app_state_events
                        .a2a_registry
                        .has_pending_messages(&agent_id_owned)
                        .await;
                    if has_pending {
                        tracing::info!(
                            agent_id = agent_id_owned,
                            "A2A auto-wake: pending messages, re-emitting wake event"
                        );
                        if let Some(ref handle) = app_handle_clone {
                            use tauri::Emitter;
                            let _ = handle.emit(
                                "agent-a2a-wake",
                                json!({
                                    "target_agent_id": agent_id_owned,
                                    "from_agent_id": "system",
                                    "from_agent_name": "System",
                                }),
                            );
                        }
                    }
                }
                _ => {}
            } // end match
        } // end loop
        tracing::info!(agent_id = agent_id_owned, "Event processing loop ended");

        // Bridge died — update agent status so it doesn't appear stuck in "Working"
        let mut agents_w = agents_cleanup.write().await;
        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_cleanup) {
            if matches!(agent.status, AgentStatus::Working) {
                tracing::warn!(
                    agent_id = agent_id_cleanup,
                    "Bridge died while agent was Working — setting status to Idle"
                );
                agent.status = AgentStatus::Idle;
                agent.is_dirty = true;
            }
        }
    });

    // Spawn tool request handler — owns tool_requests_rx directly
    let agent_id_tools = agent_id.to_string();
    let app_state_tools = Arc::clone(app_state);
    let app_handle_tools = app_handle.cloned();
    let tool_task = tauri::async_runtime::spawn(async move {
        while let Some(request) = tool_requests_rx.recv().await {
            let result = handle_tool_request(
                &agent_id_tools,
                &request,
                &app_state_tools,
                app_handle_tools.as_ref(),
            )
            .await;

            // Send tool_response back to bridge — brief lock on BRIDGES
            let response = json!({
                "type": "tool_response",
                "requestId": request.request_id,
                "result": result,
            });

            let bridges = BRIDGES.read().await;
            if let Some(entry) = bridges.get(&agent_id_tools) {
                let _ = entry.write_line(&response).await;
            }
        }
        tracing::info!(agent_id = agent_id_tools, "Tool request handler loop ended");
    });

    // Store task handles in the bridge entry so they get aborted on re-spawn
    {
        let mut bridges = BRIDGES.write().await;
        if let Some(entry) = bridges.get_mut(agent_id) {
            entry.event_task = Some(event_task);
            entry.tool_task = Some(tool_task);
        }
    }

    tracing::info!(agent_id, "Agent SDK bridge spawned");
    Ok(())
}

/// Send a query to the Agent SDK bridge.
pub async fn agent_sdk_query(
    agent_id: &str,
    prompt: &str,
    app_state: &Arc<AppState>,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<(), String> {
    // Set agent to Working
    {
        let mut agents = app_state.agents.write().await;
        if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
            agent.status = AgentStatus::Working;
            if agent.session_start.is_none() {
                agent.session_start = Some(Utc::now());
            }
            agent.is_dirty = true;
        }
    }

    // TURN-BOUNDARY INJECTION: Drain pending A2A messages
    let a2a_messages = app_state.a2a_registry.drain_messages(agent_id).await;

    if prompt.trim().is_empty() && a2a_messages.is_empty() {
        let mut agents = app_state.agents.write().await;
        if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
            agent.status = AgentStatus::Idle;
            agent.metadata.last_active = Some(Utc::now());
            agent.is_dirty = true;
        }
        return Ok(());
    }

    // Add user message to conversation only for real user prompts, not wake sentinels.
    if !prompt.trim().is_empty() {
        let mut agents = app_state.agents.write().await;
        if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
            agent.add_message(ConversationMessage {
                role: MessageRole::User,
                content: prompt.to_string(),
                content_blocks: None,
                timestamp: Utc::now(),
                token_estimate: Some(prompt.len() as u32 / 4),
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            });
        }
    }

    let effective_prompt = if a2a_messages.is_empty() {
        prompt.to_string()
    } else {
        // Wrap A2A messages in a system context block that is structurally
        // distinct from the user prompt. SDK agents (Opus/Sonnet with persona
        // conditioning) handle this reliably. The wrapper ensures the model
        // treats A2A as context metadata, not human conversation.
        let a2a_section =
            crate::runtime::engine::turn_injection::format_a2a_system_section(&a2a_messages);
        let injected = if !prompt.trim().is_empty() {
            format!(
                "[SYSTEM CONTEXT -- NOT FROM HUMAN]\n{}\n[END SYSTEM CONTEXT]\n\n{}",
                a2a_section, prompt
            )
        } else {
            format!(
                "[SYSTEM CONTEXT -- NOT FROM HUMAN]\n{}\n[END SYSTEM CONTEXT]",
                a2a_section
            )
        };
        tracing::info!(
            agent_id,
            count = a2a_messages.len(),
            "Injected A2A messages into SDK query (system context wrapper)"
        );
        injected
    };

    // Memory injection with relevance gate + session dedup.
    // Wrapped in a timeout so a slow bridge doesn't block the query from starting.
    const MEMORY_INJECTION_TIMEOUT_SECS: u64 = 5;
    let effective_prompt = if let Some(mem_engine) = app_state.get_memory_engine() {
        let agent_ns = crate::memory::MemoryEngine::agent_namespace(agent_id);
        let (mut current_msg, recent_msgs) = {
            let agents = app_state.agents.read().await;
            agents
                .iter()
                .find(|a| a.id == agent_id)
                .map(|a| {
                    let user_msgs: Vec<String> = a
                        .conversation
                        .messages
                        .iter()
                        .rev()
                        .filter(|m| m.role == crate::runtime::agent::MessageRole::User)
                        .take(3)
                        .map(|m| m.content.clone())
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    let current = user_msgs.last().cloned().unwrap_or_default();
                    let recent = if user_msgs.len() > 1 {
                        user_msgs[..user_msgs.len() - 1].to_vec()
                    } else {
                        vec![]
                    };
                    (current, recent)
                })
                .unwrap_or_default()
        };
        if !effective_prompt.trim().is_empty() {
            current_msg = effective_prompt.clone();
        }
        // Injection is retired; the budget is reported as 0, never spent.
        let budget = 0usize;
        let memory_result = tokio::time::timeout(
            Duration::from_secs(MEMORY_INJECTION_TIMEOUT_SECS),
            crate::memory::injection::build_memory_context(
                &mem_engine,
                &app_state.session_state,
                &mem_engine.embedder,
                agent_id,
                &agent_ns,
                &current_msg,
                &recent_msgs,
                budget,
            ),
        )
        .await;
        match memory_result {
            Err(_elapsed) => {
                tracing::warn!(
                    agent_id,
                    "Memory injection timed out after {}s, continuing without",
                    MEMORY_INJECTION_TIMEOUT_SECS
                );
                let trace_session_id = {
                    let agents = app_state.agents.read().await;
                    agents
                        .iter()
                        .find(|a| a.id == agent_id)
                        .and_then(|a| a.agent_session_id.clone())
                        .unwrap_or_else(|| format!("agent-sdk-{}", agent_id))
                };
                let report = MemoryPipelineReport {
                    source: "agent_sdk".to_string(),
                    agent_id: agent_id.to_string(),
                    agent_ns: Some(agent_ns.clone()),
                    session_id: trace_session_id,
                    current_message: current_msg.clone(),
                    recent_messages: recent_msgs.clone(),
                    memory_enabled: true,
                    memory_status: "timeout".to_string(),
                    memory_error: Some(format!(
                        "Timed out after {}s",
                        MEMORY_INJECTION_TIMEOUT_SECS
                    )),
                    a2a_messages_injected: a2a_messages.len(),
                    subagent_results_injected: 0,

                    memory_context: None,
                };
                if let Err(e) = write_memory_pipeline_trace(&app_state.trace_engine, &report) {
                    tracing::warn!(agent_id, "memory pipeline trace write failed: {e}");
                }
                effective_prompt
            }
            Ok(inner) => match inner {
                Ok(result) if !result.text.is_empty() => {
                    tracing::info!(
                        agent_id,
                        memory_count = result.ids.len(),
                        "Injected memories into SDK query"
                    );
                    let trace_session_id = {
                        let agents = app_state.agents.read().await;
                        agents
                            .iter()
                            .find(|a| a.id == agent_id)
                            .and_then(|a| a.agent_session_id.clone())
                            .unwrap_or_else(|| format!("agent-sdk-{}", agent_id))
                    };
                    let report = MemoryPipelineReport {
                        source: "agent_sdk".to_string(),
                        agent_id: agent_id.to_string(),
                        agent_ns: Some(agent_ns.clone()),
                        session_id: trace_session_id,
                        current_message: current_msg.clone(),
                        recent_messages: recent_msgs.clone(),
                        memory_enabled: true,
                        memory_status: "reported".to_string(),
                        memory_error: None,
                        a2a_messages_injected: a2a_messages.len(),
                        subagent_results_injected: 0,

                        memory_context: Some(result.report),
                    };
                    if let Err(e) = write_memory_pipeline_trace(&app_state.trace_engine, &report) {
                        tracing::warn!(agent_id, "memory pipeline trace write failed: {e}");
                    }
                    format!("{}\n\n{}", result.text, effective_prompt)
                }
                Ok(result) => {
                    let trace_session_id = {
                        let agents = app_state.agents.read().await;
                        agents
                            .iter()
                            .find(|a| a.id == agent_id)
                            .and_then(|a| a.agent_session_id.clone())
                            .unwrap_or_else(|| format!("agent-sdk-{}", agent_id))
                    };
                    let report = MemoryPipelineReport {
                        source: "agent_sdk".to_string(),
                        agent_id: agent_id.to_string(),
                        agent_ns: Some(agent_ns.clone()),
                        session_id: trace_session_id,
                        current_message: current_msg.clone(),
                        recent_messages: recent_msgs.clone(),
                        memory_enabled: true,
                        memory_status: "reported".to_string(),
                        memory_error: None,
                        a2a_messages_injected: a2a_messages.len(),
                        subagent_results_injected: 0,

                        memory_context: Some(result.report),
                    };
                    if let Err(e) = write_memory_pipeline_trace(&app_state.trace_engine, &report) {
                        tracing::warn!(agent_id, "memory pipeline trace write failed: {e}");
                    }
                    effective_prompt
                }
                Err(e) => {
                    tracing::warn!(agent_id, error = %e, "Memory injection failed, continuing without");
                    let trace_session_id = {
                        let agents = app_state.agents.read().await;
                        agents
                            .iter()
                            .find(|a| a.id == agent_id)
                            .and_then(|a| a.agent_session_id.clone())
                            .unwrap_or_else(|| format!("agent-sdk-{}", agent_id))
                    };
                    let report = MemoryPipelineReport {
                        source: "agent_sdk".to_string(),
                        agent_id: agent_id.to_string(),
                        agent_ns: Some(agent_ns.clone()),
                        session_id: trace_session_id,
                        current_message: current_msg.clone(),
                        recent_messages: recent_msgs.clone(),
                        memory_enabled: true,
                        memory_status: "error".to_string(),
                        memory_error: Some(e.to_string()),
                        a2a_messages_injected: a2a_messages.len(),
                        subagent_results_injected: 0,

                        memory_context: None,
                    };
                    if let Err(trace_err) =
                        write_memory_pipeline_trace(&app_state.trace_engine, &report)
                    {
                        tracing::warn!(agent_id, "memory pipeline trace write failed: {trace_err}");
                    }
                    effective_prompt
                }
            }, // end Ok(inner) => match inner
        } // end match memory_result
    } else {
        let report = MemoryPipelineReport {
            source: "agent_sdk".to_string(),
            agent_id: agent_id.to_string(),
            agent_ns: None,
            session_id: format!("agent-sdk-{}", agent_id),
            current_message: prompt.to_string(),
            recent_messages: vec![],
            memory_enabled: false,
            memory_status: "disabled".to_string(),
            memory_error: None,
            a2a_messages_injected: a2a_messages.len(),
            subagent_results_injected: 0,
            memory_context: None,
        };
        if let Err(e) = write_memory_pipeline_trace(&app_state.trace_engine, &report) {
            tracing::warn!(agent_id, "memory pipeline trace write failed: {e}");
        }
        effective_prompt
    };

    // Ensure bridge is running
    let agent_snapshot = {
        let agents = app_state.agents.read().await;
        agents
            .iter()
            .find(|a| a.id == agent_id)
            .cloned()
            .ok_or_else(|| format!("Agent {} not found", agent_id))?
    };

    get_or_spawn_bridge(agent_id, &agent_snapshot, app_state, app_handle).await?;

    // Store user message for cognitive intake (Done handler needs it)
    {
        let mut agents_w = app_state.agents.write().await;
        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
            agent.cognitive_intake.last_user_message = Some(prompt.to_string());
        }
    }

    // Send query via NDJSON
    let query_msg = json!({
        "type": "query",
        "prompt": effective_prompt,
    });

    let bridges = BRIDGES.read().await;
    let entry = bridges
        .get(agent_id)
        .ok_or_else(|| format!("Bridge not found for agent {}", agent_id))?;

    entry.write_line(&query_msg).await?;

    Ok(())
}

/// Abort the current query for an agent's bridge.
pub async fn abort_bridge(agent_id: &str) -> Result<(), String> {
    let bridges = BRIDGES.read().await;
    if let Some(entry) = bridges.get(agent_id) {
        entry.write_line(&json!({"type": "abort"})).await?;
    }
    Ok(())
}

/// Shut down a specific agent's bridge.
pub async fn shutdown_bridge(agent_id: &str) -> Result<(), String> {
    let mut bridges = BRIDGES.write().await;
    if let Some(entry) = bridges.remove(agent_id) {
        let _ = entry.write_line(&json!({"type": "shutdown"})).await;
        if entry.is_alive().await {
            entry.kill().await?;
        }
    }
    Ok(())
}

/// Shut down all bridges (called during app shutdown).
pub async fn shutdown_all_bridges() {
    let mut bridges = BRIDGES.write().await;
    let ids: Vec<String> = bridges.keys().cloned().collect();
    for id in ids {
        if let Some(entry) = bridges.remove(&id) {
            let _ = entry.write_line(&json!({"type": "shutdown"})).await;
            if entry.is_alive().await {
                let _ = entry.kill().await;
            }
        }
    }
}

// ── MCP tool call routing ───────────────────────────────────────────────

/// Handle a `tool_request` from the bridge — routes to reduced SDK tool registry.
async fn handle_tool_request(
    agent_id: &str,
    request: &ToolRequest,
    app_state: &Arc<AppState>,
    app_handle: Option<&tauri::AppHandle>,
) -> Value {
    let (working_directory, workspace_root) = {
        let agents = app_state.agents.read().await;
        match agents.iter().find(|a| a.id == agent_id) {
            Some(agent) => (
                agent.working_directory.clone(),
                agent.working_directory.clone(),
            ),
            None => return json!({"error": format!("Agent {} not found", agent_id)}),
        }
    };

    let tools_config = load_agent_tools_config(agent_id);

    // Build reduced registry (intelligence tools only — single sandbox principle)
    let tool_registry =
        ToolRegistry::build_for_sdk_agent_with_tools(&tools_config, Some(&app_state.plugin_host))
            .await;

    let tool_ctx = ToolContext {
        agent_id: agent_id.to_string(),
        working_directory,
        workspace_root,
        trace_engine: Arc::clone(&app_state.trace_engine),
        app_handle: app_handle.cloned(),
        config: Arc::new(RwLock::new(app_state.config.read().await.clone())),
        shell_manager: Arc::clone(&app_state.shell_manager),
        session_id: format!("agent-sdk-{}", agent_id),
        vfl_registry: Arc::clone(&app_state.vfl_registry),
        subagent_manager: Arc::clone(&app_state.subagent_manager),
        agents: Arc::clone(&app_state.agents),
        a2a_registry: Arc::clone(&app_state.a2a_registry),
        a2a_send_guard: None,
        app_state: Some(Arc::clone(app_state)),
        http_client: app_state.http_client.clone(),
    };

    match tool_registry
        .execute(&request.name, request.args.clone(), &tool_ctx)
        .await
    {
        Ok(result) => result.content,
        Err(e) => json!({"error": e.message}),
    }
}

// ── Availability check ──────────────────────────────────────────────────

/// Check if the Agent SDK is available (node installed).
pub async fn is_agent_sdk_available(sdk_config: &AgentSdkBlock) -> Result<String, String> {
    let node_output = tokio::process::Command::new(&sdk_config.node_path)
        .arg("--version")
        .output()
        .await
        .map_err(|e| format!("Node.js not found ({}): {}", sdk_config.node_path, e))?;

    if !node_output.status.success() {
        return Err(format!(
            "Node.js check failed: {}",
            String::from_utf8_lossy(&node_output.stderr)
        ));
    }

    let node_version = String::from_utf8_lossy(&node_output.stdout)
        .trim()
        .to_string();
    Ok(format!("node {}", node_version))
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Resolve the bridge script path from Tauri resources or development path.
fn resolve_bridge_path(
    _agent: &AgentSlot,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<String, String> {
    let resource_name = "agent-sdk-bridge.bundle.js";

    if let Some(handle) = app_handle {
        use tauri::Manager;
        if let Ok(resource_path) = handle.path().resolve(
            &format!("resources/{resource_name}"),
            tauri::path::BaseDirectory::Resource,
        ) {
            if resource_path.exists() {
                return Ok(resource_path.to_string_lossy().to_string());
            }
        }
    }

    let dev_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join(resource_name);

    if dev_path.exists() {
        return Ok(dev_path.to_string_lossy().to_string());
    }

    Err(format!(
        "Agent SDK bridge script not found. Ensure {resource_name} is in resources/"
    ))
}

/// Resolve the model for an SDK agent.
fn resolve_sdk_model(agent: &AgentSlot, global: &AgentSdkBlock) -> String {
    agent
        .anthropic_settings
        .as_ref()
        .map(|s| s.model.clone())
        .unwrap_or_else(|| global.model.clone())
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_map_token_event() {
        let event = NdjsonEvent {
            event_type: "token".to_string(),
            fields: [("text".to_string(), json!("Hello world"))]
                .into_iter()
                .collect(),
        };
        let mapped = map_ndjson_event("agent-1", &event).unwrap();
        match mapped {
            AgentSdkEvent::Token { agent_id, text } => {
                assert_eq!(agent_id, "agent-1");
                assert_eq!(text, "Hello world");
            }
            other => panic!("Expected Token, got {:?}", other),
        }
    }

    #[test]
    fn test_map_tool_call_event() {
        let event = NdjsonEvent {
            event_type: "tool_call".to_string(),
            fields: [
                ("name".to_string(), json!("Read")),
                ("args".to_string(), json!({"file_path": "/tmp/foo.rs"})),
                ("toolUseId".to_string(), json!("toolu_123")),
            ]
            .into_iter()
            .collect(),
        };
        let mapped = map_ndjson_event("agent-2", &event).unwrap();
        match mapped {
            AgentSdkEvent::ToolCall {
                name,
                args,
                tool_use_id,
                ..
            } => {
                assert_eq!(name, "Read");
                assert_eq!(args["file_path"], "/tmp/foo.rs");
                assert_eq!(tool_use_id.unwrap(), "toolu_123");
            }
            other => panic!("Expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn test_map_done_event() {
        let event = NdjsonEvent {
            event_type: "done".to_string(),
            fields: [
                ("sessionId".to_string(), json!("sess-abc")),
                ("costUsd".to_string(), json!(0.042)),
            ]
            .into_iter()
            .collect(),
        };
        let mapped = map_ndjson_event("agent-3", &event).unwrap();
        match mapped {
            AgentSdkEvent::Done {
                session_id,
                cost_usd,
                ..
            } => {
                assert_eq!(session_id.unwrap(), "sess-abc");
                assert!((cost_usd.unwrap() - 0.042).abs() < f64::EPSILON);
            }
            other => panic!("Expected Done, got {:?}", other),
        }
    }

    #[test]
    fn test_map_error_event() {
        let event = NdjsonEvent {
            event_type: "error".to_string(),
            fields: [("message".to_string(), json!("rate limited"))]
                .into_iter()
                .collect(),
        };
        let mapped = map_ndjson_event("agent-4", &event).unwrap();
        match mapped {
            AgentSdkEvent::Error { message, .. } => assert_eq!(message, "rate limited"),
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_map_result_event() {
        let event = NdjsonEvent {
            event_type: "result".to_string(),
            fields: [
                ("inputTokens".to_string(), json!(1500)),
                ("outputTokens".to_string(), json!(300)),
                ("costUsd".to_string(), json!(0.01)),
            ]
            .into_iter()
            .collect(),
        };
        let mapped = map_ndjson_event("a", &event).unwrap();
        match mapped {
            AgentSdkEvent::Result {
                input_tokens,
                output_tokens,
                cost_usd,
                ..
            } => {
                assert_eq!(input_tokens.unwrap(), 1500);
                assert_eq!(output_tokens.unwrap(), 300);
                assert!((cost_usd.unwrap() - 0.01).abs() < f64::EPSILON);
            }
            other => panic!("Expected Result, got {:?}", other),
        }
    }

    #[test]
    fn test_map_bridge_status_event() {
        let event = NdjsonEvent {
            event_type: "bridge_status".to_string(),
            fields: [
                ("bridgeKind".to_string(), json!("claude")),
                ("toolSurface".to_string(), json!("holt_mcp")),
                ("holtMcpMounted".to_string(), json!(true)),
                ("toolCount".to_string(), json!(27)),
                ("mcpUrl".to_string(), json!("http://127.0.0.1:43123/mcp")),
            ]
            .into_iter()
            .collect(),
        };
        let mapped = map_ndjson_event("agent-5", &event).unwrap();
        match mapped {
            AgentSdkEvent::BridgeStatus {
                agent_id,
                bridge_kind,
                tool_surface,
                holt_mcp_mounted,
                tool_count,
                mcp_url,
            } => {
                assert_eq!(agent_id, "agent-5");
                assert_eq!(bridge_kind, "claude");
                assert_eq!(tool_surface, "holt_mcp");
                assert!(holt_mcp_mounted);
                assert_eq!(tool_count, 27);
                assert_eq!(mcp_url.as_deref(), Some("http://127.0.0.1:43123/mcp"));
            }
            other => panic!("Expected BridgeStatus, got {:?}", other),
        }
    }

    #[test]
    fn test_map_unknown_event() {
        let event = NdjsonEvent {
            event_type: "unknown_type".to_string(),
            fields: HashMap::new(),
        };
        assert!(map_ndjson_event("a", &event).is_none());
    }

    #[test]
    fn test_flush_token_buffer_empty() {
        assert!(flush_token_buffer("", None).is_none());
    }

    #[test]
    fn test_flush_token_buffer_with_thinking() {
        let msg = flush_token_buffer("Hello", Some("thinking...")).unwrap();
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "Hello");
        assert_eq!(msg.thinking_content.as_deref(), Some("thinking..."));
    }

    #[test]
    fn test_event_to_conversation_message_tool_result() {
        let event = AgentSdkEvent::ToolResult {
            agent_id: "a".to_string(),
            name: "Read".to_string(),
            output: "file contents".to_string(),
            tool_use_id: Some("toolu_456".to_string()),
        };
        let msg = event_to_conversation_message(&event).unwrap();
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.content, "file contents");
        assert_eq!(msg.tool_call_id.as_deref(), Some("toolu_456"));
    }

    #[test]
    fn test_event_to_conversation_message_error() {
        let event = AgentSdkEvent::Error {
            agent_id: "a".to_string(),
            message: "something broke".to_string(),
        };
        let msg = event_to_conversation_message(&event).unwrap();
        assert_eq!(msg.role, MessageRole::Assistant);
        assert!(msg.content.contains("something broke"));
    }

    #[test]
    fn test_ndjson_event_parse() {
        let line = r#"{"type":"token","text":"hello"}"#;
        let event: NdjsonEvent = serde_json::from_str(line).unwrap();
        assert_eq!(event.event_type, "token");
        assert_eq!(event.fields["text"], "hello");
    }

    #[test]
    fn test_ndjson_event_parse_permission_request() {
        let line = r#"{"type":"permission_request","tool":"Bash","input":{"command":"rm -rf /tmp"},"requestId":"req-2"}"#;
        let event: NdjsonEvent = serde_json::from_str(line).unwrap();
        assert_eq!(event.event_type, "permission_request");
        assert_eq!(event.fields["tool"], "Bash");
    }

    #[test]
    fn test_resolve_sdk_model_uses_anthropic_settings() {
        use crate::runtime::agent::AnthropicSettings;

        let mut agent = test_agent_slot();
        agent.anthropic_settings = Some(AnthropicSettings {
            model: "claude-opus-4-5".to_string(),
            thinking_enabled: true,
            thinking_budget: 16384,
            temperature: 1.0,
        });

        let global = AgentSdkBlock::default();
        let resolved = resolve_sdk_model(&agent, &global);
        assert_eq!(resolved, "claude-opus-4-5");
    }

    #[test]
    fn test_resolve_sdk_model_falls_back_to_global() {
        let agent = test_agent_slot();
        let mut global = AgentSdkBlock::default();
        global.model = "claude-sonnet-4-6-20250725".to_string();
        let resolved = resolve_sdk_model(&agent, &global);
        assert_eq!(resolved, "claude-sonnet-4-6-20250725");
    }

    /// Build a minimal AgentSlot for testing.
    fn test_agent_slot() -> AgentSlot {
        use crate::runtime::agent::*;
        use crate::runtime::connection::*;

        AgentSlot {
            id: "test".to_string(),
            name: "Test".to_string(),
            color: AgentColor::default(),
            protected: false,
            connection: ConnectionConfig::AgentSdk,
            protocol: AgentProtocol::AgentSdk,
            working_directory: std::path::PathBuf::from("/tmp"),
            status: AgentStatus::Idle,
            subagents: vec![],
            conversation: ConversationHistory::default(),
            tools: vec![],
            system_prompt: "Test prompt".to_string(),
            metadata: AgentMetadata::default(),
            limits: crate::runtime::limits::AgentLimits::default(),
            context_window_size: 200_000,
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
}
