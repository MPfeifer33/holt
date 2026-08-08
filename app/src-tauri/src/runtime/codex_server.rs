//! Codex V2 app-server transport layer.
//!
//! Manages the lifecycle of `codex app-server` processes: spawn with stdio
//! transport, perform the mandatory `initialize`/`initialized` handshake,
//! send JSON-RPC requests, receive NDJSON notifications, and shutdown.
//!
//! Transport note: `codex app-server --listen unix://…` speaks WebSocket over
//! the socket (HTTP upgrade), NOT raw NDJSON — writing bare JSON lines at it
//! gets the connection dropped. NDJSON is only spoken on stdio, so we pipe
//! stdin/stdout.
//!
//! Mirrors the BRIDGES pattern from `agent_sdk.rs` with a global
//! `CODEX_SERVERS` registry keyed by agent_id.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{broadcast, oneshot, Mutex, RwLock};

use super::codex_types::*;

// =============================================================================
// Constants
// =============================================================================

const CODEX_EXECUTABLE: &str = "codex";
const EVENT_CHANNEL_CAPACITY: usize = 256;

// =============================================================================
// Global registry
// =============================================================================

/// Global registry of running Codex app-server instances, keyed by agent_id.
static CODEX_SERVERS: LazyLock<RwLock<HashMap<String, CodexServerHandle>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Atomic request ID counter for JSON-RPC requests.
static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

// =============================================================================
// Error types
// =============================================================================

#[derive(Debug, thiserror::Error)]
pub enum CodexServerError {
    #[error("Failed to spawn codex app-server: {0}")]
    Spawn(String),

    #[error("Initialize handshake failed: {0}")]
    Handshake(String),

    #[error("No server running for agent {0}")]
    NotFound(String),

    #[error("Server process died for agent {0}")]
    ProcessDied(String),

    #[error("Failed to send request: {0}")]
    Send(String),

    #[error("Request timed out (id={0})")]
    RequestTimeout(u64),

    #[error("JSON-RPC error {code}: {message}")]
    JsonRpc { code: i64, message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Response channel closed for request {0}")]
    ResponseChannelClosed(u64),
}

// =============================================================================
// Types
// =============================================================================

/// Handle to a running codex app-server instance.
struct CodexServerHandle {
    /// The app-server child process.
    child: Arc<Mutex<Child>>,
    /// The child's stdin (for sending JSON-RPC requests as NDJSON).
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
    /// Background task reading NDJSON from the child's stdout.
    reader_task: tauri::async_runtime::JoinHandle<()>,
    /// Background task draining the child's stderr into tracing.
    stderr_task: tauri::async_runtime::JoinHandle<()>,
    /// Broadcast channel for fan-out of parsed events to multiple consumers.
    events_tx: broadcast::Sender<CodexEvent>,
    /// Pending JSON-RPC requests awaiting responses, keyed by request id.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    /// Current rate limit state from Codex notifications.
    rate_limit_state: Arc<RwLock<RateLimitState>>,
}

/// Parsed event from the Codex app-server NDJSON stream.
///
/// This is a transport-level event — it contains the raw JSON-RPC notification
/// method name and params. Phase 3 (`codex_events.rs`) will parse these into
/// typed Holt events.
#[derive(Debug, Clone)]
pub enum CodexEvent {
    /// A JSON-RPC notification (method + params).
    Notification { method: String, params: Value },
    /// A server→client request (has both method and id). The transport layer
    /// auto-responds (approvals declined per our `ApprovalPolicy::Never`);
    /// this event is informational for upper layers.
    ServerRequest {
        id: Value,
        method: String,
        params: Value,
    },
    /// The NDJSON reader encountered a line it couldn't parse.
    ParseError { line: String, error: String },
    /// The reader loop exited (server disconnected or process died).
    Disconnected,
}

/// Rate limit state tracked from `account/rateLimits/updated` notifications.
///
/// Note: `has_credits` defaults to `true` — we assume credits are available
/// until the server explicitly tells us otherwise. This avoids a race where
/// the first message fires before any rate-limit notification arrives.
#[derive(Debug, Clone)]
pub struct RateLimitState {
    pub primary_used_pct: Option<f64>,
    pub secondary_used_pct: Option<f64>,
    pub resets_at: Option<i64>,
    pub has_credits: bool,
    pub limit_reached: Option<RateLimitReachedType>,
    pub plan_type: Option<PlanType>,
}

impl Default for RateLimitState {
    fn default() -> Self {
        Self {
            primary_used_pct: None,
            secondary_used_pct: None,
            resets_at: None,
            has_credits: true, // Assume credits until server says otherwise
            limit_reached: None,
            plan_type: None,
        }
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Holt MCP mount for a codex app-server process.
///
/// Translated into `-c mcp_servers.holt.*` config overrides at spawn time
/// so the app-server connects back to Holt's in-process MCP server. The
/// `-c` value portion is parsed as TOML, hence the `toml::Value` encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexMcpLaunch {
    /// Full MCP URL including the `agent_id` query param.
    pub holt_url: String,
    /// Tool names to mark `approval_mode = "approve"`. The transport
    /// auto-declines all server→client approval requests, so any tool
    /// without auto-approve would be unreachable.
    pub auto_approve_tools: Vec<String>,
}

impl CodexMcpLaunch {
    fn config_args(&self) -> Vec<String> {
        let mut args = vec![
            "-c".to_string(),
            format!(
                "mcp_servers.holt.url={}",
                toml::Value::String(self.holt_url.clone())
            ),
            "-c".to_string(),
            "mcp_servers.holt.required=true".to_string(),
        ];

        for tool_name in &self.auto_approve_tools {
            args.push("-c".to_string());
            args.push(format!(
                "mcp_servers.holt.tools.{}.approval_mode={}",
                tool_name,
                toml::Value::String("approve".to_string())
            ));
        }

        args
    }
}

/// Spawn a Codex app-server for the given agent if one isn't already running.
///
/// - `agent_id`: The agent this server belongs to.
/// - `codex_home`: Optional `CODEX_HOME` env override.
/// - `working_dir`: Working directory for the Codex process.
/// - `mcp_launch`: Optional Holt MCP mount, applied as `-c` config
///   overrides. Only takes effect on a fresh spawn — a reused live server
///   keeps the mount it was started with.
///
/// The server is spawned with `--stdio` (NDJSON over stdin/stdout) and the
/// mandatory `initialize`/`initialized` handshake is performed before this
/// function returns — the server rejects all other requests until then.
///
/// Returns an event receiver for consuming server notifications.
/// Each caller gets an independent broadcast receiver — multiple consumers are supported.
pub async fn get_or_spawn_server(
    agent_id: &str,
    codex_home: Option<&Path>,
    working_dir: &Path,
    mcp_launch: Option<&CodexMcpLaunch>,
) -> Result<broadcast::Receiver<CodexEvent>, CodexServerError> {
    // Check if we already have a live server
    {
        let servers = CODEX_SERVERS.read().await;
        if let Some(handle) = servers.get(agent_id) {
            if is_child_alive(&handle.child).await {
                // Server exists and is alive — return a new broadcast subscriber
                return Ok(handle.events_tx.subscribe());
            }
            // Server exists but process died — will clean up and respawn below
        }
    }

    // Clean up stale entry if any
    {
        let mut servers = CODEX_SERVERS.write().await;
        if let Some(mut old) = servers.remove(agent_id) {
            tracing::warn!(agent_id, "Cleaning up stale codex server");
            let _ = cleanup_handle(&mut old).await;
        }
    }

    // Spawn the app-server process
    let mut cmd = Command::new(CODEX_EXECUTABLE);
    cmd.arg("app-server").arg("--stdio");
    if let Some(launch) = mcp_launch {
        cmd.args(launch.config_args());
    }
    cmd.current_dir(working_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    if let Some(home) = codex_home {
        cmd.env("CODEX_HOME", home);
    }

    let mut child = cmd.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            CodexServerError::Spawn("`codex` not found on PATH".to_string())
        } else {
            CodexServerError::Spawn(err.to_string())
        }
    })?;

