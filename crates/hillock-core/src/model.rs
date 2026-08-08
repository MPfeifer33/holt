//! Pure domain types for the memory system.
//!
//! These types represent the data model from SYSTEM_V2.md.
//! No behavior, no dependencies on storage or indexing — just data.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

// ── Memory ──────────────────────────────────────────────────────────

/// A memory stored in the system.
///
/// Can be standalone (single embedding), a parent (full content, no embedding),
/// or a chunk (portion of a parent, with its own embedding).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: Uuid,
    pub content: String,
    /// Stored rotated for the owning space. None for parent memories (only chunks are indexed).
    pub embedding: Option<Vec<f32>>,
    /// Which space this memory belongs to.
    pub space_id: String,

    // ── Metadata (stored, filterable, NOT used for ranking) ──
    pub kind: MemoryKind,
    pub source: MemorySource,
    pub tags: Vec<String>,
    /// When the claim/memory was originally asserted. Used for recency ranking.
    /// Preserved on share/migration — NOT overwritten with import or share time.
    pub created_at: DateTime<Utc>,
    /// BLAKE3 hash of normalized content, for dedup detection.
    pub content_hash: [u8; 32],

    // ── Retained signals (stored, filterable, NOT used for ranking) ──
    /// Agent-assigned importance at creation. Inactive for ranking.
    pub importance: Option<f32>,
    /// Agent-assigned confidence. Inactive for ranking.
    pub confidence: Option<f32>,
    /// Agent-maintained validity record. Inactive for ranking.
    pub belief_state: Option<BeliefState>,

    // ── Chunking ──
    /// If this is a chunk, points to the parent memory.
    pub parent_id: Option<Uuid>,
    /// Ordering within the parent (0-based).
    pub chunk_index: Option<u16>,
    /// Whether this is a chunk of a larger memory.
    pub is_chunk: bool,

    // ── Provenance (for shared/copied memories) ──
    /// Who created or shared it.
    pub source_agent: Option<String>,
    /// Original memory ID if shared from another space.
    pub source_memory: Option<Uuid>,
    /// Originating space.
    pub source_space: Option<String>,
    /// When the memory was shared (None for non-shared memories).
    pub shared_at: Option<DateTime<Utc>>,
}

/// What kind of knowledge this memory represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Observation,
    Fact,
    Decision,
    Preference,
    Constraint,
    Procedure,
}

/// Who produced this memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    Agent,
    Human,
    System,
}

/// Validity state of a memory. Agent-maintained, not used for ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeliefState {
    Active,
    Superseded,
    Uncertain,
}

// ── Space ───────────────────────────────────────────────────────────

/// A vector space defined by a rotation key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Space {
    pub id: String,
    pub rotation_kind: RotationKind,
    /// Per-space seed, encrypted at rest. None for identity spaces.
    /// Private spaces: derived from master_password + agent_id via Argon2id.
    /// Shared/project/team spaces: randomly generated.
    pub rotation_seed: Option<[u8; 32]>,
    pub created_at: DateTime<Utc>,
    pub memory_count: u64,
}

/// How rotation is applied for this space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationKind {
    /// No rotation (R = I). Used for commons.
    Identity,
    /// Rotation matrix derived from seed at runtime. Used for private/team/project spaces.
    Derived,
}

// ── KeyRing ─────────────────────────────────────────────────────────

/// An authorization entry: this agent can search this space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyRingEntry {
    pub agent_id: String,
    pub space_id: String,
    pub granted_at: DateTime<Utc>,
    pub granted_by: String,
}

// ── Tool I/O Types ──────────────────────────────────────────────────

/// Input for the `remember` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RememberInput {
    pub content: String,
    #[serde(default)]
    pub metadata: RememberMetadata,
    /// Memory IDs to mark as `belief_state: superseded` when this memory is stored.
    #[serde(default)]
    pub supersedes: Vec<Uuid>,
    /// Override created_at timestamp (for migration). If None, uses Utc::now().
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

