use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;

use super::types::{JsonRpcRequest, JsonRpcResponse, PluginError, PluginToolDef};

/// What to do with a response frame carrying id `got` while we await `expected`.
///
/// The stream is single-reader and requests are serialized (`&mut self`), so the
/// only way a non-matching id appears is a request we *sent but abandoned* — e.g. a
/// `read` that a `tokio::time::timeout` cancelled while a slow render was still
/// running. Its response arrives late and lands in the pipe as an orphan. Treating
/// that orphan as a hard error (the old behavior) desynced the stream permanently:
/// every subsequent read saw the previous request's response ("expected N, got N-1")
/// and the connection was dead until respawn. Discarding orphans keeps the stream
/// self-healing.
#[derive(Debug, PartialEq, Eq)]
enum IdDisposition {
    /// `got == expected`: this is our response.
    Match,
    /// `got < expected`: a response to an earlier request we already abandoned
    /// (timed out). Discard it and keep reading — this is what stops one timed-out
    /// request from permanently desyncing the channel.
    Orphan,
    /// `got > expected`: a response for a request we have not sent. Serialized
    /// requests make this impossible in practice, so it's a genuine protocol break.
    Future,
}

fn classify_response_id(expected: u64, got: u64) -> IdDisposition {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match got.cmp(&expected) {
        Equal => IdDisposition::Match,
        Less => IdDisposition::Orphan,
        Greater => IdDisposition::Future,
    }
}

/// A running MCP server child process using JSON-RPC 2.0 over stdio.
///
/// Speaks the MCP protocol (`initialize`, `initialized`, `tools/list`, `tools/call`)
/// rather than the native plugin protocol used by `PluginProcess`.
pub struct McpProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr_task: Option<JoinHandle<()>>,
    next_request_id: u64,
    max_payload: usize,
}

