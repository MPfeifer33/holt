//! Session dedup state — tracks which memory IDs have been injected in the current
//! conversation to prevent re-injection. Resets after compaction (context drop >50%).
//!
//! Uses a Moka cache with 30-minute TTL to prevent unbounded growth from
//! ephemeral agents (forks, subagents, dreams).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Per-agent session dedup state.
#[derive(Clone)]
struct AgentSession {
    /// Memory IDs that have been injected this session.
    injected_ids: HashSet<String>,
    /// Last known context token count (for compaction detection).
    last_context_tokens: u64,
}

/// Manages session dedup state across all agents.
/// Entries expire after 30 minutes of inactivity, preventing unbounded growth
/// from ephemeral forks and subagents.
pub struct SessionState {
    sessions: moka::future::Cache<String, Arc<Mutex<AgentSession>>>,
    /// Context drop threshold for compaction detection (e.g., 0.50 = 50%).
    compaction_drop_threshold: f64,
}

impl SessionState {
    pub fn new(compaction_drop_threshold: f64) -> Self {
        Self {
            sessions: moka::future::Cache::builder()
                .time_to_idle(Duration::from_secs(30 * 60)) // 30 min TTL
                .max_capacity(500)
                .build(),
            compaction_drop_threshold,
        }
    }

    /// Get or create a session entry for an agent.
    async fn get_or_create(&self, agent_id: &str) -> Arc<Mutex<AgentSession>> {
        if let Some(session) = self.sessions.get(agent_id).await {
            return session;
        }
        let session = Arc::new(Mutex::new(AgentSession {
            injected_ids: HashSet::new(),
            last_context_tokens: 0,
        }));
        self.sessions
            .insert(agent_id.to_string(), Arc::clone(&session))
            .await;
        session
    }

    /// Check if a memory ID has already been injected this session.
    pub async fn is_injected(&self, agent_id: &str, memory_id: &str) -> bool {
        match self.sessions.get(agent_id).await {
            Some(session) => session.lock().await.injected_ids.contains(memory_id),
            None => false,
        }
    }

    /// Record memory IDs as injected for this session.
    pub async fn mark_injected(&self, agent_id: &str, ids: &[String]) {
        let session = self.get_or_create(agent_id).await;
        let mut guard = session.lock().await;
        for id in ids {
            guard.injected_ids.insert(id.clone());
        }
    }

    /// Filter out already-injected memory IDs, returning only new ones.
    pub async fn filter_new<'a>(&self, agent_id: &str, ids: &'a [String]) -> Vec<&'a String> {
        match self.sessions.get(agent_id).await {
            Some(session) => {
                let guard = session.lock().await;
                ids.iter()
                    .filter(|id| !guard.injected_ids.contains(*id))
                    .collect()
            }
            None => ids.iter().collect(),
        }
    }

    /// Update context token count and detect compaction.
    /// Returns true if compaction was detected (session dedup should reset).
    pub async fn update_context_tokens(&self, agent_id: &str, current_tokens: u64) -> bool {
        let session = self.get_or_create(agent_id).await;
        let mut guard = session.lock().await;

        let previous = guard.last_context_tokens;
        guard.last_context_tokens = current_tokens;

        // Detect compaction: context dropped by more than threshold
        if previous > 0 && current_tokens > 0 {
            let drop_ratio = 1.0 - (current_tokens as f64 / previous as f64);
            if drop_ratio >= self.compaction_drop_threshold {
                tracing::info!(
                    agent_id,
                    previous_tokens = previous,
                    current_tokens = current_tokens,
                    drop_pct = format!("{:.0}%", drop_ratio * 100.0),
                    "Compaction detected — resetting session dedup"
                );
                guard.injected_ids.clear();
                return true;
            }
        }

        false
    }

    /// Explicitly reset session state for an agent (e.g., on /clear).
    pub async fn reset(&self, agent_id: &str) {
        self.sessions.remove(agent_id).await;
    }

    /// Get count of injected memories for an agent (for diagnostics).
    pub async fn injected_count(&self, agent_id: &str) -> usize {
        match self.sessions.get(agent_id).await {
            Some(session) => session.lock().await.injected_ids.len(),
            None => 0,
        }
    }
}