/// Optional metadata for `remember`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RememberMetadata {
    pub kind: Option<MemoryKind>,
    pub source: Option<MemorySource>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub importance: Option<f32>,
    pub confidence: Option<f32>,
}

/// Output from the `remember` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RememberOutput {
    pub memory_id: Uuid,
    pub chunk_count: Option<u16>,
    #[serde(default)]
    pub near_duplicates: Vec<ConflictCandidate>,
    #[serde(default)]
    pub potential_conflicts: Vec<ConflictCandidate>,
}

/// A memory surfaced during conflict-candidate scanning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictCandidate {
    pub memory_id: Uuid,
    pub content_preview: String,
    pub cosine_similarity: f32,
    pub created_at: DateTime<Utc>,
}

/// Input for the `recall` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallInput {
    /// Semantic search query. Required unless `memory_id` is provided.
    pub query: Option<String>,
    /// Direct fetch by ID.
    pub memory_id: Option<Uuid>,
    /// Max results (default: 10). Ignored for direct fetch.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Post-scoring filter.
    pub filter: Option<RecallFilter>,
    /// Which spaces to search.
    #[serde(default)]
    pub spaces: SpaceScope,
    /// How to return chunked content.
    #[serde(default)]
    pub content_mode: ContentMode,
    /// Search strategy (default: cosine_only).
    #[serde(default)]
    pub search_strategy: SearchStrategy,
}

fn default_limit() -> usize {
    10
}

/// Post-scoring metadata filter. Does not alter scores — only removes candidates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecallFilter {
    pub kind: Option<MemoryKind>,
    pub source: Option<MemorySource>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub date_start: Option<DateTime<Utc>>,
    pub date_end: Option<DateTime<Utc>>,
    pub importance_min: Option<f32>,
    pub confidence_min: Option<f32>,
    pub belief_state: Option<BeliefState>,
}

/// Which spaces to search.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceScope {
    /// Search all spaces in the agent's key ring (default).
    All,
    /// Search only the agent's private space.
    Private,
    /// Search only commons.
    Commons,
    /// Search specific space IDs.
    Specific(Vec<String>),
}

impl Default for SpaceScope {
    fn default() -> Self {
        Self::All
    }
}

/// Which search strategy to use for recall.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStrategy {
    /// Cosine similarity + recency boost only (HNSW).
    CosineOnly,
    /// BM25 as candidate injector: FTS5 candidates unioned with HNSW,
    /// with conditional boost when cosine is weak. Default.
    #[default]
    HybridBm25Inject,
    /// Reciprocal Rank Fusion: merge HNSW and FTS5 ranked lists,
    /// then apply recency boost.
    HybridRrf,
}

/// How to return content for chunked memories.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentMode {
    /// Return the matched chunk content (default for chunked results).
    #[default]
    Chunk,
    /// Return the full parent content.
    Parent,
    /// Return both chunk and parent content.
    Both,
}

/// A single result from recall.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResult {
    pub memory_id: Uuid,
    pub content: String,
    /// Full parent content, if `content_mode` is Parent or Both.
    pub parent_content: Option<String>,
    pub metadata: RecallResultMetadata,
    /// Final blended score (cosine + recency).
    pub score: f32,
    /// Raw cosine similarity before recency boost.
    pub cosine_score: f32,
    pub space_id: String,
    /// For chunked results.
    pub parent_id: Option<Uuid>,
    pub chunk_index: Option<u16>,
    pub chunk_count: Option<u16>,
    /// Cross-space duplicates of this result (provenance).
    #[serde(default)]
    pub duplicates: Vec<DuplicateInfo>,
}

/// Metadata included in recall results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResultMetadata {
    pub kind: MemoryKind,
    pub source: MemorySource,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub importance: Option<f32>,
    pub confidence: Option<f32>,
    pub belief_state: Option<BeliefState>,
    pub source_agent: Option<String>,
}

