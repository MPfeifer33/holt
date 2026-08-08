use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::RwLock;

/// Tracks whether a pair was created automatically (orbital ring) or manually (A2A panel).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum PairSource {
    #[default]
    Manual,
    Auto,
}

/// Visibility/permission config for one agent pair (source → target).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VisibilityConfig {
    pub messaging_enabled: bool,
    pub workspace_visibility: bool,
    pub source: PairSource,
}

/// Summary of A2A pair changes from a membership update.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MembershipChanges {
    pub created: usize,
    pub removed: usize,
}

/// A message sent between agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AMessage {
    pub from_id: String,
    pub from_name: String,
    pub to_id: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedPair {
    source_id: String,
    target_id: String,
    config: VisibilityConfig,
}

/// An entry in the dead-letter queue for messages that couldn't be delivered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    pub message: A2AMessage,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

/// Registry managing agent-to-agent communication: visibility pairs, message queues.
pub struct A2ARegistry {
    /// Visibility config per directed pair (source_id, target_id).
    pairs: RwLock<HashMap<(String, String), VisibilityConfig>>,
    /// Pending messages to be injected at target's turn boundary.
    /// Keyed by target agent_id.
    pending_messages: RwLock<HashMap<String, Vec<A2AMessage>>>,
    /// Messages that couldn't be delivered (queue full, agent gone, etc.).
    dead_letter: RwLock<Vec<DeadLetterEntry>>,
    /// Optional storage path for persisted manual pairs.
    storage_path: Option<PathBuf>,
    /// Fallback for unconfigured pairs. When true, messaging is allowed between
    /// any pair that doesn't have an explicit VisibilityConfig entry.
    /// Loaded from `config.communication.a2a_default_enabled`.
    default_messaging_enabled: bool,
    /// Last wake time per agent, for auto-wake cooldown enforcement.
    wake_cooldowns: RwLock<HashMap<String, Instant>>,
}

impl Default for A2ARegistry {
    fn default() -> Self {
        Self::new(false)
    }
}

impl A2ARegistry {
    pub fn new(default_messaging_enabled: bool) -> Self {
        Self {
            pairs: RwLock::new(HashMap::new()),
            pending_messages: RwLock::new(HashMap::new()),
            dead_letter: RwLock::new(Vec::new()),
            storage_path: None,
            default_messaging_enabled,
            wake_cooldowns: RwLock::new(HashMap::new()),
        }
    }

