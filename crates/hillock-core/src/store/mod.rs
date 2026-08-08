//! Storage trait — the persistence seam.
//!
//! SQLite is the source of truth for all memory data. HNSW is a derived index.
//! Implementations of this trait handle metadata, content, and vector persistence.

pub mod sqlite;

use async_trait::async_trait;

use crate::model::*;
use uuid::Uuid;

/// Errors that can occur during storage operations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("memory not found: {0}")]
    NotFound(Uuid),

    #[error("cannot forget individual chunk — forget the parent instead")]
    ChunkForgetRejected,

    #[error("agent {agent} holds no key for space {space}")]
    NotAuthorized { agent: String, space: String },

    #[error("cannot share individual chunk — share the parent instead")]
    ChunkShareRejected,

    #[error("storage error: {0}")]
    Internal(#[from] anyhow::Error),
}

/// The persistence layer for memories, spaces, and key rings.
///
/// SQLite is the V1 implementation. This trait exists for testability
/// and future swapability (e.g., Qdrant), not for premature abstraction.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    // ── Memory CRUD ──

    /// Store a memory. Commits transactionally.
    async fn insert_memory(&self, memory: &Memory) -> Result<(), StoreError>;

    /// Retrieve a memory by ID.
    async fn get_memory(&self, id: Uuid) -> Result<Memory, StoreError>;

    /// Get all chunks belonging to a parent memory, ordered by chunk_index.
    async fn get_chunks(&self, parent_id: Uuid) -> Result<Vec<Memory>, StoreError>;

    /// Get the parent memory for a chunk.
    async fn get_parent(&self, chunk_id: Uuid) -> Result<Memory, StoreError>;

    /// Delete a standalone memory by ID.
    async fn delete_memory(&self, id: Uuid) -> Result<(), StoreError>;

    /// Delete a parent and all its chunks. Returns count of deleted memories.
    async fn delete_parent_and_chunks(&self, parent_id: Uuid) -> Result<u32, StoreError>;

    /// Check if a memory is a chunk (has parent_id set).
    async fn is_chunk(&self, id: Uuid) -> Result<bool, StoreError>;

    /// Check if a memory is a parent (has chunks pointing to it).
    async fn is_parent(&self, id: Uuid) -> Result<bool, StoreError>;

    /// Update belief_state for a list of memory IDs (for supersedes).
    async fn update_belief_state(&self, ids: &[Uuid], state: BeliefState)
        -> Result<(), StoreError>;

    /// Update importance for a single memory (feedback signal).
    async fn update_importance(&self, id: Uuid, importance: f32) -> Result<(), StoreError>;

    // ── Queries ──

    /// Get all memory IDs + embeddings in a space, for HNSW rebuild.
    async fn all_vectors_in_space(
        &self,
        space_id: &str,
    ) -> Result<Vec<(Uuid, Vec<f32>)>, StoreError>;

    /// Count memories in a space.
    async fn count_memories(&self, space_id: &str) -> Result<u64, StoreError>;

    /// Count tombstoned (deleted but not yet cleaned) memories.
    async fn count_tombstones(&self) -> Result<u64, StoreError>;

    /// Get growth data points for the last N days.
    async fn growth_history(&self, days: u32) -> Result<Vec<GrowthPoint>, StoreError>;

    // ── Spaces ──

    /// Create a space.
    async fn create_space(&self, space: &Space) -> Result<(), StoreError>;

    /// Get a space by ID.
    async fn get_space(&self, id: &str) -> Result<Space, StoreError>;

    /// List all spaces.
    async fn list_spaces(&self) -> Result<Vec<Space>, StoreError>;

    // ── KeyRing ──

    /// Get all key ring entries for an agent.
    async fn get_key_ring(&self, agent_id: &str) -> Result<Vec<KeyRingEntry>, StoreError>;

    /// Grant an agent access to a space.
    async fn grant_access(&self, entry: &KeyRingEntry) -> Result<(), StoreError>;

    /// Revoke an agent's access to a space.
    async fn revoke_access(&self, agent_id: &str, space_id: &str) -> Result<(), StoreError>;

    /// Clear supersession on every superseded memory, optionally scoped to one space.
    ///
    /// Returns how many were restored. Supersession used to start a deletion timer and had
    /// no reverse until 2026-07-26 — the only way back was hand-written SQL against the live
    /// database. The timer is gone too; superseded now means hidden and kept.
    async fn restore_superseded(&self, space_id: Option<&str>) -> Result<u32, StoreError>;

    /// Count superseded memories without changing them. Backs `--dry-run`.
    async fn count_superseded(&self, space_id: Option<&str>) -> Result<u32, StoreError>;

    /// Every agent holding a key to `space_id`, read from the key ring itself.
    ///
    /// Deliberately not derived from the space list: an agent can hold a grant without
    /// owning an `agent:<id>` space (`agent:demo_cloud` has memories and no space row), and
    /// enumerating access from anywhere but the access table is how a revoke silently
    /// misses someone.
    async fn agents_with_access(&self, space_id: &str) -> Result<Vec<String>, StoreError>;

    // ── Filter ──

    /// Query memories by metadata (no embeddings). Returns non-chunk memories only.
    async fn filter_memories(
        &self,
        space_ids: &[String],
        filter: &FilterInput,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<FilterResult>, u64), StoreError>;

    // ── Pinning ──

    /// Pin a memory for an agent.
    async fn pin_memory(&self, agent_id: &str, memory_id: Uuid) -> Result<(), StoreError>;

    /// Unpin a memory for an agent.
    async fn unpin_memory(&self, agent_id: &str, memory_id: Uuid) -> Result<(), StoreError>;

    /// Get all pinned memory IDs for an agent.
    async fn get_pinned(&self, agent_id: &str) -> Result<Vec<Uuid>, StoreError>;

    /// Check if a memory is pinned by an agent.
    async fn is_pinned(&self, agent_id: &str, memory_id: Uuid) -> Result<bool, StoreError>;

    // ── Maintenance ──

    /// Fetch decay candidates: non-pinned, non-chunk memories older than `min_age_days`
    /// with importance above `floor`. Paginated for memory efficiency.
    async fn decay_candidates(
        &self,
        space_ids: &[String],
        pinned_ids: &[Uuid],
        min_age_days: u32,
        importance_floor: f32,
        min_interval_hours: u32,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<DecayCandidate>, StoreError>;

    /// Batch update importance values after decay computation.
    ///
    /// Also stamps `last_decayed_at`, which is what makes decay idempotent within an
    /// interval. Decay is the only caller; if that changes, the stamp must move.
    async fn batch_update_importance(&self, updates: &[(Uuid, f32)]) -> Result<u32, StoreError>;

    /// Fetch prune candidates: non-pinned memories with importance strictly below threshold
    /// and created_at older than min_age_days. Returns IDs for deletion.
    ///
    /// The comparison is strict so that a memory clamped to `decay_floor` is never a
    /// prune candidate when `decay_floor == prune_threshold` (the default). Decay must
    /// not be able to walk a memory into deletion.
    async fn prune_candidates(
        &self,
        space_ids: &[String],
        pinned_ids: &[Uuid],
        importance_threshold: f32,
        min_age_days: u32,
        limit: usize,
    ) -> Result<Vec<Uuid>, StoreError>;

    /// Find a live, non-chunk memory in `space_id` with this exact `content_hash`.
    ///
    /// Backs hash dedup on write. `content_hash` was computed and stored from the beginning
    /// and never read, which is how 42 exact-duplicate rows accumulated.
    ///
    /// Scoped to the space deliberately: the same text remembered by two agents is two
    /// memories, not one, because each carries its own provenance and rotation.
    async fn find_by_content_hash(
        &self,
        space_id: &str,
        hash: &[u8; 32],
    ) -> Result<Option<Uuid>, StoreError>;

    /// Write a consistent snapshot of the whole store to `dest`.
    ///
    /// `dest` must not already exist. Used by the nightly pipeline to guarantee a backup
    /// exists before anything destructive runs, on every entry point rather than only the
    /// scheduled one.
    async fn backup_to(&self, dest: &std::path::Path) -> Result<(), StoreError>;

    /// Write a forget log entry (lightweight audit trail for deleted memories).
    async fn insert_forget_log(&self, entry: &ForgetLogEntry) -> Result<(), StoreError>;

    /// Read back the most recent forget log entries, newest first.
    ///
    /// The log was write-only until 2026-07-26: nothing in the codebase could read it, so
    /// "174 rows in `forget_log`" was a claim only raw SQL could check. An audit trail
    /// that its own system cannot inspect is not an audit trail.
    async fn list_forget_log(&self, limit: usize) -> Result<Vec<ForgetLogEntry>, StoreError>;

    // ── Contradiction Detection ──

    // ── Nightly Run Tracking ──

    /// Get the last nightly run time for an agent. Returns None if never run.
    async fn last_nightly_run(
        &self,
        agent_id: &str,
    ) -> Result<Option<NightlyRunRecord>, StoreError>;

    /// Record a completed nightly run.
    async fn record_nightly_run(&self, record: &NightlyRunRecord) -> Result<(), StoreError>;

    // ── Full-text search ──

    /// FTS5 search across the given spaces. Returns results ordered by BM25 relevance.
    /// Default implementation returns empty (for stores that don't support FTS).
    async fn fts_search(
        &self,
        _query: &str,
        _space_ids: &[String],
        _limit: usize,
    ) -> Result<Vec<FtsResult>, StoreError> {
        Ok(Vec::new())
    }

    /// Get the stored embedding for a memory. Used by hybrid search to compute
    /// cosine similarity for FTS-injected candidates.
    async fn get_embedding(&self, _id: Uuid) -> Result<Option<Vec<f32>>, StoreError> {
        Ok(None)
    }
}