/// Provenance info for a cross-space duplicate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateInfo {
    pub space_id: String,
    pub memory_id: Uuid,
    pub score: f32,
}

/// Output from the `recall` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallOutput {
    pub results: Vec<RecallResult>,
}

/// Input for the `forget` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgetInput {
    pub memory_id: Uuid,
}

/// Output from the `forget` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgetOutput {
    pub deleted_count: u32,
    /// "standalone", "parent" (with chunks), or "rejected" (was a chunk).
    pub delete_type: String,
}

/// Input for the `share` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareInput {
    /// Share an existing private memory by ID.
    pub memory_id: Option<Uuid>,
    /// Or embed new content directly into the target space.
    pub content: Option<String>,
    pub source: Option<MemorySource>,
    /// Target space (default: commons).
    #[serde(default = "default_commons")]
    pub target_space: String,
}

fn default_commons() -> String {
    "commons".to_string()
}

/// Output from the `share` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareOutput {
    /// Memory ID in the target space (parent ID if chunked).
    pub memory_id: Uuid,
}

/// Input for the `inspect` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectInput {
    /// "health", "growth", or a topic string.
    pub focus: Option<String>,
}

/// Output from the `inspect` tool (V1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectOutput {
    pub memory_count: u64,
    pub private_count: u64,
    pub commons_count: u64,
    pub index_healthy: bool,
    pub tombstone_count: u64,
    pub index_dirty: bool,
    pub accessible_spaces: Vec<String>,
    pub recent_growth: Vec<GrowthPoint>,
}

/// A data point for growth tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthPoint {
    pub date: DateTime<Utc>,
    pub count: u64,
}

// ── Memory Filter ──────────────────────────────────────────────────

/// How to sort filter results.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortField {
    #[default]
    CreatedAt,
    Importance,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    #[default]
    Desc,
    Asc,
}

/// Input for the `memory_filter` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterInput {
    pub kind: Option<MemoryKind>,
    pub source: Option<MemorySource>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub belief_state: Option<BeliefState>,
    pub importance_min: Option<f32>,
    pub confidence_min: Option<f32>,
    pub date_start: Option<DateTime<Utc>>,
    pub date_end: Option<DateTime<Utc>>,
    pub pinned: Option<bool>,
    #[serde(default)]
    pub spaces: SpaceScope,
    #[serde(default)]
    pub sort_by: SortField,
    #[serde(default)]
    pub sort_order: SortOrder,
    #[serde(default = "default_filter_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

impl Default for FilterInput {
    fn default() -> Self {
        Self {
            kind: None,
            source: None,
            tags: Vec::new(),
            belief_state: None,
            importance_min: None,
            confidence_min: None,
            date_start: None,
            date_end: None,
            pinned: None,
            spaces: SpaceScope::default(),
            sort_by: SortField::default(),
            sort_order: SortOrder::default(),
            limit: 20,
            offset: 0,
        }
    }
}

fn default_filter_limit() -> usize {
    20
}

/// A single result from memory_filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterResult {
    pub memory_id: Uuid,
    pub content: String,
    pub kind: MemoryKind,
    pub source: MemorySource,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub importance: Option<f32>,
    pub confidence: Option<f32>,
    pub belief_state: Option<BeliefState>,
    pub pinned: bool,
    pub space_id: String,
}

/// Output from the `memory_filter` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterOutput {
    pub results: Vec<FilterResult>,
    pub total_count: u64,
}

// ── Pin / Unpin ────────────────────────────────────────────────────

/// Output from the `pin` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinOutput {
    pub pinned: bool,
    pub pin_count: u8,
    pub error: Option<String>,
}

/// Output from the `unpin` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnpinOutput {
    pub unpinned: bool,
    pub pin_count: u8,
}

// ── Pack Context ───────────────────────────────────────────────────

