//! Terminal session manager — multi-session registry with lifecycle management.

use std::collections::HashMap;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::RwLock;

use super::session::{TerminalInfo, TerminalSession};

/// Manages all active terminal sessions.
pub struct TerminalManager {
    sessions: Arc<RwLock<HashMap<String, TerminalSession>>>,
    max_sessions: usize,
}

impl TerminalManager {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            max_sessions,
        }
    }

    /// Create a new terminal session. Returns session info on success.
    pub async fn create(
        &self,
        shell: Option<String>,
        cwd: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
        app_handle: AppHandle,
    ) -> Result<TerminalInfo, String> {
        let sessions = self.sessions.read().await;
        if sessions.len() >= self.max_sessions {
            return Err(format!(
                "Maximum terminal sessions ({}) reached",
                self.max_sessions
            ));
        }
        drop(sessions);

        let id = uuid::Uuid::new_v4().to_string();
        let cols = cols.unwrap_or(80);
        let rows = rows.unwrap_or(24);

        let session = TerminalSession::spawn(id.clone(), shell, cwd, cols, rows, app_handle)?;
        let info = session.info();

        self.sessions.write().await.insert(id, session);

        Ok(info)
    }

    /// Destroy a terminal session by ID.
    pub async fn destroy(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.write().await;
        match sessions.remove(session_id) {
            Some(mut session) => {
                session.kill();
                Ok(())
            }
            None => Err(format!("Session not found: {session_id}")),
        }
    }

    /// Write data to a session's PTY.
    pub async fn write(&self, session_id: &str, data: &[u8]) -> Result<(), String> {
        let sessions = self.sessions.read().await;
        match sessions.get(session_id) {
            Some(session) => session.write(data).await,
            None => Err(format!("Session not found: {session_id}")),
        }
    }

    /// Resize a session's PTY.
    pub async fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let sessions = self.sessions.read().await;
        match sessions.get(session_id) {
            Some(session) => session.resize(cols, rows),
            None => Err(format!("Session not found: {session_id}")),
        }
    }

    /// List all active sessions.
    pub async fn list(&self) -> Vec<TerminalInfo> {
        let sessions = self.sessions.read().await;
        sessions.values().map(|s| s.info()).collect()
    }

    /// Kill all sessions (app shutdown).
    pub async fn shutdown(&self) {
        let mut sessions = self.sessions.write().await;
        for (_, mut session) in sessions.drain() {
            session.kill();
        }
    }
}
