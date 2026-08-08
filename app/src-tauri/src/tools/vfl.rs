use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, Notify};
use tokio::time::{timeout, Duration};

use crate::config::app_config::FileLockingBlock;

use super::types::ToolError;
use super::types::ToolErrorCode;

/// Lock type for VFL operations.
#[derive(Debug, Clone, PartialEq)]
pub enum LockType {
    /// Multiple readers allowed simultaneously.
    Read,
    /// Exclusive — one agent only.
    Write,
}

/// An active lock on a file or directory path.
#[derive(Debug)]
struct ActiveLock {
    lock_type: LockType,
    /// The agent holding a write lock, or empty for shared reads.
    write_holder: Option<String>,
    /// Set of agents holding read locks.
    readers: HashSet<String>,
    /// When the lock was first acquired.
    acquired_at: Instant,
}

/// A queued request waiting for a lock.
#[derive(Debug)]
#[allow(dead_code)]
struct QueuedRequest {
    agent_id: String,
    lock_type: LockType,
    notify: Arc<Notify>,
    queued_at: Instant,
}

/// Virtual File Lock registry — queue-based file access control (PRD Section 7.3).
///
/// Agents never receive lock errors, they just wait their turn. Write locks are
/// exclusive, read locks are shared. Queued requests are processed FIFO.
pub struct VirtualFileLockRegistry {
    locks: Mutex<HashMap<PathBuf, ActiveLock>>,
    queues: Mutex<HashMap<PathBuf, VecDeque<QueuedRequest>>>,
    config: FileLockingBlock,
}

