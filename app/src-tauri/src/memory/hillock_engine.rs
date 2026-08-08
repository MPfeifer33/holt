//! Embedded Hillock engine — direct function calls to hillock-core.
//!
//! Replaces the HTTP-based HillockClient with an in-process Engine.
//! Same public API surface so callers don't need to change.

use super::store::{MemoryCategory, MemorySource};
use super::types::{HillockMemory, HillockRetrieveResult, HillockStoreResult};
use super::MemoryError;

use hillock_core::embed::fastembed_embedder::FastEmbedder;
use hillock_core::embed::Embedder as _;
use hillock_core::engine::Engine;
use hillock_core::index::memory_index::MemoryIndex;
use hillock_core::model::*;
use hillock_core::rotation::RotationManager;
use hillock_core::store::sqlite::SqliteStore;
use hillock_core::store::MemoryStore;

use chrono::Utc;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

/// Embedded Hillock engine. Wraps hillock-core::Engine with the same
/// method signatures as HillockClient for drop-in replacement.
pub struct EmbeddedHillock {
    engine: Engine,
    store: Arc<SqliteStore>,
    rotation: Arc<RotationManager>,
    default_agent_id: String,
    master_key: String,
    namespace_aliases: std::collections::HashMap<String, String>,
    /// Where the nightly pipeline writes its pre-run snapshot. Derived from the database
    /// location so a backup always lands beside the data it protects.
    backup_dir: std::path::PathBuf,
}

// ---------------------------------------------------------------------------
// Category / Kind translation
// ---------------------------------------------------------------------------

fn category_to_kind(cat: &MemoryCategory) -> MemoryKind {
    match cat {
        MemoryCategory::Episode => MemoryKind::Observation,
        MemoryCategory::Concept => MemoryKind::Fact,
        MemoryCategory::Policy => MemoryKind::Constraint,
        MemoryCategory::Identity => MemoryKind::Preference,
    }
}

/// Map a category string from a tool call to the kind it filters on.
///
/// Mirrors [`category_to_kind`] exactly, so a category means the same thing whichever tool
/// the caller reaches for. Kept in step by
/// `every_advertised_category_survives_a_round_trip`.
fn category_str_to_kind(category: &str) -> Option<MemoryKind> {
    match category {
        "episode" => Some(MemoryKind::Observation),
        "concept" => Some(MemoryKind::Fact),
        "policy" => Some(MemoryKind::Constraint),
        "identity" => Some(MemoryKind::Preference),
        _ => None,
    }
}

fn kind_to_category(kind: &MemoryKind) -> String {
    match kind {
        MemoryKind::Observation => "episode".to_string(),
        MemoryKind::Fact => "concept".to_string(),
        MemoryKind::Decision => "concept".to_string(),
        MemoryKind::Preference => "identity".to_string(),
        MemoryKind::Constraint => "policy".to_string(),
        MemoryKind::Procedure => "policy".to_string(),
    }
}

fn source_to_hillock(src: &MemorySource) -> hillock_core::model::MemorySource {
    match src {
        MemorySource::HumanStated => hillock_core::model::MemorySource::Human,
        MemorySource::AgentExplicit => hillock_core::model::MemorySource::Agent,
        MemorySource::CognitiveIntake => hillock_core::model::MemorySource::Agent,
        MemorySource::AutoCaptured => hillock_core::model::MemorySource::System,
    }
}

// ---------------------------------------------------------------------------
// Result conversion
// ---------------------------------------------------------------------------

fn recall_result_to_hillock_memory(r: &RecallResult) -> HillockMemory {
    HillockMemory {
        id: r.memory_id.to_string(),
        content: r.content.clone(),
        category: kind_to_category(&r.metadata.kind),
        tier: "warm".to_string(), // Hillock has no tiers
        importance: r.metadata.importance.unwrap_or(0.5) as f64,
        score: r.score as f64,
        tags: r.metadata.tags.clone(),
        source: r.metadata.source_agent.clone().unwrap_or_default(),
        source_type: Some(format!("{:?}", r.metadata.source)),
        created_at: r.metadata.created_at.to_rfc3339(),
        pinned: false, // Not available in RecallResult directly
    }
}

fn filter_result_to_hillock_memory(r: &FilterResult) -> HillockMemory {
    HillockMemory {
        id: r.memory_id.to_string(),
        content: r.content.clone(),
        category: kind_to_category(&r.kind),
        tier: if r.pinned { "hot" } else { "warm" }.to_string(),
        importance: r.importance.unwrap_or(0.5) as f64,
        score: 1.0,
        tags: r.tags.clone(),
        source: String::new(),
        source_type: Some(format!("{:?}", r.source)),
        created_at: r.created_at.to_rfc3339(),
        pinned: r.pinned,
    }
}

