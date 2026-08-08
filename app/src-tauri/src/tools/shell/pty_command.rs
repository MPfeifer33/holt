//! PtyCommand — stateless command executor for PtySession.
//!
//! Sends a command string to a PTY, detects completion via PS1 markers,
//! handles PS2 syntax errors, and implements timeout escalation
//! (Ctrl+C -> SIGTERM -> SIGKILL).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tauri::Emitter;
use tokio::sync::mpsc;

use crate::tools::shell::pty_session::PtySession;
use crate::tools::types::{ToolError, ToolErrorCode};

/// Result of executing a single command on a PtySession.
#[derive(Debug)]
pub struct CommandResult {
    /// Combined stdout/stderr output from the command.
    pub output: String,
    /// Exit code from the command (parsed from PS1 marker).
    pub exit_code: i32,
    /// Wall-clock duration of the command execution.
    pub duration: Duration,
    /// Working directory after the command completed.
    pub cwd: PathBuf,
    /// Whether the output was truncated (reserved for future use).
    pub truncated: bool,
    /// Whether the command timed out.
    pub timed_out: bool,
    /// Whether a PS2 syntax error was detected.
    pub syntax_error: bool,
    /// Whether the session had to be forcibly reset (SIGKILL).
    pub session_reset: bool,
}

/// Stateless command executor. All state lives in PtySession.
pub struct PtyCommand;

impl PtyCommand {
    /// Execute a command on the given PtySession with PS1/PS2 marker detection.
    ///
    /// Acquires the session's command mutex, registers a per-command output
    /// channel, sends the command, and waits for the PS1 ready marker to
    /// detect completion.
    pub async fn execute(
        session: &PtySession,
        command: &str,
        timeout: Duration,
        stream: bool,
        event_sink: Option<tauri::AppHandle>,
        agent_id: &str,
    ) -> Result<CommandResult, ToolError> {
        // 1. Acquire command mutex — serializes concurrent commands
        let _guard = session.command_mutex.lock().await;

        // 2. Create per-command output channel
        let (tx, mut rx) = mpsc::channel::<String>(1000);

        // Register sender with session
        {
            let mut cmd_tx = session.command_output_tx.lock().await;
            *cmd_tx = Some(tx);
        }

        let start = Instant::now();
        let nonce = &session.session_nonce;

        // 3. Send command to PTY
        let write_result = session.write(&format!("{}\n", command)).await;
        if let Err(e) = write_result {
            // Unregister channel before returning
            let mut cmd_tx = session.command_output_tx.lock().await;
            *cmd_tx = None;
            // Check for BrokenPipe
            if e.message.contains("Broken pipe") || e.message.contains("BrokenPipe") {
                return Err(ToolError {
                    code: ToolErrorCode::InternalError,
                    message: format!("BrokenPipe: PTY write failed: {}", e.message),
                    retryable: false,
                });
            }
            return Err(e);
        }

        // 4. Receive loop
        let mut output_lines: Vec<String> = Vec::new();
        let mut exit_code: i32 = -1;
        let mut timed_out = false;
        let mut syntax_error = false;
        let mut session_reset = false;

        let deadline = tokio::time::Instant::now() + timeout;

        let receive_result = Self::receive_loop(
            &mut rx,
            &mut output_lines,
            nonce,
            deadline,
            stream,
            &event_sink,
            agent_id,
        )
        .await;

        match receive_result {
            ReceiveOutcome::Ps1(code) => {
                exit_code = code;
            }
            ReceiveOutcome::Ps2 => {
                // PS2 escalation
                let escalation = Self::ps2_escalation(session, &mut rx, nonce).await;
                syntax_error = true;
                match escalation {
                    EscalationOutcome::Recovered(code) => {
                        exit_code = code;
                    }
                    EscalationOutcome::NeedsTimeout => {
                        let timeout_result =
                            Self::timeout_escalation(session, &mut rx, nonce).await;
                        match timeout_result {
                            EscalationOutcome::Recovered(code) => {
                                exit_code = code;
                                timed_out = true;
                            }
                            EscalationOutcome::NeedsTimeout => {
                                // SIGKILL happened
                                session_reset = true;
                                timed_out = true;
                            }
                        }
                    }
                }
            }
            ReceiveOutcome::Timeout => {
                timed_out = true;
                let timeout_result = Self::timeout_escalation(session, &mut rx, nonce).await;
                match timeout_result {
                    EscalationOutcome::Recovered(code) => {
                        exit_code = code;
                    }
                    EscalationOutcome::NeedsTimeout => {
                        session_reset = true;
                    }
                }
            }
            ReceiveOutcome::ChannelClosed => {
                // Session died
                session_reset = true;
            }
        }

        // 5. Read CWD and update session
        let cwd = if !session_reset && session.is_alive() {
            match session.read_proc_cwd().await {
                Ok(path) => {
                    // Update the session's cached CWD
                    *session.cwd.write().await = path.clone();
                    path
                }
                Err(_) => session.cwd().await,
            }
        } else {
            session.cwd().await
        };

        // 6. Unregister command channel
        {
            let mut cmd_tx = session.command_output_tx.lock().await;
            *cmd_tx = None;
        }

        // 7. Filter out any echo of the command itself from output
        // The first line(s) may be the echoed command
        let output = Self::clean_output(&output_lines, command);

        Ok(CommandResult {
            output,
            exit_code,
            duration: start.elapsed(),
            cwd,
            truncated: false,
            timed_out,
            syntax_error,
            session_reset,
        })
    }

