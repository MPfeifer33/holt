//! Engine — the dependency container.
//!
//! Holds the store, index, embedder, and rotation manager.
//! Wires tool calls to ops functions. Contains no business logic itself.

use std::sync::Arc;

use crate::embed::Embedder;
use crate::index::VectorIndex;
use crate::model::*;
use crate::ops;
use crate::rotation::RotationManager;
use crate::store::MemoryStore;

/// The engine wires dependencies to operations. That's it.
///
/// It doesn't contain business logic — each op function in `ops/` owns its pipeline.
/// The engine just knows who to hand the dependencies to.
pub struct Engine {
    store: Arc<dyn MemoryStore>,
    index: Arc<dyn VectorIndex>,
    embedder: Arc<dyn Embedder>,
    rotation: Arc<RotationManager>,
    scoring: ScoringConfig,
}

impl Engine {
    pub fn new(
        store: Arc<dyn MemoryStore>,
        index: Arc<dyn VectorIndex>,
        embedder: Arc<dyn Embedder>,
        rotation: Arc<RotationManager>,
        scoring: ScoringConfig,
    ) -> Self {
        Self {
            store,
            index,
            embedder,
            rotation,
            scoring,
        }
    }

    /// Store a memory. See `ops::remember` for the full pipeline.
    pub async fn remember(
        &self,
        agent_id: &str,
        input: RememberInput,
    ) -> Result<RememberOutput, anyhow::Error> {
        let space_id = format!("agent:{agent_id}");
        ops::remember::remember(
            &*self.store,
            &*self.index,
            &*self.embedder,
            &self.rotation,
            agent_id,
            &space_id,
            input,
        )
        .await
    }

    /// Search or fetch memories. See `ops::recall` for the full pipeline.
    pub async fn recall(
        &self,
        agent_id: &str,
        input: RecallInput,
    ) -> Result<RecallOutput, anyhow::Error> {
        ops::recall::recall(
            &*self.store,
            &*self.index,
            &*self.embedder,
            &self.rotation,
            agent_id,
            &self.scoring,
            input,
        )
        .await
    }

    /// Delete a memory. See `ops::forget` for cascade rules.
    pub async fn forget(
        &self,
        agent_id: &str,
        input: ForgetInput,
    ) -> Result<ForgetOutput, anyhow::Error> {
        ops::forget::forget(&*self.store, &*self.index, agent_id, input).await
    }

    /// Restore superseded memories to active, optionally scoped to one space.
    ///
    /// Memories are not moved or rewritten — only their belief state and supersession
    /// timestamp are cleared, so they return to normal recall and fall under decay like
    /// anything else. Reversible in the other direction by superseding again.
    pub async fn restore_superseded(&self, space_id: Option<&str>) -> Result<u32, anyhow::Error> {
        let n = self.store.restore_superseded(space_id).await?;
        tracing::info!(restored = n, space = ?space_id, "restored superseded memories");
        Ok(n)
    }

    /// Count superseded memories without changing them.
    pub async fn count_superseded(&self, space_id: Option<&str>) -> Result<u32, anyhow::Error> {
        Ok(self.store.count_superseded(space_id).await?)
    }

    /// Every agent holding a key to `space_id`. Read from the key ring, not the space list.
    pub async fn agents_with_access(&self, space_id: &str) -> Result<Vec<String>, anyhow::Error> {
        Ok(self.store.agents_with_access(space_id).await?)
    }

    /// Revoke every agent's key to a space. Returns how many grants were removed.
    ///
    /// Memories are untouched: the space becomes unreachable, not erased, so the decision
    /// stays reversible by re-granting.
    ///
    /// Exists because three code paths granted `commons` and none revoked it, so the
    /// 2026-07-25 decision to switch it off could only be applied by hand-editing the live
    /// key ring — and silently came undone when a missed copy re-granted it. A policy you
    /// can only enforce with ad-hoc SQL is a policy that drifts.
    pub async fn revoke_space(&self, space_id: &str) -> Result<u32, anyhow::Error> {
        let holders = self.store.agents_with_access(space_id).await?;
        for agent_id in &holders {
            self.store.revoke_access(agent_id, space_id).await?;
            tracing::info!(agent = %agent_id, space = %space_id, "revoked space access");
        }
        Ok(holders.len() as u32)
    }