// ---------------------------------------------------------------------------
// Bootstrap (same logic as hillock-server)
// ---------------------------------------------------------------------------

async fn bootstrap_agent(
    store: &dyn MemoryStore,
    rotation: &RotationManager,
    agent_id: &str,
    master_key: &str,
) -> Result<(), MemoryError> {
    let now = Utc::now();

    // Ensure commons space exists (identity rotation)
    if store.get_space("commons").await.is_err() {
        tracing::info!("creating commons space");
        let commons = Space {
            id: "commons".to_string(),
            rotation_kind: RotationKind::Identity,
            rotation_seed: None,
            created_at: now,
            memory_count: 0,
        };
        store
            .create_space(&commons)
            .await
            .map_err(|e| MemoryError::Database(format!("Failed to create commons space: {e}")))?;
    }
    rotation
        .load_identity_space("commons")
        .map_err(|e| MemoryError::Database(format!("Failed to load commons rotation: {e}")))?;

    // Ensure private space for this agent
    let private_space_id = format!("agent:{agent_id}");
    let seed = RotationManager::derive_private_seed(master_key.as_bytes(), agent_id)
        .map_err(|e| MemoryError::Database(format!("Failed to derive rotation seed: {e}")))?;

    if store.get_space(&private_space_id).await.is_err() {
        tracing::info!(agent = agent_id, "creating private space");
        let private = Space {
            id: private_space_id.clone(),
            rotation_kind: RotationKind::Derived,
            rotation_seed: Some(seed),
            created_at: now,
            memory_count: 0,
        };
        store
            .create_space(&private)
            .await
            .map_err(|e| MemoryError::Database(format!("Failed to create private space: {e}")))?;
    }
    rotation
        .load_space(&private_space_id, &seed)
        .map_err(|e| MemoryError::Database(format!("Failed to load space rotation: {e}")))?;

    // Ensure key ring entries
    let key_ring = store
        .get_key_ring(agent_id)
        .await
        .map_err(|e| MemoryError::Database(format!("Failed to get key ring: {e}")))?;
    let has_private = key_ring.iter().any(|k| k.space_id == private_space_id);
    let has_commons = key_ring.iter().any(|k| k.space_id == "commons");

    if !has_private {
        store
            .grant_access(&KeyRingEntry {
                agent_id: agent_id.to_string(),
                space_id: private_space_id,
                granted_at: now,
                granted_by: "bootstrap".to_string(),
            })
            .await
            .map_err(|e| MemoryError::Database(format!("Failed to grant private access: {e}")))?;
    }

    // `commons` is deliberately NOT granted (2026-07-25). It held 6 memories after five
    // months, all recorded elsewhere, and the knowledge base took over the shared-knowledge
    // role — so granting it made every agent carry a space it would never read.
    //
    // This bootstrap is one of THREE copies of the grant logic: hillock-core's
    // `Engine::register_agent`, this one, and `bootstrap_agent` in hillock-cli. Removing it
    // from core alone was not enough — this path silently re-granted commons on the next app
    // start. Fixing two of the three was not enough either: the CLI copy went on re-granting
    // until 2026-07-26, by which point the live key ring held three grants stamped
    // `granted_by: hillock-cli` against a plan doc asserting zero.
    //
    // If commons is ever wanted back, ALL THREE have to change. (This comment previously
    // said "both", and was itself wrong — the inventory is the thing that drifts.)
    let _ = has_commons;

    // Load rotation keys for any other accessible spaces
    for entry in &key_ring {
        if entry.space_id != "commons" && entry.space_id != format!("agent:{agent_id}") {
            if let Ok(space) = store.get_space(&entry.space_id).await {
                if let Some(space_seed) = &space.rotation_seed {
                    let _ = rotation.load_space(&entry.space_id, space_seed);
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Namespace resolution
// ---------------------------------------------------------------------------

/// Resolve agent namespace to the Hillock agent_id: translation, then alias.
/// Holt uses "holt_foo" for app-scoped names and Hillock uses "foo".
/// Free function for testability.
fn resolve_with_aliases(
    agent_ns: &str,
    aliases: &std::collections::HashMap<String, String>,
) -> String {
    let translated = if let Some(stripped) = agent_ns.strip_prefix("holt_") {
        stripped.to_string()
    } else {
        agent_ns.to_string()
    };
    aliases.get(&translated).cloned().unwrap_or(translated)
}

/// Lane provenance, derived from the RAW identity BEFORE alias collapse.
/// Public Holt has no built-in multi-chair identity; projects can add aliases
/// without receiving app-authored lane tags.
fn lane_tag_with_aliases(
    agent_ns: &str,
    aliases: &std::collections::HashMap<String, String>,
) -> Option<&'static str> {
    let translated = if let Some(stripped) = agent_ns.strip_prefix("holt_") {
        stripped.to_string()
    } else {
        agent_ns.to_string()
    };
    let _canonical = aliases.get(&translated).cloned().unwrap_or(translated);
    None
}

// ---------------------------------------------------------------------------
// EmbeddedHillock implementation
// ---------------------------------------------------------------------------

impl EmbeddedHillock {
    /// Initialize the embedded engine. Creates/opens the SQLite database,
    /// loads the embedding model, bootstraps the default agent, and rebuilds indices.
    pub async fn try_init(
        db_path: &Path,
        agent_id: &str,
        master_key: &str,
        aliases: std::collections::HashMap<String, String>,
    ) -> Result<Self, MemoryError> {
        tracing::info!("initializing embedded Hillock engine");

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MemoryError::Database(format!("Failed to create db directory: {e}"))
            })?;
        }

        // Initialize components
        tracing::info!("loading embedding model...");
        let embedder = Arc::new(FastEmbedder::new_bge_base().map_err(|e| {
            MemoryError::Database(format!("Failed to initialize embedding model: {e}"))
        })?);
        let dim = embedder.dimension();
        tracing::info!(
            dimension = dim,
            model = embedder.model_name(),
            "embedder ready"
        );

        let store = Arc::new(
            SqliteStore::open(db_path)
                .map_err(|e| MemoryError::Database(format!("Failed to open database: {e}")))?,
        );
        let index = Arc::new(MemoryIndex::new(dim));
        let rotation = Arc::new(RotationManager::new(dim));

        // Bootstrap default agent
        bootstrap_agent(&*store, &rotation, agent_id, master_key).await?;

        // Bootstrap all other agents whose spaces already exist in the database.
        // Without this, non-default agents (e.g. Codex lane) lose their rotation
        // keys on restart since only the default agent is bootstrapped above.
        let all_spaces = store.list_spaces().await.unwrap_or_default();
        for space in &all_spaces {
            if space.id == "commons" || space.id == format!("agent:{agent_id}") {
                continue; // Already loaded above
            }
            if let Some(ref seed) = space.rotation_seed {
                if let Err(e) = rotation.load_space(&space.id, seed) {
                    tracing::warn!(space = %space.id, error = %e, "failed to load rotation for existing space");
                }
            } else if space.rotation_kind == RotationKind::Identity {
                let _ = rotation.load_identity_space(&space.id);
            }
        }

        // Build engine
        let engine = Engine::new(
            store.clone(),
            index.clone(),
            embedder,
            rotation.clone(),
            ScoringConfig::default(),
        );

        // Rebuild indices from SQLite
        tracing::info!("rebuilding vector indices...");
        engine
            .rebuild_indices()
            .await
            .map_err(|e| MemoryError::Database(format!("Failed to rebuild indices: {e}")))?;
        tracing::info!("indices ready");

        Ok(Self {
            engine,
            store,
            rotation,
            default_agent_id: agent_id.to_string(),
            master_key: master_key.to_string(),
            namespace_aliases: aliases,
            backup_dir: db_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("backups"),
        })
    }

    /// Resolve agent namespace to the Hillock agent_id.
    /// Holt uses "holt_foo" for app-scoped names and Hillock uses "foo".
    /// Alias-aware: consults `namespace_aliases` after translation.
    fn resolve_agent_id(&self, agent_ns: &str) -> String {
        resolve_with_aliases(agent_ns, &self.namespace_aliases)
    }

    /// Lane provenance tag for an agent namespace, derived from the raw
    /// identity before alias collapse. `None` for single-chair agents.
    fn lane_tag_for(&self, agent_ns: &str) -> Option<&'static str> {
        lane_tag_with_aliases(agent_ns, &self.namespace_aliases)
    }

    // ── Health ────────────────────────────────────────────────────────

    /// Always returns true — embedded engine is always reachable.
    pub async fn health_check(&self) -> bool {
        true
    }

    // ── Store ────────────────────────────────────────────────────────

    /// Store a memory. Returns the memory ID.
    pub async fn store(
        &self,
        content: &str,
        category: &MemoryCategory,
        importance: f64,
        _source: &MemorySource,
        agent_ns: &str,
        tags: &[String],
    ) -> Result<String, MemoryError> {
        let result = self
            .store_detailed(content, category, importance, _source, agent_ns, tags)
            .await?;
        Ok(result.memory_id)
    }

    /// Store a memory and return the detailed result.
    pub async fn store_detailed(
        &self,
        content: &str,
        category: &MemoryCategory,
        importance: f64,
        source: &MemorySource,
        agent_ns: &str,
        tags: &[String],
    ) -> Result<HillockStoreResult, MemoryError> {
        let agent_id = self.resolve_agent_id(agent_ns);
        let mut tags = tags.to_vec();
        if let Some(lane) = self.lane_tag_for(agent_ns) {
            if !tags.iter().any(|t| t == lane) {
                tags.push(lane.to_string());
            }
        }
        let input = RememberInput {
            content: content.to_string(),
            metadata: RememberMetadata {
                kind: Some(category_to_kind(category)),
                source: Some(source_to_hillock(source)),
                tags,
                importance: Some(importance as f32),
                confidence: None,
            },
            supersedes: vec![],
            created_at: None,
        };

        let output = self
            .engine
            .remember(&agent_id, input)
            .await
            .map_err(|e| MemoryError::Database(format!("Remember failed: {e}")))?;

        Ok(HillockStoreResult {
            memory_id: output.memory_id.to_string(),
            status: "created".to_string(),
            message: "Memory stored successfully".to_string(),
            verification_status: "not_required".to_string(),
        })
    }

    /// Seed a fixture memory (bypasses dedup in the caller, but Engine still runs full pipeline).
    pub async fn fixture_seed(
        &self,
        content: &str,
        category: &MemoryCategory,
        importance: f64,
        agent_ns: &str,
        tags: &[String],
    ) -> Result<HillockStoreResult, MemoryError> {
        // fixture_seed uses the same remember path — Hillock doesn't have a separate fixture endpoint
        // when embedded. The server's fixture_seed was just a batch wrapper.
        self.store_detailed(
            content,
            category,
            importance,
            &MemorySource::AutoCaptured,
            agent_ns,
            tags,
        )
        .await
    }

    // ── Retrieve ─────────────────────────────────────────────────────

    /// Retrieve memories by semantic search.
    ///
    /// `spaces` controls which memory spaces are searched. Defaults to
    /// `Private` so agents only see their own namespace unless they
    /// explicitly opt in to commons or all spaces.
    pub async fn retrieve(
        &self,
        query: &str,
        limit: usize,
        source_filter: Option<&str>,
        _cluster_ids: Option<&[String]>,
        category: Option<&str>,
        _recall_mode: Option<&str>,
        spaces: Option<SpaceScope>,
    ) -> Result<HillockRetrieveResult, MemoryError> {
        let agent_id = source_filter
            .map(|s| self.resolve_agent_id(s))
            .unwrap_or_else(|| self.default_agent_id.clone());

        // `category` was previously accepted and discarded — the parameter was
        // underscore-prefixed, so nothing warned, and `recall(category: "concept")`
        // silently returned episodes. An unknown category yields no filter rather than
        // an empty result, matching how `memory_filter` treats one.
        let filter = category
            .and_then(category_str_to_kind)
            .map(|kind| RecallFilter {
                kind: Some(kind),
                ..Default::default()
            });

        let input = RecallInput {
            query: Some(query.to_string()),
            memory_id: None,
            limit,
            filter,
            spaces: spaces.unwrap_or(SpaceScope::Private),
            content_mode: ContentMode::Chunk,
            search_strategy: SearchStrategy::HybridBm25Inject,
        };

        let output = self
            .engine
            .recall(&agent_id, input)
            .await
            .map_err(|e| MemoryError::Database(format!("Recall failed: {e}")))?;

        let memories: Vec<HillockMemory> = output
            .results
            .iter()
            .map(recall_result_to_hillock_memory)
            .collect();
        let total_found = memories.len();

        Ok(HillockRetrieveResult {
            memories,
            total_found,
            near_misses: None,
            retrieval_confidence: None,
            temporal_distribution: None,
            result_mix: None,
            retrieval_interpretation: None,
            recommended_followup: None,
            highlights: None,
        })
    }

    // ── Filter / Query ───────────────────────────────────────────────

    /// Query memories by metadata (no embedding search).
    pub async fn query_by_metadata(
        &self,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, MemoryError> {
        let agent_id = args
            .get("source")
            .or_else(|| args.get("agent_id"))
            .and_then(|v| v.as_str())
            .map(|s| self.resolve_agent_id(s))
            .unwrap_or_else(|| self.default_agent_id.clone());

        let input = parse_filter_input(&args);

        let output = self
            .engine
            .filter(&agent_id, input)
            .await
            .map_err(|e| MemoryError::Database(format!("Filter failed: {e}")))?;

        let memories: Vec<serde_json::Value> = output
            .results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.memory_id.to_string(),
                    "content": r.content,
                    "category": kind_to_category(&r.kind),
                    "importance": r.importance.unwrap_or(0.5),
                    "tags": r.tags,
                    "created_at": r.created_at.to_rfc3339(),
                    "pinned": r.pinned,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "memories": memories,
            "total_count": output.total_count,
        }))
    }

    /// Query memories by metadata and return parsed HillockMemory rows.
    pub async fn query_memories_by_metadata(
        &self,
        args: serde_json::Value,
    ) -> Result<Vec<HillockMemory>, MemoryError> {
        let agent_id = args
            .get("source")
            .or_else(|| args.get("agent_id"))
            .and_then(|v| v.as_str())
            .map(|s| self.resolve_agent_id(s))
            .unwrap_or_else(|| self.default_agent_id.clone());

        let input = parse_filter_input(&args);

        let output = self
            .engine
            .filter(&agent_id, input)
            .await
            .map_err(|e| MemoryError::Database(format!("Filter failed: {e}")))?;

        Ok(output
            .results
            .iter()
            .map(filter_result_to_hillock_memory)
            .collect())
    }

    // ── Forget ───────────────────────────────────────────────────────

    /// Delete a memory on behalf of `agent_ns`, which is recorded in `forget_log`.
    ///
    /// The namespace used to be accepted and dropped one layer up (`delete`'s
    /// `_agent_ns`), so a deliberate deletion reached the engine with no actor attached.
    pub async fn forget(&self, memory_id: &str, agent_ns: &str) -> Result<(), MemoryError> {
        let id = parse_memory_id(memory_id)?;
        let agent_id = self.resolve_agent_id(agent_ns);
        self.engine
            .forget(&agent_id, ForgetInput { memory_id: id })
            .await
            .map_err(|e| MemoryError::Database(format!("Forget failed: {e}")))?;
        Ok(())
    }

    // ── Pin / Tier ───────────────────────────────────────────────────

    /// Pin a memory.
    pub async fn promote(&self, memory_id: &str) -> Result<(), MemoryError> {
        let id = parse_memory_id(memory_id)?;
        let agent_id = &self.default_agent_id;
        self.engine
            .pin(agent_id, id)
            .await
            .map_err(|e| MemoryError::Database(format!("Pin failed: {e}")))?;
        Ok(())
    }

    /// Pin or unpin a memory.
    pub async fn set_tier(
        &self,
        memory_id: &str,
        _tier: &str,
        pinned: Option<bool>,
    ) -> Result<(), MemoryError> {
        let id = parse_memory_id(memory_id)?;
        let agent_id = &self.default_agent_id;
        if pinned == Some(true) {
            self.engine
                .pin(agent_id, id)
                .await
                .map_err(|e| MemoryError::Database(format!("Pin failed: {e}")))?;
        } else {
            self.engine
                .unpin(agent_id, id)
                .await
                .map_err(|e| MemoryError::Database(format!("Unpin failed: {e}")))?;
        }
        Ok(())
    }

    /// Get memories by tier. Maps hot→pinned, warm→high importance, cold→low importance.
    pub async fn get_by_tier(
        &self,
        tier: &str,
        limit: usize,
        source_filter: Option<&str>,
    ) -> Result<Vec<HillockMemory>, MemoryError> {
        let agent_id = source_filter
            .map(|s| self.resolve_agent_id(s))
            .unwrap_or_else(|| self.default_agent_id.clone());

        let input = match tier {
            "hot" => FilterInput {
                pinned: Some(true),
                limit,
                ..Default::default()
            },
            "warm" => FilterInput {
                importance_min: Some(0.5),
                limit,
                ..Default::default()
            },
            "cold" => FilterInput {
                limit,
                sort_by: SortField::Importance,
                sort_order: SortOrder::Asc,
                ..Default::default()
            },
            _ => FilterInput {
                limit,
                ..Default::default()
            },
        };

        let output = self
            .engine
            .filter(&agent_id, input)
            .await
            .map_err(|e| MemoryError::Database(format!("Filter failed: {e}")))?;

        let mut memories: Vec<HillockMemory> = output
            .results
            .iter()
            .map(filter_result_to_hillock_memory)
            .collect();

        // Backfill tier and score for compatibility
        for memory in &mut memories {
            if memory.score == 0.0 {
                memory.score = 1.0;
            }
            memory.tier = tier.to_string();
        }

        Ok(memories)
    }

    // ── Feedback ─────────────────────────────────────────────────────

    /// Send feedback (positive/negative) on a memory.
    pub async fn feedback(
        &self,
        memory_id: &str,
        signal: &str,
        _source: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let id = parse_memory_id(memory_id)?;
        let delta: f32 = match signal {
            "positive" => 0.05,
            "negative" => -0.10,
            _ => 0.0,
        };

        // Get current importance, apply delta, clamp
        let memory = self
            .engine
            .get_memory(id)
            .await
            .map_err(|e| MemoryError::Database(format!("Get memory failed: {e}")))?;

        let current = memory.importance.unwrap_or(0.5);
        let new_importance = (current + delta).clamp(0.05, 1.0);

        self.engine
            .update_importance(id, new_importance)
            .await
            .map_err(|e| MemoryError::Database(format!("Update importance failed: {e}")))?;

        Ok(serde_json::json!({
            "memory_id": memory_id,
            "signal": signal,
            "previous_importance": current,
            "new_importance": new_importance,
        }))
    }

    // ── Crystallize ──────────────────────────────────────────────────

    /// Crystallize episodes into a concept.
    pub async fn crystallize(
        &self,
        summary: &str,
        source_memory_ids: &[String],
        _source: &str,
        tags: &[String],
    ) -> Result<serde_json::Value, MemoryError> {
        let agent_id = &self.default_agent_id;
        let source_ids: Result<Vec<Uuid>, _> = source_memory_ids
            .iter()
            .map(|s| parse_memory_id(s))
            .collect();
        let source_ids = source_ids?;

        let input = CrystallizeInput {
            source_ids,
            summary: summary.to_string(),
            kind: Some(MemoryKind::Fact),
            tags: tags.to_vec(),
            importance: None,
        };

        let output = self
            .engine
            .crystallize(agent_id, input)
            .await
            .map_err(|e| MemoryError::Database(format!("Crystallize failed: {e}")))?;

        Ok(serde_json::json!({
            "memory_id": output.memory_id.to_string(),
            "superseded": output.superseded.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        }))
    }

    // ── Maintenance pipelines ────────────────────────────────────────

    /// Trigger nightly maintenance.
    pub async fn run_nightly(
        &self,
        agent_ns: Option<&str>,
        dry_run: bool,
    ) -> Result<serde_json::Value, MemoryError> {
        let agent_id = agent_ns
            .map(|s| self.resolve_agent_id(s))
            .unwrap_or_else(|| self.default_agent_id.clone());

        // Take a snapshot first. hillock's `ops::nightly::run` performs the backup, but
        // only when one is configured, and `NightlyConfig::default()` leaves it None — so
        // this tool path reached decay and prune unprotected while the CLI did not. That
        // asymmetry is exactly why `memory_cleanup` had to be treated as dangerous. A
        // failed backup now aborts the run rather than proceeding without it.
        //
        // `dry_run` must be threaded all the way down. It used to stop at consolidation:
        // `memory_cleanup(dry_run: true)` still reached decay, prune and superseded-prune
        // live, then reported "no changes made." The parameter was accepted and dropped.
        let config = NightlyConfig {
            backup: Some(BackupConfig::new(self.backup_dir.clone())),
            maintenance: MaintenanceConfig {
                dry_run,
                ..MaintenanceConfig::default()
            },
            ..NightlyConfig::default()
        };

        let output = self
            .engine
            .nightly(&agent_id, config)
            .await
            .map_err(|e| MemoryError::Database(format!("Nightly failed: {e}")))?;

        Ok(serde_json::to_value(&output).unwrap_or_else(|_| serde_json::json!({"status": "ok"})))
    }

    // ── Pack context ─────────────────────────────────────────────────

    /// Pack memories into a token budget for context injection.
    pub async fn pack_context(
        &self,
        agent_ns: &str,
        token_budget: usize,
        _recall_mode: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let agent_id = self.resolve_agent_id(agent_ns);
        let input = PackContextInput {
            token_budget,
            query: None,
            spaces: SpaceScope::All,
            exclude_ids: vec![],
        };

        let output = self
            .engine
            .pack_context(&agent_id, input)
            .await
            .map_err(|e| MemoryError::Database(format!("Pack context failed: {e}")))?;

        let memories: Vec<serde_json::Value> = output
            .memories
            .iter()
            .map(|m| {
                // Serialize kind and source using serde's snake_case rename
                let kind_str = serde_json::to_value(&m.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| "observation".to_string());
                let source_str = serde_json::to_value(&m.source)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| "system".to_string());
                serde_json::json!({
                    "id": m.memory_id.to_string(),
                    "content": m.content,
                    "kind": kind_str,
                    "source": source_str,
                    "importance": m.importance.unwrap_or(0.5),
                    "pinned": m.pinned,
                    "tags": m.tags,
                    "created_at": m.created_at.to_rfc3339(),
                })
            })
            .collect();

        Ok(serde_json::json!({
            "memories": memories,
            "tokens_used": output.total_tokens,
            "memories_included": memories.len(),
            "truncated": output.truncated,
        }))
    }

    // ── Export ────────────────────────────────────────────────────────

    /// Export memories for UI stats or migration.
    pub async fn export(
        &self,
        category: Option<&str>,
        _tier: Option<&str>,
        source_filter: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<HillockMemory>, MemoryError> {
        let agent_id = source_filter
            .map(|s| self.resolve_agent_id(s))
            .unwrap_or_else(|| self.default_agent_id.clone());

        // Was a second, divergent copy of this mapping that omitted "state" — so
        // `memory_filter(category: "state")` silently applied no filter at all while
        // `remember(category: "state")` stored an Observation. Same helper now.
        let kind = category.and_then(category_str_to_kind);

        let input = FilterInput {
            kind,
            limit: limit.unwrap_or(100),
            ..Default::default()
        };

        let output = self
            .engine
            .filter(&agent_id, input)
            .await
            .map_err(|e| MemoryError::Database(format!("Export failed: {e}")))?;

        Ok(output
            .results
            .iter()
            .map(filter_result_to_hillock_memory)
            .collect())
    }

    /// Get memory system status.
    pub async fn status(&self) -> Result<serde_json::Value, MemoryError> {
        let agent_id = &self.default_agent_id;
        let output = self
            .engine
            .inspect(agent_id)
            .await
            .map_err(|e| MemoryError::Database(format!("Inspect failed: {e}")))?;

        Ok(serde_json::json!({
            "memory_count": output.memory_count,
            "private_count": output.private_count,
            "commons_count": output.commons_count,
            "index_healthy": output.index_healthy,
            "tombstone_count": output.tombstone_count,
            "accessible_spaces": output.accessible_spaces,
        }))
    }

    // ── Namespace lifecycle ──────────────────────────────────────────

    /// Register a namespace (agent) — creates space, grants access, loads rotation.
    pub async fn namespace_register(
        &self,
        namespace: &str,
        _display_name: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let agent_id = self.resolve_agent_id(namespace);
        bootstrap_agent(&*self.store, &self.rotation, &agent_id, &self.master_key).await?;

        // Rebuild indices to pick up any new space
        let _ = self.engine.rebuild_indices().await;

        Ok(serde_json::json!({
            "status": "registered",
            "agent_id": agent_id,
        }))
    }

    /// Deactivate a namespace — revoke access, orphan memories.
    pub async fn namespace_deactivate(
        &self,
        namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let agent_id = self.resolve_agent_id(namespace);
        let count = self
            .engine
            .deactivate_agent(&agent_id)
            .await
            .map_err(|e| MemoryError::Database(format!("Deactivate failed: {e}")))?;

        Ok(serde_json::json!({
            "status": "deactivated",
            "agent_id": agent_id,
            "memories_orphaned": count,
        }))
    }

    // ── Compaction (no-op — Holt-internal) ───────────────────────

    pub async fn compaction_report(
        &self,
        _agent_id: &str,
        _session_id: &str,
        _event_type: &str,
        _message_count: Option<usize>,
        _tool_call_count: Option<u64>,
    ) -> Result<serde_json::Value, MemoryError> {
        Ok(
            serde_json::json!({"status": "noop", "message": "Compaction tracking handled by Holt internally"}),
        )
    }

    pub async fn compaction_pressure(
        &self,
        _agent_id: &str,
        _session_id: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        Ok(
            serde_json::json!({"pressure": 0.0, "message": "Compaction tracking handled by Holt internally"}),
        )
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_memory_id(id: &str) -> Result<Uuid, MemoryError> {
    Uuid::parse_str(id)
        .map_err(|e| MemoryError::Validation(format!("Invalid memory ID '{id}': {e}")))
}

/// Parse a `spaces` parameter from JSON args into a `SpaceScope`.
/// Defaults to `Private` if absent — agents must opt in to commons.
fn parse_space_scope(args: &serde_json::Value) -> SpaceScope {
    match args.get("spaces").and_then(|v| v.as_str()) {
        Some("all") => SpaceScope::All,
        Some("commons") => SpaceScope::Commons,
        Some("private") | None => SpaceScope::Private,
        Some(_) => SpaceScope::Private, // unknown value → safe default
    }
}

fn parse_filter_input(args: &serde_json::Value) -> FilterInput {
    let kind = args
        .get("kind")
        .or_else(|| args.get("category"))
        .and_then(|v| v.as_str())
        .and_then(|s| match s {
            "episode" | "observation" => Some(MemoryKind::Observation),
            "concept" | "fact" => Some(MemoryKind::Fact),
            "policy" | "constraint" => Some(MemoryKind::Constraint),
            "identity" | "preference" => Some(MemoryKind::Preference),
            "decision" => Some(MemoryKind::Decision),
            "procedure" => Some(MemoryKind::Procedure),
            _ => None,
        });

    let pinned = args.get("pinned").and_then(|v| parse_bool_lenient(v));
    let limit = args
        .get("limit")
        .and_then(|v| parse_u64_lenient(v))
        .unwrap_or(20) as usize;
    let importance_min = args
        .get("importance_min")
        .and_then(|v| parse_f64_lenient(v))
        .map(|v| v as f32);

    let tags = args
        .get("tags")
        .map(|v| parse_tags_lenient(v))
        .unwrap_or_default();

    let (sort_by, sort_order) = match args.get("sort").and_then(|v| v.as_str()) {
        Some("oldest") => (SortField::CreatedAt, SortOrder::Asc),
        Some("importance") => (SortField::Importance, SortOrder::Desc),
        _ => (SortField::CreatedAt, SortOrder::Desc), // "newest" or default
    };

    let spaces = parse_space_scope(args);

    FilterInput {
        kind,
        tags,
        pinned,
        importance_min,
        limit,
        sort_by,
        sort_order,
        spaces,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_collapses_configured_identity() {
        let aliases =
            std::collections::HashMap::from([("agent-terminal".to_string(), "agent".to_string())]);
        assert_eq!(resolve_with_aliases("agent-terminal", &aliases), "agent");
        assert_eq!(resolve_with_aliases("holt_foo", &aliases), "foo"); // app prefix stripped
        assert_eq!(resolve_with_aliases("sample", &aliases), "sample"); // untouched
    }

    #[test]
    fn lane_tag_has_no_public_built_in_identity() {
        let aliases =
            std::collections::HashMap::from([("agent-terminal".to_string(), "agent".to_string())]);
        assert_eq!(lane_tag_with_aliases("agent-terminal", &aliases), None);
        assert_eq!(lane_tag_with_aliases("agent", &aliases), None);
        assert_eq!(lane_tag_with_aliases("sample", &aliases), None);
    }

    /// Every category a tool advertises must survive a round trip.
    ///
    /// The memory tools' schemas offer five categories. `state` and `episode` both map to
    /// `MemoryKind::Observation`, and `Observation` maps back to `"episode"` — so a memory
    /// stored as `state` can never read back as `state`, and filtering by `state` returns
    /// every episode memory instead. Five advertised categories, four distinguishable.
    ///
    /// This test exists to force that decision rather than let it stay invisible: either
    /// `state` earns a distinct kind, or it comes out of the schemas.
    #[test]
    fn every_advertised_category_survives_a_round_trip() {
        // Exactly the enum the remember/recall/memory_filter schemas advertise.
        const ADVERTISED: [&str; 4] = ["episode", "concept", "policy", "identity"];

        let mut broken = Vec::new();
        for c in ADVERTISED {
            let kind = category_str_to_kind(c).expect("advertised category must map to a kind");
            let back = kind_to_category(&kind);
            if back != c {
                broken.push(format!("{c} -> {kind:?} -> {back}"));
            }

            // Fifth translator: the one `remember` uses to turn the argument into a
            // category. It lived inline in the tool until 2026-07-26 and was uncounted.
            let via_remember = super::super::store::MemoryCategory::from_tool_arg(Some(c));
            assert_eq!(
                category_to_kind(&via_remember),
                kind,
                "category \"{c}\" stores as a different kind than it filters on"
            );
        }

        assert!(
            broken.is_empty(),
            "advertised categories that cannot round-trip: {broken:?}"
        );
    }

    /// All translators from a category string to a kind must agree.
    ///
    /// There are three: `category_str_to_kind`, `category_to_kind`, and the match inside
    /// `parse_filter_input`. The existing consistency test below covers the first two. This
    /// covers the third, which is the one that drifted — it accepts aliases the others
    /// reject and produces kinds they cannot.
    #[test]
    fn parse_filter_input_agrees_with_the_other_category_translators() {
        const ADVERTISED: [&str; 4] = ["episode", "concept", "policy", "identity"];

        for c in ADVERTISED {
            let direct = category_str_to_kind(c);
            let via_filter = parse_filter_input(&serde_json::json!({ "category": c })).kind;
            assert_eq!(
                via_filter, direct,
                "category \"{c}\" resolves to a different kind when filtering than when storing"
            );
        }
    }

    /// The category a tool accepts must mean the same thing on every path.
    ///
    /// `recall` used to take `category` and discard it (the parameter was
    /// underscore-prefixed, so nothing warned), and `memory_filter` carried a second copy
    /// of the mapping that omitted "state" entirely.
    #[test]
    fn category_strings_map_consistently_everywhere() {
        use super::{category_str_to_kind, category_to_kind};

        for (s, cat) in [
            ("episode", MemoryCategory::Episode),
            ("concept", MemoryCategory::Concept),
            ("policy", MemoryCategory::Policy),
            ("identity", MemoryCategory::Identity),
        ] {
            assert_eq!(
                category_str_to_kind(s),
                Some(category_to_kind(&cat)),
                "category \"{s}\" filters on a different kind than remember stores it as"
            );
        }

        // An unrecognised category applies no filter rather than matching nothing.
        assert_eq!(category_str_to_kind("nonsense"), None);
        assert_eq!(category_str_to_kind(""), None);
    }
}