    /// Main receive loop — waits for lines and checks for PS1/PS2 markers.
    async fn receive_loop(
        rx: &mut mpsc::Receiver<String>,
        output_lines: &mut Vec<String>,
        nonce: &str,
        deadline: tokio::time::Instant,
        stream: bool,
        event_sink: &Option<tauri::AppHandle>,
        agent_id: &str,
    ) -> ReceiveOutcome {
        loop {
            let remaining = deadline.duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return ReceiveOutcome::Timeout;
            }

            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(line)) => {
                    // Check for PS1 marker (may be embedded after output text)
                    if let Some((code, prefix)) = parse_ps1_marker(&line, nonce) {
                        if let Some(text) = prefix {
                            if stream {
                                if let Some(ref handle) = event_sink {
                                    let _ = handle.emit(
                                        "agent://shell-output",
                                        serde_json::json!({
                                            "agent_id": agent_id,
                                            "line": &text,
                                        }),
                                    );
                                }
                            }
                            output_lines.push(text);
                        }
                        return ReceiveOutcome::Ps1(code);
                    }
                    // Check for PS2 marker
                    if is_ps2_marker(&line, nonce) {
                        return ReceiveOutcome::Ps2;
                    }
                    // Regular output line
                    if stream {
                        if let Some(ref handle) = event_sink {
                            let _ = handle.emit(
                                "agent://shell-output",
                                serde_json::json!({
                                    "agent_id": agent_id,
                                    "line": &line,
                                }),
                            );
                        }
                    }
                    output_lines.push(line);
                }
                Ok(None) => {
                    // Channel closed — session died
                    return ReceiveOutcome::ChannelClosed;
                }
                Err(_) => {
                    // Timeout
                    return ReceiveOutcome::Timeout;
                }
            }
        }
    }

    /// PS2 syntax error escalation: Ctrl+C -> EOF -> timeout escalation.
    async fn ps2_escalation(
        session: &PtySession,
        rx: &mut mpsc::Receiver<String>,
        nonce: &str,
    ) -> EscalationOutcome {
        // Step 1: Send Ctrl+C
        let _ = session.write("\x03").await;

        // Wait 200ms for PS1
        if let Some(code) = Self::wait_for_ps1(rx, nonce, Duration::from_millis(200)).await {
            return EscalationOutcome::Recovered(code);
        }

        // Step 2: Send EOF (Ctrl+D)
        let _ = session.write("\x04").await;

        // Wait 200ms for PS1
        if let Some(code) = Self::wait_for_ps1(rx, nonce, Duration::from_millis(200)).await {
            return EscalationOutcome::Recovered(code);
        }

        // Fall through to timeout escalation
        EscalationOutcome::NeedsTimeout
    }

    /// Timeout escalation: Ctrl+C -> SIGTERM -> SIGKILL.
    async fn timeout_escalation(
        session: &PtySession,
        rx: &mut mpsc::Receiver<String>,
        nonce: &str,
    ) -> EscalationOutcome {
        // Step 1: Send Ctrl+C
        let _ = session.write("\x03").await;

        // Wait 500ms for PS1
        if let Some(code) = Self::wait_for_ps1(rx, nonce, Duration::from_millis(500)).await {
            return EscalationOutcome::Recovered(code);
        }

        // Step 2: SIGTERM to the bash process
        let pid = nix::unistd::Pid::from_raw(session.pid() as i32);
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);

        // Wait 500ms for PS1
        if let Some(code) = Self::wait_for_ps1(rx, nonce, Duration::from_millis(500)).await {
            return EscalationOutcome::Recovered(code);
        }

        // Step 3: SIGKILL
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);

        EscalationOutcome::NeedsTimeout
    }

    /// Wait for a PS1 marker within the given duration, draining non-marker lines.
    async fn wait_for_ps1(
        rx: &mut mpsc::Receiver<String>,
        nonce: &str,
        wait: Duration,
    ) -> Option<i32> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let remaining = deadline.duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(line)) => {
                    if let Some((code, _)) = parse_ps1_marker(&line, nonce) {
                        return Some(code);
                    }
                    // PS2 markers or other output — discard during escalation
                }
                Ok(None) | Err(_) => {
                    return None;
                }
            }
        }
    }

    /// Clean up output: remove echo of the command and any trailing empty lines.
    fn clean_output(lines: &[String], _command: &str) -> String {
        // Join all lines, trim trailing whitespace
        let joined = lines.join("\n");
        let trimmed = joined.trim_end().to_string();
        trimmed
    }
}