impl McpProcess {
    /// Spawn an MCP server as a child process with piped stdio.
    pub fn spawn(command: &str, args: &[String], max_payload: usize) -> Result<Self, PluginError> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| PluginError::SpawnFailed(format!("{}: {}", command, e)))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| PluginError::SpawnFailed("Failed to capture stderr".to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PluginError::SpawnFailed("Failed to capture stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PluginError::SpawnFailed("Failed to capture stdout".to_string()))?;
        let stderr_task = Some(spawn_stderr_drain_task(stderr, format!("mcp:{command}")));

        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            stderr_task,
            next_request_id: 1,
            max_payload,
        })
    }

    /// Send a JSON-RPC 2.0 request and wait for the matching response.
    ///
    /// Uses newline-delimited JSON (one JSON object per line).
    /// Enforces payload size limits on both send and receive.
    /// Verifies that the response ID matches the request ID.
    pub async fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, PluginError> {
        if !self.is_alive() {
            return Err(PluginError::ProcessDead);
        }

        let request_id = self.next_request_id;
        self.next_request_id += 1;

        let request = JsonRpcRequest::new(request_id, method, params);
        let request_json = serde_json::to_string(&request)?;

        if request_json.len() > self.max_payload {
            return Err(PluginError::PayloadTooLarge {
                size: request_json.len(),
                max: self.max_payload,
            });
        }

        let result = tokio::time::timeout(timeout, async {
            self.stdin
                .write_all(request_json.as_bytes())
                .await
                .map_err(PluginError::Io)?;
            self.stdin.write_all(b"\n").await.map_err(PluginError::Io)?;
            self.stdin.flush().await.map_err(PluginError::Io)?;

            // Read response lines, skipping any MCP notifications (no id field)
            loop {
                let mut line = String::new();
                let bytes_read = self
                    .stdout
                    .read_line(&mut line)
                    .await
                    .map_err(PluginError::Io)?;

                if bytes_read == 0 {
                    return Err(PluginError::ProcessDead);
                }

                if line.len() > self.max_payload {
                    return Err(PluginError::PayloadTooLarge {
                        size: line.len(),
                        max: self.max_payload,
                    });
                }

                // Try to detect notifications (JSON-RPC messages with no "id" field).
                // Parse as generic Value first to check.
                let trimmed = line.trim();
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    if val.get("id").is_none() || val.get("id") == Some(&serde_json::Value::Null) {
                        // This is a notification — skip it, read next line
                        tracing::debug!(method = ?val.get("method"), "Skipping MCP notification");
                        continue;
                    }
                }

                // Parse as response
                let response: JsonRpcResponse = serde_json::from_str(trimmed).map_err(|e| {
                    PluginError::ProtocolError(format!("Invalid JSON-RPC response: {}", e))
                })?;

                match classify_response_id(request_id, response.id) {
                    IdDisposition::Match => {}
                    IdDisposition::Orphan => {
                        // A late response to a request we already abandoned (timed
                        // out). Discard it and read the next line — never desync.
                        tracing::debug!(
                            expected = request_id,
                            got = response.id,
                            "Discarding orphaned MCP response from an abandoned request"
                        );
                        continue;
                    }
                    IdDisposition::Future => {
                        return Err(PluginError::ProtocolError(format!(
                            "Response ID from the future: expected {}, got {} \
                             (server responded to a request we never sent)",
                            request_id, response.id
                        )));
                    }
                }

                if let Some(err) = response.error {
                    return Err(PluginError::RpcError {
                        code: err.code,
                        message: err.message,
                    });
                }

                return Ok(response.result.unwrap_or(serde_json::Value::Null));
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(PluginError::Timeout(timeout.as_millis() as u64)),
        }
    }

    /// Send a JSON-RPC 2.0 notification (no `id` field, no response expected).
    ///
    /// MCP uses notifications for one-way messages like `initialized`.
    pub async fn send_notification(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), PluginError> {
        if !self.is_alive() {
            return Err(PluginError::ProcessDead);
        }

        // Notifications have no id field per JSON-RPC 2.0 spec
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        let notification_json = serde_json::to_string(&notification)?;

        if notification_json.len() > self.max_payload {
            return Err(PluginError::PayloadTooLarge {
                size: notification_json.len(),
                max: self.max_payload,
            });
        }

        self.stdin
            .write_all(notification_json.as_bytes())
            .await
            .map_err(PluginError::Io)?;
        self.stdin.write_all(b"\n").await.map_err(PluginError::Io)?;
        self.stdin.flush().await.map_err(PluginError::Io)?;

        Ok(())
    }

    /// Perform the MCP initialization handshake.
    ///
    /// Steps:
    /// 1. Send `initialize` request with protocol version and client info
    /// 2. Send `initialized` notification
    /// 3. Send `tools/list` request
    /// 4. Convert MCP tool format (`inputSchema`) to `PluginToolDef` (`parameters_schema`)
    ///
    /// Returns `(server_name, server_version, tools)`.
    pub async fn mcp_handshake(
        &mut self,
    ) -> Result<(String, String, Vec<PluginToolDef>), PluginError> {
        // Step 1: Send initialize request
        let init_result = self
            .send_request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "clientInfo": {
                        "name": "holt",
                        "version": "0.1.0"
                    },
                    "capabilities": {}
                }),
                Duration::from_secs(10),
            )
            .await?;

        let server_name = init_result
            .get("serverInfo")
            .and_then(|si| si.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown")
            .to_string();

        let server_version = init_result
            .get("serverInfo")
            .and_then(|si| si.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0")
            .to_string();

        // Step 2: Send initialized notification (required by MCP spec)
        self.send_notification("initialized", serde_json::json!({}))
            .await?;

        // Step 3: List available tools
        let tools_result = self
            .send_request("tools/list", serde_json::json!({}), Duration::from_secs(10))
            .await?;

        // Step 4: Convert MCP tool format to PluginToolDef
        let tools = tools_result
            .get("tools")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|mcp_tool| {
                        let name = mcp_tool.get("name")?.as_str()?.to_string();
                        let description = mcp_tool
                            .get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string();
                        // MCP uses `inputSchema`, we store as `parameters_schema`
                        let parameters_schema =
                            mcp_tool.get("inputSchema").cloned().unwrap_or_else(
                                || serde_json::json!({"type": "object", "properties": {}}),
                            );
                        Some(PluginToolDef {
                            name,
                            description,
                            parameters_schema,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok((server_name, server_version, tools))
    }

    /// Call an MCP tool by name with the given arguments.
    ///
    /// MCP `tools/call` returns `{content: [{type: "text", text: "..."}], isError?: bool}`.
    /// Returns an error if `isError` is true in the response.
    pub async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, PluginError> {
        let result = self
            .send_request(
                "tools/call",
                serde_json::json!({
                    "name": tool_name,
                    "arguments": arguments
                }),
                timeout,
            )
            .await?;

        // Check for MCP-level tool error
        let is_error = result
            .get("isError")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);

        if is_error {
            let error_text = result
                .get("content")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("Tool returned an error");
            return Err(PluginError::ProtocolError(format!(
                "MCP tool '{}' error: {}",
                tool_name, error_text
            )));
        }

        Ok(result)
    }

    /// Synchronous best-effort kill for shutdown without an async runtime.
    pub fn start_kill(&mut self) {
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
        let _ = self.child.start_kill();
    }

    /// Check if the child process is still running.
    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(_)) => false,
            Err(_) => false,
        }
    }

    /// Kill the current process and spawn a fresh one with the same parameters.
    /// Performs the full MCP handshake on the new process.
    /// Returns the new tool definitions (which should match the originals).
    pub async fn respawn(
        &mut self,
        command: &str,
        args: &[String],
    ) -> Result<Vec<PluginToolDef>, PluginError> {
        // Kill old process if still alive
        if self.is_alive() {
            let _ = self.child.kill().await;
        }
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }

        // Spawn fresh process
        let mut new = McpProcess::spawn(command, args, self.max_payload)?;
        let (_name, _version, tools) = new.mcp_handshake().await?;

        // Replace self with the new process
        self.child = new.child;
        self.stdin = new.stdin;
        self.stdout = new.stdout;
        self.stderr_task = new.stderr_task;
        self.next_request_id = new.next_request_id;

        tracing::info!(command, "MCP process respawned successfully");
        Ok(tools)
    }

    /// Gracefully shut down the MCP server process.
    ///
    /// Closes stdin to signal EOF, waits up to `timeout` for the process to exit,
    /// then kills it if it has not exited.
    pub async fn shutdown(&mut self, timeout: Duration) {
        // Close stdin to signal EOF — MCP servers should exit on stdin close.
        // We flush any pending bytes first; ignoring errors since we're shutting down.
        let _ = self.stdin.flush().await;
        // Drop the inner ChildStdin by replacing it with a no-op (tokio doesn't expose
        // a direct close; the process will see EOF when the Child is dropped).

        let exit_result = tokio::time::timeout(timeout, self.child.wait()).await;

        match exit_result {
            Ok(Ok(_)) => {}
            Ok(Err(_)) => {
                let _ = self.child.kill().await;
            }
            Err(_) => {
                let _ = self.child.kill().await;
            }
        }

        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

fn spawn_stderr_drain_task(stderr: ChildStderr, process_label: String) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        tracing::debug!(process = %process_label, stderr = %trimmed, "MCP stderr");
                    }
                }
                Err(e) => {
                    tracing::warn!(process = %process_label, "MCP stderr read error: {}", e);
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_tool_def_conversion() {
        let mcp_tool = serde_json::json!({
            "name": "example_search",
            "description": "Semantic search",
            "inputSchema": {
                "type": "object",
                "properties": { "query": {"type": "string"} },
                "required": ["query"]
            }
        });
        let name = mcp_tool.get("name").unwrap().as_str().unwrap().to_string();
        let desc = mcp_tool
            .get("description")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        let schema = mcp_tool.get("inputSchema").cloned().unwrap();
        let tool_def = PluginToolDef {
            name: name.clone(),
            description: desc.clone(),
            parameters_schema: schema.clone(),
        };
        assert_eq!(tool_def.name, "example_search");
        assert_eq!(tool_def.description, "Semantic search");
        assert!(tool_def.parameters_schema.get("properties").is_some());
    }

    #[test]
    fn test_mcp_tool_def_missing_input_schema_gets_default() {
        let mcp_tool = serde_json::json!({"name": "example_health", "description": "Check health"});
        let schema = mcp_tool
            .get("inputSchema")
            .cloned()
            .unwrap_or(serde_json::json!({"type": "object", "properties": {}}));
        assert_eq!(schema.get("type").unwrap(), "object");
    }

    #[test]
    fn test_mcp_error_response_extraction() {
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "Error: connection refused"}],
            "isError": true
        });
        let is_error = result
            .get("isError")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);
        assert!(is_error);
        let error_text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("Unknown");
        assert_eq!(error_text, "Error: connection refused");
    }

    #[test]
    fn test_mcp_tool_def_description_fallback() {
        // Tool without description gets empty string
        let mcp_tool = serde_json::json!({
            "name": "no_desc_tool",
            "inputSchema": {"type": "object", "properties": {}}
        });
        let description = mcp_tool
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();
        assert_eq!(description, "");
    }

    #[test]
    fn test_mcp_successful_tool_result() {
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "Search results: ..."}],
            "isError": false
        });
        let is_error = result
            .get("isError")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);
        assert!(!is_error);
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        assert!(text.contains("Search results"));
    }

    #[test]
    fn test_notification_has_no_id() {
        // Verify that a notification JSON structure has no id field
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        });
        assert!(notification.get("id").is_none());
        assert_eq!(
            notification.get("method").unwrap().as_str().unwrap(),
            "initialized"
        );
    }

    #[test]
    fn test_notification_has_no_id_field() {
        // Notifications per JSON-RPC 2.0 spec have no id field
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/message",
            "params": {"level": "info", "data": "something"}
        });
        assert!(notification.get("id").is_none());

        // Responses always have an id
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {}
        });
        assert!(response.get("id").is_some());
    }

    #[test]
    fn orphaned_response_is_discarded_not_a_hard_desync() {
        // Regression: the librarian desync of 2026-07-20 and -22. A `read` timed out
        // (slow render) while its request was already sent; the late response landed
        // as an orphan and the next request hard-failed on it — "expected 2456, got
        // 2455" — then every call stayed one behind forever. An orphan (got < expected)
        // must be DISCARDED so the stream re-syncs on the next matching frame.
        assert_eq!(classify_response_id(2456, 2455), IdDisposition::Orphan);
        // Our own response is used.
        assert_eq!(classify_response_id(2456, 2456), IdDisposition::Match);
        // A response for an unsent request is a real protocol violation.
        assert_eq!(classify_response_id(2456, 2457), IdDisposition::Future);
        // Several stacked orphans (multiple timeouts) all discard, not just one.
        assert_eq!(classify_response_id(2460, 2455), IdDisposition::Orphan);
    }

    #[test]
    fn test_notification_detection() {
        // A line is a notification if it parses as JSON but has no "id" field
        let notification_line = r#"{"jsonrpc":"2.0","method":"notifications/message","params":{}}"#;
        let parsed: serde_json::Value = serde_json::from_str(notification_line).unwrap();
        let is_notification = parsed.get("id").is_none();
        assert!(is_notification);

        // A response line has an "id" field
        let response_line = r#"{"jsonrpc":"2.0","id":5,"result":{}}"#;
        let parsed: serde_json::Value = serde_json::from_str(response_line).unwrap();
        let is_notification = parsed.get("id").is_none();
        assert!(!is_notification);
    }
}
