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

/// Full retrieval result from Hillock, including transparency metadata.
#[derive(Debug, Clone)]
pub struct HillockRetrieveResult {
    pub memories: Vec<HillockMemory>,
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
}
