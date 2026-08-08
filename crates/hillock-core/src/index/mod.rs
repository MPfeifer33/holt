//! Vector index trait — the search seam.
//!
//! HNSW is the V1 implementation. This is a derived index — SQLite is
//! the source of truth. On startup, rebuild from store. If runtime update
//! fails, mark dirty and fall back to rebuild on next startup.

pub mod memory_index;

use async_trait::async_trait;
use uuid::Uuid;

/// Errors from the vector index.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("index not built for space: {0}")]
    SpaceNotIndexed(String),

    #[error("dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },

    #[error("index error: {0}")]
    Internal(#[from] anyhow::Error),
}

/// A single search result from the vector index.
#[derive(Debug, Clone)]
pub struct IndexResult {
    pub memory_id: Uuid,
    /// Cosine similarity in [0, 1]. Higher = more similar.
    pub cosine_score: f32,
}

/// The vector search layer.
///
/// One HNSW index per space. Manages insertion, deletion (tombstone),
/// search, and rebuild.
#[async_trait]
pub trait VectorIndex: Send + Sync {
    /// Build or rebuild the index for a space from a set of vectors.
    /// This is the authoritative rebuild path — called on startup.
    async fn build_space(
        &self,
        space_id: &str,
        vectors: Vec<(Uuid, Vec<f32>)>,
    ) -> Result<(), IndexError>;

    /// Add a single vector to a space's index.
    /// If this fails, the index should be marked dirty for rebuild.
    async fn insert(
        &self,
        space_id: &str,
        memory_id: Uuid,
        vector: &[f32],
    ) -> Result<(), IndexError>;

    /// Flag a vector as deleted (tombstone). Excluded from future searches.
    /// Cleaned up on next rebuild.
    async fn delete(&self, space_id: &str, memory_id: Uuid) -> Result<(), IndexError>;

    /// Search for the top-k most similar vectors to the query in a space.
    /// Returns results ordered by cosine similarity (highest first).
    ///
    /// `cosine_score` must be similarity in [0, 1], not distance.
    /// If the underlying library returns distance, convert:
    /// `cosine_score = 1.0 - cosine_distance`
    async fn search(
        &self,
        space_id: &str,
        query: &[f32],
        k: usize,
    ) -> Result<Vec<IndexResult>, IndexError>;

    /// Check if the index for a space is dirty (needs rebuild).
    fn is_dirty(&self, space_id: &str) -> bool;

    /// Check if a space has been indexed.
    fn has_space(&self, space_id: &str) -> bool;
}
