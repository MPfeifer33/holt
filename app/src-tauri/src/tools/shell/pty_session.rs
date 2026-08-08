//! PtySession — owns a persistent PTY + bash process.
//!
//! Handles spawning bash with a PTY, background reading via vt100,
//! CWD tracking via `/proc/<pid>/cwd`, and graceful kill with
//! SIGTERM → SIGKILL escalation.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::agent_config::ExecutionSandboxBlock;
use crate::tools::types::{ToolError, ToolErrorCode};

/// Maximum lines retained in the rolling output buffer.
const OUTPUT_BUFFER_MAX_LINES: usize = 1000;
const MAX_LINE_BYTES: usize = 102_400; // 100KB per line

/// Spawn helper: uses tokio::spawn in tests, tauri::async_runtime::spawn otherwise.
#[cfg(test)]
fn spawn_task<F>(f: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(f)
}

#[cfg(not(test))]
fn spawn_task<F>(f: F) -> tauri::async_runtime::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    tauri::async_runtime::spawn(f)
}

/// A persistent PTY session owning a bash process.
pub struct PtySession {
    /// Write half of the PTY (read half is owned by the background reader task).
    pty_writer: Arc<Mutex<pty_process::OwnedWritePty>>,
    /// Bash shell PID — used for `/proc/<pid>/cwd`.
    pid: u32,
    /// Thread-safe current working directory.
    pub(crate) cwd: Arc<RwLock<PathBuf>>,
    /// Rolling output buffer (last N lines).
    output_buffer: Arc<Mutex<VecDeque<String>>>,
    /// Per-command output channel. PtyCommand registers a sender here before
    /// executing a command; the background reader forwards lines to it.
    pub(crate) command_output_tx: Arc<Mutex<Option<tokio::sync::mpsc::Sender<String>>>>,
    /// UUID nonce embedded in PS1/PS2 patterns for marker detection.
    pub session_nonce: String,
    /// Serializes command execution so only one command runs at a time.
    pub(crate) command_mutex: tokio::sync::Mutex<()>,
    /// Cancellation token for the background reader task.
    reader_cancel: CancellationToken,
    /// Whether the bash process is still alive.
    alive: Arc<AtomicBool>,
    /// Tokio child handle for proper reaping (avoids zombie processes).
    child: Option<Mutex<tokio::process::Child>>,
}