    tracing::info!(
        agent_id,
        pid = child.id().unwrap_or(0),
        holt_mcp = mcp_launch.is_some(),
        "Spawned codex app-server (stdio)"
    );

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| CodexServerError::Spawn("child stdin not captured".to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CodexServerError::Spawn("child stdout not captured".to_string()))?;
    let stderr = child.stderr.take();

    // Drain stderr into tracing — an unread pipe fills up and blocks the child.
    let stderr_agent_id = agent_id.to_string();
    let stderr_task = tauri::async_runtime::spawn(async move {
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(agent_id = %stderr_agent_id, "codex stderr: {line}");
            }
        }
    });

    // Set up channels and shared state
    let (events_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
    let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let rate_limit_state = Arc::new(RwLock::new(RateLimitState::default()));
    let writer = Arc::new(Mutex::new(BufWriter::new(stdin)));

    // Get a subscriber BEFORE spawning the reader (so we don't miss early events)
    let events_rx = events_tx.subscribe();

    // Spawn the NDJSON reader loop
    let reader_pending = Arc::clone(&pending);
    let reader_events_tx = events_tx.clone();
    let reader_rate_state = Arc::clone(&rate_limit_state);
    let reader_writer = Arc::clone(&writer);
    let reader_agent_id = agent_id.to_string();
    let reader_task = tauri::async_runtime::spawn(async move {
        ndjson_reader_loop(
            stdout,
            reader_pending,
            reader_events_tx,
            reader_rate_state,
            reader_writer,
            &reader_agent_id,
        )
        .await;
    });

    let handle = CodexServerHandle {
        child: Arc::new(Mutex::new(child)),
        writer,
        reader_task,
        stderr_task,
        events_tx,
        pending,
        rate_limit_state,
    };

    {
        let mut servers = CODEX_SERVERS.write().await;
        servers.insert(agent_id.to_string(), handle);
    }

    // Mandatory handshake: initialize request, then initialized notification.
    // Any other request first is rejected ("Not initialized").
    if let Err(e) = initialize_handshake(agent_id).await {
        let _ = shutdown_server(agent_id).await;
        return Err(CodexServerError::Handshake(e.to_string()));
    }

    // Runtime proof the holt mount was accepted — logging only, never
    // fatal (MCP servers can still be mid-connect this early; the config
    // marks holt `required=true`, so codex surfaces hard failures itself).
    if mcp_launch.is_some() {
        log_holt_mcp_status(agent_id).await;
    }

    Ok(events_rx)
}