    /// Load persisted manual pairs from disk.
    pub fn load(config_dir: PathBuf, default_messaging_enabled: bool) -> Self {
        let path = config_dir.join("a2a_pairs.json");
        let pairs = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(data) => match serde_json::from_str::<Vec<PersistedPair>>(&data) {
                    Ok(parsed) => parsed
                        .into_iter()
                        .map(|pair| ((pair.source_id, pair.target_id), pair.config))
                        .collect(),
                    Err(e) => {
                        tracing::error!("a2a_pairs.json is corrupt, backing up: {e}");
                        let backup = config_dir.join("a2a_pairs.json.corrupt");
                        let _ = std::fs::copy(&path, &backup);
                        HashMap::new()
                    }
                },
                Err(e) => {
                    tracing::warn!("Failed to read a2a_pairs.json: {e}");
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        tracing::info!(
            "Loaded {} persisted A2A pair(s) from {:?}",
            pairs.len(),
            path
        );

        Self {
            pairs: RwLock::new(pairs),
            pending_messages: RwLock::new(HashMap::new()),
            dead_letter: RwLock::new(Vec::new()),
            storage_path: Some(path),
            default_messaging_enabled,
            wake_cooldowns: RwLock::new(HashMap::new()),
        }
    }

    async fn save_manual_pairs(&self) {
        let Some(path) = &self.storage_path else {
            return;
        };

        let persisted: Vec<PersistedPair> = self
            .pairs
            .read()
            .await
            .iter()
            .filter(|(_, config)| config.source == PairSource::Manual)
            .map(|((source_id, target_id), config)| PersistedPair {
                source_id: source_id.clone(),
                target_id: target_id.clone(),
                config: config.clone(),
            })
            .collect();

        let tmp_path = path.with_extension("json.tmp");
        match serde_json::to_string_pretty(&persisted) {
            Ok(data) => {
                if let Err(e) = std::fs::write(&tmp_path, &data) {
                    tracing::warn!("Failed to write a2a_pairs tmp file: {e}");
                    return;
                }
                if let Err(e) = std::fs::rename(&tmp_path, path) {
                    tracing::warn!("Failed to rename a2a_pairs tmp file: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("Failed to serialize persisted A2A pairs: {e}");
            }
        }
    }

    /// Set visibility config for a directed agent pair.
    pub async fn set_visibility(&self, source_id: &str, target_id: &str, config: VisibilityConfig) {
        let should_persist = config.source == PairSource::Manual;
        self.pairs
            .write()
            .await
            .insert((source_id.to_string(), target_id.to_string()), config);
        if should_persist {
            self.save_manual_pairs().await;
        }
    }

    /// Approve collaboration reciprocally for a pair.
    /// If a manual config already exists for either direction, it is preserved
    /// to prevent accidentally broadening a restrictive manual configuration.
    pub async fn approve_collaboration(
        &self,
        source_id: &str,
        target_id: &str,
        workspace_visibility: bool,
    ) {
        let mut pairs = self.pairs.write().await;
        let forward_key = (source_id.to_string(), target_id.to_string());
        let reverse_key = (target_id.to_string(), source_id.to_string());

        // Preserve existing manual configs — don't overwrite
        let forward_manual = pairs
            .get(&forward_key)
            .map(|c| c.source == PairSource::Manual)
            .unwrap_or(false);
        let reverse_manual = pairs
            .get(&reverse_key)
            .map(|c| c.source == PairSource::Manual)
            .unwrap_or(false);

        if forward_manual && reverse_manual {
            // Both directions already have manual config — nothing to do
            return;
        }

        let config = VisibilityConfig {
            messaging_enabled: true,
            workspace_visibility,
            source: PairSource::Manual,
        };

        if !forward_manual {
            pairs.insert(forward_key, config.clone());
        }
        if !reverse_manual {
            pairs.insert(reverse_key, config);
        }

        drop(pairs);
        self.save_manual_pairs().await;
    }

    /// Get visibility config for a directed agent pair. Returns None if not configured.
    pub async fn get_visibility(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Option<VisibilityConfig> {
        self.pairs
            .read()
            .await
            .get(&(source_id.to_string(), target_id.to_string()))
            .cloned()
    }

    /// Check if messaging is enabled from source to target.
    /// Falls back to `default_messaging_enabled` when no explicit pair config exists.
    pub async fn is_messaging_enabled(&self, source_id: &str, target_id: &str) -> bool {
        self.pairs
            .read()
            .await
            .get(&(source_id.to_string(), target_id.to_string()))
            .map(|v| v.messaging_enabled)
            .unwrap_or(self.default_messaging_enabled)
    }

    /// Check if workspace visibility is enabled from source to target.
    pub async fn is_workspace_visible(&self, source_id: &str, target_id: &str) -> bool {
        self.pairs
            .read()
            .await
            .get(&(source_id.to_string(), target_id.to_string()))
            .map(|v| v.workspace_visibility)
            .unwrap_or(false)
    }

    /// Send a message from one agent to another.
    /// Returns Ok(()) if messaging is enabled, Err with reason otherwise.
    pub async fn send_message(
        &self,
        from_id: &str,
        from_name: &str,
        to_id: &str,
        content: &str,
    ) -> Result<(), String> {
        if !self.is_messaging_enabled(from_id, to_id).await {
            return Err(format!(
                "Messaging not enabled from agent '{}' to '{}'",
                from_id, to_id
            ));
        }

        let msg = A2AMessage {
            from_id: from_id.to_string(),
            from_name: from_name.to_string(),
            to_id: to_id.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
        };

        const MAX_PENDING_PER_AGENT: usize = 100;
        const MAX_DEAD_LETTER: usize = 500;

        let mut pending = self.pending_messages.write().await;
        let queue = pending.entry(to_id.to_string()).or_default();
        if queue.len() >= MAX_PENDING_PER_AGENT {
            // Route to dead-letter instead of hard error
            tracing::warn!(
                "Message queue full for agent '{}' ({} pending), routing to dead-letter",
                to_id,
                MAX_PENDING_PER_AGENT
            );
            drop(pending);
            let mut dl = self.dead_letter.write().await;
            // Evict oldest entries if at capacity
            while dl.len() >= MAX_DEAD_LETTER {
                dl.remove(0);
            }
            dl.push(DeadLetterEntry {
                message: msg,
                reason: format!("Queue full for agent '{}'", to_id),
                timestamp: Utc::now(),
            });
            return Ok(());
        }
        queue.push(msg);

        Ok(())
    }

    /// Check if an agent has pending messages (non-consuming).
    pub async fn has_pending_messages(&self, agent_id: &str) -> bool {
        self.pending_messages
            .read()
            .await
            .get(agent_id)
            .map(|msgs| !msgs.is_empty())
            .unwrap_or(false)
    }

    /// Check if enough time has passed since the last auto-wake for this agent.
    /// Returns true if wake is allowed, false if cooldown is still active.
    pub async fn check_wake_cooldown(&self, agent_id: &str) -> bool {
        const COOLDOWN_SECS: u64 = 5;
        let cooldowns = self.wake_cooldowns.read().await;
        match cooldowns.get(agent_id) {
            Some(last) => last.elapsed().as_secs() >= COOLDOWN_SECS,
            None => true,
        }
    }

    /// Record that an agent was just woken via A2A auto-wake.
    pub async fn record_wake(&self, agent_id: &str) {
        let mut cooldowns = self.wake_cooldowns.write().await;
        cooldowns.insert(agent_id.to_string(), Instant::now());
    }

    /// Drain pending messages for an agent (called at turn boundary).
    pub async fn drain_messages(&self, agent_id: &str) -> Vec<A2AMessage> {
        self.pending_messages
            .write()
            .await
            .remove(agent_id)
            .unwrap_or_default()
    }

    /// Drain dead-letter entries for a specific agent (by target), returning and clearing them.
    pub async fn drain_dead_letter(&self, agent_id: &str) -> Vec<DeadLetterEntry> {
        let mut dl = self.dead_letter.write().await;
        let (matching, remaining): (Vec<_>, Vec<_>) = dl
            .drain(..)
            .partition(|entry| entry.message.to_id == agent_id);
        *dl = remaining;
        matching
    }

    /// Get the total dead-letter count (for diagnostics).
    pub async fn dead_letter_count(&self) -> usize {
        self.dead_letter.read().await.len()
    }

    /// Get all configured pairs (for UI display).
    pub async fn list_pairs(&self) -> Vec<((String, String), VisibilityConfig)> {
        self.pairs
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Remove all Auto-sourced pairs where the given agent is source OR target.
    /// Returns the number of pairs removed.
    pub async fn remove_auto_pairs_for_agent(&self, agent_id: &str) -> usize {
        let mut pairs = self.pairs.write().await;
        let before = pairs.len();
        pairs.retain(|(src, tgt), config| {
            if config.source != PairSource::Auto {
                return true;
            }
            !(src == agent_id || tgt == agent_id)
        });
        before - pairs.len()
    }

    /// Remove ALL pairs (Manual and Auto) where the given agent is source OR target.
    /// Used during agent deletion cleanup.
    pub async fn remove_all_pairs_for_agent(&self, agent_id: &str) -> usize {
        let mut pairs = self.pairs.write().await;
        let before = pairs.len();
        pairs.retain(|(src, tgt), _| !(src == agent_id || tgt == agent_id));
        let removed = before - pairs.len();
        drop(pairs);
        if removed > 0 {
            self.save_manual_pairs().await;
        }
        removed
    }

    /// Remove all pending messages for a deleted agent:
    /// 1. Remove messages SENT TO the agent (their inbox)
    /// 2. Remove messages SENT BY the agent from other agents' queues
    /// Without step 2, orphaned messages accumulate in other agents' queues
    /// indefinitely after the sender is deleted.
    pub async fn drain_pending_messages_for_cleanup(&self, agent_id: &str) {
        let mut pending = self.pending_messages.write().await;
        // Remove the deleted agent's inbox
        pending.remove(agent_id);
        // Remove messages the deleted agent sent to other agents
        for queue in pending.values_mut() {
            queue.retain(|msg| msg.from_id != agent_id);
        }
    }

    /// Atomically rebuild all Auto pairs from a team map.
    /// Removes all existing Auto pairs, then creates bidirectional Auto pairs within each team.
    /// Manual pairs are preserved. Returns the number of new pairs created.
    pub async fn rebuild_all_auto_pairs(&self, teams: HashMap<String, Vec<String>>) -> usize {
        let mut pairs = self.pairs.write().await;
        pairs.retain(|_, config| config.source != PairSource::Auto);

        let mut created = 0;
        for (_team_id, members) in &teams {
            for i in 0..members.len() {
                for j in 0..members.len() {
                    if i == j {
                        continue;
                    }
                    let key = (members[i].clone(), members[j].clone());
                    if let std::collections::hash_map::Entry::Vacant(entry) = pairs.entry(key) {
                        entry.insert(VisibilityConfig {
                            messaging_enabled: true,
                            workspace_visibility: true,
                            source: PairSource::Auto,
                        });
                        created += 1;
                    }
                }
            }
        }
        created
    }

    /// Update membership for a single agent.
    /// Removes all Auto pairs for this agent, then creates bidirectional Auto pairs
    /// with the given teammates.
    pub async fn update_agent_membership(
        &self,
        agent_id: &str,
        teammates: &[String],
    ) -> MembershipChanges {
        let mut pairs = self.pairs.write().await;

        let before = pairs.len();
        pairs.retain(|(src, tgt), config| {
            if config.source != PairSource::Auto {
                return true;
            }
            !(src == agent_id || tgt == agent_id)
        });
        let removed = before - pairs.len();

        let mut created = 0;
        for mate in teammates {
            if mate == agent_id {
                continue;
            }
            let key_fwd = (agent_id.to_string(), mate.clone());
            if let std::collections::hash_map::Entry::Vacant(entry) = pairs.entry(key_fwd) {
                entry.insert(VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: true,
                    source: PairSource::Auto,
                });
                created += 1;
            }
            let key_rev = (mate.clone(), agent_id.to_string());
            if let std::collections::hash_map::Entry::Vacant(entry) = pairs.entry(key_rev) {
                entry.insert(VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: true,
                    source: PairSource::Auto,
                });
                created += 1;
            }
        }

        MembershipChanges { created, removed }
    }

    pub async fn is_messaging_enabled_bidirectional(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> bool {
        self.is_messaging_enabled(source_id, target_id).await
            && self.is_messaging_enabled(target_id, source_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_messaging_disabled_by_default() {
        let registry = A2ARegistry::new(false);
        assert!(!registry.is_messaging_enabled("a", "b").await);
    }

    #[tokio::test]
    async fn test_enable_and_send_message() {
        let registry = A2ARegistry::new(false);
        registry
            .set_visibility(
                "agent-1",
                "agent-2",
                VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: false,
                    source: PairSource::Manual,
                },
            )
            .await;

        let result = registry
            .send_message("agent-1", "Claude", "agent-2", "Hello!")
            .await;
        assert!(result.is_ok());

        let messages = registry.drain_messages("agent-2").await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello!");
        assert_eq!(messages[0].from_name, "Claude");
    }

    #[tokio::test]
    async fn test_send_message_denied_when_explicitly_disabled() {
        let registry = A2ARegistry::new(false);
        registry
            .set_visibility(
                "agent-1",
                "agent-2",
                VisibilityConfig {
                    messaging_enabled: false,
                    workspace_visibility: false,
                    source: PairSource::Manual,
                },
            )
            .await;
        let result = registry
            .send_message("agent-1", "Claude", "agent-2", "Hello!")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_drain_empties_queue() {
        let registry = A2ARegistry::new(false);
        registry
            .set_visibility(
                "a",
                "b",
                VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: false,
                    source: PairSource::Manual,
                },
            )
            .await;

        registry.send_message("a", "A", "b", "msg1").await.unwrap();
        registry.send_message("a", "A", "b", "msg2").await.unwrap();

        let msgs = registry.drain_messages("b").await;
        assert_eq!(msgs.len(), 2);

        let msgs = registry.drain_messages("b").await;
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn test_workspace_visibility() {
        let registry = A2ARegistry::new(false);
        assert!(!registry.is_workspace_visible("a", "b").await);

        registry
            .set_visibility(
                "a",
                "b",
                VisibilityConfig {
                    messaging_enabled: false,
                    workspace_visibility: true,
                    source: PairSource::Manual,
                },
            )
            .await;

        assert!(registry.is_workspace_visible("a", "b").await);
        assert!(!registry.is_messaging_enabled("a", "b").await);
    }

    #[tokio::test]
    async fn test_directed_pairs_explicit_disable() {
        let registry = A2ARegistry::new(false);
        // Explicitly disable b→a
        registry
            .set_visibility(
                "b",
                "a",
                VisibilityConfig {
                    messaging_enabled: false,
                    workspace_visibility: false,
                    source: PairSource::Manual,
                },
            )
            .await;

        // Default-closed: a→b disabled until explicitly configured
        assert!(!registry.is_messaging_enabled("a", "b").await);
        // Explicitly disabled
        assert!(!registry.is_messaging_enabled("b", "a").await);
    }

    #[tokio::test]
    async fn test_set_auto_pair_and_remove_by_source() {
        let registry = A2ARegistry::new(false);
        registry
            .set_visibility(
                "a",
                "b",
                VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: true,
                    source: PairSource::Auto,
                },
            )
            .await;
        registry
            .set_visibility(
                "a",
                "c",
                VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: false,
                    source: PairSource::Manual,
                },
            )
            .await;

        let removed = registry.remove_auto_pairs_for_agent("a").await;
        assert_eq!(removed, 1);
        // Auto pair removed — but default-open still allows messaging
        // Manual pair still explicitly exists
        assert!(registry.get_visibility("a", "b").await.is_none());
        assert!(registry.get_visibility("a", "c").await.is_some());
    }

    #[tokio::test]
    async fn test_remove_auto_pairs_both_directions() {
        let registry = A2ARegistry::new(false);
        registry
            .set_visibility(
                "a",
                "b",
                VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: true,
                    source: PairSource::Auto,
                },
            )
            .await;
        registry
            .set_visibility(
                "b",
                "a",
                VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: true,
                    source: PairSource::Auto,
                },
            )
            .await;

        let removed = registry.remove_auto_pairs_for_agent("a").await;
        assert_eq!(removed, 2);
    }

    #[tokio::test]
    async fn test_rebuild_all_auto_pairs() {
        let registry = A2ARegistry::new(false);
        registry
            .set_visibility(
                "a",
                "x",
                VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: false,
                    source: PairSource::Manual,
                },
            )
            .await;

        let mut teams = HashMap::new();
        teams.insert(
            "sun0_ring0".to_string(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );
        teams.insert("sun0_ring1".to_string(), vec!["d".to_string()]);

        let created = registry.rebuild_all_auto_pairs(teams).await;
        assert_eq!(created, 6);
        // Auto pairs created for ring members
        assert!(registry.get_visibility("a", "b").await.is_some());
        assert!(registry.get_visibility("b", "a").await.is_some());
        assert!(registry.get_visibility("a", "c").await.is_some());
        // d is alone in ring1 — no auto pair to a
        assert!(registry.get_visibility("d", "a").await.is_none());
        // Manual pair preserved
        assert!(registry.get_visibility("a", "x").await.is_some());
    }

    #[tokio::test]
    async fn test_update_single_agent_membership() {
        let registry = A2ARegistry::new(false);
        let mut teams = HashMap::new();
        teams.insert(
            "sun0_ring0".to_string(),
            vec!["a".to_string(), "b".to_string()],
        );
        registry.rebuild_all_auto_pairs(teams).await;
        assert!(registry.get_visibility("a", "b").await.is_some());

        let changes = registry.update_agent_membership("a", &[]).await;
        // Auto pairs removed
        assert!(registry.get_visibility("a", "b").await.is_none());
        assert!(registry.get_visibility("b", "a").await.is_none());
        assert_eq!(changes.removed, 2);
        assert_eq!(changes.created, 0);
    }

    #[tokio::test]
    async fn test_remove_all_pairs_for_agent() {
        let registry = A2ARegistry::new(false);
        registry
            .set_visibility(
                "agent-1",
                "agent-2",
                VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: false,
                    source: PairSource::Manual,
                },
            )
            .await;
        registry
            .set_visibility(
                "agent-3",
                "agent-1",
                VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: false,
                    source: PairSource::Auto,
                },
            )
            .await;
        registry
            .set_visibility(
                "agent-2",
                "agent-3",
                VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: false,
                    source: PairSource::Manual,
                },
            )
            .await;

