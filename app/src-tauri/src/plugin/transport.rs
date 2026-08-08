use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;

use super::types::{JsonRpcRequest, JsonRpcResponse, PluginError};

/// A running plugin child process with JSON-RPC over stdio transport.
pub struct PluginProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr_task: Option<JoinHandle<()>>,
    next_request_id: u64,
    max_payload: usize,
}

impl PluginProcess {
    /// Spawn a plugin executable as a child process with piped stdio.
    pub fn spawn(
        binary_path: &Path,
        plugin_id: &str,
        max_payload: usize,
    ) -> Result<Self, PluginError> {
        let mut child = Command::new(binary_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| PluginError::SpawnFailed(format!("{}: {}", binary_path.display(), e)))?;

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
        let stderr_task = Some(spawn_stderr_drain_task(
            stderr,
            format!("native-plugin:{plugin_id}"),
        ));

        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            stderr_task,
            next_request_id: 1,
            max_payload,
        })
    }

    /// Send a JSON-RPC request and wait for the response.
    ///
    /// Uses newline-delimited JSON: one JSON object per line.
    /// Enforces payload size limits on both send and receive.
    pub async fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, PluginError> {
        // Check if process is still alive
        if !self.is_alive() {
            return Err(PluginError::ProcessDead);
        }

        let request_id = self.next_request_id;
        self.next_request_id += 1;

        let request = JsonRpcRequest::new(request_id, method, params);
        let request_json = serde_json::to_string(&request)?;

        // Check send payload size
        if request_json.len() > self.max_payload {
            return Err(PluginError::PayloadTooLarge {
                size: request_json.len(),
                max: self.max_payload,
            });
        }

        // Write request as a single line + newline
        let result = tokio::time::timeout(timeout, async {
            self.stdin
                .write_all(request_json.as_bytes())
                .await
                .map_err(PluginError::Io)?;
            self.stdin.write_all(b"\n").await.map_err(PluginError::Io)?;
            self.stdin.flush().await.map_err(PluginError::Io)?;

            // Read response line
            let mut line = String::new();
            let bytes_read = self
                .stdout
                .read_line(&mut line)
                .await
                .map_err(PluginError::Io)?;

            if bytes_read == 0 {
                return Err(PluginError::ProcessDead);
            }

            // Check receive payload size
            if line.len() > self.max_payload {
                return Err(PluginError::PayloadTooLarge {
                    size: line.len(),
                    max: self.max_payload,
                });
            }

            let response: JsonRpcResponse = serde_json::from_str(line.trim()).map_err(|e| {
                PluginError::ProtocolError(format!("Invalid JSON-RPC response: {}", e))
            })?;

            // Verify response ID matches request
            if response.id != request_id {
                return Err(PluginError::ProtocolError(format!(
                    "Response ID mismatch: expected {}, got {}",
                    request_id, response.id
                )));
            }

            // Check for JSON-RPC error
            if let Some(err) = response.error {
                return Err(PluginError::RpcError {
                    code: err.code,
                    message: err.message,
                });
            }

            Ok(response.result.unwrap_or(serde_json::Value::Null))
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(PluginError::Timeout(timeout.as_millis() as u64)),
        }
    }

    /// Send shutdown command and wait for process to exit.
    pub async fn shutdown(&mut self, timeout: Duration) {
        // Try graceful shutdown via JSON-RPC
        let _ = self
            .send_request("shutdown", serde_json::json!({}), Duration::from_secs(2))
            .await;

        // Wait for process to exit
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

    /// Kill the current process and spawn a fresh one with the same binary.
    pub async fn respawn(
        &mut self,
        binary_path: &std::path::Path,
        plugin_id: &str,
    ) -> Result<(), PluginError> {
        // Kill old process if still alive
        if self.is_alive() {
            let _ = self.child.kill().await;
        }
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }

        let new = PluginProcess::spawn(binary_path, plugin_id, self.max_payload)?;
        self.child = new.child;
        self.stdin = new.stdin;
        self.stdout = new.stdout;
        self.stderr_task = new.stderr_task;
        self.next_request_id = new.next_request_id;

        tracing::info!(plugin_id, "Native plugin process respawned successfully");
        Ok(())
    }

    /// Synchronous best-effort kill — for use during shutdown when no async runtime is available.
    pub fn start_kill(&mut self) {
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
        let _ = self.child.start_kill();
    }

    /// Check if the child process is still running.
    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,     // Still running
            Ok(Some(_)) => false, // Exited
            Err(_) => false,      // Error checking
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
                        tracing::debug!(process = %process_label, stderr = %trimmed, "Plugin stderr");
                    }
                }
                Err(e) => {
                    tracing::warn!(process = %process_label, "Plugin stderr read error: {}", e);
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::types::MAX_IPC_PAYLOAD_BYTES;
    use super::*;

    #[test]
    fn test_payload_size_check() {
        // Verify MAX_IPC_PAYLOAD_BYTES constant
        assert_eq!(MAX_IPC_PAYLOAD_BYTES, 100 * 1024 * 1024);
    }

    #[test]
    fn test_request_serialization_size() {
        let request = JsonRpcRequest::new(1, "test", serde_json::json!({"data": "x".repeat(100)}));
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.len() < MAX_IPC_PAYLOAD_BYTES);
    }
}
