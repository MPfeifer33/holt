//! Individual terminal session — wraps a PTY master + child process.

use portable_pty::{CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Info returned to frontend about a terminal session.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TerminalInfo {
    pub id: String,
    pub shell: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub alive: bool,
}

/// A running terminal session.
///
/// All PTY resources are behind Mutex/Arc for Sync safety since portable-pty
/// types are Send but not Sync.
pub struct TerminalSession {
    pub id: String,
    pub shell: String,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    pub created_at: Instant,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Master held behind std Mutex for resize (blocking but fast).
    master: Arc<std::sync::Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    reader_handle: Option<JoinHandle<()>>,
    child: Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    alive: Arc<std::sync::atomic::AtomicBool>,
}

// Safety: All non-Sync fields are behind Mutex.
unsafe impl Sync for TerminalSession {}

impl TerminalSession {
    /// Spawn a new terminal session.
    pub fn spawn(
        id: String,
        shell: Option<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        app_handle: AppHandle,
    ) -> Result<Self, String> {
        let shell_path = shell
            .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()));

        let working_dir = cwd
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));

        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Failed to open PTY: {e}"))?;

        // If the shell argument contains spaces it's a compound command line
        // (e.g. "claude --mcp-config /path --strict-mcp-config ..."), not a
        // bare binary path. Dispatch through /bin/sh -c so the args parse.
        let mut cmd = if shell_path.contains(' ') {
            let mut c = CommandBuilder::new("/bin/sh");
            c.arg("-c");
            c.arg(&shell_path);
            c
        } else {
            CommandBuilder::new(&shell_path)
        };
        cmd.cwd(&working_dir);
        // Inherit environment
        for (key, value) in std::env::vars() {
            cmd.env(key, value);
        }
        // Set TERM for proper escape sequence support
        cmd.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Failed to spawn shell: {e}"))?;

        // Drop the slave — we only need the master side
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("Failed to clone PTY reader: {e}"))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("Failed to take PTY writer: {e}"))?;

        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let alive_clone = alive.clone();
        let session_id = id.clone();

        // Background thread: read from PTY and emit events to frontend.
        // PTY I/O is blocking so we use a real thread, wrapped in a tokio JoinHandle
        // for cancellation.
        let reader_thread = std::thread::spawn(move || {
            Self::reader_loop(reader, session_id, app_handle, alive_clone);
        });

        let join_handle = tokio::spawn(async move {
            let _ = tokio::task::spawn_blocking(move || reader_thread.join()).await;
        });

        Ok(Self {
            id,
            shell: shell_path,
            cwd: working_dir,
            cols,
            rows,
            created_at: Instant::now(),
            writer: Arc::new(Mutex::new(writer)),
            master: Arc::new(std::sync::Mutex::new(pair.master)),
            reader_handle: Some(join_handle),
            child: Arc::new(std::sync::Mutex::new(child)),
            alive,
        })
    }

    /// Read loop — runs on a dedicated thread (blocking I/O).
    fn reader_loop(
        mut reader: Box<dyn Read + Send>,
        session_id: String,
        app_handle: AppHandle,
        alive: Arc<std::sync::atomic::AtomicBool>,
    ) {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    alive.store(false, std::sync::atomic::Ordering::Relaxed);
                    let _ = app_handle.emit(
                        "terminal-exit",
                        serde_json::json!({
                            "session_id": session_id,
                        }),
                    );
                    break;
                }
                Ok(n) => {
                    let data = buf[..n].to_vec();
                    let _ = app_handle.emit(
                        "terminal-output",
                        serde_json::json!({
                            "session_id": session_id,
                            "data": data,
                        }),
                    );
                }
                Err(e) => {
                    tracing::debug!(session_id, "PTY read error: {e}");
                    alive.store(false, std::sync::atomic::Ordering::Relaxed);
                    let _ = app_handle.emit(
                        "terminal-exit",
                        serde_json::json!({
                            "session_id": session_id,
                        }),
                    );
                    break;
                }
            }
        }
    }

    /// Write data to the PTY (user input).
    pub async fn write(&self, data: &[u8]) -> Result<(), String> {
        let mut writer = self.writer.lock().await;
        writer
            .write_all(data)
            .map_err(|e| format!("Write failed: {e}"))?;
        writer.flush().map_err(|e| format!("Flush failed: {e}"))
    }

    /// Resize the PTY.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        let master = self
            .master
            .lock()
            .map_err(|e| format!("Lock poisoned: {e}"))?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Resize failed: {e}"))
    }

    /// Check if the shell process is still alive.
    pub fn is_alive(&self) -> bool {
        self.alive.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Kill the child process and clean up.
    pub fn kill(&mut self) {
        self.alive
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
    }

    /// Get session info for frontend.
    pub fn info(&self) -> TerminalInfo {
        TerminalInfo {
            id: self.id.clone(),
            shell: self.shell.clone(),
            cwd: self.cwd.to_string_lossy().to_string(),
            cols: self.cols,
            rows: self.rows,
            alive: self.is_alive(),
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.kill();
    }
}
