use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tauri::async_runtime::JoinHandle;
use tokio::sync::RwLock;

use super::agent::ConversationHistory;
use super::connection::ConnectionConfig;

const RESULT_TTL_SECS: u64 = 3600;

/// Status of a subagent (distinct from top-level AgentStatus for clarity).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SubagentStatus {
    Spawning,
    Working,
    Completed,
    Failed(String),
    Cancelled,
}

/// Full subagent slot (replaces the placeholder from Session 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSlot {
    pub job_id: String,
    pub parent_id: String,
    pub name: String,
    pub connection: ConnectionConfig,
    pub status: SubagentStatus,
    pub conversation: ConversationHistory,
    pub task: String,
    pub model_override: Option<String>,
    pub created_at: DateTime<Utc>,
    pub result: Option<String>,
}

/// Result delivered from a completed subagent to its parent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    pub job_id: String,
    pub parent_id: String,
    pub name: String,
    pub summary: String,
    pub status: SubagentStatus,
    #[serde(skip, default = "Instant::now")]
    pub created_at: Instant,
}

/// Info for listing subagents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentInfo {
    pub job_id: String,
    pub name: String,
    pub task: String,
    pub status: SubagentStatus,
    pub elapsed_ms: u64,
    pub created_at: String,
}

/// Manages subagent lifecycle: spawning, tracking, cancellation, and result delivery.
pub struct SubagentManager {
    /// Running subagent task handles (for cancellation via abort).
    handles: RwLock<HashMap<String, JoinHandle<()>>>,
    /// Subagent slot state (for status queries).
    slots: RwLock<HashMap<String, SubagentSlot>>,
    /// Pending results to be injected at parent's turn boundary.
    /// Keyed by parent_id.
    pending_results: RwLock<HashMap<String, Vec<SubagentResult>>>,
}

impl Default for SubagentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentManager {
    pub fn new() -> Self {
        Self {
            handles: RwLock::new(HashMap::new()),
            slots: RwLock::new(HashMap::new()),
            pending_results: RwLock::new(HashMap::new()),
        }
    }

    /// Register a subagent slot and its task handle atomically.
    /// Both locks are acquired before either insert to prevent a window
    /// where a slot exists without its corresponding handle.
    pub async fn register(&self, slot: SubagentSlot, handle: JoinHandle<()>) {
        let job_id = slot.job_id.clone();
        let (mut slots, mut handles) = tokio::join!(self.slots.write(), self.handles.write());
        slots.insert(job_id.clone(), slot);
        handles.insert(job_id, handle);
    }

    /// Update a subagent's status.
    pub async fn update_status(&self, job_id: &str, status: SubagentStatus) {
        if let Some(slot) = self.slots.write().await.get_mut(job_id) {
            slot.status = status;
        }
    }

    /// Record a subagent as completed, queue the result for parent injection, and remove the slot.
    pub async fn complete(&self, job_id: &str, result: String) {
        // Remove task handle — it's done
        self.handles.write().await.remove(job_id);

        // Extract slot data and remove it from the map
        let slot_data = {
            let mut slots = self.slots.write().await;
            slots.remove(job_id)
        };

        if let Some(slot) = slot_data {
            let subagent_result = SubagentResult {
                job_id: job_id.to_string(),
                parent_id: slot.parent_id.clone(),
                name: slot.name.clone(),
                summary: result,
                status: SubagentStatus::Completed,

                created_at: Instant::now(),
            };

            let mut pending = self.pending_results.write().await;
            pending
                .entry(subagent_result.parent_id.clone())
                .or_insert_with(Vec::new)
                .push(subagent_result);
        }
    }

    /// Record a subagent as failed, queue the result for parent injection, and remove the slot.
    pub async fn fail(&self, job_id: &str, reason: String) {
        // Remove task handle — it's done
        self.handles.write().await.remove(job_id);

        // Extract slot data and remove it from the map
        let slot_data = {
            let mut slots = self.slots.write().await;
            slots.remove(job_id)
        };

        if let Some(slot) = slot_data {
            let subagent_result = SubagentResult {
                job_id: job_id.to_string(),
                parent_id: slot.parent_id.clone(),
                name: slot.name.clone(),
                summary: format!("Failed: {}", reason),
                status: SubagentStatus::Failed(reason),

                created_at: Instant::now(),
            };

            let mut pending = self.pending_results.write().await;
            pending
                .entry(subagent_result.parent_id.clone())
                .or_insert_with(Vec::new)
                .push(subagent_result);
        }
    }