/// Query `mcpServerStatus/list` and log whether the holt MCP server made
/// it into the app-server's roster.
async fn log_holt_mcp_status(agent_id: &str) {
    let params = json!({ "detail": "toolsAndAuthOnly" });
    match send_request(agent_id, "mcpServerStatus/list", params).await {
        Ok(result) => {
            let servers = result
                .get("data")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let holt = servers
                .iter()
                .find(|s| s.get("name").and_then(Value::as_str) == Some("holt"));
            match holt {
                Some(status) => {
                    let tool_count = status
                        .get("tools")
                        .and_then(Value::as_object)
                        .map(|t| t.len())
                        .unwrap_or(0);
                    tracing::info!(
                        agent_id,
                        tool_count,
                        "Holt MCP server mounted in codex app-server"
                    );
                }
                None => {
                    let names: Vec<&str> = servers
                        .iter()
                        .filter_map(|s| s.get("name").and_then(Value::as_str))
                        .collect();
                    tracing::warn!(
                        agent_id,
                        ?names,
                        "Holt MCP server missing from codex app-server status list"
                    );
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                agent_id,
                error = %e,
                "mcpServerStatus/list failed — cannot verify holt MCP mount"
            );
        }
    }
}

/// Perform the `initialize` request + `initialized` notification handshake.
async fn initialize_handshake(agent_id: &str) -> Result<(), CodexServerError> {
    let params = json!({
        "clientInfo": {
            "name": "holt",
            "title": "Holt",
            "version": env!("CARGO_PKG_VERSION"),
        },
        // Some methods we use (e.g. thread/settings/update) are gated behind
        // the experimental API capability.
        "capabilities": { "experimentalApi": true },
    });
    let result = send_request(agent_id, "initialize", params).await?;
    tracing::info!(
        agent_id,
        user_agent = %result.get("userAgent").and_then(|v| v.as_str()).unwrap_or(""),
        codex_home = %result.get("codexHome").and_then(|v| v.as_str()).unwrap_or(""),
        "Codex app-server initialized"
    );
    send_notification(agent_id, "initialized").await
}

/// Send a JSON-RPC request to the agent's Codex app-server and await the response.
///
/// Returns the `result` field from the JSON-RPC response, or an error if the
/// response contains an `error` field.
pub async fn send_request<P: Serialize>(
    agent_id: &str,
    method: &str,
    params: P,
) -> Result<Value, CodexServerError> {
    let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);

    let request = JsonRpcRequest {
        jsonrpc: "2.0",
        id,
        method: method.to_string(),
        params,
    };

    let request_json = serde_json::to_string(&request).map_err(CodexServerError::Serde)?;

    // Register the pending response channel BEFORE sending
    let rx = {
        let servers = CODEX_SERVERS.read().await;
        let handle = servers
            .get(agent_id)
            .ok_or_else(|| CodexServerError::NotFound(agent_id.to_string()))?;

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = handle.pending.lock().await;
            pending.insert(id, tx);
        }

        // Write the request
        {
            let mut writer = handle.writer.lock().await;
            writer
                .write_all(request_json.as_bytes())
                .await
                .map_err(|e| CodexServerError::Send(e.to_string()))?;
            writer
                .write_all(b"\n")
                .await
                .map_err(|e| CodexServerError::Send(e.to_string()))?;
            writer
                .flush()
                .await
                .map_err(|e| CodexServerError::Send(e.to_string()))?;
        }

        rx
    };

    tracing::debug!(agent_id, id, method, "Sent JSON-RPC request");

    // Await the response with a timeout
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
        .await
        .map_err(|_| CodexServerError::RequestTimeout(id))?
        .map_err(|_| CodexServerError::ResponseChannelClosed(id))?;

    // Check if the response is an error
    if let Some(error) = result.get("error") {
        let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error")
            .to_string();
        return Err(CodexServerError::JsonRpc { code, message });
    }

    // Return the result field (or null if missing)
    Ok(result.get("result").cloned().unwrap_or(Value::Null))
}

/// Send a JSON-RPC notification (no id, no response expected) to the agent's
/// Codex app-server.
pub async fn send_notification(agent_id: &str, method: &str) -> Result<(), CodexServerError> {
    let notification = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "method": method,
    }))
    .map_err(CodexServerError::Serde)?;

    let servers = CODEX_SERVERS.read().await;
    let handle = servers
        .get(agent_id)
        .ok_or_else(|| CodexServerError::NotFound(agent_id.to_string()))?;

    let mut writer = handle.writer.lock().await;
    writer
        .write_all(notification.as_bytes())
        .await
        .map_err(|e| CodexServerError::Send(e.to_string()))?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|e| CodexServerError::Send(e.to_string()))?;
    writer
        .flush()
        .await
        .map_err(|e| CodexServerError::Send(e.to_string()))?;
    Ok(())
}

