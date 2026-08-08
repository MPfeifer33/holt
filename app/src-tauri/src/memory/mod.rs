pub mod compaction;
pub mod embeddings;
pub mod hillock_engine;
pub mod injection;
pub mod session;
pub mod store;
pub mod types;

use self::embeddings::EmbeddingService;
use self::hillock_engine::EmbeddedHillock;
use self::types::{HillockMemory, HillockRetrieveResult, HillockStoreResult};
use crate::config::app_config::MemoryBlock;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("Hillock error: {0}")]
    Database(String),
    #[error("Tool error: {0}")]
    ToolError(String),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Embedding error: {0}")]
    Embedding(String),
    #[error("Memory not found: {0}")]
    NotFound(String),
    #[error("Memory system not available")]
    Unavailable,
    #[error("Pin cap reached: {0}")]
    PinCapReached(String),
    #[error("Memory too large to pin: {0}")]
    PinTooLarge(String),
}

pub struct MemoryEngine {
    hillock: EmbeddedHillock,
    pub(crate) embedder: Arc<EmbeddingService>,
    pub(crate) config: MemoryBlock,
}

impl MemoryEngine {
    /// Initialize the memory engine with embedded Hillock.
    pub async fn try_init(config: &MemoryBlock) -> Option<Self> {
        if !config.enabled {
            tracing::info!("Memory system disabled by config");
            return None;
        }

        let embedder = match EmbeddingService::new() {
            Ok(svc) => Arc::new(svc),
            Err(e) => {
                tracing::warn!("Memory system unavailable — embedding service failed: {e}");
                return None;
            }
        };

        // Resolve database path
        let db_path = if !config.hillock_db_path.is_empty() {
            PathBuf::from(&config.hillock_db_path)
        } else {
            // XDG fallback, used ONLY when hillock_db_path is empty:
            //   ~/.local/share/holt/hillock.db
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("holt")
                .join("hillock.db")
        };

        let master_key = if !config.hillock_master_key.is_empty() {
            config.hillock_master_key.clone()
        } else {
            "default-dev-key".to_string()
        };

        // Default namespace for system-level memory operations.
        let default_agent_id = "default";

        let namespace_aliases = config.memory_namespace_aliases.clone();

        match EmbeddedHillock::try_init(&db_path, default_agent_id, &master_key, namespace_aliases)
            .await
        {
            Ok(hillock) => {
                tracing::info!(
                    db = %db_path.display(),
                    agent = default_agent_id,
                    "Memory engine initialized (embedded Hillock)"
                );
                Some(Self {
                    hillock,
                    embedder,
                    config: config.clone(),
                })
            }
            Err(e) => {
                tracing::warn!("Memory system unavailable — Hillock init failed: {e}");
                None
            }
        }
    }

    /// Store a memory.
    pub async fn store(&self, input: store::StoreInput) -> Result<String, MemoryError> {
        self.hillock
            .store(
                &input.content,
                &input.category,
                input.importance,
                &input.source,
                &input.agent_ns,
                &input.tags,
            )
            .await
    }

    /// Store a memory and return the detailed ingest result.
    pub async fn store_detailed(
        &self,
        input: store::StoreInput,
    ) -> Result<HillockStoreResult, MemoryError> {
        self.hillock
            .store_detailed(
                &input.content,
                &input.category,
                input.importance,
                &input.source,
                &input.agent_ns,
                &input.tags,
            )
            .await
    }

    /// Pack memories into a token budget for dynamic persona injection.
    pub async fn pack_context_for_persona(
        &self,
        agent_ns: &str,
        token_budget: usize,
    ) -> Result<serde_json::Value, MemoryError> {
        self.hillock
            .pack_context(agent_ns, token_budget, "context")
            .await
    }

    /// Seed a fixture memory directly into a namespace.
    pub async fn fixture_seed(
        &self,
        input: store::StoreInput,
    ) -> Result<HillockStoreResult, MemoryError> {
        self.hillock
            .fixture_seed(
                &input.content,
                &input.category,
                input.importance,
                &input.agent_ns,
                &input.tags,
            )
            .await
    }

    /// Retrieve memories by semantic search.
    ///
    /// `spaces` controls which memory spaces are searched. Defaults to
    /// `Private` (agent's own namespace only). Pass `Some(SpaceScope::All)`
    /// to include commons.
    pub async fn retrieve(
        &self,
        query: &str,
        limit: usize,
        agent_ns: &str,
        cluster_ids: Option<&[String]>,
        category: Option<&str>,
        recall_mode: Option<&str>,
        spaces: Option<hillock_core::model::SpaceScope>,
    ) -> Result<HillockRetrieveResult, MemoryError> {
        self.hillock
            .retrieve(
                query,
                limit,
                Some(agent_ns),
                cluster_ids,
                category,
                recall_mode,
                spaces,
            )
            .await
    }

    /// Get memories by tier, optionally scoped to a namespace.
    pub async fn get_by_tier(
        &self,
        tier: &str,
        limit: usize,
        agent_ns: Option<&str>,
    ) -> Result<Vec<HillockMemory>, MemoryError> {
        self.hillock.get_by_tier(tier, limit, agent_ns).await
    }