    /// Check the status of a subagent.
    pub async fn check(&self, job_id: &str) -> Option<SubagentInfo> {
        let slots = self.slots.read().await;
        slots.get(job_id).map(|slot| SubagentInfo {
            job_id: slot.job_id.clone(),
            name: slot.name.clone(),
            task: slot.task.clone(),
            status: slot.status.clone(),
            elapsed_ms: (Utc::now() - slot.created_at).num_milliseconds().max(0) as u64,
            created_at: slot.created_at.to_rfc3339(),
        })
    }

    /// Cancel a subagent. Aborts the task, queues a cancellation result for the parent, and removes the slot.
    pub async fn cancel(&self, job_id: &str) -> Option<String> {
        // Abort the task handle
        if let Some(handle) = self.handles.write().await.remove(job_id) {
            handle.abort();
        }

        // Remove slot and queue a cancellation result for the parent
        let mut slots = self.slots.write().await;
        if let Some(slot) = slots.remove(job_id) {
            let cancellation_result = SubagentResult {
                job_id: job_id.to_string(),
                parent_id: slot.parent_id.clone(),
                name: slot.name.clone(),
                summary: "Subagent was cancelled".to_string(),
                status: SubagentStatus::Cancelled,

                created_at: Instant::now(),
            };

            // Must drop slots lock before acquiring pending_results lock
            let existing_result = slot.result.clone();
            drop(slots);

            let mut pending = self.pending_results.write().await;
            pending
                .entry(cancellation_result.parent_id.clone())
                .or_insert_with(Vec::new)
                .push(cancellation_result);

            existing_result
        } else {
            None
        }
    }

    /// Cancel all subagents for a parent agent.
    pub async fn cancel_all_for_parent(&self, parent_id: &str) {
        let job_ids: Vec<String> = {
            let slots = self.slots.read().await;
            slots
                .values()
                .filter(|s| {
                    s.parent_id == parent_id
                        && matches!(s.status, SubagentStatus::Spawning | SubagentStatus::Working)
                })
                .map(|s| s.job_id.clone())
                .collect()
        };

        for job_id in job_ids {
            self.cancel(&job_id).await;
        }
    }

    /// List all subagents for a parent agent.
    pub async fn list(&self, parent_id: &str) -> Vec<SubagentInfo> {
        let slots = self.slots.read().await;
        slots
            .values()
            .filter(|s| s.parent_id == parent_id)
            .map(|slot| SubagentInfo {
                job_id: slot.job_id.clone(),
                name: slot.name.clone(),
                task: slot.task.clone(),
                status: slot.status.clone(),
                elapsed_ms: (Utc::now() - slot.created_at).num_milliseconds().max(0) as u64,
                created_at: slot.created_at.to_rfc3339(),
            })
            .collect()
    }

    /// Count active (Spawning or Working) subagents for a parent.
    pub async fn active_count(&self, parent_id: &str) -> usize {
        let slots = self.slots.read().await;
        slots
            .values()
            .filter(|s| {
                s.parent_id == parent_id
                    && matches!(s.status, SubagentStatus::Spawning | SubagentStatus::Working)
            })
            .count()
    }

    /// Count total active subagents across all parents (for global limit enforcement).
    pub async fn total_active_count(&self) -> usize {
        let slots = self.slots.read().await;
        slots
            .values()
            .filter(|s| matches!(s.status, SubagentStatus::Spawning | SubagentStatus::Working))
            .count()
    }

    /// Drain pending results for a parent agent (called at turn boundary).
    /// Returns completed/failed subagent results to be injected as system messages.
    /// Filters out results older than RESULT_TTL_SECS.
    pub async fn drain_results(&self, parent_id: &str) -> Vec<SubagentResult> {
        let mut pending = self.pending_results.write().await;
        let results = pending.remove(parent_id).unwrap_or_default();
        let ttl = Duration::from_secs(RESULT_TTL_SECS);
        results
            .into_iter()
            .filter(|r| r.created_at.elapsed() < ttl)
            .collect()
    }