/// Subscribe to the event stream for an agent's Codex server.
///
/// Returns `None` if no server is running for the agent.
pub async fn subscribe_events(agent_id: &str) -> Option<broadcast::Receiver<CodexEvent>> {
    let servers = CODEX_SERVERS.read().await;
    servers.get(agent_id).map(|h| h.events_tx.subscribe())
}

/// Gracefully shut down the Codex server for a specific agent.
pub async fn shutdown_server(agent_id: &str) -> Result<(), CodexServerError> {
    let mut servers = CODEX_SERVERS.write().await;
    if let Some(mut handle) = servers.remove(agent_id) {
        cleanup_handle(&mut handle).await?;
        tracing::info!(agent_id, "Codex server shut down");
        Ok(())
    } else {
        Ok(()) // Already gone — idempotent
    }
}

/// Shut down all running Codex servers (called at app exit).
pub async fn shutdown_all_servers() {
    let mut servers = CODEX_SERVERS.write().await;
    let agent_ids: Vec<String> = servers.keys().cloned().collect();
    for agent_id in &agent_ids {
        if let Some(mut handle) = servers.remove(agent_id) {
            if let Err(e) = cleanup_handle(&mut handle).await {
                tracing::warn!(agent_id, error = %e, "Error shutting down codex server");
            }
        }
    }
    tracing::info!("All codex servers shut down");
}

/// Check if a Codex server is alive for the given agent.
pub async fn is_server_alive(agent_id: &str) -> bool {
    let servers = CODEX_SERVERS.read().await;
    if let Some(handle) = servers.get(agent_id) {
        is_child_alive(&handle.child).await
    } else {
        false
    }
}

/// Get the current rate limit state for an agent's Codex server.
pub async fn get_rate_limit_state(agent_id: &str) -> Option<RateLimitState> {
    let servers = CODEX_SERVERS.read().await;
    let handle = servers.get(agent_id)?;
    let state = handle.rate_limit_state.read().await.clone();
    Some(state)
}

// =============================================================================
// Internal helpers
// =============================================================================

/// Background task: reads NDJSON lines from the child's stdout, dispatches
/// responses to pending request channels and notifications to the event
/// channel, and answers server→client requests.
async fn ndjson_reader_loop(
    stdout: ChildStdout,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    events_tx: broadcast::Sender<CodexEvent>,
    rate_limit_state: Arc<RwLock<RateLimitState>>,
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
    agent_id: &str,
) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                // EOF — server disconnected
                tracing::info!(agent_id, "Codex app-server stdout closed (EOF)");
                let _ = events_tx.send(CodexEvent::Disconnected);
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue; // Skip blank lines
                }

                match serde_json::from_str::<Value>(trimmed) {
                    Ok(json) => {
                        let reply = handle_json_message(
                            json,
                            &pending,
                            &events_tx,
                            &rate_limit_state,
                            agent_id,
                        )
                        .await;
                        // Server→client requests must be answered or the
                        // server blocks the turn waiting on us.
                        if let Some(reply) = reply {
                            if let Ok(reply_json) = serde_json::to_string(&reply) {
                                let mut w = writer.lock().await;
                                let write_result = async {
                                    w.write_all(reply_json.as_bytes()).await?;
                                    w.write_all(b"\n").await?;
                                    w.flush().await
                                }
                                .await;
                                if let Err(e) = write_result {
                                    tracing::warn!(
                                        agent_id,
                                        error = %e,
                                        "Failed to answer codex server request"
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            agent_id,
                            error = %e,
                            line = trimmed,
                            "Failed to parse NDJSON line"
                        );
                        let _ = events_tx.send(CodexEvent::ParseError {
                            line: trimmed.to_string(),
                            error: e.to_string(),
                        });
                    }
                }
            }
            Err(e) => {
                tracing::error!(agent_id, error = %e, "Error reading from codex socket");
                let _ = events_tx.send(CodexEvent::Disconnected);
                break;
            }
        }
    }
}

