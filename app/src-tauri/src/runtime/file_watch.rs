use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::Emitter;
use tokio::sync::{mpsc, RwLock};

use crate::runtime::triggers::{JobType, TriggerRegistry, WatchEventType};
use crate::AppState;

const DEBOUNCE_MS: u64 = 2000;
const COOLDOWN_MS: u64 = 10000;
const SELF_WRITE_BUFFER_TTL_MS: u64 = 2000;

/// Auto-excluded directory names
const EXCLUDED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "__pycache__",
    ".svelte-kit",
    "build",
    "dist",
];

/// Auto-excluded file extensions
const EXCLUDED_EXTENSIONS: &[&str] = &["pyc", "o", "swp", "swo"];

/// Auto-excluded file suffixes
const EXCLUDED_SUFFIXES: &[&str] = &["~"];

pub struct FileWatchManager {
    /// Self-change suppression buffer: (agent_id, path, timestamp)
    self_writes: Arc<RwLock<Vec<(String, PathBuf, Instant)>>>,
    /// Live watcher handle for runtime registration
    watcher: Arc<tokio::sync::Mutex<Option<RecommendedWatcher>>>,
}

impl FileWatchManager {
    pub fn new() -> Self {
        Self {
            self_writes: Arc::new(RwLock::new(Vec::new())),
            watcher: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Register a new path with the live watcher. Called by watch_path tool.
    pub async fn register_watch(&self, path: &Path, recursive: bool) {
        if !path.exists() {
            tracing::debug!("Skipping watch for non-existent path: {}", path.display());
            return;
        }
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        let mut watcher_guard = self.watcher.lock().await;
        if let Some(ref mut w) = *watcher_guard {
            if let Err(e) = w.watch(path, mode) {
                tracing::warn!("Failed to register watch for {}: {}", path.display(), e);
            } else {
                tracing::debug!("Registered watch: {}", path.display());
            }
        }
    }

    /// Unregister a path from the live watcher. Called by unwatch_path tool.
    pub async fn unregister_watch(&self, path: &Path) {
        let mut watcher_guard = self.watcher.lock().await;
        if let Some(ref mut w) = *watcher_guard {
            let _ = w.unwatch(path);
            tracing::debug!("Unregistered watch: {}", path.display());
        }
    }

    /// Record that an agent wrote a file (for self-change suppression).
    pub async fn record_self_write(&self, agent_id: &str, path: &Path) {
        let mut writes = self.self_writes.write().await;
        writes.push((agent_id.to_string(), path.to_path_buf(), Instant::now()));
        // Prune old entries
        let cutoff = Instant::now() - Duration::from_millis(SELF_WRITE_BUFFER_TTL_MS);
        writes.retain(|(_, _, ts)| *ts > cutoff);
    }

    /// Check if an agent caused a change (suppress echo notification).
    async fn is_self_change(&self, agent_id: &str, path: &Path) -> bool {
        let writes = self.self_writes.read().await;
        let cutoff = Instant::now() - Duration::from_millis(SELF_WRITE_BUFFER_TTL_MS);
        writes
            .iter()
            .any(|(aid, p, ts)| aid == agent_id && p == path && *ts > cutoff)
    }

    /// Check if a path should be excluded.
    fn is_excluded(path: &Path) -> bool {
        for component in path.components() {
            let name = component.as_os_str().to_str().unwrap_or("");
            if EXCLUDED_DIRS.contains(&name) {
                return true;
            }
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if EXCLUDED_EXTENSIONS.contains(&ext) {
                return true;
            }
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            for suffix in EXCLUDED_SUFFIXES {
                if name.ends_with(suffix) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if a path matches a glob filter.
    fn matches_glob(path: &Path, glob_filter: &Option<String>) -> bool {
        match glob_filter {
            None => true,
            Some(pattern) => {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    glob_match::glob_match(pattern, name)
                } else {
                    false
                }
            }
        }
    }

    /// Map notify EventKind to our WatchEventType.
    fn map_event_kind(kind: &EventKind) -> Option<WatchEventType> {
        match kind {
            EventKind::Create(_) => Some(WatchEventType::Create),
            // Modify includes renames (ModifyKind::Name) in notify v6 —
            // catches vim/editor temp+rename save pattern
            EventKind::Modify(_) => Some(WatchEventType::Modify),
            EventKind::Remove(_) => Some(WatchEventType::Delete),
            EventKind::Access(_) | EventKind::Other => None,
            // Any = catch-all, treat as modify to be safe
            EventKind::Any => Some(WatchEventType::Modify),
        }
    }

    /// Spawn the file watch event loop.
    pub fn spawn(
        self: &Arc<Self>,
        trigger_registry: Arc<TriggerRegistry>,
        state: Arc<AppState>,
        app_handle: tauri::AppHandle,
    ) {
        let manager = Arc::clone(self);
        let registry = Arc::clone(&trigger_registry);

        tauri::async_runtime::spawn(async move {
            let (tx, mut rx) = mpsc::channel::<notify::Result<notify::Event>>(256);

            let tx_clone = tx.clone();
            let watcher = match RecommendedWatcher::new(
                move |res| {
                    if let Err(e) = tx_clone.try_send(res) {
                        tracing::warn!("File watch event dropped (channel full or closed): {e}");
                    }
                },
                notify::Config::default().with_poll_interval(Duration::from_secs(2)),
            ) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!("Failed to create file watcher: {}", e);
                    return;
                }
            };

            // Store watcher in shared handle for runtime registration
            {
                let mut watcher_guard = manager.watcher.lock().await;
                *watcher_guard = Some(watcher);
            }

            // Register existing file watch paths
            let jobs = registry.list().await;
            let mut watched_count = 0u32;
            for job in &jobs {
                if job.job_type == JobType::FileWatch && job.enabled {
                    if let Some(ref config) = job.watch_config {
                        manager.register_watch(&config.path, config.recursive).await;
                        watched_count += 1;
                    }
                }
            }

            tracing::info!(
                "FileWatchManager started with {} active watches",
                watched_count
            );

            let mut last_seen: HashMap<PathBuf, Instant> = HashMap::new();
            let mut cooldowns: HashMap<PathBuf, Instant> = HashMap::new();
            let mut consecutive_errors = 0u32;

            while let Some(event_result) = rx.recv().await {
                let event = match event_result {
                    Ok(e) => {
                        consecutive_errors = 0;
                        e
                    }
                    Err(e) => {
                        consecutive_errors += 1;
                        tracing::warn!("File watch error: {}", e);
                        if consecutive_errors > 10 {
                            tracing::error!(
                                "FileWatchManager: >10 consecutive errors, waiting for recovery"
                            );
                        }
                        continue;
                    }
                };

                let watch_event = match Self::map_event_kind(&event.kind) {
                    Some(e) => e,
                    None => continue,
                };

                for path in &event.paths {
                    if Self::is_excluded(path) {
                        continue;
                    }

                    let now = Instant::now();

                    // Debounce
                    if let Some(last) = last_seen.get(path) {
                        if now.duration_since(*last) < Duration::from_millis(DEBOUNCE_MS) {
                            continue;
                        }
                    }
                    last_seen.insert(path.clone(), now);

                    // Cooldown
                    if let Some(last_notify) = cooldowns.get(path) {
                        if now.duration_since(*last_notify) < Duration::from_millis(COOLDOWN_MS) {
                            continue;
                        }
                    }

                    // Find matching FileWatch jobs
                    let jobs = registry.list().await;
                    for job in &jobs {
                        if job.job_type != JobType::FileWatch || !job.enabled {
                            continue;
                        }
                        let config = match &job.watch_config {
                            Some(c) => c,
                            None => continue,
                        };

                        if !path.starts_with(&config.path) {
                            continue;
                        }

                        if !config.events.contains(&watch_event) {
                            continue;
                        }

                        if !Self::matches_glob(path, &config.glob_filter) {
                            continue;
                        }

                        if manager.is_self_change(&job.agent_id, path).await {
                            continue;
                        }

                        // Check agent is idle
                        let is_idle = {
                            let agents = state.agents.read().await;
                            agents
                                .iter()
                                .find(|a| a.id == job.agent_id)
                                .map(|a| {
                                    matches!(a.status, crate::runtime::agent::AgentStatus::Idle)
                                })
                                .unwrap_or(false)
                        };

                        if !is_idle {
                            registry.record_skip(&job.id).await;
                            let _ = app_handle.emit(
                                "trigger-skipped",
                                serde_json::json!({
                                    "job_id": job.id,
                                    "agent_id": job.agent_id,
                                    "job_name": job.name,
                                    "reason": "agent_busy",
                                }),
                            );
                            continue;
                        }

                        let event_label = match watch_event {
                            WatchEventType::Create => "created",
                            WatchEventType::Modify => "modified",
                            WatchEventType::Delete => "deleted",
                        };
                        let message = format!(
                            "[File Watch: {}] {} was {}",
                            job.name,
                            path.display(),
                            event_label
                        );

                        let state_clone = state.clone();
                        let app_clone = app_handle.clone();
                        let agent_id = job.agent_id.clone();
                        let job_name = job.name.clone();

                        tauri::async_runtime::spawn(async move {
                            // Ensure TurnManager worker exists
                            crate::commands::agent_commands::messaging::register_turn_worker(
                                &state_clone,
                                &app_clone,
                                &agent_id,
                            )
                            .await;

                            let protocol = {
                                let agents = state_clone.agents.read().await;
                                agents
                                    .iter()
                                    .find(|a| a.id == agent_id)
                                    .map(|a| a.protocol.clone())
                                    .unwrap_or_default()
                            };
                            let turn_protocol =
                                crate::runtime::turn_manager::TurnProtocol::from_message(
                                    &protocol, message, None, None,
                                );
                            let request = crate::runtime::turn_manager::TurnRequest::new(
                                turn_protocol,
                                crate::runtime::turn_manager::TurnSource::FileWatch { job_name },
                                crate::runtime::turn_manager::TurnPriority::Background,
                            );
                            if let Some(handle) = state_clone
                                .turn_manager
                                .submit_turn(&agent_id, request)
                                .await
                            {
                                if let crate::runtime::turn_manager::TurnResult::Error(e) =
                                    handle.result().await
                                {
                                    tracing::warn!(
                                        "File watch fire failed for agent {}: {}",
                                        agent_id,
                                        e
                                    );
                                }
                            }
                        });

                        registry.record_fire(&job.id).await;

                        let _ = app_handle.emit(
                            "trigger-fired",
                            serde_json::json!({
                                "job_id": job.id,
                                "agent_id": job.agent_id,
                                "job_name": job.name,
                            }),
                        );

                        cooldowns.insert(path.clone(), now);
                    }
                }

                // Prune old debounce/cooldown entries after every batch
                if last_seen.len() > 200 || cooldowns.len() > 200 {
                    let cutoff = Instant::now() - Duration::from_secs(30);
                    last_seen.retain(|_, v| *v > cutoff);
                    cooldowns.retain(|_, v| *v > cutoff);
                }
            }
        });
    }
}
