pub mod bash;
pub mod process;
pub mod pty_command;
pub mod pty_session;
pub mod shell_config;

use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::agent_config::ExecutionSandboxBlock;
use crate::tools::types::ToolError;
use process::BackgroundProcess;
use pty_session::PtySession;

pub struct ShellManager {
    pub sessions: HashMap<String, PtySession>,
    pub processes: HashMap<String, BackgroundProcess>,
    pub hil_senders: HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>,
    /// Per-agent sandbox config + workspace root, stored at spawn time for respawns.
    pub sandbox_configs: HashMap<String, (ExecutionSandboxBlock, PathBuf)>,
}

impl ShellManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            processes: HashMap::new(),
            hil_senders: HashMap::new(),
            sandbox_configs: HashMap::new(),
        }
    }

    /// Spawn a new PTY session for an agent with sandbox configuration.
    ///
    /// The sandbox config is stored so that broken-pipe respawns preserve
    /// the same isolation level.
    pub async fn spawn_session(
        &mut self,
        agent_id: String,
        working_directory: PathBuf,
        sandbox_config: &ExecutionSandboxBlock,
        workspace_root: &std::path::Path,
    ) -> Result<(), ToolError> {
        // Don't spawn a session for restricted agents — they have no shell access.
        if sandbox_config.is_restricted() {
            tracing::info!(agent_id, "Skipping PTY spawn for restricted agent");
            return Ok(());
        }

        let session = if sandbox_config.is_sandboxed() {
            PtySession::spawn_sandboxed(working_directory.clone(), workspace_root, sandbox_config)
                .await?
        } else {
            PtySession::spawn(working_directory.clone()).await?
        };

        // Store config for respawns
        self.sandbox_configs.insert(
            agent_id.clone(),
            (sandbox_config.clone(), workspace_root.to_path_buf()),
        );
        self.sessions.insert(agent_id, session);
        Ok(())
    }

    /// Respawn a PTY session using the previously stored sandbox config.
    ///
    /// Used by bash.rs for on-demand spawns and broken-pipe recovery.
    /// Falls back to unrestricted if no stored config exists (legacy agents).
    pub async fn respawn_session(
        &mut self,
        agent_id: String,
        working_directory: PathBuf,
    ) -> Result<(), ToolError> {
        let (config, workspace_root) =
            self.sandbox_configs
                .get(&agent_id)
                .cloned()
                .unwrap_or_else(|| {
                    tracing::warn!(
                        agent_id,
                        "No stored sandbox config for respawn, defaulting to unrestricted"
                    );
                    (
                        ExecutionSandboxBlock::legacy_default(),
                        working_directory.clone(),
                    )
                });

        self.spawn_session(agent_id, working_directory, &config, &workspace_root)
            .await
    }

    /// Destroy an agent's PTY session.
    pub async fn destroy_session(&mut self, agent_id: &str) {
        if let Some(session) = self.sessions.remove(agent_id) {
            session.kill().await;
        }
    }

    /// Kill all background processes belonging to an agent and remove them from tracking.
    pub async fn kill_processes_for_agent(&mut self, agent_id: &str) {
        let owned_ids: Vec<String> = self
            .processes
            .iter()
            .filter(|(_, p)| p.agent_id == agent_id)
            .map(|(id, _)| id.clone())
            .collect();

        for id in &owned_ids {
            if let Some(process) = self.processes.remove(id) {
                let mut running = process.running.lock().await;
                *running = false;
                tracing::debug!(agent_id, process_id = %id, "Killed orphaned background process on agent deletion");
            }
        }
    }

    /// Kill all sessions and processes (for app shutdown).
    pub async fn shutdown_all(&mut self) {
        for (_, session) in self.sessions.drain() {
            session.kill().await;
        }
        self.processes.clear();
    }
}

impl Default for ShellManager {
    fn default() -> Self {
        Self::new()
    }
}