impl VirtualFileLockRegistry {
    pub fn new(config: FileLockingBlock) -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
            queues: Mutex::new(HashMap::new()),
            config,
        }
    }

    /// Acquire a lock on a path. Blocks silently if the lock is held incompatibly.
    /// Returns a LockGuard that auto-releases on drop.
    ///
    /// If VFL is disabled in config, returns a no-op guard immediately.
    pub async fn acquire(
        self: &Arc<Self>,
        path: &Path,
        lock_type: LockType,
        agent_id: &str,
    ) -> Result<LockGuard, ToolError> {
        if !self.config.enabled {
            return Ok(LockGuard {
                registry: Arc::clone(self),
                path: path.to_path_buf(),
                agent_id: agent_id.to_string(),
                lock_type: lock_type.clone(),
                noop: true,
            });
        }

        let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

        // Try to acquire immediately
        if self
            .try_acquire_lock(&canonical, &lock_type, agent_id)
            .await
        {
            return Ok(LockGuard {
                registry: Arc::clone(self),
                path: canonical,
                agent_id: agent_id.to_string(),
                lock_type,
                noop: false,
            });
        }

        // Queue and wait
        let notify = Arc::new(Notify::new());
        {
            let mut queues = self.queues.lock().await;
            queues
                .entry(canonical.clone())
                .or_insert_with(VecDeque::new)
                .push_back(QueuedRequest {
                    agent_id: agent_id.to_string(),
                    lock_type: lock_type.clone(),
                    notify: Arc::clone(&notify),
                    queued_at: Instant::now(),
                });
        }

        // Wait with timeout
        let queue_timeout = Duration::from_secs(self.config.queue_timeout_seconds as u64);
        match timeout(queue_timeout, async {
            loop {
                notify.notified().await;
                if self
                    .try_acquire_lock(&canonical, &lock_type, agent_id)
                    .await
                {
                    return;
                }
                // Spurious wake — re-wait
            }
        })
        .await
        {
            Ok(()) => Ok(LockGuard {
                registry: Arc::clone(self),
                path: canonical,
                agent_id: agent_id.to_string(),
                lock_type,
                noop: false,
            }),
            Err(_) => {
                // Timeout — remove from queue
                self.remove_from_queue(&canonical, agent_id).await;
                Err(ToolError {
                    code: ToolErrorCode::VflTimeout,
                    message: format!("Timed out waiting for file lock on '{}'", path.display()),
                    retryable: true,
                })
            }
        }
    }

    /// Try to acquire a lock without blocking. Returns true if successful.
    async fn try_acquire_lock(&self, path: &Path, lock_type: &LockType, agent_id: &str) -> bool {
        let mut locks = self.locks.lock().await;

        match locks.get_mut(path) {
            None => {
                // No lock exists — acquire
                let lock = match lock_type {
                    LockType::Read => {
                        let mut readers = HashSet::new();
                        readers.insert(agent_id.to_string());
                        ActiveLock {
                            lock_type: LockType::Read,
                            write_holder: None,
                            readers,
                            acquired_at: Instant::now(),
                        }
                    }
                    LockType::Write => ActiveLock {
                        lock_type: LockType::Write,
                        write_holder: Some(agent_id.to_string()),
                        readers: HashSet::new(),
                        acquired_at: Instant::now(),
                    },
                };
                locks.insert(path.to_path_buf(), lock);
                true
            }
            Some(existing) => {
                match (&existing.lock_type, lock_type) {
                    // Read + Read = compatible (shared)
                    (LockType::Read, LockType::Read) => {
                        existing.readers.insert(agent_id.to_string());
                        true
                    }
                    // Any other combination = incompatible
                    _ => false,
                }
            }
        }
    }

    /// Release a lock held by an agent on a specific path.
    pub async fn release(&self, path: &Path, agent_id: &str) {
        let should_notify = {
            let mut locks = self.locks.lock().await;

            if let Some(lock) = locks.get_mut(path) {
                match &lock.lock_type {
                    LockType::Write => {
                        if lock.write_holder.as_deref() == Some(agent_id) {
                            locks.remove(path);
                            true
                        } else {
                            false
                        }
                    }
                    LockType::Read => {
                        lock.readers.remove(agent_id);
                        if lock.readers.is_empty() {
                            locks.remove(path);
                            true
                        } else {
                            false
                        }
                    }
                }
            } else {
                false
            }
        };

        if should_notify {
            self.notify_next_in_queue(path).await;
        }
    }

    /// Release all locks held by an agent (used during interrupt cleanup).
    pub async fn release_all_for_agent(&self, agent_id: &str) {
        let paths_to_notify: Vec<PathBuf> = {
            let mut locks = self.locks.lock().await;
            let mut paths = Vec::new();

            // Collect paths that need cleanup
            let keys: Vec<PathBuf> = locks.keys().cloned().collect();
            for path in keys {
                if let Some(lock) = locks.get_mut(&path) {
                    let should_remove = match &lock.lock_type {
                        LockType::Write => lock.write_holder.as_deref() == Some(agent_id),
                        LockType::Read => {
                            lock.readers.remove(agent_id);
                            lock.readers.is_empty()
                        }
                    };
                    if should_remove {
                        locks.remove(&path);
                        paths.push(path);
                    }
                }
            }

            paths
        };

        // Also remove from all queues
        {
            let mut queues = self.queues.lock().await;
            for queue in queues.values_mut() {
                queue.retain(|r| r.agent_id != agent_id);
            }
        }

        // Notify next in queue for each released path
        for path in paths_to_notify {
            self.notify_next_in_queue(&path).await;
        }
    }

    /// Notify the next request in the queue for a path.
    async fn notify_next_in_queue(&self, path: &Path) {
        let mut queues = self.queues.lock().await;
        if let Some(queue) = queues.get_mut(path) {
            // Notify all waiting — they'll re-check if they can acquire.
            // Only the first compatible one will succeed; others re-wait.
            for req in queue.iter() {
                req.notify.notify_one();
            }
        }
    }

    /// Remove a specific agent's request from the queue.
    async fn remove_from_queue(&self, path: &Path, agent_id: &str) {
        let mut queues = self.queues.lock().await;
        if let Some(queue) = queues.get_mut(path) {
            queue.retain(|r| r.agent_id != agent_id);
            if queue.is_empty() {
                queues.remove(path);
            }
        }
    }

    /// Force-release locks held longer than max_hold_seconds.
    /// Called by the background reaper task.
    pub async fn reap_stale_locks(&self) {
        let max_hold = Duration::from_secs(self.config.max_hold_seconds as u64);
        let paths_to_notify: Vec<PathBuf> = {
            let mut locks = self.locks.lock().await;
            let mut stale_paths = Vec::new();

            let keys: Vec<PathBuf> = locks.keys().cloned().collect();
            for path in keys {
                if let Some(lock) = locks.get(&path) {
                    if lock.acquired_at.elapsed() > max_hold {
                        stale_paths.push(path);
                    }
                }
            }

            for path in &stale_paths {
                locks.remove(path);
            }

            stale_paths
        };

        for path in paths_to_notify {
            self.notify_next_in_queue(&path).await;
        }
    }

    /// Spawn a background reaper task that checks for stale locks every 10 seconds.
    pub fn spawn_reaper(self: &Arc<Self>, shutdown_token: tokio_util::sync::CancellationToken) {
        let registry = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                tokio::select! {
                    _ = shutdown_token.cancelled() => {
                        tracing::info!("VFL reaper task shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        registry.reap_stale_locks().await;
                    }
                }
            }
        });
    }

    /// Get a snapshot of active locks (for debugging/UI).
    pub async fn active_lock_count(&self) -> usize {
        self.locks.lock().await.len()
    }

    /// Get a snapshot of queued requests (for debugging/UI).
    pub async fn queued_request_count(&self) -> usize {
        let queues = self.queues.lock().await;
        queues.values().map(|q| q.len()).sum()
    }
}

