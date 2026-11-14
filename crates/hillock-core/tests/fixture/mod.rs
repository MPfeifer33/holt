//! Shared wiring for the integration tests: an engine over in-memory SQLite whose
//! embedder the test scripts, so cosine similarity is chosen rather than hoped for.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use hillock_core::embed::mock::MockEmbedder;
use hillock_core::embed::{EmbedError, Embedder};
use hillock_core::engine::Engine;
use hillock_core::index::memory_index::MemoryIndex;
use hillock_core::index::VectorIndex as _;
use hillock_core::model::*;
use hillock_core::rotation::RotationManager;
use hillock_core::store::sqlite::SqliteStore;
use hillock_core::store::MemoryStore as _;
use uuid::Uuid;

pub const DIM: usize = 32;
pub const AGENT: &str = "demo";
pub const SPACE: &str = "agent:demo";
/// The text every scripted query uses; it embeds to the axis cosines are measured from.
pub const QUERY: &str = "the scripted query";

/// Embeds text containing a placed needle to a vector with a chosen cosine to
/// [`QUERY`]; anything else falls back to the hash-based mock.
pub struct ScriptedEmbedder {
    placed: Mutex<Vec<(String, Vec<f32>)>>,
    fallback: MockEmbedder,
    fail_batches: AtomicBool,
}

impl ScriptedEmbedder {
    pub fn new() -> Arc<Self> {
        let e = Self {
            placed: Mutex::new(Vec::new()),
            fallback: MockEmbedder::new(DIM),
            fail_batches: AtomicBool::new(false),
        };
        e.place(QUERY, 1.0);
        Arc::new(e)
    }

    /// Any text containing `needle` embeds at `cosine` to the query.
    pub fn place(&self, needle: &str, cosine: f32) {
        let mut v = vec![0.0f32; DIM];
        v[0] = cosine;
        v[1] = (1.0 - cosine * cosine).max(0.0).sqrt();
        self.placed.lock().unwrap().push((needle.to_string(), v));
    }

    /// Make `embed_batch` (the long-content path) fail, or work again.
    pub fn fail_batches(&self, on: bool) {
        self.fail_batches.store(on, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl Embedder for ScriptedEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let placed = self
            .placed
            .lock()
            .unwrap()
            .iter()
            .find(|(needle, _)| text.contains(needle.as_str()))
            .map(|(_, v)| v.clone());
        match placed {
            Some(v) => Ok(v),
            None => self.fallback.embed(text).await,
        }
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if self.fail_batches.load(Ordering::SeqCst) {
            return Err(EmbedError::NotReady);
        }
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            out.push(self.embed(t).await?);
        }
        Ok(out)
    }

    fn dimension(&self) -> usize {
        DIM
    }

    fn model_name(&self) -> &str {
        "scripted-test-embedder"
    }
}

/// An engine with one private space (`agent:demo`), and the store handle.
pub async fn engine_with(
    embedder: Arc<dyn Embedder>,
    scoring: ScoringConfig,
) -> (Engine, Arc<SqliteStore>) {
    let store = Arc::new(SqliteStore::in_memory().unwrap());
    let index = Arc::new(MemoryIndex::new(DIM));
    let rotation = Arc::new(RotationManager::new(DIM));
    store
        .create_space(&Space {
            id: SPACE.into(),
            rotation_kind: RotationKind::Derived,
            rotation_seed: Some([42u8; 32]),
            created_at: Utc::now(),
            memory_count: 0,
        })
        .await
        .unwrap();
    rotation.load_space(SPACE, &[42u8; 32]).unwrap();
    store
        .grant_access(&KeyRingEntry {
            agent_id: AGENT.into(),
            space_id: SPACE.into(),
            granted_at: Utc::now(),
            granted_by: "operator".into(),
        })
        .await
        .unwrap();
    index.build_space(SPACE, vec![]).await.unwrap();
    let engine = Engine::new(store.clone(), index, embedder, rotation, scoring);
    (engine, store)
}

/// A plain remember input; tests set the fields they care about.
pub fn input(content: &str) -> RememberInput {
    RememberInput {
        content: content.to_string(),
        metadata: RememberMetadata::default(),
        supersedes: vec![],
        created_at: None,
    }
}

/// `input` with the metadata these tests vary most.
pub fn input_with(
    content: &str,
    kind: MemoryKind,
    tags: &[&str],
    importance: Option<f32>,
) -> RememberInput {
    let mut i = input(content);
    i.metadata.kind = Some(kind);
    i.metadata.tags = tags.iter().map(|t| t.to_string()).collect();
    i.metadata.importance = importance;
    i
}

pub async fn remember(engine: &Engine, i: RememberInput) -> Uuid {
    engine.remember(AGENT, i).await.unwrap().memory_id
}

/// Vector-only semantic recall of [`QUERY`] in the private space, asked on purpose.
pub fn query(limit: usize) -> RecallInput {
    RecallInput {
        query: Some(QUERY.to_string()),
        memory_id: None,
        limit,
        filter: None,
        spaces: SpaceScope::Private,
        content_mode: ContentMode::Chunk,
        search_strategy: SearchStrategy::CosineOnly,
        exclude_tags: Vec::new(),
        reinforce: true,
    }
}

pub fn fetch(id: Uuid) -> RecallInput {
    RecallInput {
        query: None,
        memory_id: Some(id),
        ..query(1)
    }
}

/// Ids of a recall's results, in rank order.
pub async fn recall_ids(engine: &Engine, i: RecallInput) -> Vec<Uuid> {
    engine
        .recall(AGENT, i)
        .await
        .unwrap()
        .results
        .iter()
        .map(|r| r.memory_id)
        .collect()
}

/// Content long enough to take the chunked path, every fragment containing `needle`.
pub fn long_content(needle: &str) -> String {
    (0..60)
        .map(|i| format!("{needle} sentence number {i} of a long document about the same thing. "))
        .collect()
}

pub async fn importance_of(engine: &Engine, id: Uuid) -> Option<f32> {
    engine.get_memory(id).await.unwrap().importance
}