impl PtySession {
    /// Spawn a new bash process on a fresh PTY (unrestricted — no sandbox).
    ///
    /// `initial_cwd` is the directory bash will start in.
    pub async fn spawn(initial_cwd: PathBuf) -> Result<Self, ToolError> {
        // 1. Allocate PTY
        let (pty, pts) = Self::open_pty()?;

        // 2. Build command
        let mut cmd = pty_process::Command::new("bash");
        cmd = cmd
            .arg("--norc")
            .arg("--noprofile")
            .current_dir(&initial_cwd)
            .env("TERM", "dumb")
            .env("NO_COLOR", "1")
            .env("LANG", "C.UTF-8")
            .env("HISTFILE", "/dev/null")
            .kill_on_drop(false);

        // 3. Spawn child
        let child = cmd.spawn(pts).map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to spawn bash: {e}"),
            retryable: false,
        })?;

        Self::from_child(child, pty, initial_cwd, false).await
    }

    /// Spawn a sandboxed bash process using bubblewrap (bwrap) namespace isolation.
    ///
    /// The shell runs inside a bwrap namespace with:
    /// - Workspace root: read-write
    /// - System paths (/usr, /bin, /lib, etc.): read-only
    /// - Home tool configs (.cargo, .rustup, etc.): read-only
    /// - Separate PID namespace
    /// - Ephemeral /tmp, /proc, /dev
    pub async fn spawn_sandboxed(
        initial_cwd: PathBuf,
        workspace_root: &std::path::Path,
        config: &ExecutionSandboxBlock,
    ) -> Result<Self, ToolError> {
        // 1. Verify bwrap is available
        Self::check_bwrap_available()?;

        // 2. Build bwrap argument list
        let bwrap_args = Self::build_bwrap_args(&initial_cwd, workspace_root, config)?;

        // 3. Allocate PTY
        let (pty, pts) = Self::open_pty()?;

        // 4. Build command with bwrap wrapper
        let mut cmd = pty_process::Command::new("bwrap");
        cmd = cmd
            .args(&bwrap_args)
            .current_dir(&initial_cwd)
            .env("TERM", "dumb")
            .env("NO_COLOR", "1")
            .env("LANG", "C.UTF-8")
            .env("HISTFILE", "/dev/null")
            .kill_on_drop(false);

        // 5. Spawn child (bwrap wrapping bash)
        let child = cmd.spawn(pts).map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to spawn sandboxed bash: {e}"),
            retryable: false,
        })?;

        Self::from_child(child, pty, initial_cwd, true).await
    }

    /// Allocate a PTY pair.
    fn open_pty() -> Result<(pty_process::Pty, pty_process::Pts), ToolError> {
        pty_process::open().map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to open PTY: {e}"),
            retryable: false,
        })
    }

    /// Shared post-spawn setup: extract PID, split PTY, start reader, initialize shell.
    async fn from_child(
        child: tokio::process::Child,
        pty: pty_process::Pty,
        initial_cwd: PathBuf,
        sandboxed: bool,
    ) -> Result<Self, ToolError> {
        let pid = child.id().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "Bash process has no PID".into(),
            retryable: false,
        })?;

        if sandboxed {
            tracing::info!(pid, "Spawned sandboxed bash session via bwrap");
        }

        // Split PTY into read/write halves
        let (pty_reader, pty_writer) = pty.into_split();

        // Generate session nonce
        let session_nonce = uuid::Uuid::new_v4().to_string();

        // Shared state
        let alive = Arc::new(AtomicBool::new(true));
        let output_buffer: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        let command_output_tx: Arc<Mutex<Option<tokio::sync::mpsc::Sender<String>>>> =
            Arc::new(Mutex::new(None));
        let reader_cancel = CancellationToken::new();
        let cwd = Arc::new(RwLock::new(initial_cwd));

        // Start background reader task
        {
            let alive = Arc::clone(&alive);
            let output_buffer = Arc::clone(&output_buffer);
            let command_output_tx = Arc::clone(&command_output_tx);
            let cancel = reader_cancel.clone();

            spawn_task(Self::reader_loop(
                pty_reader,
                alive,
                output_buffer,
                command_output_tx,
                cancel,
            ));
        }

        let pty_writer = Arc::new(Mutex::new(pty_writer));

        let session = Self {
            pty_writer,
            pid,
            cwd,
            output_buffer,
            command_output_tx,
            session_nonce,
            command_mutex: tokio::sync::Mutex::new(()),
            reader_cancel,
            alive,
            child: Some(Mutex::new(child)),
        };

        // Run initialization sequence
        session.initialize().await?;

        Ok(session)
    }

    /// Check that bubblewrap (bwrap) is installed and runnable.
    fn check_bwrap_available() -> Result<(), ToolError> {
        match std::process::Command::new("bwrap")
            .arg("--version")
            .output()
        {
            Ok(output) if output.status.success() => Ok(()),
            _ => Err(ToolError {
                code: ToolErrorCode::InternalError,
                message: "bubblewrap (bwrap) is not installed. Install it with: \
                         sudo apt install bubblewrap (Debian/Ubuntu) or \
                         sudo dnf install bubblewrap (Fedora). \
                         Alternatively, set sandbox level to 'unrestricted'."
                    .into(),
                retryable: false,
            }),
        }
    }

    /// Build the bwrap argument list for a sandboxed shell session.
    fn build_bwrap_args(
        initial_cwd: &std::path::Path,
        workspace_root: &std::path::Path,
        config: &ExecutionSandboxBlock,
    ) -> Result<Vec<String>, ToolError> {
        let mut args = Vec::new();

        // === System paths (read-only) ===
        for path in &["/usr", "/bin", "/lib", "/lib64", "/sbin"] {
            if std::path::Path::new(path).exists() {
                args.extend_from_slice(&["--ro-bind".into(), path.to_string(), path.to_string()]);
            }
        }

        // === Essential config (read-only) ===
        for path in &[
            "/etc/resolv.conf",
            "/etc/hosts",
            "/etc/ssl",
            "/etc/ca-certificates",
            "/etc/pki",
            "/etc/passwd",
            "/etc/group",
        ] {
            if std::path::Path::new(path).exists() {
                args.extend_from_slice(&["--ro-bind".into(), path.to_string(), path.to_string()]);
            }
        }

        // === Optional package store (read-only, if exists) ===
        if std::path::Path::new("/nix").exists() {
            args.extend_from_slice(&["--ro-bind".into(), "/nix".into(), "/nix".into()]);
        }

        // === Home directory tool configs (read-only) ===
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
        for dotdir in &[".cargo", ".rustup", ".npm", ".nvm", ".local", ".gitconfig"] {
            let full = format!("{}/{}", home, dotdir);
            if std::path::Path::new(&full).exists() {
                args.extend_from_slice(&["--ro-bind".into(), full.clone(), full]);
            }
        }

        // === Virtual filesystems ===
        // Must come BEFORE workspace/extra mounts so workspace can overlay /tmp if needed.
        args.extend_from_slice(&[
            "--tmpfs".into(),
            "/tmp".into(),
            "--proc".into(),
            "/proc".into(),
            "--dev".into(),
            "/dev".into(),
        ]);

        // === Workspace (read-write) ===
        // After virtual filesystems so a workspace under /tmp still works.
        let ws = workspace_root.to_str().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "Workspace root path is not valid UTF-8".into(),
            retryable: false,
        })?;
        args.extend_from_slice(&["--bind".into(), ws.into(), ws.into()]);

        // === Extra mounts from config ===
        for path in &config.mounts.extra_ro_paths {
            if std::path::Path::new(path).exists() {
                args.extend_from_slice(&["--ro-bind".into(), path.clone(), path.clone()]);
            }
        }
        for path in &config.mounts.extra_rw_paths {
            if std::path::Path::new(path).exists() {
                args.extend_from_slice(&["--bind".into(), path.clone(), path.clone()]);
            }
        }

        // === Isolation flags ===
        args.extend_from_slice(&["--unshare-pid".into(), "--die-with-parent".into()]);

        // === Network isolation (optional) ===
        if !config.network.allow_outbound {
            args.push("--unshare-net".into());
        }

        // === Set HOME inside sandbox ===
        args.extend_from_slice(&[
            "--setenv".into(),
            "HOME".into(),
            initial_cwd.to_str().unwrap_or("/tmp").into(),
        ]);

        // === The actual command ===
        args.extend_from_slice(&[
            "--".into(),
            "bash".into(),
            "--norc".into(),
            "--noprofile".into(),
        ]);

        Ok(args)
    }

    /// Background reader task: reads raw bytes from PTY, splits on newlines,
    /// and appends to the output buffer. Uses vt100 to strip ANSI escape
    /// sequences from each chunk.
    async fn reader_loop(
        mut pty_reader: pty_process::OwnedReadPty,
        alive: Arc<AtomicBool>,
        output_buffer: Arc<Mutex<VecDeque<String>>>,
        command_output_tx: Arc<Mutex<Option<tokio::sync::mpsc::Sender<String>>>>,
        cancel: CancellationToken,
    ) {
        let mut buf = [0u8; 4096];
        // Accumulates partial lines between reads.
        let mut partial = String::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    break;
                }
                result = pty_reader.read(&mut buf) => {
                    match result {
                        Ok(0) => {
                            // EOF
                            break;
                        }
                        Ok(n) => {
                            // Feed raw bytes to vt100 for ANSI stripping, then
                            // use screen contents to get clean text. But for
                            // line-based output, we process raw text directly
                            // (TERM=dumb means minimal escapes).
                            let raw = String::from_utf8_lossy(&buf[..n]);
                            partial.push_str(&raw);

                            // Split on newlines; the last element may be partial.
                            let mut lines: Vec<&str> = partial.split('\n').collect();
                            // Keep the last (possibly incomplete) segment.
                            let remainder = lines.pop().unwrap_or("").to_string();

                            if !lines.is_empty() {
                                let mut ob = output_buffer.lock().await;
                                let tx = command_output_tx.lock().await;

                                for line in &lines {
                                    // Strip \r from line endings
                                    let clean = line.trim_end_matches('\r').to_string();
                                    let truncated = if clean.len() > MAX_LINE_BYTES {
                                        let mut t = clean[..MAX_LINE_BYTES].to_string();
                                        t.push_str("... [truncated]");
                                        t
                                    } else {
                                        clean
                                    };
                                    ob.push_back(truncated.clone());
                                    while ob.len() > OUTPUT_BUFFER_MAX_LINES {
                                        ob.pop_front();
                                    }
                                    if let Some(ref sender) = *tx {
                                        if sender.try_send(truncated).is_err() {
                                            tracing::warn!("PTY command output channel full, dropping line — consider increasing channel capacity");
                                        }
                                    }
                                }
                            }

                            partial = remainder;
                        }
                        Err(_) => {
                            // Read error (fd closed, etc.)
                            break;
                        }
                    }
                }
            }
        }

        // Flush any remaining partial line
        if !partial.is_empty() {
            let clean = partial.trim_end_matches('\r').to_string();
            let truncated = if clean.len() > MAX_LINE_BYTES {
                let mut t = clean[..MAX_LINE_BYTES].to_string();
                t.push_str("... [truncated]");
                t
            } else {
                clean
            };
            let mut ob = output_buffer.lock().await;
            ob.push_back(truncated.clone());
            if let Some(ref sender) = *command_output_tx.lock().await {
                if sender.try_send(truncated).is_err() {
                    tracing::warn!("PTY command output channel full, dropping line — consider increasing channel capacity");
                }
            }
        }

        alive.store(false, Ordering::Release);
    }

    /// Run the initialization sequence: disable echo, set PS1/PS2, wait for ready marker.
    async fn initialize(&self) -> Result<(), ToolError> {
        // Wait for bash to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Disable echo
        self.write_raw("stty -echo\n").await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Set PS1 with nonce
        let ps1_cmd = format!(
            "PS1='___HOLT_RDY_{nonce}___:$?___\n'\n",
            nonce = self.session_nonce
        );
        self.write_raw(&ps1_cmd).await?;

        // Set PS2 with nonce
        let ps2_cmd = format!(
            "PS2='___HOLT_PS2_{nonce}___\n'\n",
            nonce = self.session_nonce
        );
        self.write_raw(&ps2_cmd).await?;

        // Wait for PS1 marker to appear
        let marker = format!("___HOLT_RDY_{}___:", self.session_nonce);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(2000);

        loop {
            if tokio::time::Instant::now() >= deadline {
                self.kill().await;
                return Err(ToolError {
                    code: ToolErrorCode::InternalError,
                    message: "PTY initialization timed out waiting for PS1 marker".into(),
                    retryable: true,
                });
            }

            {
                let ob = self.output_buffer.lock().await;
                if ob.iter().any(|line| line.contains(&marker)) {
                    break;
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Clear initialization output
        {
            let mut ob = self.output_buffer.lock().await;
            ob.clear();
        }

        Ok(())
    }

    /// Write raw bytes to the PTY.
    async fn write_raw(&self, data: &str) -> Result<(), ToolError> {
        let mut writer = self.pty_writer.lock().await;
        writer
            .write_all(data.as_bytes())
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("PTY write failed: {e}"),
                retryable: false,
            })?;
        writer.flush().await.map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("PTY flush failed: {e}"),
            retryable: false,
        })?;
        Ok(())
    }

    // ── Public API ──────────────────────────────────────────────────────

    /// Get the current working directory (from cached state).
    pub async fn cwd(&self) -> PathBuf {
        self.cwd.read().await.clone()
    }

    /// Read the actual CWD from `/proc/<pid>/cwd`.
    pub async fn read_proc_cwd(&self) -> Result<PathBuf, ToolError> {
        let link = format!("/proc/{}/cwd", self.pid);
        tokio::fs::read_link(&link).await.map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to read {link}: {e}"),
            retryable: false,
        })
    }

    /// Change the working directory of the bash process.
    ///
    /// Uses PtyCommand::execute with PS1 marker detection for reliable
    /// completion detection.
    pub async fn set_cwd(&self, path: PathBuf) -> Result<(), ToolError> {
        // No pre-check: the `cd` exit code and /proc verification catch failures,
        // avoiding both a blocking syscall and a TOCTOU race.
        let cmd = format!("cd '{}'", path.display());
        let result = crate::tools::shell::pty_command::PtyCommand::execute(
            self,
            &cmd,
            std::time::Duration::from_secs(5),
            false,
            None,
            "__internal",
        )
        .await?;

        if result.exit_code != 0 {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!(
                    "cd failed with exit code {}: {}",
                    result.exit_code, result.output
                ),
                retryable: false,
            });
        }

        // CWD is already updated by PtyCommand::execute via /proc/<pid>/cwd
        Ok(())
    }

    /// Whether the bash process is still alive.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Kill the bash process: SIGTERM, wait 100ms, SIGKILL if still alive,
    /// then reap via the tokio Child handle.
    pub async fn kill(&self) {
        // Cancel reader task first
        self.reader_cancel.cancel();

        let pid = nix::unistd::Pid::from_raw(self.pid as i32);

        // SIGTERM
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Check if still alive, SIGKILL if so
        if nix::sys::signal::kill(pid, None).is_ok() {
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
        }

        self.alive.store(false, Ordering::Release);

        // Reap zombie via the tokio Child handle
        if let Some(ref child_mutex) = self.child {
            let mut child = child_mutex.lock().await;
            let _ = child.wait().await;
        }
    }

    /// Write a string to the PTY (public).
    pub async fn write(&self, data: &str) -> Result<(), ToolError> {
        self.write_raw(data).await
    }

    /// Clone the current contents of the output buffer.
    pub async fn read_buffer(&self) -> Vec<String> {
        let ob = self.output_buffer.lock().await;
        ob.iter().cloned().collect()
    }

    /// Get the bash process PID.
    pub fn pid(&self) -> u32 {
        self.pid
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // Best-effort kill on drop
        self.reader_cancel.cancel();
        let pid = nix::unistd::Pid::from_raw(self.pid as i32);
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);

        // Reap via the Child handle (synchronous start_kill + try_wait).
        // We can't async-wait in Drop, but taking the child out ensures
        // tokio's Child destructor will attempt cleanup.
        if let Some(child_mutex) = self.child.take() {
            let mut child = child_mutex.into_inner();
            let _ = child.start_kill();
            let _ = child.try_wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ── Sandbox argument builder tests ──────────────────────────────

    #[test]
    fn test_build_bwrap_args_contains_required_flags() {
        let config = ExecutionSandboxBlock::default();
        let args =
            PtySession::build_bwrap_args("/tmp".as_ref(), "/home/user/project".as_ref(), &config)
                .unwrap();

        // Must contain workspace bind (read-write)
        let ws_idx = args
            .windows(3)
            .position(|w| w[0] == "--bind" && w[1] == "/home/user/project")
            .expect("workspace root should be bound read-write");
        assert_eq!(args[ws_idx + 2], "/home/user/project");

        // Must contain virtual filesystems
        assert!(args.contains(&"--tmpfs".to_string()));
        assert!(args.contains(&"--proc".to_string()));
        assert!(args.contains(&"--dev".to_string()));

        // Must contain isolation flags
        assert!(args.contains(&"--unshare-pid".to_string()));
        assert!(args.contains(&"--die-with-parent".to_string()));

        // Must end with -- bash --norc --noprofile
        let len = args.len();
        assert_eq!(args[len - 4], "--");
        assert_eq!(args[len - 3], "bash");
        assert_eq!(args[len - 2], "--norc");
        assert_eq!(args[len - 1], "--noprofile");
    }

    #[test]
    fn test_build_bwrap_args_system_paths_readonly() {
        let config = ExecutionSandboxBlock::default();
        let args =
            PtySession::build_bwrap_args("/tmp".as_ref(), "/home/user/project".as_ref(), &config)
                .unwrap();

        // System paths should be --ro-bind, not --bind
        for path in &["/usr", "/bin", "/lib"] {
            if std::path::Path::new(path).exists() {
                let idx = args
                    .windows(3)
                    .position(|w| w[0] == "--ro-bind" && w[1] == *path)
                    .unwrap_or_else(|| panic!("{path} should be mounted read-only"));
                assert_eq!(args[idx + 2], *path);
            }
        }
    }

    #[test]
    fn test_build_bwrap_args_extra_mounts() {
        let config = ExecutionSandboxBlock {
            mounts: crate::config::agent_config::SandboxMountsBlock {
                extra_ro_paths: vec!["/tmp".to_string()], // /tmp exists
                extra_rw_paths: vec!["/tmp".to_string()],
            },
            ..Default::default()
        };
        let args =
            PtySession::build_bwrap_args("/tmp".as_ref(), "/home/user/project".as_ref(), &config)
                .unwrap();

        // Extra paths should appear (at least once)
        assert!(
            args.windows(3)
                .any(|w| w[0] == "--ro-bind" && w[1] == "/tmp" && w[2] == "/tmp"),
            "extra_ro_paths should be mounted"
        );
    }

    #[test]
    fn test_build_bwrap_args_network_isolation() {
        let mut config = ExecutionSandboxBlock::default();
        config.network.allow_outbound = false;
        let args = PtySession::build_bwrap_args("/tmp".as_ref(), "/tmp".as_ref(), &config).unwrap();
        assert!(
            args.contains(&"--unshare-net".to_string()),
            "should include --unshare-net when outbound is disabled"
        );

        // Default (allow_outbound = true) should NOT have --unshare-net
        let config2 = ExecutionSandboxBlock::default();
        let args2 =
            PtySession::build_bwrap_args("/tmp".as_ref(), "/tmp".as_ref(), &config2).unwrap();
        assert!(
            !args2.contains(&"--unshare-net".to_string()),
            "should NOT include --unshare-net when outbound is allowed"
        );
    }

    #[test]
    fn test_build_bwrap_args_non_utf8_workspace_errors() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let bad_path = std::path::PathBuf::from(OsStr::from_bytes(&[0xFF, 0xFE]));
        let config = ExecutionSandboxBlock::default();
        let result = PtySession::build_bwrap_args("/tmp".as_ref(), &bad_path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_check_bwrap_available_on_this_system() {
        // This test passes if bwrap is installed, errors with a helpful message if not.
        let result = PtySession::check_bwrap_available();
        if result.is_err() {
            let err = result.unwrap_err();
            assert!(err.message.contains("bubblewrap"));
            assert!(err.message.contains("apt install"));
        }
    }

    // ── Live sandboxed spawn tests ──────────────────────────────────

    #[tokio::test]
    async fn test_spawn_sandboxed_basic() {
        if PtySession::check_bwrap_available().is_err() {
            eprintln!("bwrap not available, skipping test");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let config = ExecutionSandboxBlock::default();
        let session = PtySession::spawn_sandboxed(dir.path().to_path_buf(), dir.path(), &config)
            .await
            .unwrap();
        assert!(session.is_alive());

        // Run a command inside the sandbox
        let result = crate::tools::shell::pty_command::PtyCommand::execute(
            &session,
            "echo sandbox_works",
            Duration::from_secs(5),
            false,
            None,
            "__test",
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(
            result.output.contains("sandbox_works"),
            "expected 'sandbox_works' in output, got: {}",
            result.output
        );
        session.kill().await;
    }

    #[tokio::test]
    async fn test_sandboxed_workspace_write() {
        if PtySession::check_bwrap_available().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let config = ExecutionSandboxBlock::default();
        let session = PtySession::spawn_sandboxed(dir.path().to_path_buf(), dir.path(), &config)
            .await
            .unwrap();

        let test_file = dir.path().join("sandbox_test.txt");
        let cmd = format!("echo hello > '{}'", test_file.display());
        let result = crate::tools::shell::pty_command::PtyCommand::execute(
            &session,
            &cmd,
            Duration::from_secs(5),
            false,
            None,
            "__test",
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0, "write to workspace should succeed");
        assert!(test_file.exists(), "file should exist in workspace");
        session.kill().await;
    }

    #[tokio::test]
    async fn test_sandboxed_outside_workspace_blocked() {
        if PtySession::check_bwrap_available().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let config = ExecutionSandboxBlock::default();
        let session = PtySession::spawn_sandboxed(dir.path().to_path_buf(), dir.path(), &config)
            .await
            .unwrap();

        // Attempt to write outside workspace. Use touch on a path that doesn't
        // exist in the sandbox namespace, and verify via the HOST filesystem.
        let outside_path = format!("/tmp/sandbox_escape_test_{}", std::process::id());
        let cmd = format!("touch '{}'", outside_path);
        let _result = crate::tools::shell::pty_command::PtyCommand::execute(
            &session,
            &cmd,
            Duration::from_secs(5),
            false,
            None,
            "__test",
        )
        .await
        .unwrap();
        // Even if exit code is 0 inside bwrap's tmpfs, the file should NOT exist
        // on the HOST filesystem (bwrap's /tmp is an isolated tmpfs).
        assert!(
            !std::path::Path::new(&outside_path).exists(),
            "file written inside sandbox /tmp must NOT appear on host filesystem"
        );
        // Cleanup just in case
        let _ = std::fs::remove_file(&outside_path);
        session.kill().await;
    }

    #[tokio::test]
    async fn test_sandboxed_system_paths_readable() {
        if PtySession::check_bwrap_available().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let config = ExecutionSandboxBlock::default();
        let session = PtySession::spawn_sandboxed(dir.path().to_path_buf(), dir.path(), &config)
            .await
            .unwrap();

        // Verify system tools are accessible
        let result = crate::tools::shell::pty_command::PtyCommand::execute(
            &session,
            "which bash",
            Duration::from_secs(5),
            false,
            None,
            "__test",
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0, "should find bash in system paths");
        session.kill().await;
    }

    #[tokio::test]
    async fn test_sandboxed_ps1_markers_work() {
        if PtySession::check_bwrap_available().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let config = ExecutionSandboxBlock::default();
        let session = PtySession::spawn_sandboxed(dir.path().to_path_buf(), dir.path(), &config)
            .await
            .unwrap();

        // If we got here, PS1 markers worked during initialization.
        // Run a command to double-confirm marker detection.
        let result = crate::tools::shell::pty_command::PtyCommand::execute(
            &session,
            "echo marker_test && true",
            Duration::from_secs(5),
            false,
            None,
            "__test",
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.output.contains("marker_test"));
        session.kill().await;
    }

    // ── Existing PtySession tests ──────────────────────────────────

    #[tokio::test]
    async fn test_pty_session_spawn() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        assert!(session.is_alive());
        assert_eq!(session.cwd().await, std::path::PathBuf::from("/tmp"));
    }

    #[tokio::test]
    async fn test_pty_session_write_and_read() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        session.write("echo hello_pty_test\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let buf = session.read_buffer().await;
        assert!(
            buf.iter().any(|line| line.contains("hello_pty_test")),
            "Expected 'hello_pty_test' in output, got: {:?}",
            buf
        );
    }

    #[tokio::test]
    async fn test_pty_session_set_cwd() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        session.set_cwd("/home".into()).await.unwrap();
        let cwd = session.cwd().await;
        let expected = tokio::fs::canonicalize("/home")
            .await
            .unwrap_or("/home".into());
        let actual = tokio::fs::canonicalize(&cwd).await.unwrap_or(cwd.clone());
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_pty_session_set_cwd_nonexistent() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let result = session.set_cwd("/nonexistent_path_12345".into()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_pty_session_kill() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        assert!(session.is_alive());
        session.kill().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!session.is_alive());
    }

    #[tokio::test]
    async fn test_pty_session_cwd_via_proc() {
        let session = PtySession::spawn("/tmp".into()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        session.write("cd /var\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let cwd = session.read_proc_cwd().await.unwrap();
        let expected = tokio::fs::canonicalize("/var")
            .await
            .unwrap_or("/var".into());
        let actual = tokio::fs::canonicalize(&cwd).await.unwrap_or(cwd.clone());
        assert_eq!(actual, expected);
    }
}