/// RAII guard that releases the VFL lock when dropped.
pub struct LockGuard {
    registry: Arc<VirtualFileLockRegistry>,
    path: PathBuf,
    agent_id: String,
    lock_type: LockType,
    /// If true, this is a no-op guard (VFL disabled).
    noop: bool,
}

impl std::fmt::Debug for LockGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockGuard")
            .field("path", &self.path)
            .field("agent_id", &self.agent_id)
            .field("lock_type", &self.lock_type)
            .field("noop", &self.noop)
            .finish()
    }
}

impl LockGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn lock_type(&self) -> &LockType {
        &self.lock_type
    }

    /// Explicitly release the lock. Preferred over relying on Drop, which
    /// spawns a fire-and-forget task and cannot guarantee release ordering.
    pub async fn release(mut self) {
        if self.noop {
            return;
        }
        self.registry.release(&self.path, &self.agent_id).await;
        self.noop = true; // Prevent Drop from double-releasing
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if self.noop {
            return;
        }

        // Safety net: if the caller forgot to call release().await, spawn a
        // fire-and-forget task. This is a fallback — prefer explicit release.
        #[cfg(debug_assertions)]
        eprintln!(
            "[VFL] Warning: LockGuard for '{}' (agent {}) released via Drop instead of explicit release(). \
             This may cause race conditions.",
            self.path.display(),
            self.agent_id
        );

        let registry = Arc::clone(&self.registry);
        let path = self.path.clone();
        let agent_id = self.agent_id.clone();

        tauri::async_runtime::spawn(async move {
            registry.release(&path, &agent_id).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn test_config() -> FileLockingBlock {
        FileLockingBlock {
            enabled: true,
            max_hold_seconds: 60,
            stale_timeout_seconds: 30,
            queue_timeout_seconds: 5, // Short for tests
        }
    }

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vfl_test_{}", name))
    }

    #[tokio::test]
    async fn test_acquire_write_lock() {
        let registry = Arc::new(VirtualFileLockRegistry::new(test_config()));
        let path = test_path("write1");

        let guard = registry
            .acquire(&path, LockType::Write, "agent-1")
            .await
            .unwrap();

        assert_eq!(registry.active_lock_count().await, 1);
        drop(guard);

        // Give the spawned release task time to run
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(registry.active_lock_count().await, 0);
    }

    #[tokio::test]
    async fn test_shared_read_locks() {
        let registry = Arc::new(VirtualFileLockRegistry::new(test_config()));
        let path = test_path("read_shared");

        let _guard1 = registry
            .acquire(&path, LockType::Read, "agent-1")
            .await
            .unwrap();
        let _guard2 = registry
            .acquire(&path, LockType::Read, "agent-2")
            .await
            .unwrap();

        // Both should succeed — shared read
        assert_eq!(registry.active_lock_count().await, 1); // One path, shared
    }

    #[tokio::test]
    async fn test_write_blocks_second_writer() {
        let registry = Arc::new(VirtualFileLockRegistry::new(test_config()));
        let path = test_path("write_block");

        let order = Arc::new(AtomicU32::new(0));

        let guard1 = registry
            .acquire(&path, LockType::Write, "agent-1")
            .await
            .unwrap();

        let reg2 = Arc::clone(&registry);
        let path2 = path.clone();
        let order2 = Arc::clone(&order);

        let handle = tauri::async_runtime::spawn(async move {
            // This should block until agent-1 releases
            let _guard2 = reg2
                .acquire(&path2, LockType::Write, "agent-2")
                .await
                .unwrap();
            order2.store(2, Ordering::SeqCst);
        });

        // Give agent-2 a moment to queue up
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(order.load(Ordering::SeqCst), 0); // agent-2 still waiting

        order.store(1, Ordering::SeqCst);
        drop(guard1); // Release agent-1's lock

        handle.await.unwrap();
        assert_eq!(order.load(Ordering::SeqCst), 2); // agent-2 proceeded
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_fifo_ordering() {
        let registry = Arc::new(VirtualFileLockRegistry::new(test_config()));
        let path = test_path("fifo");
        let completion_order = Arc::new(Mutex::new(Vec::new()));

        let guard1 = registry
            .acquire(&path, LockType::Write, "agent-1")
            .await
            .unwrap();

        // Spawn agent-2, wait for it to be queued, then spawn agent-3
        let reg2 = Arc::clone(&registry);
        let p2 = path.clone();
        let order2 = Arc::clone(&completion_order);
        let h2 = tokio::spawn(async move {
            let _guard = reg2.acquire(&p2, LockType::Write, "agent-2").await.unwrap();
            order2.lock().await.push(2);
            tokio::time::sleep(Duration::from_millis(50)).await;
        });

        // Wait until agent-2 is in the queue before spawning agent-3
        loop {
            let queues = registry.queues.lock().await;
            if queues.get(&path).map(|q| q.len()).unwrap_or(0) >= 1 {
                break;
            }
            drop(queues);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let reg3 = Arc::clone(&registry);
        let p3 = path.clone();
        let order3 = Arc::clone(&completion_order);
        let h3 = tokio::spawn(async move {
            let _guard = reg3.acquire(&p3, LockType::Write, "agent-3").await.unwrap();
            order3.lock().await.push(3);
            tokio::time::sleep(Duration::from_millis(50)).await;
        });

        // Wait until agent-3 is also queued
        loop {
            let queues = registry.queues.lock().await;
            if queues.get(&path).map(|q| q.len()).unwrap_or(0) >= 2 {
                break;
            }
            drop(queues);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Release agent-1 — agent-2 should acquire first (FIFO)
        drop(guard1);

        h2.await.unwrap();
        h3.await.unwrap();

        let order = completion_order.lock().await;
        assert_eq!(*order, vec![2, 3]);
    }

    #[tokio::test]
    async fn test_queue_timeout() {
        let mut config = test_config();
        config.queue_timeout_seconds = 1; // 1 second timeout
        let registry = Arc::new(VirtualFileLockRegistry::new(config));
        let path = test_path("timeout");

        let _guard1 = registry
            .acquire(&path, LockType::Write, "agent-1")
            .await
            .unwrap();

        // Agent-2 should timeout
        let result = registry.acquire(&path, LockType::Write, "agent-2").await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ToolErrorCode::VflTimeout);
        assert!(err.retryable);
    }

    #[tokio::test]
    async fn test_release_all_for_agent() {
        let registry = Arc::new(VirtualFileLockRegistry::new(test_config()));
        let path1 = test_path("release_all_1");
        let path2 = test_path("release_all_2");

        let _guard1 = registry
            .acquire(&path1, LockType::Write, "agent-1")
            .await
            .unwrap();
        let _guard2 = registry
            .acquire(&path2, LockType::Write, "agent-1")
            .await
            .unwrap();

        assert_eq!(registry.active_lock_count().await, 2);

        // Force release bypasses the guards
        registry.release_all_for_agent("agent-1").await;
        assert_eq!(registry.active_lock_count().await, 0);
    }

    #[tokio::test]
    async fn test_stale_lock_reaper() {
        let mut config = test_config();
        config.max_hold_seconds = 0; // Immediately stale
        let registry = Arc::new(VirtualFileLockRegistry::new(config));
        let path = test_path("reaper");

        let _guard = registry
            .acquire(&path, LockType::Write, "agent-1")
            .await
            .unwrap();

        assert_eq!(registry.active_lock_count().await, 1);

        // Run reaper
        registry.reap_stale_locks().await;
        assert_eq!(registry.active_lock_count().await, 0);
    }

    #[tokio::test]
    async fn test_disabled_vfl_returns_noop_guard() {
        let config = FileLockingBlock {
            enabled: false,
            ..test_config()
        };
        let registry = Arc::new(VirtualFileLockRegistry::new(config));
        let path = test_path("disabled");

        let guard = registry
            .acquire(&path, LockType::Write, "agent-1")
            .await
            .unwrap();

        assert!(guard.noop);
        assert_eq!(registry.active_lock_count().await, 0); // No lock held
    }

    #[tokio::test]
    async fn test_write_blocks_reader() {
        let registry = Arc::new(VirtualFileLockRegistry::new(test_config()));
        let path = test_path("write_blocks_read");

        let guard1 = registry
            .acquire(&path, LockType::Write, "agent-1")
            .await
            .unwrap();

        let reg2 = Arc::clone(&registry);
        let path2 = path.clone();
        let completed = Arc::new(AtomicU32::new(0));
        let completed2 = Arc::clone(&completed);

        let handle = tauri::async_runtime::spawn(async move {
            let _guard2 = reg2
                .acquire(&path2, LockType::Read, "agent-2")
                .await
                .unwrap();
            completed2.store(1, Ordering::SeqCst);
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(completed.load(Ordering::SeqCst), 0); // Still waiting

        drop(guard1);
        handle.await.unwrap();
        assert_eq!(completed.load(Ordering::SeqCst), 1);
    }
}
