//! FastEmbed-based embedder using ONNX Runtime.
//!
//! V1 default: BGE-Base-EN-v1.5 (768d). Model downloads on first use
//! and is cached locally. No external server needed.

use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::sync::Mutex;

use super::{EmbedError, Embedder};

/// FastEmbed embedder wrapping ONNX Runtime for local inference.
pub struct FastEmbedder {
    model: Mutex<TextEmbedding>,
    dimension: usize,
    model_name: String,
}

impl FastEmbedder {
    /// Create a new FastEmbed embedder with BGE-Base-EN-v1.5 (768d).
    pub fn new_bge_base() -> Result<Self, EmbedError> {
        Self::new(EmbeddingModel::BGEBaseENV15)
    }

    /// Create a FastEmbed embedder with a specific model.
    pub fn new(model_id: EmbeddingModel) -> Result<Self, EmbedError> {
        let mut model = TextEmbedding::try_new(InitOptions::new(model_id.clone()))
            .map_err(|e| EmbedError::Internal(anyhow::anyhow!("fastembed init: {e}")))?;

        // Get dimension from a test embedding
        let test = model
            .embed(vec!["test"], None)
            .map_err(|e| EmbedError::Internal(anyhow::anyhow!("fastembed test embed: {e}")))?;

        let dimension = test.first().map(|v| v.len()).unwrap_or(768);
        let model_name = format!("{model_id:?}");

        Ok(Self {
            model: Mutex::new(model),
            dimension,
            model_name,
        })
    }
}

#[async_trait]
impl Embedder for FastEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let mut model = self
            .model
            .lock()
            .map_err(|e| EmbedError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?;

        let results = model
            .embed(vec![text], None)
            .map_err(|e| EmbedError::Internal(anyhow::anyhow!("embed: {e}")))?;

        results
            .into_iter()
            .next()
            .ok_or_else(|| EmbedError::Internal(anyhow::anyhow!("no embedding returned")))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut model = self
            .model
            .lock()
            .map_err(|e| EmbedError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?;

        let texts_owned: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
        let results = model
            .embed(texts_owned, None)
            .map_err(|e| EmbedError::Internal(anyhow::anyhow!("embed_batch: {e}")))?;

        Ok(results)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
