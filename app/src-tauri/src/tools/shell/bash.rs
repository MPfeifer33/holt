use serde_json::json;
use std::time::Duration;

use crate::tools::shell::pty_command::{CommandResult, PtyCommand};
use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};

pub struct BashTool;

fn normalize_html_escaped_shell(command: &str) -> String {
    command
        .replace("&amp;&amp;", "&&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

#[async_trait::async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Run a shell command. The shell session persists between calls (cd, env vars, etc. carry over). Use this for builds, tests, git, pipelines, and anything the other tools don't cover."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"command": "cargo test"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "timeout_seconds": {
                    "type": "integer",
                    "description": "Override default timeout in seconds"
                },
                "stream": {
                    "type": "boolean",
                    "description": "Stream output lines to frontend in real-time"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let command = arguments
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: command".to_string(),
                retryable: false,
            })?;

        let command = normalize_html_escaped_shell(command);

        let timeout_seconds = arguments
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);
        let stream = arguments
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Ensure session exists — should be eager-created, fall back to on-demand
        {
            let manager = context.shell_manager.read().await;
            if !manager.sessions.contains_key(&context.agent_id) {
                drop(manager);
                eprintln!(
                    "[bash] PTY session not found for agent {}, spawning on demand",
                    context.agent_id
                );
                let mut manager = context.shell_manager.write().await;
                manager
                    .respawn_session(context.agent_id.clone(), context.working_directory.clone())
                    .await?;
            }
        }

        // Execute command
        let result = {
            let manager = context.shell_manager.read().await;
            let session = manager
                .sessions
                .get(&context.agent_id)
                .ok_or_else(|| ToolError {
                    code: ToolErrorCode::InternalError,
                    message: format!("No PTY session found for agent {}", context.agent_id),
                    retryable: true,
                })?;
            PtyCommand::execute(
                session,
                &command,
                Duration::from_secs(timeout_seconds),
                stream,
                context.app_handle.clone(),
                &context.agent_id,
            )
            .await
        };

        match result {
            Ok(cmd_result) => Ok(build_tool_result(cmd_result)),
            Err(e) if is_broken_pipe(&e) => {
                // PTY died — respawn and retry once
                eprintln!(
                    "[bash] PTY session died for agent {}, respawning",
                    context.agent_id
                );
                let cwd = {
                    let manager = context.shell_manager.read().await;
                    match manager.sessions.get(&context.agent_id) {
                        Some(s) => s.cwd().await,
                        None => context.working_directory.clone(),
                    }
                };
                {
                    let mut manager = context.shell_manager.write().await;
                    manager.destroy_session(&context.agent_id).await;
                    manager
                        .respawn_session(context.agent_id.clone(), cwd)
                        .await?;
                }
                // Retry
                let manager = context.shell_manager.read().await;
                let session = manager
                    .sessions
                    .get(&context.agent_id)
                    .ok_or_else(|| ToolError {
                        code: ToolErrorCode::InternalError,
                        message: format!(
                            "PTY session missing after respawn for agent {}",
                            context.agent_id
                        ),
                        retryable: false,
                    })?;
                let mut cmd_result = PtyCommand::execute(
                    session,
                    &command,
                    Duration::from_secs(timeout_seconds),
                    stream,
                    context.app_handle.clone(),
                    &context.agent_id,
                )
                .await?;
                cmd_result.session_reset = true;
                Ok(build_tool_result(cmd_result))
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_html_escaped_shell;

    #[test]
    fn normalizes_common_html_escaped_shell_operators() {
        let command =
            "cd tests/scenarios/targets/compound-bash-fail &amp;&amp; set -o pipefail &amp;&amp; cargo check 2&gt;&amp;1 | tail -n 20";
        let normalized = normalize_html_escaped_shell(command);
        assert_eq!(
            normalized,
            "cd tests/scenarios/targets/compound-bash-fail && set -o pipefail && cargo check 2>&1 | tail -n 20"
        );
    }

    #[test]
    fn leaves_plain_shell_command_unchanged() {
        let command = "cd app/src-tauri && cargo check";
        assert_eq!(normalize_html_escaped_shell(command), command);
    }
}

fn is_broken_pipe(e: &ToolError) -> bool {
    e.message.contains("BrokenPipe")
        || e.message.contains("broken pipe")
        || e.message.contains("Broken pipe")
}

fn build_tool_result(cmd_result: CommandResult) -> ToolResult {
    let truncated = cmd_result.output.len() > 50_000;
    let display_output = if truncated {
        let boundary = cmd_result.output.floor_char_boundary(50_000);
        format!(
            "{}...\n\n[Output truncated at {} chars]",
            &cmd_result.output[..boundary],
            boundary
        )
    } else {
        cmd_result.output
    };
    let execution_status = if cmd_result.timed_out {
        "timed_out"
    } else if cmd_result.syntax_error {
        "syntax_error"
    } else if cmd_result.exit_code == 0 {
        "success"
    } else {
        "failed"
    };
    let tool_result_status =
        if cmd_result.timed_out || cmd_result.syntax_error || cmd_result.exit_code != 0 {
            "failed"
        } else {
            "success"
        };
    let summary = if cmd_result.exit_code == 0 {
        format!(
            "Command completed successfully in {}ms",
            cmd_result.duration.as_millis()
        )
    } else {
        format!(
            "Command exited with code {} after {}ms",
            cmd_result.exit_code,
            cmd_result.duration.as_millis()
        )
    };

    ToolResult {
        content: json!({
            "tool_result_status": tool_result_status,
            "output": display_output,
            "exit_code": cmd_result.exit_code,
            "duration_ms": cmd_result.duration.as_millis() as u64,
            "truncated": truncated,
            "timed_out": cmd_result.timed_out,
            "syntax_error": cmd_result.syntax_error,
            "session_reset": cmd_result.session_reset,
            "cwd": cmd_result.cwd.display().to_string(),
            "receipt": ToolExecutionReceipt {
                authority_class: ToolAuthorityClass::Effectful,
                executed: true,
                execution_status: execution_status.to_string(),
                verified: false,
                verification_status: ToolVerificationStatus::NotRequired,
                execution_id: None,
                tool_name: None,
                tool_call_id: None,
                tool_call_trace_id: None,
                tool_result_trace_id: None,
                summary: Some(summary),
            },
        }),
        truncated,
        trace_id: None,
        image_content: None,
    }
}
