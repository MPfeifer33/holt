//! Embedding trait — the vectorization seam.
//!
//! V1 uses BGE-Base-EN-v1.5 (768d) via fastembed or a local model server.
//! The trait exists for testability and model swapability.

/// Errors from the embedding layer.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("embedding model not loaded")]
    NotReady,

    #[error("content too long for single embedding: {tokens} tokens (max {max})")]
    ContentTooLong { tokens: usize, max: usize },

    #[error("embedding error: {0}")]
    Internal(#[from] anyhow::Error),
}

pub mod fastembed_embedder;
pub mod mock;

use async_trait::async_trait;

/// The embedding layer.
///
/// Converts text content to dense vectors. The model and dimension are
/// deployment configuration — the system doesn't care what produces the
/// vector as long as it's consistent within a deployment.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Embed a single piece of text. Returns a vector of `dimension()` length.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError>;

    /// Embed multiple texts in a batch. More efficient than individual calls.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;

    /// The dimensionality of the embedding vectors (e.g., 768 for BGE-Base).
    fn dimension(&self) -> usize;

    /// The model name (e.g., "BAAI/bge-base-en-v1.5"). Used for migration validation.
    fn model_name(&self) -> &str;
}