/// Route a parsed JSON message to a pending request, the event channel, or —
/// for server→client requests — build the reply the caller must write back.
///
/// Message shapes (JSON-RPC 2.0):
/// - `method` + `id`  → server→client REQUEST (approvals, auth-token refresh).
///   The server's id space is independent of ours, so this must be checked
///   BEFORE response routing or it collides with our pending request ids.
/// - `id` only        → response to one of our requests.
/// - `method` only    → notification.
async fn handle_json_message(
    json: Value,
    pending: &Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    events_tx: &broadcast::Sender<CodexEvent>,
    rate_limit_state: &Arc<RwLock<RateLimitState>>,
    agent_id: &str,
) -> Option<Value> {
    let method = json
        .get("method")
        .and_then(|v| v.as_str())
        .map(String::from);
    let has_id = json.get("id").is_some();

    // Server→client request: method + id
    if let (Some(method), true) = (method.as_deref(), has_id) {
        let id = json.get("id").cloned().unwrap_or(Value::Null);
        let params = json.get("params").cloned().unwrap_or(Value::Null);
        tracing::warn!(
            agent_id,
            method,
            "Codex server→client request — auto-responding"
        );
        let reply = build_server_request_reply(&id, method);
        let _ = events_tx.send(CodexEvent::ServerRequest {
            id,
            method: method.to_string(),
            params,
        });
        return Some(reply);
    }

    // Response to one of our requests: id, no method
    if has_id {
        if let Some(id) = json.get("id").and_then(|v| v.as_u64()) {
            let mut pending_map = pending.lock().await;
            if let Some(tx) = pending_map.remove(&id) {
                let _ = tx.send(json);
            } else {
                tracing::warn!(agent_id, id, "Received response for unknown request id");
            }
        } else {
            tracing::warn!(agent_id, json = %json, "Response with non-numeric id");
        }
        return None;
    }

    // Notification: method, no id
    if let Some(method_str) = method {
        let params = json.get("params").cloned().unwrap_or(Value::Null);

        // Update rate limit state inline for relevant notifications
        if method_str == "account/rateLimits/updated" {
            update_rate_limit_state(&params, rate_limit_state).await;
        }

        let _ = events_tx.send(CodexEvent::Notification {
            method: method_str,
            params,
        });
        return None;
    }

    // Unknown message shape — log and forward as parse error
    tracing::debug!(
        agent_id,
        json = %json,
        "Received JSON message with no id or method"
    );
    None
}

/// Build the JSON-RPC reply for a server→client request.
///
/// The lane runs Codex at full trust (`danger-full-access` sandbox +
/// `ApprovalPolicy::Never` on every thread entry path), so approval requests
/// are not expected — but if one arrives it must be answered or the turn
/// hangs. Nothing is auto-declined: silent declines killed turns with no
/// visible reason (an agent hitting a sandbox wall had its escalation request
/// eaten in transit). Approvals are granted; anything else gets a
/// method-not-found error so the server fails fast instead of blocking.
fn build_server_request_reply(id: &Value, method: &str) -> Value {
    match method {
        // v2 approval requests — command execution and file changes share the
        // simple decision shape
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "decision": "accept" },
        }),
        // v2 permission expansion — the reply carries a granted permission
        // profile, not a decision (0.146.0 schema). Grant filesystem + network
        // for the session; the full-access sandbox makes this a formality.
        "item/permissions/requestApproval" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "permissions": {
                    "fileSystem": {
                        "entries": [
                            { "access": "read", "path": { "type": "special", "value": { "kind": "root" } } },
                            { "access": "write", "path": { "type": "special", "value": { "kind": "root" } } },
                        ],
                    },
                    "network": { "enabled": true },
                },
                "scope": "session",
            },
        }),
        // legacy approval requests (legacy ReviewDecision vocabulary)
        "execCommandApproval" | "applyPatchApproval" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "decision": "approved" },
        }),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("server-to-client request not supported by this client: {method}"),
            },
        }),
    }
}

/// Update the rate limit state from an account/rateLimits/updated notification.
async fn update_rate_limit_state(params: &Value, state: &Arc<RwLock<RateLimitState>>) {
    let mut s = state.write().await;
    let rate_limits = params.get("rateLimits").unwrap_or(params);

    if let Some(primary) = rate_limits.get("primary") {
        s.primary_used_pct = primary.get("usedPercent").and_then(|v| v.as_f64());
        s.resets_at = primary.get("resetsAt").and_then(|v| v.as_i64());
    }
    if let Some(secondary) = rate_limits.get("secondary") {
        s.secondary_used_pct = secondary.get("usedPercent").and_then(|v| v.as_f64());
    }
    if let Some(credits) = rate_limits.get("credits") {
        s.has_credits = credits
            .get("hasCredits")
            .and_then(|v| v.as_bool())
            .unwrap_or(true); // Assume credits if field is missing/malformed
    }
    if let Some(reached_type) = rate_limits.get("rateLimitReachedType") {
        if let Ok(rlt) = serde_json::from_value::<RateLimitReachedType>(reached_type.clone()) {
            s.limit_reached = Some(rlt);
        }
    } else {
        s.limit_reached = None; // Clear if not present
    }
    if let Some(plan) = rate_limits.get("planType") {
        if let Ok(pt) = serde_json::from_value::<PlanType>(plan.clone()) {
            s.plan_type = Some(pt);
        }
    }
}

/// Check if a child process is still alive.
async fn is_child_alive(child: &Arc<Mutex<Child>>) -> bool {
    let mut guard = child.lock().await;
    match guard.try_wait() {
        Ok(None) => true,     // Still running
        Ok(Some(_)) => false, // Exited
        Err(_) => false,      // Error checking — assume dead
    }
}

