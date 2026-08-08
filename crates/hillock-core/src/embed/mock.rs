//! Mock embedder for testing.
//!
//! Produces deterministic embeddings based on content hash.
//! Not semantically meaningful, but consistent — same content always
//! produces the same vector, enabling deterministic tests.

use async_trait::async_trait;

use super::{EmbedError, Embedder};

/// Mock embedder that produces deterministic vectors from content.
pub struct MockEmbedder {
    dimension: usize,
}

impl MockEmbedder {
    pub fn new(dimension: usize) -> Self {
        Self { dimension }
    }
}

#[async_trait]
impl Embedder for MockEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        Ok(deterministic_vector(text, self.dimension))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts
            .iter()
            .map(|t| deterministic_vector(t, self.dimension))
            .collect())
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        "mock-embedder"
    }
}

/// Generate a deterministic unit vector from text content.
/// Uses a simple hash-based approach — not semantically meaningful,
/// but the same text always produces the same vector.
fn deterministic_vector(text: &str, dimension: usize) -> Vec<f32> {
    let hash = blake3::hash(text.as_bytes());
    let hash_bytes = hash.as_bytes();

    let mut vec = Vec::with_capacity(dimension);
    for i in 0..dimension {
        // Use hash bytes cyclically, spread across dimensions
        let byte_idx = i % 32;
        let shift = (i / 32) % 8;
        let val = hash_bytes[byte_idx].wrapping_add(shift as u8);
        // Map to [-1, 1] range
        vec.push((val as f32 / 128.0) - 1.0);
    }

    // Normalize to unit vector
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    }

    vec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deterministic_embeddings() {
        let embedder = MockEmbedder::new(768);
        let v1 = embedder.embed("hello world").await.unwrap();
        let v2 = embedder.embed("hello world").await.unwrap();
        assert_eq!(v1, v2);
        assert_eq!(v1.len(), 768);
    }

    #[tokio::test]
    async fn different_content_different_vectors() {
        let embedder = MockEmbedder::new(768);
        let v1 = embedder.embed("hello").await.unwrap();
        let v2 = embedder.embed("world").await.unwrap();
        assert_ne!(v1, v2);
    }

    #[tokio::test]
    async fn unit_vectors() {
        let embedder = MockEmbedder::new(768);
        let v = embedder.embed("test").await.unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn batch_embed() {
        let embedder = MockEmbedder::new(3);
        let results = embedder.embed_batch(&["a", "b", "c"]).await.unwrap();
        assert_eq!(results.len(), 3);
        assert_ne!(results[0], results[1]);
    }
}