    /// Read the deletion audit trail, newest first.
    ///
    /// Not scoped to an agent: the log records deletions across every space, and the point
    /// of it is to answer "what disappeared, and who asked" without knowing where to look.
    pub async fn forget_log(&self, limit: usize) -> Result<Vec<ForgetLogEntry>, anyhow::Error> {
        Ok(self.store.list_forget_log(limit).await?)
    }

    /// Share a memory to another space. See `ops::share` for modes.
    pub async fn share(
        &self,
        agent_id: &str,
        input: ShareInput,
    ) -> Result<ShareOutput, anyhow::Error> {
        ops::share::share(
            &*self.store,
            &*self.index,
            &*self.embedder,
            &self.rotation,
            agent_id,
            input,
        )
        .await
    }

    /// Filter memories by metadata. See `ops::filter`.
    pub async fn filter(
        &self,
        agent_id: &str,
        input: FilterInput,
    ) -> Result<FilterOutput, anyhow::Error> {
        ops::filter::filter(&*self.store, agent_id, input).await
    }

    /// Pin a memory. See `ops::pin`.
    pub async fn pin(
        &self,
        agent_id: &str,
        memory_id: uuid::Uuid,
    ) -> Result<PinOutput, anyhow::Error> {
        ops::pin::pin(&*self.store, agent_id, memory_id).await
    }

    /// Unpin a memory. See `ops::pin`.
    pub async fn unpin(
        &self,
        agent_id: &str,
        memory_id: uuid::Uuid,
    ) -> Result<UnpinOutput, anyhow::Error> {
        ops::pin::unpin(&*self.store, agent_id, memory_id).await
    }

    /// Pack context for injection. See `ops::pack_context`.
    pub async fn pack_context(
        &self,
        agent_id: &str,
        input: PackContextInput,
    ) -> Result<PackContextOutput, anyhow::Error> {
        ops::pack_context::pack_context(
            &*self.store,
            &*self.index,
            &*self.embedder,
            &self.rotation,
            agent_id,
            &self.scoring,
            input,
        )
        .await
    }

    /// Crystallize episodes into a concept. See `ops::crystallize`.
    pub async fn crystallize(
        &self,
        agent_id: &str,
        input: CrystallizeInput,
    ) -> Result<CrystallizeOutput, anyhow::Error> {
        let space_id = format!("agent:{agent_id}");
        ops::crystallize::crystallize(
            &*self.store,
            &*self.index,
            &*self.embedder,
            &self.rotation,
            agent_id,
            &space_id,
            input,
        )
        .await
    }

    /// Get diagnostics. See `ops::inspect`.
    pub async fn inspect(&self, agent_id: &str) -> Result<InspectOutput, anyhow::Error> {
        ops::inspect::inspect(&*self.store, &*self.index, agent_id).await
    }

    /// Run the maintenance pipeline (decay, prune, index GC).
    /// Triggered externally on a schedule — not agent-facing.
    pub async fn maintenance(
        &self,
        agent_id: &str,
        config: MaintenanceConfig,
    ) -> Result<MaintenanceOutput, anyhow::Error> {
        ops::maintenance::run(&*self.store, &*self.index, agent_id, config).await
    }

    /// Get a memory by ID. Passthrough to store.
    pub async fn get_memory(&self, id: uuid::Uuid) -> Result<Memory, anyhow::Error> {
        Ok(self.store.get_memory(id).await?)
    }

    /// Update importance for a single memory (feedback signal). Passthrough to store.
    pub async fn update_importance(
        &self,
        id: uuid::Uuid,
        importance: f32,
    ) -> Result<(), anyhow::Error> {
        Ok(self.store.update_importance(id, importance).await?)
    }

    /// Update belief state for memory IDs. Passthrough to store.
    pub async fn update_belief_state(
        &self,
        ids: &[uuid::Uuid],
        state: BeliefState,
    ) -> Result<(), anyhow::Error> {
        Ok(self.store.update_belief_state(ids, state).await?)
    }