/// Input for `pack_context`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackContextInput {
    /// Max tokens to return (heuristic: chars / 4).
    pub token_budget: usize,
    /// If provided, KNN search; if None, importance-ordered fill.
    pub query: Option<String>,
    #[serde(default)]
    pub spaces: SpaceScope,
    /// Memory IDs already in context (skip these).
    #[serde(default)]
    pub exclude_ids: Vec<Uuid>,
}

/// A memory packed for context injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackedMemory {
    pub memory_id: Uuid,
    pub content: String,
    pub kind: MemoryKind,
    pub source: MemorySource,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub importance: Option<f32>,
    pub pinned: bool,
    pub score: Option<f32>,
    pub space_id: String,
}

/// Output from `pack_context`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackContextOutput {
    pub memories: Vec<PackedMemory>,
    pub total_tokens: usize,
    pub truncated: bool,
}

// ── Crystallize ────────────────────────────────────────────────────

/// Input for the `crystallize` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrystallizeInput {
    /// Episode memory IDs to synthesize (min 2).
    pub source_ids: Vec<Uuid>,
    /// The distilled concept (agent-authored).
    pub summary: String,
    /// Kind for the crystallized memory (default: Fact).
    pub kind: Option<MemoryKind>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Default: max of source importances.
    pub importance: Option<f32>,
}

/// Output from the `crystallize` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrystallizeOutput {
    pub memory_id: Uuid,
    pub superseded: Vec<Uuid>,
    pub chunk_count: Option<u16>,
}

// ── Maintenance ───────────────────────────────────────────────────

/// A memory eligible for importance decay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayCandidate {
    pub memory_id: Uuid,
    pub importance: f32,
    pub created_at: DateTime<Utc>,
    pub space_id: String,
}

/// Configuration for the maintenance pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MaintenanceConfig {
    /// Decay multiplier per cycle (default: 0.98).
    pub decay_factor: f32,
    /// Memories younger than this are exempt from decay (default: 7 days).
    pub decay_min_age_days: u32,
    /// Stop decaying at this value (default: 0.05).
    pub decay_floor: f32,
    /// Memories below this importance AND older than prune_min_age_days are deleted (default: 0.05).
    pub prune_threshold: f32,
    /// Minimum age before a memory can be pruned (default: 30 days).
    pub prune_min_age_days: u32,
    /// Max memories to process per cycle (default: 500).
    pub batch_size: usize,
    /// Minimum hours between two decays of the same memory (default: 20).
    ///
    /// Decay is multiplicative, so running the pipeline twice in a day applies `0.98²`.
    /// Nothing prevented that: two full runs were observed on 2026-07-17 and twenty-plus
    /// at 30-minute intervals on 2026-07-03. Set to 0 to decay on every run — useful in
    /// tests that simulate consecutive days, but it restores the compounding.
    pub decay_min_interval_hours: u32,
    /// Report what maintenance *would* do without touching the store (default: false).
    ///
    /// Every count in `MaintenanceOutput` becomes a projection: rows are selected exactly
    /// as a live run would select them, then counted instead of written. Nothing is
    /// decayed, deleted, logged or indexed.
    ///
    /// When this config reaches `ops::nightly::run` inside a `NightlyConfig` it governs
    /// the **whole** pipeline — the pre-run snapshot, the contradiction scan and the
    /// `nightly_runs` record are skipped too. It lives here, on the config for the
    /// destructive stage, so there is exactly one flag and no way for a pipeline-level
    /// `dry_run` to disagree with a stage-level one.
    ///
    /// Added 2026-07-26. `memory_cleanup` accepted `dry_run: true`, forwarded it to
    /// consolidation only, then ran decay + prune + superseded-prune live and reported
    /// "Dry run complete — no changes made."
    pub dry_run: bool,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            decay_factor: 0.98,
            decay_min_age_days: 7,
            decay_floor: 0.05,
            prune_threshold: 0.05,
            prune_min_age_days: 30,
            batch_size: 500,
            // Matches the nightly overdue threshold, so a normal daily run is never
            // skipped while a same-day rerun is.
            decay_min_interval_hours: 20,
            dry_run: false,
        }
    }
}

