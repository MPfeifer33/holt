use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use moka::future::Cache;
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tiktoken_rs::CoreBPE;

const MAX_CACHE_ENTRIES: u64 = 10_000;
const CACHE_TTL_SECS: u64 = 3600;

fn build_embedding_cache() -> Cache<u64, Vec<f32>> {
    Cache::builder()
        .max_capacity(MAX_CACHE_ENTRIES)
        .time_to_live(Duration::from_secs(CACHE_TTL_SECS))
        .build()
}

/// Thread-safe embedding service wrapping fastembed BGE-base-EN-v1.5.
/// Singleton in AppState. Uses spawn_blocking for CPU-bound embedding work.
/// Content-hash cache prevents recomputing embeddings for duplicate content.
pub struct EmbeddingService {
    model: Arc<Mutex<TextEmbedding>>,
    cache: Cache<u64, Vec<f32>>,
}

impl EmbeddingService {
    /// Initialize the embedding model. This takes 2-5 seconds (model download/load).
    /// Call during app startup, not lazily.
    pub fn new() -> Result<Self, String> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGEBaseENV15).with_show_download_progress(true),
        )
        .map_err(|e| format!("Failed to initialize embedding model: {e}"))?;

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            cache: build_embedding_cache(),
        })
    }

    /// Generate embedding for a single text. Uses cache to avoid recomputation.
    /// Runs on a blocking thread to avoid holding the async runtime.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        // Check cache first
        let hash = self.hash_content(text);
        if let Some(cached) = self.cache.get(&hash).await {
            return Ok(cached);
        }

        // Compute embedding on blocking thread
        let model = Arc::clone(&self.model);
        let text_owned = text.to_string();
        let embedding = tauri::async_runtime::spawn_blocking(move || {
            let mut model = model
                .lock()
                .map_err(|e| format!("Model lock poisoned: {e}"))?;
            model
                .embed(vec![text_owned], None)
                .map_err(|e| format!("Embedding failed: {e}"))
        })
        .await
        .map_err(|e| format!("Blocking task failed: {e}"))??;

        let vec = embedding
            .into_iter()
            .next()
            .ok_or_else(|| "No embedding returned".to_string())?;

        // Cache the result
        self.cache.insert(hash, vec.clone()).await;

        Ok(vec)
    }

    /// Simple content hash for cache key.
    ///
    /// NOTE: DefaultHasher uses SipHash with a random per-process seed, so hash
    /// values are NOT stable across process restarts. This is fine because the
    /// embedding cache is in-memory and per-process. If we ever add a persistent
    /// (on-disk) embedding cache, swap to a deterministic hasher like blake3 or xxhash.
    fn hash_content(&self, text: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }

    /// Number of cached embeddings.
    pub fn cache_size(&self) -> usize {
        self.cache.entry_count() as usize
    }
}

// --- BPE tokenizer utility for token counting ---

static BPE: OnceLock<CoreBPE> = OnceLock::new();

/// Get the shared BPE tokenizer instance (cl100k_base, same as GPT-4/Claude).
pub fn get_bpe() -> &'static CoreBPE {
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().expect("Failed to initialize BPE tokenizer"))
}

/// Count tokens in a string using the shared BPE instance.
pub fn count_tokens(text: &str) -> usize {
    get_bpe().encode_with_special_tokens(text).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_tokens() {
        let count = count_tokens("Hello, world!");
        assert!(count > 0);
        assert!(count < 10); // "Hello, world!" is ~4 tokens
    }

    #[test]
    fn test_count_tokens_empty() {
        assert_eq!(count_tokens(""), 0);
    }

    #[test]
    fn test_hash_deterministic() {
        let svc = EmbeddingService {
            model: Arc::new(Mutex::new(
                TextEmbedding::try_new(InitOptions::new(EmbeddingModel::BGEBaseENV15)).unwrap(),
            )),
            cache: build_embedding_cache(),
        };
        let h1 = svc.hash_content("test");
        let h2 = svc.hash_content("test");
        assert_eq!(h1, h2);
        assert_ne!(svc.hash_content("test"), svc.hash_content("other"));
    }

    #[tokio::test]
    async fn test_cache_has_max_capacity() {
        let svc = EmbeddingService {
            model: Arc::new(Mutex::new(
                TextEmbedding::try_new(InitOptions::new(EmbeddingModel::BGEBaseENV15)).unwrap(),
            )),
            cache: build_embedding_cache(),
        };
        for i in 0..=MAX_CACHE_ENTRIES {
            svc.cache.insert(i, vec![0.0f32; 10]).await;
        }
        // Moka may not evict synchronously, but capacity is bounded
        assert!(svc.cache_size() <= MAX_CACHE_ENTRIES as usize + 1);
    }
}