    /// Remove pending results for a specific parent (used during agent deletion).
    pub async fn cleanup_pending_for_parent(&self, parent_id: &str) {
        self.pending_results.write().await.remove(parent_id);
    }

    /// Remove pending results for parents that no longer exist.
    /// Returns the number of parent entries removed.
    pub async fn sweep_orphaned_results(&self, active_agent_ids: &[String]) -> usize {
        let mut pending = self.pending_results.write().await;
        let before = pending.len();
        pending.retain(|parent_id, _| active_agent_ids.contains(parent_id));
        before - pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_slot(job_id: &str, parent_id: &str) -> SubagentSlot {
        SubagentSlot {
            job_id: job_id.to_string(),
            parent_id: parent_id.to_string(),
            name: format!("sub-{}", job_id),
            connection: ConnectionConfig::Local {
                host: "localhost".to_string(),
                port: 8080,
                model: None,
            },
            status: SubagentStatus::Working,
            conversation: ConversationHistory::default(),
            task: "test task".to_string(),
            model_override: None,
            created_at: Utc::now(),
            result: None,
        }
    }

    #[tokio::test]
    async fn test_register_and_check() {
        let manager = SubagentManager::new();
        let slot = test_slot("job-1", "parent-1");
        let handle = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
        manager.register(slot, handle).await;

        let info = manager.check("job-1").await.unwrap();
        assert_eq!(info.job_id, "job-1");
        assert_eq!(info.status, SubagentStatus::Working);
    }

    #[tokio::test]
    async fn test_complete_queues_result_and_removes_slot() {
        let manager = SubagentManager::new();
        let slot = test_slot("job-1", "parent-1");
        let handle = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
        manager.register(slot, handle).await;

        manager.complete("job-1", "Done!".to_string()).await;

        // Slot should be removed after completion
        assert!(manager.check("job-1").await.is_none());

        // But result should be queued for parent
        let results = manager.drain_results("parent-1").await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].summary, "Done!");
    }

    #[tokio::test]
    async fn test_drain_results_empties_queue() {
        let manager = SubagentManager::new();
        let slot = test_slot("job-1", "parent-1");
        let handle = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
        manager.register(slot, handle).await;

        manager.complete("job-1", "Done!".to_string()).await;
        let results = manager.drain_results("parent-1").await;
        assert_eq!(results.len(), 1);

        // Second drain should be empty
        let results = manager.drain_results("parent-1").await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_cancel_removes_slot() {
        let manager = SubagentManager::new();
        let slot = test_slot("job-1", "parent-1");
        let handle = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        }));
        manager.register(slot, handle).await;

        manager.cancel("job-1").await;

        // Slot should be removed after cancellation
        assert!(manager.check("job-1").await.is_none());
        assert_eq!(manager.active_count("parent-1").await, 0);
    }

    #[tokio::test]
    async fn test_cancel_all_for_parent() {
        let manager = SubagentManager::new();

        for i in 1..=3 {
            let slot = test_slot(&format!("job-{}", i), "parent-1");
            let handle = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            }));
            manager.register(slot, handle).await;
        }

        assert_eq!(manager.active_count("parent-1").await, 3);
        manager.cancel_all_for_parent("parent-1").await;
        assert_eq!(manager.active_count("parent-1").await, 0);
    }

    #[tokio::test]
    async fn test_list() {
        let manager = SubagentManager::new();
        let slot1 = test_slot("job-1", "parent-1");
        let slot2 = test_slot("job-2", "parent-1");
        let slot3 = test_slot("job-3", "parent-2"); // Different parent

        manager
            .register(
                slot1,
                tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {})),
            )
            .await;
        manager
            .register(
                slot2,
                tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {})),
            )
            .await;
        manager
            .register(
                slot3,
                tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {})),
            )
            .await;

        let list = manager.list("parent-1").await;
        assert_eq!(list.len(), 2);

        let list = manager.list("parent-2").await;
        assert_eq!(list.len(), 1);
    }

    #[tokio::test]
    async fn test_fail_queues_result_and_removes_slot() {
        let manager = SubagentManager::new();
        let slot = test_slot("job-1", "parent-1");
        manager
            .register(
                slot,
                tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {})),
            )
            .await;

        manager
            .fail("job-1", "Something went wrong".to_string())
            .await;

        // Slot should be removed after failure
        assert!(manager.check("job-1").await.is_none());

        // But result should be queued for parent
        let results = manager.drain_results("parent-1").await;
        assert_eq!(results.len(), 1);
        assert!(results[0].summary.contains("Failed"));
    }

    #[tokio::test]
    async fn test_drain_results_filters_expired() {
        let manager = SubagentManager::new();
        let slot = test_slot("job-1", "parent-1");
        let handle = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
        manager.register(slot, handle).await;
        manager.complete("job-1", "Done!".to_string()).await;

        // Manually backdate the result's created_at
        {
            let mut pending = manager.pending_results.write().await;
            if let Some(results) = pending.get_mut("parent-1") {
                results[0].created_at = Instant::now() - Duration::from_secs(RESULT_TTL_SECS + 1);
            }
        }

        let results = manager.drain_results("parent-1").await;
        assert!(results.is_empty(), "Expired result should be filtered out");
    }

    #[tokio::test]
    async fn test_sweep_orphaned_results() {
        let manager = SubagentManager::new();

        let slot1 = test_slot("job-1", "parent-1");
        let handle1 = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
        manager.register(slot1, handle1).await;
        manager.complete("job-1", "Done 1".to_string()).await;

        let slot2 = test_slot("job-2", "parent-2");
        let handle2 = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
        manager.register(slot2, handle2).await;
        manager.complete("job-2", "Done 2".to_string()).await;

        // Only parent-1 is active
        let active_ids = vec!["parent-1".to_string()];
        let removed = manager.sweep_orphaned_results(&active_ids).await;
        assert_eq!(removed, 1, "Should remove parent-2's orphaned results");

        let results = manager.drain_results("parent-1").await;
        assert_eq!(results.len(), 1);

        let results = manager.drain_results("parent-2").await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_cleanup_pending_for_parent() {
        let manager = SubagentManager::new();
        let slot = test_slot("job-1", "parent-1");
        let handle = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
        manager.register(slot, handle).await;
        manager.complete("job-1", "Done!".to_string()).await;

        manager.cleanup_pending_for_parent("parent-1").await;
        let results = manager.drain_results("parent-1").await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_cleanup_is_idempotent() {
        let manager = SubagentManager::new();
        manager.cleanup_pending_for_parent("nonexistent").await;
        manager.sweep_orphaned_results(&[]).await;
        // Should not panic
    }

    // ── 4C-3: Atomic registration ───────────────────────────────────

    #[tokio::test]
    async fn test_register_inserts_slot_and_handle_atomically() {
        let manager = SubagentManager::new();
        let slot = test_slot("atomic-1", "parent-1");
        let handle = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
        manager.register(slot, handle).await;

        // Both maps should contain the entry
        let slots = manager.slots.read().await;
        let handles = manager.handles.read().await;
        assert!(slots.contains_key("atomic-1"));
        assert!(handles.contains_key("atomic-1"));
    }

    #[tokio::test]
    async fn test_concurrent_registers_dont_interleave() {
        let manager = std::sync::Arc::new(SubagentManager::new());
        let mut join_handles = vec![];

        for i in 0..10 {
            let mgr = manager.clone();
            let jh = tokio::spawn(async move {
                let slot = test_slot(&format!("conc-{i}"), "parent-1");
                let handle = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
                mgr.register(slot, handle).await;
            });
            join_handles.push(jh);
        }

        for jh in join_handles {
            jh.await.unwrap();
        }

        // All 10 slots should have matching handles
        let slots = manager.slots.read().await;
        let handles = manager.handles.read().await;
        assert_eq!(slots.len(), 10);
        assert_eq!(handles.len(), 10);
        for key in slots.keys() {
            assert!(handles.contains_key(key), "Handle missing for slot {key}");
        }
    }
}