        let removed = registry.remove_all_pairs_for_agent("agent-1").await;
        assert_eq!(removed, 2);

        let pairs = registry.list_pairs().await;
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, ("agent-2".to_string(), "agent-3".to_string()));
    }

    #[tokio::test]
    async fn test_drain_pending_messages_for_cleanup() {
        let registry = A2ARegistry::new(false);
        registry
            .set_visibility(
                "agent-2",
                "agent-1",
                VisibilityConfig {
                    messaging_enabled: true,
                    workspace_visibility: false,
                    source: PairSource::Manual,
                },
            )
            .await;
        let _ = registry
            .send_message("agent-2", "Claude", "agent-1", "hello")
            .await;
        assert!(registry.has_pending_messages("agent-1").await);

        registry.drain_pending_messages_for_cleanup("agent-1").await;
        assert!(!registry.has_pending_messages("agent-1").await);
    }

    #[tokio::test]
    async fn test_cleanup_idempotent() {
        let registry = A2ARegistry::new(false);
        registry.remove_all_pairs_for_agent("nonexistent").await;
        registry
            .drain_pending_messages_for_cleanup("nonexistent")
            .await;
        // Should not panic
    }

    #[tokio::test]
    async fn test_approve_collaboration_is_reciprocal() {
        let registry = A2ARegistry::new(false);
        registry.approve_collaboration("a", "b", true).await;

        assert!(registry.is_messaging_enabled("a", "b").await);
        assert!(registry.is_messaging_enabled("b", "a").await);
        assert!(registry.is_workspace_visible("a", "b").await);
        assert!(registry.is_workspace_visible("b", "a").await);
    }

    #[tokio::test]
    async fn test_manual_pairs_persist_and_reload() {
        let dir = tempdir().unwrap();
        let registry = A2ARegistry::load(dir.path().to_path_buf(), false);
        registry.approve_collaboration("a", "b", false).await;

        let reloaded = A2ARegistry::load(dir.path().to_path_buf(), false);
        assert!(reloaded.is_messaging_enabled("a", "b").await);
        assert!(reloaded.is_messaging_enabled("b", "a").await);
        assert!(!reloaded.is_workspace_visible("a", "b").await);
        assert!(!reloaded.is_workspace_visible("b", "a").await);
    }

    // ── 4C-1: Dead-letter queue ──────────────────────────────────────

    #[tokio::test]
    async fn test_full_queue_routes_to_dead_letter() {
        let registry = A2ARegistry::new(false);
        registry
            .approve_collaboration("sender", "target", false)
            .await;

        // Fill queue to capacity
        for i in 0..100 {
            let result = registry
                .send_message("sender", "Sender", "target", &format!("msg {i}"))
                .await;
            assert!(result.is_ok());
        }

        // 101st message should go to dead-letter, not error
        let result = registry
            .send_message("sender", "Sender", "target", "overflow msg")
            .await;
        assert!(result.is_ok());

        assert_eq!(registry.dead_letter_count().await, 1);
    }

    #[tokio::test]
    async fn test_dead_letter_capped_at_500() {
        let registry = A2ARegistry::new(false);
        registry.approve_collaboration("s", "t", false).await;

        // Fill the main queue first
        for i in 0..100 {
            registry
                .send_message("s", "S", "t", &format!("fill {i}"))
                .await
                .unwrap();
        }

        // Push 501 messages to dead-letter
        for i in 0..501 {
            registry
                .send_message("s", "S", "t", &format!("dl {i}"))
                .await
                .unwrap();
        }

        assert_eq!(registry.dead_letter_count().await, 500);
    }

    #[tokio::test]
    async fn test_drain_dead_letter_returns_and_clears() {
        let registry = A2ARegistry::new(false);
        registry.approve_collaboration("s", "t1", false).await;
        registry.approve_collaboration("s", "t2", false).await;

        // Fill t1 queue, then overflow
        for i in 0..100 {
            registry
                .send_message("s", "S", "t1", &format!("m{i}"))
                .await
                .unwrap();
        }
        registry
            .send_message("s", "S", "t1", "overflow-t1")
            .await
            .unwrap();

        // Fill t2 queue, then overflow
        for i in 0..100 {
            registry
                .send_message("s", "S", "t2", &format!("m{i}"))
                .await
                .unwrap();
        }
        registry
            .send_message("s", "S", "t2", "overflow-t2")
            .await
            .unwrap();

        assert_eq!(registry.dead_letter_count().await, 2);

        let drained = registry.drain_dead_letter("t1").await;
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].message.content, "overflow-t1");

        // t2's entry should still be there
        assert_eq!(registry.dead_letter_count().await, 1);
    }

    // ── 4C-2: approve_collaboration preserves manual config ──────────

    #[tokio::test]
    async fn test_approve_preserves_existing_manual_config() {
        let registry = A2ARegistry::new(false);

        // Set restrictive manual config (no workspace visibility)
        registry.approve_collaboration("a", "b", false).await;
        assert!(!registry.is_workspace_visible("a", "b").await);

        // Try to approve with broader permissions
        registry.approve_collaboration("a", "b", true).await;

        // Original restrictive config should be preserved
        assert!(!registry.is_workspace_visible("a", "b").await);
        assert!(!registry.is_workspace_visible("b", "a").await);
    }

    #[tokio::test]
    async fn test_approve_creates_bidirectional_pair() {
        let registry = A2ARegistry::new(false);
        registry.approve_collaboration("x", "y", true).await;

        assert!(registry.is_messaging_enabled("x", "y").await);
        assert!(registry.is_messaging_enabled("y", "x").await);
        assert!(registry.is_workspace_visible("x", "y").await);
        assert!(registry.is_workspace_visible("y", "x").await);
    }
}