    /// Every agent id that owns a private space, sorted.
    ///
    /// Spaces are named `agent:<id>`; `commons` and anything else is skipped. Used by
    /// `nightly --all-agents`, because the scheduled unit sets no `HILLOCK_AGENT_ID` and
    /// so maintains only the CLI default while every other space stays frozen.
    pub async fn list_agent_ids(&self) -> Result<Vec<String>, anyhow::Error> {
        let mut ids: Vec<String> = self
            .store
            .list_spaces()
            .await?
            .into_iter()
            .filter_map(|s| s.id.strip_prefix("agent:").map(str::to_string))
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// Run the nightly pipeline: snapshot, then maintenance (decay, prune, index GC).
    /// Persists run state in SQLite.
    ///
    /// Decay is idempotent within `MaintenanceConfig::decay_min_interval_hours` (per
    /// memory, via `last_decayed_at`). It previously was not, despite this doc claiming
    /// otherwise: decay is multiplicative, so a same-day rerun applied `0.98²`.
    pub async fn nightly(
        &self,
        agent_id: &str,
        config: NightlyConfig,
    ) -> Result<NightlyOutput, anyhow::Error> {
        ops::nightly::run(&*self.store, &*self.index, agent_id, config).await
    }

    /// Check if a nightly run is overdue (last run > max_gap_hours ago, or never run).
    pub async fn is_nightly_overdue(
        &self,
        agent_id: &str,
        max_gap_hours: u32,
    ) -> Result<bool, anyhow::Error> {
        ops::nightly::is_overdue(&*self.store, agent_id, max_gap_hours).await
    }

    /// Register a new agent: create private space, grant access, load rotation.
    pub async fn register_agent(
        &self,
        agent_id: &str,
        master_key: &str,
    ) -> Result<String, anyhow::Error> {
        let now = chrono::Utc::now();
        let private_space_id = format!("agent:{agent_id}");
        let seed = RotationManager::derive_private_seed(master_key.as_bytes(), agent_id)?;

        // Create space if it doesn't exist
        if self.store.get_space(&private_space_id).await.is_err() {
            let space = Space {
                id: private_space_id.clone(),
                rotation_kind: RotationKind::Derived,
                rotation_seed: Some(seed),
                created_at: now,
                memory_count: 0,
            };
            self.store.create_space(&space).await?;
        }
        self.rotation.load_space(&private_space_id, &seed)?;

        // Grant access if not already granted
        let key_ring = self.store.get_key_ring(agent_id).await?;
        if !key_ring.iter().any(|k| k.space_id == private_space_id) {
            self.store
                .grant_access(&KeyRingEntry {
                    agent_id: agent_id.to_string(),
                    space_id: private_space_id.clone(),
                    granted_at: now,
                    granted_by: "namespace_register".to_string(),
                })
                .await?;
        }
        // `commons` is deliberately not auto-granted.
        //
        // Shared spaces can be useful, but granting one to every new agent makes all
        // agents carry memory they may never read. Nothing is deleted: the space and
        // its rows remain, simply unreachable, and re-granting is one `grant_access`
        // call if the role is wanted for a deployment.

        // Build empty index for the new space
        self.index.build_space(&private_space_id, vec![]).await?;

        Ok(private_space_id)
    }

    /// Deactivate an agent: revoke access, mark memories as orphaned.
    /// Does NOT delete memories — they remain for potential recovery.
    pub async fn deactivate_agent(&self, agent_id: &str) -> Result<u64, anyhow::Error> {
        let private_space_id = format!("agent:{agent_id}");

        // Revoke access
        self.store
            .revoke_access(agent_id, &private_space_id)
            .await?;
        self.store.revoke_access(agent_id, "commons").await?;

        // Count memories in the space (they're now orphaned)
        let count = self.store.count_memories(&private_space_id).await?;

        Ok(count)
    }

    /// Rebuild HNSW indices from SQLite (source of truth).
    /// Called on startup and when an index is marked dirty.
    pub async fn rebuild_indices(&self) -> Result<(), anyhow::Error> {
        let spaces = self.store.list_spaces().await?;
        for space in &spaces {
            let vectors = self.store.all_vectors_in_space(&space.id).await?;
            tracing::info!(
                space = %space.id,
                vectors = vectors.len(),
                // Not HNSW -- `MemoryIndex::search` is a linear cosine scan over every
                // vector in the space. Keep this label honest; the old one cost a
                // code-verification pass to disprove.
                "rebuilding vector index"
            );
            self.index.build_space(&space.id, vectors).await?;
        }
        Ok(())
    }
}