/// Output from a maintenance run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceOutput {
    pub memories_decayed: u32,
    pub memories_pruned: u32,
    pub tombstones_cleaned: u32,
    pub duration_ms: u64,
}

/// A lightweight audit log entry for pruned memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgetLogEntry {
    pub memory_id: Uuid,
    pub content_hash: [u8; 32],
    pub deleted_at: DateTime<Utc>,
    pub reason: ForgetReason,
    /// Which agent asked for this deletion, or `None` when maintenance did it.
    ///
    /// Deliberately not filled in for automated pruning: nothing *requested* it, a
    /// threshold was crossed. Inventing an actor there would make the column useless for
    /// telling a deliberate delete from a scheduled one.
    pub actor: Option<String>,
}

/// Why a memory was pruned by maintenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgetReason {
    /// Importance decayed below prune threshold.
    DecayedBelowThreshold,
    /// Memory was superseded and past retention window.
    ///
    /// **Historical only.** Nothing writes this since 2026-07-26 — supersession no longer
    /// schedules deletion. The variant stays so the 174 existing `forget_log` rows remain
    /// readable; removing it would make the audit trail unparseable.
    SupersededExpired,
    /// Deliberately deleted via the `forget` operation, by the actor on the entry.
    Requested,
}

impl ForgetReason {
    /// The stored form. Paired with [`ForgetReason::from_str`] so the write and read sides
    /// of the audit log cannot drift apart — they were separate inline matches before.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DecayedBelowThreshold => "decayed_below_threshold",
            Self::SupersededExpired => "superseded_expired",
            Self::Requested => "requested",
        }
    }

    /// Parse the stored form. `None` for anything this binary does not recognise.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "decayed_below_threshold" => Some(Self::DecayedBelowThreshold),
            "superseded_expired" => Some(Self::SupersededExpired),
            "requested" => Some(Self::Requested),
            _ => None,
        }
    }
}

// ── Contradiction Detection ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContradictionResolution {
    /// The memory that was kept.
    pub kept: Uuid,
    /// The memory that was superseded.
    pub superseded: Uuid,
    /// Optional reason from the human.
    pub reason: Option<String>,
}

// ── Nightly Maintenance ──────────────────────────────────────────

/// Configuration for the combined nightly pipeline.
/// Wraps configs for all three sub-pipelines.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NightlyConfig {
    /// Maintenance config.
    pub maintenance: MaintenanceConfig,
    /// Pre-run snapshot. `None` disables it (and the safety it provides).
    pub backup: Option<BackupConfig>,
}

impl Default for NightlyConfig {
    fn default() -> Self {
        Self {
            maintenance: MaintenanceConfig::default(),
            backup: None,
        }
    }
}

/// Pre-run snapshot settings for the nightly pipeline.
///
/// The pipeline deletes rows (maintenance) and supersedes them (dream-lite), so a
/// snapshot is taken *before any stage runs*. This lives in the pipeline rather than in
/// the systemd unit because `ExecStartPre` protects only the scheduled path — tool and
/// CLI invocations reach the same destructive code with no backup at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    /// Directory to write snapshots into. Created if missing.
    pub dir: PathBuf,
    /// Snapshots older than this are removed after a successful backup.
    pub keep_days: u32,
}

impl BackupConfig {
    /// Default retention, matching the `backup-hillock.sh` the pipeline supersedes.
    pub const DEFAULT_KEEP_DAYS: u32 = 7;

    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            keep_days: Self::DEFAULT_KEEP_DAYS,
        }
    }
}

/// Output from a nightly run — combined report from all three pipelines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NightlyOutput {
    // ── Maintenance ──
    pub memories_decayed: u32,
    pub memories_pruned: u32,
    pub tombstones_cleaned: u32,

    // ── Overall ──
    pub total_duration_ms: u64,
    pub ran_at: DateTime<Utc>,
}

