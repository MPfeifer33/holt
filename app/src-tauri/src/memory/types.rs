//! Shared types for the memory subsystem.
//!
//! These are Holt's internal representations of memory data,
//! independent of the backend (embedded Hillock or HTTP).

/// A memory record returned from Hillock.
#[derive(Debug, Clone)]
pub struct HillockMemory {
    pub id: String,
    pub content: String,
    pub category: String,
    pub tier: String,
    pub importance: f64,
    pub score: f64,
    pub tags: Vec<String>,
    pub source: String,
    pub source_type: Option<String>,
    pub created_at: String,
    pub pinned: bool,
}

/// One semantic-recall hit, as the engine reported it.
#[derive(Debug, Clone)]
pub struct RecalledMemory {
    /// The row that matched. When `parent_id` is set this is a fragment's own id.
    pub id: String,
    /// The memory a fragment belongs to. `None` when the hit is a whole memory.
    pub parent_id: Option<String>,
    /// Position of a fragment within its memory. Set only for fragments.
    pub chunk_index: Option<u16>,
    pub chunk_count: Option<u16>,
    pub content: String,
    pub category: String,
    pub importance: f64,
    /// Rank score: similarity blended with recency, plus the importance term.
    pub score: f64,
    /// Similarity as the engine reports it, before recency and importance.
    pub cosine: f64,
    pub tags: Vec<String>,
    pub source: String,
    pub source_type: Option<String>,
    pub created_at: String,
    /// Read from the asking agent's pin table.
    pub pinned: bool,
    pub superseded: bool,
}

impl RecalledMemory {
    /// The id that names this memory to `pin_memory` and `forget`: the parent's when the
    /// hit is a fragment, because the engine refuses both on a fragment.
    pub fn memory_id(&self) -> &str {
        self.parent_id.as_deref().unwrap_or(&self.id)
    }
}

/// Full retrieval result from Hillock, including transparency metadata.
#[derive(Debug, Clone)]
pub struct HillockRetrieveResult {
    pub memories: Vec<RecalledMemory>,
    pub total_found: usize,
    pub near_misses: Option<serde_json::Value>,
    pub retrieval_confidence: Option<serde_json::Value>,
    pub temporal_distribution: Option<serde_json::Value>,
    pub result_mix: Option<serde_json::Value>,
    pub retrieval_interpretation: Option<serde_json::Value>,
    pub recommended_followup: Option<serde_json::Value>,
    pub highlights: Option<serde_json::Value>,
}

/// Detailed ingest result returned from Hillock.
#[derive(Debug, Clone)]
pub struct HillockStoreResult {
    pub memory_id: String,
    pub status: String,
    pub message: String,
    pub verification_status: String,
    /// Existing memories close to what was written, nearest first. At most three.
    pub similar: Vec<SimilarMemory>,
}

/// An existing memory the engine found close to a new write.
#[derive(Debug, Clone)]
pub struct SimilarMemory {
    pub id: String,
    pub similarity: f32,
    pub preview: String,
}