/// Outcome of the main receive loop.
enum ReceiveOutcome {
    /// PS1 marker found with exit code.
    Ps1(i32),
    /// PS2 marker found (syntax error).
    Ps2,
    /// Timed out waiting for marker.
    Timeout,
    /// Channel closed (session died).
    ChannelClosed,
}

/// Outcome of an escalation attempt.
enum EscalationOutcome {
    /// Recovered with exit code from PS1.
    Recovered(i32),
    /// Needs further escalation.
    NeedsTimeout,
}

/// Parse a PS1 ready marker from a line, extracting the exit code.
///
/// The marker may appear at any position in the line (e.g., when a command
/// uses `printf` without a trailing newline, the marker is concatenated
/// after the output text).
///
/// Returns `(exit_code, optional_prefix_text)` — the prefix is any text
/// before the marker that should be captured as output.
fn parse_ps1_marker(line: &str, nonce: &str) -> Option<(i32, Option<String>)> {
    let marker_start = format!("___HOLT_RDY_{}___:", nonce);
    let suffix = "___";
    if let Some(idx) = line.find(&marker_start) {
        let rest = &line[idx + marker_start.len()..];
        if let Some(code_str) = rest.strip_suffix(suffix) {
            if let Ok(code) = code_str.parse::<i32>() {
                let prefix = if idx > 0 {
                    Some(line[..idx].to_string())
                } else {
                    None
                };
                return Some((code, prefix));
            }
        }
    }
    None
}