/// A record of a past nightly run, persisted in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NightlyRunRecord {
    pub ran_at: DateTime<Utc>,
    pub agent_id: String,
    pub duration_ms: u64,
    pub summary: String,
}

// ── FTS5 Results ───────────────────────────────────────────────────

/// A single FTS5 (BM25) search result.
#[derive(Debug, Clone)]
pub struct FtsResult {
    pub memory_id: Uuid,
    /// BM25 relevance score (higher = better match).
    pub bm25_score: f32,
    pub space_id: String,
}

// ── Scoring ─────────────────────────────────────────────────────────

/// Configuration for the retrieval scoring formula.
#[derive(Debug, Clone)]
pub struct ScoringConfig {
    /// Weight for recency boost (default: 0.02).
    ///
    /// Was 0.15, which made recency outrank relevance outright. Seen live on 2026-07-25:
    /// a result with cosine 0.716 ranked *fourth*, behind one at 0.691.
    ///
    /// The remediation plan proposed 0.05. **That is not enough.** Recency spans
    /// `1.0` (today) to `0.1815` (90 days) — a swing of `0.8185` — so the maximum boost is
    /// `0.8185 × w`, against a cosine advantage worth `Δcos × (1 - w)`:
    ///
    /// | `w` | max recency boost | overturns a 0.025 cosine edge? |
    /// |---|---|---|
    /// | 0.15 | 0.1228 | yes |
    /// | 0.05 | 0.0409 | yes |
    /// | 0.03 | 0.0246 | yes |
    /// | **0.02** | **0.0164** | **no** |
    ///
    /// The audit's 0.0722 figure measured a *1-day*-vs-90-day gap; against a same-day
    /// memory the swing is nearly double, which is what 0.05 fails to cover.
    ///
    /// At 0.02 the boost stays below both a 0.025 cosine edge and the measured median
    /// top-10 spread of 0.054, so relevance decides the ranking and recency only separates
    /// matches that are genuinely comparable.
    pub recency_weight: f32,
    /// Oversample multiplier for candidate retrieval.
    pub oversample_factor: usize,
    /// Minimum candidates scored per space (default: 500).
    ///
    /// Was 50, which meant a default recall inspected **0.69%** of a 7,229-memory store and
    /// filters were applied to whatever happened to land in that slice. The index is a
    /// linear cosine scan (`index/memory_index.rs`), so raising this costs a few thousand
    /// extra dot products — sub-millisecond — and multiplies effective recall.
    pub min_candidates: usize,
    /// Minimum candidates scored when the caller supplies a `RecallFilter` (default: 5000).
    ///
    /// Filters are applied *after* the candidate pool is cut, so a filtered recall could
    /// only ever match rows that were already in the top-N by cosine. `kind=decision`
    /// matched 17 of 7,229 rows and returned nothing. A filtered query is asking a
    /// different question — "the best X that is also Y" — and needs a wide enough pool for
    /// the Y to exist in it.
    pub filtered_min_candidates: usize,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            recency_weight: 0.02,
            oversample_factor: 4,
            min_candidates: 500,
            filtered_min_candidates: 5000,
        }
    }
}

/// Compute the recency score for a memory.
///
/// `recency = 1 / (1 + ln(1 + age_days))`
///
/// Returns a value in (0, 1] where 1.0 = just created, decaying gently with age.
pub fn recency_score(created_at: DateTime<Utc>, now: DateTime<Utc>) -> f32 {
    let age_days = (now - created_at).num_seconds().max(0) as f64 / 86400.0;
    (1.0 / (1.0 + (1.0 + age_days).ln())) as f32
}

/// Blend cosine similarity with recency.
///
/// `score = cosine * (1 - w) + recency * w`
///
/// Cosine must be similarity in [0, 1], not distance.
pub fn blend_score(cosine: f32, recency: f32, w: f32) -> f32 {
    cosine * (1.0 - w) + recency * w
}

// ── Content Hashing ─────────────────────────────────────────────────

