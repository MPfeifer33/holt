use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use crate::providers::types::{StreamEvent, StreamEventType};
use crate::runtime::agent::{AgentSlot, ConversationMessage, MessageRole};
use crate::tools::shell::ShellManager;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

/// Tracks the state of an active sandbox session for an agent.
/// Serializable for crash recovery via AgentSession.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxState {
    /// The agent's original working directory (restored on exit)
    pub original_working_dir: PathBuf,
    /// Path to the git worktree directory
    pub worktree_path: PathBuf,
    /// Name of the sandbox git branch
    pub worktree_branch: String,
    /// The command that triggered the sandbox (re-run to verify fixes)
    pub trigger_command: String,
    /// Current iteration (0-based, max_iterations exclusive)
    pub iteration: u32,
    /// Current attempt within iteration: 0 = A, 1 = B
    pub attempt: u8,
    /// Maximum iterations before escalation
    pub max_iterations: u32,
}

/// Build/test command patterns that trigger sandbox on failure.
const BUILD_TEST_PATTERNS: &[&str] = &[
    "cargo build",
    "cargo test",
    "cargo check",
    "npm run build",
    "npm test",
    "npm run tauri dev",
    "npm run tauri build",
    "pytest",
    "python -m pytest",
    "go build",
    "go test",
    "make",
    "cmake --build",
    "tauri build",
    "tauri dev",
];

/// Check if a bash command matches a build/test pattern.
/// Handles chained commands like "cd /path && cargo build" by checking
/// each segment separated by && or ;.
/// Only checks the actual command name (first tokens), not quoted string contents.
pub fn is_build_test_command(command: &str) -> bool {
    // Strip quoted content first so "echo 'a && cargo build'" doesn't false-positive
    let stripped = strip_quoted_content(&command.trim().to_lowercase());
    // Check each segment of chained commands (cd /path && cargo build)
    for segment in stripped.split("&&").chain(stripped.split(';')) {
        let effective = extract_command(segment.trim());
        if BUILD_TEST_PATTERNS
            .iter()
            .any(|pattern| effective.starts_with(pattern))
        {
            return true;
        }
    }
    false
}