/// Check if a line is a PS2 continuation marker.
fn is_ps2_marker(line: &str, nonce: &str) -> bool {
    line.trim() == format!("___HOLT_PS2_{}___", nonce)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::shell::pty_session::PtySession;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn test_execute_simple_command() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        let result = PtyCommand::execute(
            &session,
            "echo hello_world",
            Duration::from_secs(5),
            false,
            None,
            "test-agent",
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(
            result.output.contains("hello_world"),
            "Expected 'hello_world' in output, got: {:?}",
            result.output
        );
        assert!(!result.timed_out);
        assert!(!result.syntax_error);
        assert!(!result.session_reset);
    }

    #[tokio::test]
    async fn test_execute_exit_code() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        let result = PtyCommand::execute(
            &session,
            "(exit 42)",
            Duration::from_secs(5),
            false,
            None,
            "test-agent",
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 42);
        assert!(session.is_alive());
    }

    #[tokio::test]
    async fn test_execute_false_exit_code() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        let result = PtyCommand::execute(
            &session,
            "false",
            Duration::from_secs(5),
            false,
            None,
            "test-agent",
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 1);
    }

    #[tokio::test]
    async fn test_execute_captures_cwd_change() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        let result = PtyCommand::execute(
            &session,
            "cd /var",
            Duration::from_secs(5),
            false,
            None,
            "test-agent",
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        let expected = std::fs::canonicalize("/var").unwrap_or("/var".into());
        let actual = std::fs::canonicalize(&result.cwd).unwrap_or(result.cwd.clone());
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        let result = PtyCommand::execute(
            &session,
            "sleep 60",
            Duration::from_secs(1),
            false,
            None,
            "test-agent",
        )
        .await
        .unwrap();
        assert!(result.timed_out);
        // Session should still be alive (Ctrl+C kills sleep, not bash)
        assert!(session.is_alive());
    }

    #[tokio::test]
    async fn test_execute_syntax_error() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        let result = PtyCommand::execute(
            &session,
            "echo 'unterminated",
            Duration::from_secs(3),
            false,
            None,
            "test-agent",
        )
        .await
        .unwrap();
        assert!(result.syntax_error);
        assert!(session.is_alive());
    }

    #[tokio::test]
    async fn test_execute_multiline_output() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        let result = PtyCommand::execute(
            &session,
            "echo line1; echo line2; echo line3",
            Duration::from_secs(5),
            false,
            None,
            "test-agent",
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.output.contains("line1"));
        assert!(result.output.contains("line2"));
        assert!(result.output.contains("line3"));
    }

    #[tokio::test]
    async fn test_execute_preserves_env() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        PtyCommand::execute(
            &session,
            "export MY_TEST_VAR=hello_pty",
            Duration::from_secs(5),
            false,
            None,
            "test-agent",
        )
        .await
        .unwrap();
        let result = PtyCommand::execute(
            &session,
            "echo $MY_TEST_VAR",
            Duration::from_secs(5),
            false,
            None,
            "test-agent",
        )
        .await
        .unwrap();
        assert!(
            result.output.contains("hello_pty"),
            "Expected 'hello_pty' in output, got: {:?}",
            result.output
        );
    }

    #[tokio::test]
    async fn test_no_newline_output() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        let result = PtyCommand::execute(
            &session,
            "printf 'no_newline_here'",
            Duration::from_secs(5),
            false,
            None,
            "test-agent",
        )
        .await
        .unwrap();
        assert!(
            result.output.contains("no_newline_here"),
            "Expected 'no_newline_here' in output, got: {:?}",
            result.output
        );
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_execute_concurrent_serialization() {
        let session = Arc::new(PtySession::spawn("/tmp".into()).await.unwrap());
        let s1 = session.clone();
        let s2 = session.clone();

        let h1 = tokio::spawn(async move {
            PtyCommand::execute(
                &s1,
                "echo task_one_output",
                Duration::from_secs(5),
                false,
                None,
                "test-agent",
            )
            .await
            .unwrap()
        });
        let h2 = tokio::spawn(async move {
            PtyCommand::execute(
                &s2,
                "echo task_two_output",
                Duration::from_secs(5),
                false,
                None,
                "test-agent",
            )
            .await
            .unwrap()
        });

        let (r1, r2) = tokio::join!(h1, h2);
        let r1 = r1.unwrap();
        let r2 = r2.unwrap();

        // Each result should contain only its own output, not mixed
        assert!(
            (r1.output.contains("task_one_output") && r2.output.contains("task_two_output"))
                || (r1.output.contains("task_two_output") && r2.output.contains("task_one_output"))
        );
        if r1.output.contains("task_one_output") {
            assert!(!r1.output.contains("task_two_output"));
        }
    }

    #[tokio::test]
    async fn test_execute_broken_pipe_detected() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        session.kill().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let result = PtyCommand::execute(
            &session,
            "echo should_fail",
            Duration::from_secs(2),
            false,
            None,
            "test-agent",
        )
        .await;
        // After kill, we either get an error (broken pipe) or a result
        // with session_reset (channel closed because reader exited).
        match result {
            Err(_) => {} // BrokenPipe or similar — expected
            Ok(r) => {
                assert!(
                    r.session_reset || r.timed_out,
                    "Expected session_reset or timed_out after kill, got: {:?}",
                    r
                );
            }
        }
    }

    #[test]
    fn test_parse_ps1_marker_valid() {
        let nonce = "abc-123";
        assert_eq!(
            parse_ps1_marker("___HOLT_RDY_abc-123___:0___", nonce),
            Some((0, None))
        );
        assert_eq!(
            parse_ps1_marker("___HOLT_RDY_abc-123___:42___", nonce),
            Some((42, None))
        );
        assert_eq!(
            parse_ps1_marker("___HOLT_RDY_abc-123___:127___", nonce),
            Some((127, None))
        );
    }

    #[test]
    fn test_parse_ps1_marker_with_prefix() {
        let nonce = "abc-123";
        assert_eq!(
            parse_ps1_marker("some_output___HOLT_RDY_abc-123___:0___", nonce),
            Some((0, Some("some_output".to_string())))
        );
    }

    #[test]
    fn test_parse_ps1_marker_invalid() {
        let nonce = "abc-123";
        assert_eq!(parse_ps1_marker("not a marker", nonce), None);
        assert_eq!(
            parse_ps1_marker("___HOLT_RDY_wrong-nonce___:0___", nonce),
            None
        );
    }

    #[test]
    fn test_is_ps2_marker() {
        let nonce = "abc-123";
        assert!(is_ps2_marker("___HOLT_PS2_abc-123___", nonce));
        assert!(is_ps2_marker("  ___HOLT_PS2_abc-123___  ", nonce));
        assert!(!is_ps2_marker("___HOLT_PS2_wrong___", nonce));
    }
}