    /// Export memories by metadata without semantic ranking.
    pub async fn export(
        &self,
        category: Option<&str>,
        tier: Option<&str>,
        agent_ns: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<HillockMemory>, MemoryError> {
        self.hillock.export(category, tier, agent_ns, limit).await
    }

    /// Get hot-tier memories for an agent.
    pub async fn get_hot_memories(
        &self,
        agent_ns: &str,
    ) -> Result<Vec<HillockMemory>, MemoryError> {
        self.hillock.get_by_tier("hot", 50, Some(agent_ns)).await
    }

    /// Delete a memory.
    pub async fn delete(&self, id: &str, agent_ns: &str) -> Result<(), MemoryError> {
        self.hillock.forget(id, agent_ns).await
    }

    /// Query memories by metadata filters (no embedding search).
    pub async fn query_by_metadata(
        &self,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, MemoryError> {
        self.hillock.query_by_metadata(args).await
    }

    /// Query memories by metadata filters and parse them into HillockMemory rows.
    pub async fn query_memories_by_metadata(
        &self,
        args: serde_json::Value,
    ) -> Result<Vec<HillockMemory>, MemoryError> {
        self.hillock.query_memories_by_metadata(args).await
    }

    /// Set tier on a memory.
    pub async fn set_tier(&self, id: &str, _agent_ns: &str, tier: &str) -> Result<(), MemoryError> {
        self.hillock.set_tier(id, tier, None).await
    }

    /// Promote a memory to hot tier.
    pub async fn promote(&self, id: &str) -> Result<(), MemoryError> {
        self.hillock.promote(id).await
    }

    /// Pin a memory: validate cap (max 5) and token size (~1K), promote to hot, set pinned=true.
    pub async fn pin(&self, id: &str, agent_ns: &str) -> Result<(), MemoryError> {
        // Check pinned cap
        let hot = self.hillock.get_by_tier("hot", 50, Some(agent_ns)).await?;
        let pinned_count = hot.iter().filter(|m| m.pinned).count();
        if pinned_count >= 5 {
            return Err(MemoryError::PinCapReached(
                "Maximum 5 pinned memories. Unpin one first.".into(),
            ));
        }

        // Check token size — search hot tier first, then warm/cold
        let memory = hot.iter().find(|m| m.id == id);
        let mem_to_check = if let Some(m) = memory {
            Some(m.clone())
        } else {
            let warm = self
                .hillock
                .get_by_tier("warm", 100, Some(agent_ns))
                .await
                .unwrap_or_default();
            warm.into_iter().find(|m| m.id == id).or_else(|| {
                tracing::debug!(id, "Memory not found in hot/warm for pin token validation");
                None
            })
        };

        if let Some(mem) = mem_to_check {
            let token_count = embeddings::count_tokens(&mem.content);
            if token_count > 1024 {
                return Err(MemoryError::PinTooLarge(format!(
                    "Memory is {} tokens. Pinned memories must be under 1024 tokens.",
                    token_count
                )));
            }
        }

        // Promote to hot + set pinned flag
        self.hillock.set_tier(id, "hot", Some(true)).await
    }

    /// Unpin a memory: clear pinned flag and demote to warm.
    pub async fn unpin(&self, id: &str) -> Result<(), MemoryError> {
        self.hillock.set_tier(id, "warm", Some(false)).await
    }

    /// Send feedback on a memory.
    pub async fn feedback(
        &self,
        memory_id: &str,
        signal: &str,
        source: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        self.hillock.feedback(memory_id, signal, source).await
    }

    /// Crystallize episodes into a concept.
    pub async fn bridge_crystallize(
        &self,
        summary: &str,
        source_memory_ids: &[String],
        source: &str,
        tags: &[String],
    ) -> Result<serde_json::Value, MemoryError> {
        self.hillock
            .crystallize(summary, source_memory_ids, source, tags)
            .await
    }

    /// Trigger nightly maintenance.
    pub async fn run_nightly(
        &self,
        agent_ns: Option<&str>,
        dry_run: bool,
    ) -> Result<serde_json::Value, MemoryError> {
        self.hillock.run_nightly(agent_ns, dry_run).await
    }

    /// Generate the agent namespace string.
    pub fn agent_namespace(agent_id: &str) -> String {
        format!("holt_{agent_id}")
    }

    /// Register a namespace for a new agent.
    /// Idempotent — safe to call if namespace already exists.
    pub async fn register_namespace(
        &self,
        agent_ns: &str,
        agent_name: &str,
    ) -> Result<(), MemoryError> {
        self.hillock
            .namespace_register(agent_ns, agent_name)
            .await?;
        Ok(())
    }

    /// Deactivate a namespace — marks all memories as orphaned.
    /// Called when an agent is deleted. Does NOT delete memories.
    pub async fn deactivate_namespace(&self, agent_ns: &str) -> Result<(), MemoryError> {
        self.hillock.namespace_deactivate(agent_ns).await?;
        Ok(())
    }

    /// Report a compaction event for pressure tracking.
    pub async fn compaction_report(
        &self,
        agent_id: &str,
        session_id: &str,
        event_type: &str,
        message_count: Option<usize>,
        tool_call_count: Option<u64>,
    ) -> Result<serde_json::Value, MemoryError> {
        self.hillock
            .compaction_report(
                agent_id,
                session_id,
                event_type,
                message_count,
                tool_call_count,
            )
            .await
    }

    /// Query current compaction pressure for an agent session.
    pub async fn compaction_pressure(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        self.hillock.compaction_pressure(agent_id, session_id).await
    }
}
