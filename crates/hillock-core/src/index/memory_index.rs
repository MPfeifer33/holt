//! In-memory vector index with brute-force cosine search.
//!
//! V1 implementation. At our scale (<10K vectors, 768d), brute-force
//! cosine scan is sub-millisecond. The VectorIndex trait is the
//! abstraction boundary — swap to HNSW when scale demands it.
//!
//! Source of truth is SQLite. This index is rebuilt from store on startup.

use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use uuid::Uuid;

use super::{IndexError, IndexResult, VectorIndex};

/// Per-space index state.
struct SpaceIndex {
    /// memory_id → embedding vector
    vectors: HashMap<Uuid, Vec<f32>>,
    /// Tombstoned memory IDs (excluded from search, cleaned on rebuild)
    tombstones: HashSet<Uuid>,
    /// Whether the index needs a rebuild (e.g., runtime insert failed)
    dirty: bool,
}

impl SpaceIndex {
    fn new() -> Self {
        Self {
            vectors: HashMap::new(),
            tombstones: HashSet::new(),
            dirty: false,
        }
    }
}

/// In-memory vector index using brute-force cosine similarity.
pub struct MemoryIndex {
    dimension: usize,
    spaces: RwLock<HashMap<String, SpaceIndex>>,
}

impl MemoryIndex {
    pub fn new(dimension: usize) -> Self {
        Self {
            dimension,
            spaces: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl VectorIndex for MemoryIndex {
    async fn build_space(
        &self,
        space_id: &str,
        vectors: Vec<(Uuid, Vec<f32>)>,
    ) -> Result<(), IndexError> {
        let mut space = SpaceIndex::new();

        for (id, vec) in vectors {
            if vec.len() != self.dimension {
                return Err(IndexError::DimensionMismatch {
                    expected: self.dimension,
                    got: vec.len(),
                });
            }
            space.vectors.insert(id, vec);
        }

        self.spaces
            .write()
            .map_err(|e| IndexError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?
            .insert(space_id.to_string(), space);

        Ok(())
    }

    async fn insert(
        &self,
        space_id: &str,
        memory_id: Uuid,
        vector: &[f32],
    ) -> Result<(), IndexError> {
        if vector.len() != self.dimension {
            return Err(IndexError::DimensionMismatch {
                expected: self.dimension,
                got: vector.len(),
            });
        }

        let mut spaces = self
            .spaces
            .write()
            .map_err(|e| IndexError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?;

        let space = spaces
            .entry(space_id.to_string())
            .or_insert_with(SpaceIndex::new);

        space.vectors.insert(memory_id, vector.to_vec());
        space.tombstones.remove(&memory_id);

        Ok(())
    }

    async fn delete(&self, space_id: &str, memory_id: Uuid) -> Result<(), IndexError> {
        let mut spaces = self
            .spaces
            .write()
            .map_err(|e| IndexError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?;

        if let Some(space) = spaces.get_mut(space_id) {
            space.vectors.remove(&memory_id);
            space.tombstones.remove(&memory_id);
        }

        Ok(())
    }

    async fn search(
        &self,
        space_id: &str,
        query: &[f32],
        k: usize,
    ) -> Result<Vec<IndexResult>, IndexError> {
        if query.len() != self.dimension {
            return Err(IndexError::DimensionMismatch {
                expected: self.dimension,
                got: query.len(),
            });
        }

        let spaces = self
            .spaces
            .read()
            .map_err(|e| IndexError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?;

        let space = spaces
            .get(space_id)
            .ok_or_else(|| IndexError::SpaceNotIndexed(space_id.to_string()))?;

        // Brute-force cosine similarity against all non-tombstoned vectors
        let mut scored: Vec<IndexResult> = space
            .vectors
            .iter()
            .filter(|(id, _)| !space.tombstones.contains(id))
            .map(|(id, vec)| {
                let sim = cosine_similarity(query, vec);
                IndexResult {
                    memory_id: *id,
                    cosine_score: sim.clamp(0.0, 1.0),
                }
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| {
            b.cosine_score
                .partial_cmp(&a.cosine_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        scored.truncate(k);
        Ok(scored)
    }

    fn is_dirty(&self, space_id: &str) -> bool {
        self.spaces
            .read()
            .map(|s| s.get(space_id).map_or(false, |si| si.dirty))
            .unwrap_or(false)
    }

    fn has_space(&self, space_id: &str) -> bool {
        self.spaces
            .read()
            .map(|s| s.contains_key(space_id))
            .unwrap_or(false)
    }
}

/// Cosine similarity between two vectors.
/// Returns value in [-1, 1]. Caller should clamp to [0, 1] for scoring.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        return 0.0;
    }
    dot / denom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn build_and_search() {
        let index = MemoryIndex::new(3);
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let id3 = Uuid::new_v4();

        index
            .build_space(
                "test",
                vec![
                    (id1, vec![1.0, 0.0, 0.0]),
                    (id2, vec![0.0, 1.0, 0.0]),
                    (id3, vec![0.9, 0.1, 0.0]),
                ],
            )
            .await
            .unwrap();

        // Query close to id1 and id3
        let results = index.search("test", &[1.0, 0.0, 0.0], 2).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].memory_id, id1); // exact match
        assert_eq!(results[1].memory_id, id3); // close
    }

    #[tokio::test]
    async fn tombstone_excludes_from_search() {
        let index = MemoryIndex::new(3);
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

        index
            .build_space(
                "test",
                vec![(id1, vec![1.0, 0.0, 0.0]), (id2, vec![0.9, 0.1, 0.0])],
            )
            .await
            .unwrap();

        index.delete("test", id1).await.unwrap();

        let results = index.search("test", &[1.0, 0.0, 0.0], 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory_id, id2);
    }

    #[tokio::test]
    async fn insert_after_build() {
        let index = MemoryIndex::new(3);
        index.build_space("test", vec![]).await.unwrap();

        let id = Uuid::new_v4();
        index.insert("test", id, &[1.0, 0.0, 0.0]).await.unwrap();

        let results = index.search("test", &[1.0, 0.0, 0.0], 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory_id, id);
        assert!((results[0].cosine_score - 1.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn dimension_mismatch() {
        let index = MemoryIndex::new(3);
        index.build_space("test", vec![]).await.unwrap();

        let result = index.insert("test", Uuid::new_v4(), &[1.0, 0.0]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn unknown_space_search() {
        let index = MemoryIndex::new(3);
        let result = index.search("nonexistent", &[1.0, 0.0, 0.0], 10).await;
        assert!(result.is_err());
    }

    #[test]
    fn cosine_similarity_correctness() {
        // Identical vectors = 1.0
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 0.001);
        // Orthogonal = 0.0
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 0.001);
        // Opposite = -1.0
        assert!((cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 0.001);
        // Zero vector = 0.0
        assert!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]).abs() < 0.001);
    }
}