/// Strip content inside single and double quotes, leaving empty quotes behind.
/// `echo 'cargo build' file` → `echo '' file`
fn strip_quoted_content(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\'' || c == '"' {
            result.push(c);
            // Consume until matching close quote
            for inner in chars.by_ref() {
                if inner == c {
                    result.push(inner);
                    break;
                }
                // Don't emit inner content
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Extract the actual command from a segment, skipping env var assignments
/// and stripping subshell parens.
/// `(RUST_LOG=debug cargo test)` → `cargo test`
/// `/usr/bin/cargo build` → `cargo build`
fn extract_command(segment: &str) -> String {
    // Strip outer parens (subshell)
    let segment = segment.trim_start_matches('(').trim_end_matches(')').trim();
    let tokens: Vec<&str> = segment.split_whitespace().collect();
    if tokens.is_empty() {
        return String::new();
    }
    // Skip leading env var assignments (tokens containing '=')
    let cmd_start = tokens.iter().position(|t| !t.contains('=')).unwrap_or(0);
    // Take basename of the command token to handle full paths like /usr/bin/cargo
    let mut effective_tokens: Vec<&str> = tokens[cmd_start..].to_vec();
    if let Some(first) = effective_tokens.first_mut() {
        if let Some(basename) = first.rsplit('/').next() {
            *first = basename;
        }
    }
    effective_tokens.join(" ")
}

/// File patterns that should never be auto-committed by the sandbox.
/// After staging, these are unstaged as a safety net against secret leaks.
const SECRET_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "credentials.json",
    "secrets.json",
    "*.secret",
    ".secrets",
    "id_rsa",
    "id_ed25519",
    "*.keystore",
    "service-account*.json",
];

/// Stage all changes while excluding known secret file patterns.
/// Uses `git add .` then unstages anything matching SECRET_PATTERNS.
/// Returns the list of patterns that were actually unstaged (for logging).
async fn safe_stage_all(dir: &Path) -> Vec<String> {
    // Stage everything (respects .gitignore)
    let _ = git_command(dir, &["add", "."]).await;

    let mut unstaged = Vec::new();
    for pattern in SECRET_PATTERNS {
        // Check if any staged files match this pattern before resetting
        if let Ok(matched) =
            git_command(dir, &["diff", "--cached", "--name-only", "--", pattern]).await
        {
            if !matched.is_empty() {
                let _ = git_command(dir, &["reset", "HEAD", "--", pattern]).await;
                for file in matched.lines() {
                    tracing::warn!(
                        file = file,
                        "sandbox: unstaged secret-pattern file from auto-commit"
                    );
                    unstaged.push(file.to_string());
                }
            }
        }
    }
    unstaged
}

/// Clamp command output to at most `max_bytes`, backing off to a char
/// boundary — slicing at a fixed byte offset panics on multi-byte characters.
fn clamp_output(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Sanitize an agent ID for use in git branch names.
/// Replaces non-alphanumeric characters (except -) with -.
pub fn sanitize_for_git(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_test_command_detection() {
        assert!(is_build_test_command("cargo build"));
        assert!(is_build_test_command("cargo test --release"));
        assert!(is_build_test_command("npm run build"));
        assert!(is_build_test_command("npm run tauri dev"));
        assert!(is_build_test_command("npm run tauri build"));
        assert!(is_build_test_command("pytest tests/"));
        assert!(is_build_test_command("CARGO BUILD")); // case insensitive
        assert!(!is_build_test_command("ls -la"));
        assert!(!is_build_test_command("curl http://example.com"));
        assert!(!is_build_test_command("echo hello"));
        assert!(!is_build_test_command("cat Cargo.toml"));
        // Chained commands
        assert!(is_build_test_command("cd /some/path && cargo build"));
        assert!(is_build_test_command("cd /tmp/test && npm run build"));
        assert!(is_build_test_command("source env.sh; cargo test"));
        assert!(!is_build_test_command("cd /some/path && ls"));
        // Env var prefix
        assert!(is_build_test_command("RUST_LOG=debug cargo test"));
        assert!(is_build_test_command("FOO=bar BAZ=1 cargo build"));
        // False positive: build command mentioned in quoted string content
        assert!(!is_build_test_command(
            "echo 'cargo build failed' >> log.md"
        ));
        assert!(!is_build_test_command(
            "echo -e '\\n`cargo build`\\n' >> post-mortem.md"
        ));

        // --- Edge case stress tests ---

        // 1. Pipe: build command after pipe should still match (it's a real command)
        //    But split on && and ; won't catch pipes. Pipes are tricky —
        //    `cat file | cargo build` is not a real pattern, so not matching is OK.
        assert!(!is_build_test_command("cat Cargo.toml | grep version"));

        // 2. Subshell: (cargo build) — parens stripped
        assert!(is_build_test_command("(cargo build)"));
        assert!(is_build_test_command("(cd /tmp && cargo test)"));

        // 3. Redirect before command: should not confuse the parser
        assert!(!is_build_test_command("2>/dev/null echo 'cargo test'"));

        // 4. Heredoc with build command in body — should NOT trigger
        assert!(!is_build_test_command("cat << 'EOF'\ncargo build\nEOF"));

        // 5. grep/sed searching for build patterns — should NOT trigger
        assert!(!is_build_test_command("grep 'cargo build' Makefile"));
        assert!(!is_build_test_command(
            "sed -i 's/cargo build/cargo check/' script.sh"
        ));

        // 6. Variable assignment that looks like env prefix but IS the command
        //    `FOO=bar` alone is a valid bash command (sets var), not a build command
        assert!(!is_build_test_command("CARGO_TARGET_DIR=/tmp/build"));

        // 7. Path prefix on command — basename extracted
        assert!(is_build_test_command("/usr/bin/cargo build"));
        assert!(is_build_test_command("/usr/local/bin/pytest tests/"));
        assert!(!is_build_test_command("/usr/bin/echo 'cargo build'"));

        // 8. Multiple env vars then build
        assert!(is_build_test_command("CC=gcc CXX=g++ CFLAGS=-O2 make"));
        assert!(is_build_test_command(
            "RUST_BACKTRACE=1 RUST_LOG=debug cargo test -- --nocapture"
        ));

        // 9. cd with spaces in path then build
        assert!(is_build_test_command(
            "cd '/path with spaces/project' && cargo build"
        ));

        // 10. Comment after command — should still match the command
        assert!(is_build_test_command(
            "cargo build # this builds the project"
        ));

        // 11. echo piping into a file with build-like filename
        assert!(!is_build_test_command(
            "echo 'test' > cargo_build_output.txt"
        ));

        // 12. tee/redirect with build command name in the path
        assert!(!is_build_test_command(
            "echo 'done' | tee cargo_test_results.log"
        ));

        // 13. String with && inside quotes — quoted content stripped before splitting
        assert!(!is_build_test_command("echo 'a && cargo build'"));
        assert!(!is_build_test_command(
            "echo \"cd /tmp && cargo test\" > script.sh"
        ));
        assert!(!is_build_test_command(
            "printf '%s\\n' 'npm run build && npm test'"
        ));

        // 14. Empty and whitespace-only commands
        assert!(!is_build_test_command(""));
        assert!(!is_build_test_command("   "));

        // 15. Just the command with trailing whitespace
        assert!(is_build_test_command("  cargo build  "));
        assert!(is_build_test_command("cargo test  \n"));
    }

    #[test]
    fn test_secret_patterns_not_empty() {
        // Ensure the secret patterns list is populated and reasonable
        assert!(!SECRET_PATTERNS.is_empty());
        assert!(SECRET_PATTERNS.contains(&".env"));
        assert!(SECRET_PATTERNS.contains(&"*.pem"));
        assert!(SECRET_PATTERNS.contains(&"*.key"));
        assert!(SECRET_PATTERNS.contains(&"credentials.json"));
    }

    #[test]
    fn test_secret_patterns_are_valid_globs() {
        // Each pattern should be a non-empty string that doesn't contain path separators
        // (they're meant to match filenames, not paths)
        for pattern in SECRET_PATTERNS {
            assert!(!pattern.is_empty(), "Empty secret pattern");
            assert!(
                !pattern.contains('/'),
                "Secret pattern '{}' contains path separator — use filename-only patterns",
                pattern
            );
        }
    }

    #[test]
    fn test_secret_patterns_cover_common_secrets() {
        let patterns_joined = SECRET_PATTERNS.join(" ");
        // Env files
        assert!(patterns_joined.contains(".env"));
        // TLS/SSH keys
        assert!(patterns_joined.contains("*.pem"));
        assert!(patterns_joined.contains("*.key"));
        assert!(patterns_joined.contains("id_rsa"));
        assert!(patterns_joined.contains("id_ed25519"));
        // Credential files
        assert!(patterns_joined.contains("credentials.json"));
        assert!(patterns_joined.contains("secrets.json"));
        // Java/Android keystores
        assert!(patterns_joined.contains("*.keystore"));
        // GCP service accounts
        assert!(patterns_joined.contains("service-account"));
    }

    #[test]
    fn test_sanitize_for_git() {
        assert_eq!(sanitize_for_git("abc-123"), "abc-123");
        assert_eq!(sanitize_for_git("my agent"), "my-agent");
        assert_eq!(sanitize_for_git("agent/feature"), "agent-feature");
        assert_eq!(sanitize_for_git("a~b^c:d"), "a-b-c-d");
    }
}

/// Run a git command in a given directory, returning stdout on success.
async fn git_command(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("Failed to run git: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("git {} failed: {}", args.join(" "), stderr.trim()))
    }
}

/// Run a shell command in a given directory, returning (exit_code, combined output).
/// Used for re-running the trigger command in the sandbox.
pub async fn run_command(dir: &Path, command: &str) -> (i32, String) {
    let result = tokio::process::Command::new("bash")
        .args(["-c", command])
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match result {
        Ok(output) => {
            let code = output.status.code().unwrap_or(-1);
            let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                combined.push_str("\n--- stderr ---\n");
                combined.push_str(&stderr);
            }
            if combined.len() > 50_000 {
                combined.truncate(50_000);
                combined.push_str("\n... (output truncated)");
            }
            (code, combined)
        }
        Err(e) => (-1, format!("Failed to execute command: {e}")),
    }
}

/// Create a sandbox worktree from the current branch.
pub async fn create_worktree(
    project_dir: &Path,
    agent_id: &str,
) -> Result<(PathBuf, String), String> {
    // Safety snapshot — commit any uncommitted changes (excluding secrets)
    safe_stage_all(project_dir).await;
    let _ = git_command(
        project_dir,
        &["commit", "-m", "sandbox: safety snapshot (auto)"],
    )
    .await;

    let sanitized = sanitize_for_git(agent_id);
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let branch_name = format!("sandbox-{}-{}", sanitized, timestamp);
    let worktree_dir = project_dir.join(format!(".sandbox-{}", sanitized));

    // Remove stale worktree if it exists
    if worktree_dir.exists() {
        let _ = git_command(
            project_dir,
            &[
                "worktree",
                "remove",
                "--force",
                &worktree_dir.display().to_string(),
            ],
        )
        .await;
    }

    git_command(
        project_dir,
        &[
            "worktree",
            "add",
            &worktree_dir.display().to_string(),
            "-b",
            &branch_name,
            "HEAD",
        ],
    )
    .await
    .map_err(|e| format!("Failed to create worktree: {e}"))?;

    // Initial snapshot commit in the worktree
    let _ = git_command(
        &worktree_dir,
        &["commit", "--allow-empty", "-m", "sandbox: initial snapshot"],
    )
    .await;

    Ok((worktree_dir, branch_name))
}

/// Rollback worktree to the last committed state (for switching from attempt A to B).
pub async fn rollback_worktree(worktree_dir: &Path) -> Result<(), String> {
    git_command(worktree_dir, &["checkout", "HEAD", "--", "."]).await?;
    git_command(worktree_dir, &["clean", "-fd"]).await?;
    Ok(())
}

/// Commit current worktree state as a snapshot before the next attempt.
pub async fn snapshot_worktree(worktree_dir: &Path, message: &str) -> Result<(), String> {
    safe_stage_all(worktree_dir).await;
    let _ = git_command(worktree_dir, &["commit", "-m", message]).await;
    Ok(())
}

/// Merge sandbox branch back into the original branch and clean up.
pub async fn merge_and_cleanup(
    project_dir: &Path,
    worktree_dir: &Path,
    branch_name: &str,
) -> Result<(), String> {
    let original_branch = git_command(project_dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .unwrap_or_else(|_| "main".to_string());

    // Commit any remaining changes in the worktree (excluding secrets)
    safe_stage_all(worktree_dir).await;
    let _ = git_command(worktree_dir, &["commit", "-m", "sandbox: final fix"]).await;

    // Remove worktree (branch survives removal)
    git_command(
        project_dir,
        &["worktree", "remove", &worktree_dir.display().to_string()],
    )
    .await
    .map_err(|e| format!("Failed to remove worktree: {e}"))?;

    // Verify we're on the expected branch before merging
    let current_branch = git_command(project_dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .unwrap_or_default();
    if current_branch != original_branch {
        git_command(project_dir, &["checkout", &original_branch])
            .await
            .map_err(|e| format!("Failed to checkout {}: {e}", original_branch))?;
    }

    // Merge sandbox branch
    git_command(project_dir, &["merge", branch_name])
        .await
        .map_err(|e| format!("Merge failed (may need manual resolution): {e}"))?;

    // Delete sandbox branch
    let _ = git_command(project_dir, &["branch", "-d", branch_name]).await;

    Ok(())
}

/// Remove a worktree without merging (for cleanup after escalation or abort).
pub async fn remove_worktree(project_dir: &Path, worktree_dir: &Path) {
    let _ = git_command(
        project_dir,
        &[
            "worktree",
            "remove",
            "--force",
            &worktree_dir.display().to_string(),
        ],
    )
    .await;
}

// ── Sandbox lifecycle functions ─────────────────────────────────────────────

/// Return type for `advance()` — tells the caller whether to continue the loop or escalate.
#[derive(Debug, Clone, PartialEq)]
pub enum SandboxAction {
    /// The sandbox advanced successfully; the agent should continue fixing.
    Continue,
    /// All iterations exhausted; the caller should escalate to the user.
    Escalate,
}

/// Enter the sandbox: create a worktree, redirect the agent's working directory
/// and shell session into it, and inject an instruction message.
pub async fn enter(
    agents: &Arc<RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    shell_manager: &Arc<RwLock<ShellManager>>,
    app_handle: Option<&AppHandle>,
    trigger_command: &str,
    failure_output: &str,
    max_iterations: u32,
) -> Result<(), String> {
    // 1. Read the agent's current working directory (read lock, then drop)
    let original_dir = {
        let agents_read = agents.read().await;
        let agent = agents_read
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or("Agent not found")?;
        agent.working_directory.clone()
    };

    // 2. Create the worktree (no lock held)
    let (worktree_path, branch_name) = create_worktree(&original_dir, agent_id).await?;

    // 3. Write lock to update agent state
    {
        let mut agents_write = agents.write().await;
        let agent = agents_write
            .iter_mut()
            .find(|a| a.id == agent_id)
            .ok_or("Agent not found")?;

        agent.sandbox = Some(SandboxState {
            original_working_dir: original_dir,
            worktree_path: worktree_path.clone(),
            worktree_branch: branch_name,
            trigger_command: trigger_command.to_string(),
            iteration: 0,
            attempt: 0,
            max_iterations,
        });

        agent.working_directory = worktree_path.clone();

        let msg = format!(
            "[SANDBOX] A build/test command failed. You are now working in an isolated sandbox.\n\
             Failed command: `{}`\n\
             Output:\n```\n{}\n```\n\
             Fix the issue and the command will be re-verified automatically. \
             You have {} iteration(s) with 2 attempts each before escalation.",
            trigger_command,
            clamp_output(&failure_output, 4000),
            max_iterations,
        );
        agent.add_message(ConversationMessage {
            role: MessageRole::System,
            content: msg,
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

    // 4. Update shell cwd
    {
        let sm = shell_manager.read().await;
        if let Some(session) = sm.sessions.get(agent_id) {
            let _ = session.set_cwd(worktree_path.clone()).await;
        }
    }

    // 5. Emit event
    if let Some(handle) = app_handle {
        let _ = handle.emit(
            "agent-stream",
            StreamEvent {
                agent_id: agent_id.to_string(),
                event_type: StreamEventType::SandboxStatus,
                data: serde_json::json!({
                    "action": "enter",
                    "message": format!("Entered sandbox for: {}", trigger_command),
                }),
            },
        );
    }

    Ok(())
}

/// Advance the sandbox after a verification failure.
///
/// - attempt 0 → rollback to snapshot, switch to attempt B (attempt = 1)
/// - attempt 1 → snapshot current state, advance to next iteration (attempt = 0)
///   - If iterations exhausted → return `SandboxAction::Escalate`
///
/// CRITICAL: Uses clone-then-drop to avoid holding locks across async git ops.
pub async fn advance(
    agents: &Arc<RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    failure_output: &str,
) -> Result<SandboxAction, String> {
    // 1. Read state with read lock, clone what we need, drop lock
    let (attempt, iteration, max_iterations, worktree_path) = {
        let agents_read = agents.read().await;
        let agent = agents_read
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or("Agent not found")?;
        let sandbox = agent.sandbox.as_ref().ok_or("Agent is not in a sandbox")?;
        (
            sandbox.attempt,
            sandbox.iteration,
            sandbox.max_iterations,
            sandbox.worktree_path.clone(),
        )
    }; // lock dropped

    if attempt == 0 {
        // Attempt A failed → rollback to snapshot, switch to attempt B
        // 2. Git operations with NO lock held
        rollback_worktree(&worktree_path).await?;

        // 3. Re-acquire write lock to update state and inject message
        {
            let mut agents_write = agents.write().await;
            if let Some(agent) = agents_write.iter_mut().find(|a| a.id == agent_id) {
                if let Some(sandbox) = agent.sandbox.as_mut() {
                    sandbox.attempt = 1;
                }
                let msg = format!(
                    "[SANDBOX] Solution A failed verification. The worktree has been rolled back.\n\
                     Try a different approach (Solution B).\n\
                     Failure output:\n```\n{}\n```",
                    clamp_output(&failure_output, 4000),
                );
                agent.add_message(ConversationMessage {
                    role: MessageRole::System,
                    content: msg,
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

        Ok(SandboxAction::Continue)
    } else {
        // Attempt B failed → snapshot, advance iteration
        // 2. Git operations with NO lock held
        let snap_msg = format!("sandbox: iteration {} attempt B snapshot", iteration);
        snapshot_worktree(&worktree_path, &snap_msg).await?;

        let new_iteration = iteration + 1;
        if new_iteration >= max_iterations {
            return Ok(SandboxAction::Escalate);
        }

        // 3. Re-acquire write lock to update state and inject message
        {
            let mut agents_write = agents.write().await;
            if let Some(agent) = agents_write.iter_mut().find(|a| a.id == agent_id) {
                if let Some(sandbox) = agent.sandbox.as_mut() {
                    sandbox.iteration = new_iteration;
                    sandbox.attempt = 0;
                }
                let msg = format!(
                    "[SANDBOX] Both attempts failed in iteration {}. Starting iteration {} of {}.\n\
                     Previous failure output:\n```\n{}\n```\n\
                     Try a fundamentally different approach.",
                    iteration,
                    new_iteration + 1,
                    max_iterations,
                    clamp_output(&failure_output, 4000),
                );
                agent.add_message(ConversationMessage {
                    role: MessageRole::System,
                    content: msg,
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

        Ok(SandboxAction::Continue)
    }
}

/// Verify the sandbox fix by re-running the trigger command. On success, merge
/// the worktree back and restore the agent to its original working directory.
pub async fn verify_and_exit(
    agents: &Arc<RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    shell_manager: &Arc<RwLock<ShellManager>>,
    app_handle: Option<&AppHandle>,
) -> Result<(), String> {
    // 1. Read state (clone what we need, drop lock)
    let (worktree_path, branch, original_dir, trigger_command) = {
        let agents_read = agents.read().await;
        let agent = agents_read
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or("Agent not found")?;
        let sandbox = agent.sandbox.as_ref().ok_or("Agent is not in a sandbox")?;
        (
            sandbox.worktree_path.clone(),
            sandbox.worktree_branch.clone(),
            sandbox.original_working_dir.clone(),
            sandbox.trigger_command.clone(),
        )
    };

    // 2. Run the trigger command to verify the fix (no lock held)
    let (exit_code, output) = run_command(&worktree_path, &trigger_command).await;
    if exit_code != 0 {
        return Err(format!(
            "Verification failed (exit code {}):\n{}",
            exit_code,
            clamp_output(&output, 4000),
        ));
    }

    // 3. Merge and cleanup (no lock held)
    merge_and_cleanup(&original_dir, &worktree_path, &branch).await?;

    // 4. Write lock to restore agent state
    {
        let mut agents_write = agents.write().await;
        if let Some(agent) = agents_write.iter_mut().find(|a| a.id == agent_id) {
            agent.sandbox = None;
            agent.working_directory = original_dir.clone();

            let msg = format!(
                "[SANDBOX] Fix verified successfully! `{}` passed.\n\
                 Changes have been merged back to the original branch. \
                 Resuming normal operation.",
                trigger_command,
            );
            agent.add_message(ConversationMessage {
                role: MessageRole::System,
                content: msg,
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

    // 5. Restore shell cwd
    {
        let sm = shell_manager.read().await;
        if let Some(session) = sm.sessions.get(agent_id) {
            let _ = session.set_cwd(original_dir.clone()).await;
        }
    }

    // 6. Emit event
    if let Some(handle) = app_handle {
        let _ = handle.emit(
            "agent-stream",
            StreamEvent {
                agent_id: agent_id.to_string(),
                event_type: StreamEventType::SandboxStatus,
                data: serde_json::json!({
                    "action": "exit",
                    "message": "Sandbox fix verified and merged",
                }),
            },
        );
    }

    Ok(())
}

/// Escalate: the sandbox exhausted all iterations without a passing fix.
/// Clears sandbox state, restores the agent to its original directory, and
/// injects an escalation message. The worktree is preserved for inspection.
pub async fn escalate(
    agents: &Arc<RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    shell_manager: &Arc<RwLock<ShellManager>>,
    app_handle: Option<&AppHandle>,
) {
    // 1. Read state (clone what we need, drop lock)
    let (original_dir, worktree_path) = {
        let agents_read = agents.read().await;
        let Some(agent) = agents_read.iter().find(|a| a.id == agent_id) else {
            return;
        };
        let Some(sandbox) = agent.sandbox.as_ref() else {
            return;
        };
        (
            sandbox.original_working_dir.clone(),
            sandbox.worktree_path.clone(),
        )
    };

    // 2. Write lock to clear sandbox and restore state
    {
        let mut agents_write = agents.write().await;
        if let Some(agent) = agents_write.iter_mut().find(|a| a.id == agent_id) {
            agent.sandbox = None;
            agent.working_directory = original_dir.clone();

            let msg = format!(
                "[SANDBOX] All fix attempts exhausted. Escalating to user.\n\
                 The sandbox worktree is preserved at `{}` for manual inspection.\n\
                 Resuming normal operation in the original directory.",
                worktree_path.display(),
            );
            agent.add_message(ConversationMessage {
                role: MessageRole::System,
                content: msg,
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

    // 3. Restore shell cwd
    {
        let sm = shell_manager.read().await;
        if let Some(session) = sm.sessions.get(agent_id) {
            let _ = session.set_cwd(original_dir.clone()).await;
        }
    }

    // 4. Emit event
    if let Some(handle) = app_handle {
        let _ = handle.emit(
            "agent-stream",
            StreamEvent {
                agent_id: agent_id.to_string(),
                event_type: StreamEventType::SandboxStatus,
                data: serde_json::json!({
                    "action": "escalate",
                    "message": "Sandbox exhausted all iterations, escalating to user",
                }),
            },
        );
    }
}

/// Recover a sandbox session after a crash or restart.
///
/// Returns `true` if the agent was in a sandbox and recovery succeeded (or
/// the sandbox was cleaned up because the worktree was missing).
/// Returns `false` if the agent had no sandbox state.
pub async fn recover(
    agents: &Arc<RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    shell_manager: &Arc<RwLock<ShellManager>>,
) -> bool {
    // 1. Read state (check if sandbox exists, clone what we need)
    let sandbox_info = {
        let agents_read = agents.read().await;
        let Some(agent) = agents_read.iter().find(|a| a.id == agent_id) else {
            return false;
        };
        let Some(sandbox) = agent.sandbox.as_ref() else {
            return false;
        };
        (
            sandbox.worktree_path.clone(),
            sandbox.iteration,
            sandbox.max_iterations,
        )
    };
    let (worktree_path, iteration, max_iterations) = sandbox_info;

    // 2. Check if the worktree still exists on disk
    if !worktree_path.exists() {
        // Worktree is gone — clear sandbox state
        let mut agents_write = agents.write().await;
        if let Some(agent) = agents_write.iter_mut().find(|a| a.id == agent_id) {
            let original_dir = agent
                .sandbox
                .as_ref()
                .map(|s| s.original_working_dir.clone())
                .unwrap_or_else(|| agent.working_directory.clone());
            agent.sandbox = None;
            agent.working_directory = original_dir.clone();

            agent.add_message(ConversationMessage {
                role: MessageRole::System,
                content: "[SANDBOX] Recovery: sandbox worktree no longer exists. \
                         Sandbox state cleared, resuming normal operation."
                    .to_string(),
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

            // Restore shell cwd
            let sm = shell_manager.read().await;
            if let Some(session) = sm.sessions.get(agent_id) {
                let _ = session.set_cwd(original_dir.clone()).await;
            }
        }
        return true;
    }

    // 3. Worktree exists — restore working directory and shell cwd into it
    {
        let mut agents_write = agents.write().await;
        if let Some(agent) = agents_write.iter_mut().find(|a| a.id == agent_id) {
            agent.working_directory = worktree_path.clone();

            let msg = format!(
                "[SANDBOX] Recovery: resuming sandbox session.\n\
                 Iteration {} of {}, worktree at `{}`.\n\
                 Continue fixing the issue.",
                iteration + 1,
                max_iterations,
                worktree_path.display(),
            );
            agent.add_message(ConversationMessage {
                role: MessageRole::System,
                content: msg,
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

    // 4. Restore shell cwd
    {
        let sm = shell_manager.read().await;
        if let Some(session) = sm.sessions.get(agent_id) {
            let _ = session.set_cwd(worktree_path).await;
        }
    }

    true
}