/// Compute BLAKE3 hash of normalized content for dedup detection.
pub fn content_hash(content: &str) -> [u8; 32] {
    let normalized = content.trim().to_lowercase();
    *blake3::hash(normalized.as_bytes()).as_bytes()
}

// ── Tag Normalization ───────────────────────────────────────────────

/// Normalize a tag: lowercase, replace spaces with underscores, strip non-alphanumeric.
pub fn normalize_tag(tag: &str) -> String {
    tag.trim()
        .to_lowercase()
        .replace(' ', "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

// ── Lenient JSON Parsing ───────────────────────────────────────────
//
// LLMs frequently serialize values in unexpected forms: numbers as strings,
// booleans as strings, single items instead of arrays, comma-separated strings
// instead of arrays. These helpers accept all reasonable forms gracefully
// instead of silently dropping input via strict `.as_*()` accessors.

/// Parse tags from a JSON value, accepting both array and comma-separated string.
///
/// Thin wrapper around `parse_array_lenient` that extracts strings.
pub fn parse_tags_lenient(value: &serde_json::Value) -> Vec<String> {
    parse_array_lenient(value)
        .into_iter()
        .filter_map(|v| match v {
            serde_json::Value::String(s) if !s.is_empty() => Some(s),
            other => other.as_str().filter(|s| !s.is_empty()).map(String::from),
        })
        .collect()
}

/// Parse a JSON value into a Vec<Value>, accepting arrays, single values, or
/// comma-separated strings.
///
/// - Array → returned as-is
/// - Single non-null value → wrapped in a one-element vec
/// - Comma-separated string → split and returned as Vec<Value::String>
/// - Null/missing → empty vec
pub fn parse_array_lenient(value: &serde_json::Value) -> Vec<serde_json::Value> {
    match value {
        serde_json::Value::Array(arr) => arr.clone(),
        serde_json::Value::String(s) if s.contains(',') => s
            .split(',')
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .map(|t| serde_json::Value::String(t.to_string()))
            .collect(),
        serde_json::Value::Null => vec![],
        other => vec![other.clone()],
    }
}

/// Parse a JSON value as f64, accepting both numbers and string-encoded numbers.
///
/// - Number → extracted directly
/// - String "0.3" → parsed
/// - Anything else → None
pub fn parse_f64_lenient(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Parse a JSON value as u64, accepting numbers and string-encoded integers.
///
/// - Integer number → extracted directly
/// - Float number → truncated (e.g., 5.0 → 5)
/// - String "10" → parsed
/// - Anything else → None
pub fn parse_u64_lenient(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f as u64)),
        serde_json::Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

/// Parse a JSON value as bool, accepting booleans and string-encoded booleans.
///
/// - Bool → extracted directly
/// - String "true"/"false" (case-insensitive) → parsed
/// - Anything else → None
pub fn parse_bool_lenient(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::String(s) => match s.trim().to_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Normalize a list of tags, removing empty results and deduplicating.
pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut result: Vec<String> = tags
        .iter()
        .map(|t| normalize_tag(t))
        .filter(|t| !t.is_empty())
        .collect();
    result.sort();
    result.dedup();
    result
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── parse_tags_lenient ─────────────────────────────────────────

    #[test]
    fn tags_lenient_array() {
        let v = json!(["foo", "bar"]);
        assert_eq!(parse_tags_lenient(&v), vec!["foo", "bar"]);
    }

    #[test]
    fn tags_lenient_comma_string() {
        let v = json!("foo, bar, baz");
        assert_eq!(parse_tags_lenient(&v), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn tags_lenient_single_string() {
        let v = json!("solo");
        assert_eq!(parse_tags_lenient(&v), vec!["solo"]);
    }

    #[test]
    fn tags_lenient_null() {
        let v = json!(null);
        assert!(parse_tags_lenient(&v).is_empty());
    }

    #[test]
    fn tags_lenient_empty_string() {
        let v = json!("");
        assert!(parse_tags_lenient(&v).is_empty());
    }

    #[test]
    fn tags_lenient_empty_array() {
        let v = json!([]);
        assert!(parse_tags_lenient(&v).is_empty());
    }

    // ── parse_array_lenient ────────────────────────────────────────

    #[test]
    fn array_lenient_proper_array() {
        let v = json!(["a", "b"]);
        assert_eq!(parse_array_lenient(&v), vec![json!("a"), json!("b")]);
    }

    #[test]
    fn array_lenient_single_value() {
        let v = json!("single-uuid");
        assert_eq!(parse_array_lenient(&v), vec![json!("single-uuid")]);
    }

    #[test]
    fn array_lenient_comma_split() {
        let v = json!("a, b, c");
        assert_eq!(
            parse_array_lenient(&v),
            vec![json!("a"), json!("b"), json!("c")]
        );
    }

    #[test]
    fn array_lenient_null() {
        assert!(parse_array_lenient(&json!(null)).is_empty());
    }

    #[test]
    fn array_lenient_number() {
        let v = json!(42);
        assert_eq!(parse_array_lenient(&v), vec![json!(42)]);
    }

    // ── parse_f64_lenient ──────────────────────────────────────────

    #[test]
    fn f64_lenient_number() {
        assert_eq!(parse_f64_lenient(&json!(0.3)), Some(0.3));
    }

    #[test]
    fn f64_lenient_string() {
        assert_eq!(parse_f64_lenient(&json!("0.3")), Some(0.3));
    }

    #[test]
    fn f64_lenient_integer() {
        assert_eq!(parse_f64_lenient(&json!(5)), Some(5.0));
    }

    #[test]
    fn f64_lenient_null() {
        assert_eq!(parse_f64_lenient(&json!(null)), None);
    }

    #[test]
    fn f64_lenient_garbage() {
        assert_eq!(parse_f64_lenient(&json!("not-a-number")), None);
    }

    #[test]
    fn f64_lenient_empty_string() {
        assert_eq!(parse_f64_lenient(&json!("")), None);
    }

    // ── parse_u64_lenient ──────────────────────────────────────────

    #[test]
    fn u64_lenient_integer() {
        assert_eq!(parse_u64_lenient(&json!(10)), Some(10));
    }

    #[test]
    fn u64_lenient_string() {
        assert_eq!(parse_u64_lenient(&json!("10")), Some(10));
    }

    #[test]
    fn u64_lenient_float() {
        assert_eq!(parse_u64_lenient(&json!(5.0)), Some(5));
    }

    #[test]
    fn u64_lenient_null() {
        assert_eq!(parse_u64_lenient(&json!(null)), None);
    }

    #[test]
    fn u64_lenient_garbage() {
        assert_eq!(parse_u64_lenient(&json!("abc")), None);
    }

    #[test]
    fn u64_lenient_negative_string() {
        assert_eq!(parse_u64_lenient(&json!("-5")), None);
    }

    // ── parse_bool_lenient ─────────────────────────────────────────

    #[test]
    fn bool_lenient_true() {
        assert_eq!(parse_bool_lenient(&json!(true)), Some(true));
    }

    #[test]
    fn bool_lenient_false() {
        assert_eq!(parse_bool_lenient(&json!(false)), Some(false));
    }

    #[test]
    fn bool_lenient_string_true() {
        assert_eq!(parse_bool_lenient(&json!("true")), Some(true));
    }

    #[test]
    fn bool_lenient_string_false() {
        assert_eq!(parse_bool_lenient(&json!("false")), Some(false));
    }

    #[test]
    fn bool_lenient_string_case_insensitive() {
        assert_eq!(parse_bool_lenient(&json!("True")), Some(true));
        assert_eq!(parse_bool_lenient(&json!("FALSE")), Some(false));
    }

    #[test]
    fn bool_lenient_null() {
        assert_eq!(parse_bool_lenient(&json!(null)), None);
    }

    #[test]
    fn bool_lenient_garbage() {
        assert_eq!(parse_bool_lenient(&json!("yes")), None);
    }
}