/// Clean up a server handle: kill process, abort reader and stderr tasks.
async fn cleanup_handle(handle: &mut CodexServerHandle) -> Result<(), CodexServerError> {
    // Abort the background tasks
    handle.reader_task.abort();
    handle.stderr_task.abort();

    // Kill the child process
    {
        let mut child = handle.child.lock().await;
        let _ = child.kill().await;
        let _ = child.wait().await; // Reap the zombie
    }

    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_id_increments() {
        let id1 = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let id2 = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        assert!(id2 > id1);
    }

    #[test]
    fn mcp_launch_config_args_mount_holt_with_auto_approve() {
        let launch = CodexMcpLaunch {
            holt_url: "http://127.0.0.1:4100/mcp?agent_id=agent-1".to_string(),
            auto_approve_tools: vec!["recall".to_string(), "remember".to_string()],
        };

        let args = launch.config_args();

        // Every value rides behind its own `-c` flag
        assert_eq!(args.iter().filter(|a| *a == "-c").count(), 4);
        assert_eq!(args[0], "-c");
        // URL is TOML-quoted (the -c value portion is parsed as TOML)
        assert_eq!(
            args[1],
            "mcp_servers.holt.url=\"http://127.0.0.1:4100/mcp?agent_id=agent-1\""
        );
        assert!(args.contains(&"mcp_servers.holt.required=true".to_string()));
        // Auto-approve per tool — the transport declines approval requests,
        // so tools without this would be unreachable
        assert!(
            args.contains(&"mcp_servers.holt.tools.recall.approval_mode=\"approve\"".to_string())
        );
        assert!(
            args.contains(&"mcp_servers.holt.tools.remember.approval_mode=\"approve\"".to_string())
        );
    }

    #[test]
    fn mcp_launch_config_args_without_tools_still_mounts_server() {
        let launch = CodexMcpLaunch {
            holt_url: "http://127.0.0.1:4100/mcp".to_string(),
            auto_approve_tools: vec![],
        };

        let args = launch.config_args();
        assert_eq!(
            args,
            vec![
                "-c",
                "mcp_servers.holt.url=\"http://127.0.0.1:4100/mcp\"",
                "-c",
                "mcp_servers.holt.required=true",
            ]
        );
    }

    #[test]
    fn json_rpc_request_serializes_correctly() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 42,
            method: "thread/start".to_string(),
            params: ThreadStartParams {
                model: Some("o3-pro".to_string()),
                developer_instructions: Some("Be helpful".to_string()),
                ..Default::default()
            },
        };

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 42);
        assert_eq!(json["method"], "thread/start");
        assert_eq!(json["params"]["model"], "o3-pro");
        assert_eq!(json["params"]["developerInstructions"], "Be helpful");
        // Optional fields should be absent (skip_serializing_if)
        assert!(json["params"].get("sandboxMode").is_none());
    }

    #[tokio::test]
    async fn rate_limit_state_updates_from_notification() {
        let state = Arc::new(RwLock::new(RateLimitState::default()));

        let params = json!({
            "primary": {
                "usedPercent": 45.5,
                "resetsAt": 1720000000000_i64,
                "windowDurationMins": 60
            },
            "secondary": {
                "usedPercent": 12.0,
                "resetsAt": 1720000060000_i64,
                "windowDurationMins": 1
            },
            "credits": {
                "hasCredits": true,
                "balance": 100.0,
                "unlimited": false
            },
            "planType": "pro"
        });

        update_rate_limit_state(&params, &state).await;

        let s = state.read().await;
        assert!((s.primary_used_pct.unwrap() - 45.5).abs() < f64::EPSILON);
        assert!((s.secondary_used_pct.unwrap() - 12.0).abs() < f64::EPSILON);
        assert_eq!(s.resets_at, Some(1720000000000));
        assert!(s.has_credits);
        assert!(s.limit_reached.is_none());
        assert!(matches!(s.plan_type, Some(PlanType::Pro)));
    }

    #[tokio::test]
    async fn rate_limit_state_with_limit_reached() {
        let state = Arc::new(RwLock::new(RateLimitState::default()));

        let params = json!({
            "primary": { "usedPercent": 100.0, "resetsAt": 1720000000000_i64, "windowDurationMins": 60 },
            "credits": { "hasCredits": false, "balance": 0.0, "unlimited": false },
            "rateLimitReachedType": "rate_limit_reached",
            "planType": "plus"
        });

        update_rate_limit_state(&params, &state).await;

        let s = state.read().await;
        assert!((s.primary_used_pct.unwrap() - 100.0).abs() < f64::EPSILON);
        assert!(!s.has_credits);
        assert!(matches!(
            s.limit_reached,
            Some(RateLimitReachedType::RateLimitReached)
        ));
        assert!(matches!(s.plan_type, Some(PlanType::Plus)));
    }

    #[tokio::test]
    async fn rate_limit_clears_reached_when_absent() {
        let state = Arc::new(RwLock::new(RateLimitState {
            limit_reached: Some(RateLimitReachedType::RateLimitReached),
            ..Default::default()
        }));

        // Notification without rateLimitReachedType clears it
        let params = json!({
            "primary": { "usedPercent": 20.0, "resetsAt": 1720000000000_i64, "windowDurationMins": 60 },
            "credits": { "hasCredits": true, "balance": 50.0, "unlimited": false }
        });

        update_rate_limit_state(&params, &state).await;

        let s = state.read().await;
        assert!(s.limit_reached.is_none());
    }

    #[tokio::test]
    async fn handle_json_response_routes_to_pending() {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, mut events_rx) = broadcast::channel(16);
        let rate_state = Arc::new(RwLock::new(RateLimitState::default()));

        let (tx, rx) = oneshot::channel();
        {
            let mut p = pending.lock().await;
            p.insert(42, tx);
        }

        let response = json!({
            "jsonrpc": "2.0",
            "id": 42,
            "result": { "threadId": "t_abc", "status": "ready" }
        });

        handle_json_message(response, &pending, &events_tx, &rate_state, "test").await;

        // Response should arrive on the oneshot channel
        let result = rx.await.unwrap();
        assert_eq!(result["result"]["threadId"], "t_abc");

        // No event should be emitted for responses
        assert!(events_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn handle_json_notification_routes_to_events() {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, mut events_rx) = broadcast::channel(16);
        let rate_state = Arc::new(RwLock::new(RateLimitState::default()));

        let notification = json!({
            "jsonrpc": "2.0",
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "t_abc",
                "turnId": "turn_1",
                "delta": "Hello "
            }
        });

        handle_json_message(notification, &pending, &events_tx, &rate_state, "test").await;

        let event = events_rx.try_recv().unwrap();
        match event {
            CodexEvent::Notification { method, params } => {
                assert_eq!(method, "item/agentMessage/delta");
                assert_eq!(params["delta"], "Hello ");
            }
            other => panic!("Expected Notification, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_rate_limit_notification_updates_state_and_emits_event() {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, mut events_rx) = broadcast::channel(16);
        let rate_state = Arc::new(RwLock::new(RateLimitState::default()));

        let notification = json!({
            "jsonrpc": "2.0",
            "method": "account/rateLimits/updated",
            "params": {
                "rateLimits": {
                    "primary": { "usedPercent": 80.0, "resetsAt": 1720000000000_i64, "windowDurationMins": 60 },
                    "credits": { "hasCredits": true, "balance": 200.0, "unlimited": false },
                    "planType": "pro"
                }
            }
        });

        handle_json_message(notification, &pending, &events_tx, &rate_state, "test").await;

        // Rate limit state should be updated
        let s = rate_state.read().await;
        assert!((s.primary_used_pct.unwrap() - 80.0).abs() < f64::EPSILON);
        assert!(matches!(s.plan_type, Some(PlanType::Pro)));
        drop(s);

        // Event should also be emitted for downstream consumers
        let event = events_rx.try_recv().unwrap();
        assert!(
            matches!(event, CodexEvent::Notification { method, .. } if method == "account/rateLimits/updated")
        );
    }

    #[tokio::test]
    async fn server_request_not_swallowed_by_pending_response_with_same_id() {
        // Server→client requests carry the SERVER's id — which can collide
        // with our own request ids. They must be answered, not routed to
        // pending, or the awaiting caller gets the wrong payload AND the
        // server hangs waiting for an approval reply.
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, mut events_rx) = broadcast::channel(16);
        let rate_state = Arc::new(RwLock::new(RateLimitState::default()));

        let (tx, mut rx) = oneshot::channel();
        {
            let mut p = pending.lock().await;
            p.insert(7, tx);
        }

        let server_request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "item/commandExecution/requestApproval",
            "params": { "threadId": "t_1", "itemId": "i_1" }
        });

        let reply = handle_json_message(server_request, &pending, &events_tx, &rate_state, "test")
            .await
            .expect("server request must produce a reply");

        // Approval is granted — nothing in this lane is auto-declined
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["result"]["decision"], "accept");

        // Our pending request with the same id is untouched
        assert!(rx.try_recv().is_err()); // still empty
        assert!(pending.lock().await.contains_key(&7));

        // Upper layers are informed
        let event = events_rx.try_recv().unwrap();
        assert!(matches!(
            event,
            CodexEvent::ServerRequest { method, .. }
                if method == "item/commandExecution/requestApproval"
        ));
    }

    #[tokio::test]
    async fn unknown_server_request_gets_error_reply() {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, _events_rx) = broadcast::channel(16);
        let rate_state = Arc::new(RwLock::new(RateLimitState::default()));

        let server_request = json!({
            "id": "srv-1",
            "method": "account/chatgptAuthTokens/refresh",
            "params": { "reason": "unauthorized" }
        });

        let reply = handle_json_message(server_request, &pending, &events_tx, &rate_state, "test")
            .await
            .expect("server request must produce a reply");

        assert_eq!(reply["id"], "srv-1");
        assert_eq!(reply["error"]["code"], -32601);
    }

    #[test]
    fn legacy_approval_reply_uses_approved() {
        let reply = build_server_request_reply(&json!(3), "execCommandApproval");
        assert_eq!(reply["result"]["decision"], "approved");
    }

    #[test]
    fn permissions_approval_grants_full_profile() {
        // Permission replies carry a granted profile, not a decision —
        // a `decision` here would be schema-invalid at codex-cli 0.146.0.
        let reply = build_server_request_reply(&json!(4), "item/permissions/requestApproval");
        assert!(reply["result"].get("decision").is_none());
        assert_eq!(reply["result"]["permissions"]["network"]["enabled"], true);
        assert_eq!(reply["result"]["scope"], "session");
    }

    #[tokio::test]
    async fn notifications_and_responses_produce_no_reply() {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, _events_rx) = broadcast::channel(16);
        let rate_state = Arc::new(RwLock::new(RateLimitState::default()));

        let notification = json!({ "method": "turn/started", "params": {} });
        assert!(
            handle_json_message(notification, &pending, &events_tx, &rate_state, "test")
                .await
                .is_none()
        );

        let response = json!({ "id": 99, "result": {} });
        assert!(
            handle_json_message(response, &pending, &events_tx, &rate_state, "test")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn is_child_alive_detects_exited_process() {
        // Spawn a process that exits immediately
        let child = Command::new("true")
            .spawn()
            .expect("failed to spawn `true`");

        let child_arc = Arc::new(Mutex::new(child));

        // Wait a moment for it to exit
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(!is_child_alive(&child_arc).await);
    }

    #[tokio::test]
    async fn codex_event_variants() {
        // Verify CodexEvent enum can be constructed and debug-printed
        let notif = CodexEvent::Notification {
            method: "test".to_string(),
            params: json!({}),
        };
        let parse_err = CodexEvent::ParseError {
            line: "bad json".to_string(),
            error: "expected value".to_string(),
        };
        let disconnected = CodexEvent::Disconnected;

        // Just verify they're Debug-printable (compile-time check mostly)
        assert!(!format!("{:?}", notif).is_empty());
        assert!(!format!("{:?}", parse_err).is_empty());
        assert!(!format!("{:?}", disconnected).is_empty());
    }

    /// Live smoke test against a real `codex app-server` — requires the codex
    /// CLI on PATH and a logged-in ~/.codex. No model turn is started, so it
    /// consumes no quota. Run with: cargo test codex_server_live_smoke -- --ignored
    #[tokio::test]
    #[ignore]
    async fn codex_server_live_smoke() {
        let agent_id = "live-smoke-test";
        let working_dir = std::env::temp_dir();

        // Spawns, performs the initialize/initialized handshake
        let _events_rx = get_or_spawn_server(agent_id, None, &working_dir, None)
            .await
            .expect("spawn + handshake");

        // Auth status over the real protocol
        let account = send_request(agent_id, "account/read", json!({ "refreshToken": false }))
            .await
            .expect("account/read");
        assert!(
            account.get("account").is_some(),
            "account/read returned: {account}"
        );

        // Ephemeral thread — not persisted to history. Uses the TYPED path
        // (thread_ops + ThreadResponse) so struct drift against the real
        // server fails here, not in production.
        let thread_resp = crate::runtime::codex_thread_ops::thread_start(
            agent_id,
            ThreadStartParams {
                ephemeral: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("thread/start (typed)");
        assert!(!thread_resp.thread.id.is_empty());

        // Thread-reuse validation semantics (regression: stale threads after
        // app restart). The reuse path in codex.rs validates with
        // thread/resume because metadata reads (thread/goal/get) pass for
        // threads persisted from a PRIOR process while turn/start rejects
        // them ("thread not found"). Resume is rollout-based: it loads from
        // the on-disk rollout (written once a turn runs) and is idempotent
        // on loaded threads. Quota-free, we can only pin the error side:
        // resume must FAIL for threads without a rollout — unknown ids and
        // rollout-less threads alike — so the fresh-start fallback fires
        // instead of a doomed turn/start.
        let resume_no_rollout = crate::runtime::codex_thread_ops::thread_resume(
            agent_id,
            ThreadResumeParams {
                thread_id: thread_resp.thread.id.clone(),
                ..Default::default()
            },
        )
        .await;
        assert!(
            resume_no_rollout.is_err(),
            "thread/resume on a rollout-less (ephemeral) thread should error"
        );

        let resume_unknown = crate::runtime::codex_thread_ops::thread_resume(
            agent_id,
            ThreadResumeParams {
                thread_id: "019f0000-0000-7000-8000-000000000000".to_string(),
                ..Default::default()
            },
        )
        .await;
        assert!(
            resume_unknown.is_err(),
            "thread/resume on an unknown thread must error (fresh-start fallback depends on it)"
        );

        shutdown_server(agent_id).await.expect("shutdown");
    }

    #[test]
    fn default_rate_limit_state() {
        let state = RateLimitState::default();
        assert!(state.primary_used_pct.is_none());
        assert!(state.secondary_used_pct.is_none());
        assert!(state.resets_at.is_none());
        assert!(state.has_credits); // Defaults to true — assume credits until told otherwise
        assert!(state.limit_reached.is_none());
        assert!(state.plan_type.is_none());
    }
}
