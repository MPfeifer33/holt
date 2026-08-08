//! The `recall` operation — retrieval pipeline.
//!
//! Three search strategies:
//!   CosineOnly:       embed → rotate per space → HNSW → recency boost → filter → dedup
//!   HybridBm25Inject: HNSW candidates + FTS5 candidates → compute cosine for FTS-only → recency boost → filter → dedup
//!   HybridRrf:        HNSW ranked list + FTS5 ranked list → RRF merge → recency boost → filter → dedup

use std::collections::HashMap;

use crate::embed::Embedder;
use crate::index::VectorIndex;
use crate::model::*;
use crate::rotation::RotationManager;
use crate::store::{MemoryStore, StoreError};
use chrono::Utc;
use uuid::Uuid;

/// Execute the recall operation.
pub async fn recall(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    embedder: &dyn Embedder,
    rotation: &RotationManager,
    agent_id: &str,
    scoring: &ScoringConfig,
    input: RecallInput,
) -> Result<RecallOutput, anyhow::Error> {
    // ── Direct fetch mode ──
    if let Some(memory_id) = input.memory_id {
        return direct_fetch(store, memory_id, input.content_mode).await;
    }

    // ── Semantic search mode ──
    let query = input
        .query
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("recall requires either query or memory_id"))?;

    let now = Utc::now();

    // 1. Resolve which spaces to search
    let key_ring = store.get_key_ring(agent_id).await?;
    let search_spaces = resolve_spaces(&key_ring, &input.spaces, agent_id);

    // 2. Embed query
    let query_embedding = embedder.embed(query).await?;

    // 3. Per-space HNSW search (shared by all strategies)
    // A filtered recall needs a much wider pool: filters are applied to the candidate list
    // *after* it is cut, so anything matching the filter but outside the cosine top-N was
    // unreachable. Asking for "the best X that is also a decision" requires enough
    // candidates for the decisions to appear at all.
    let floor = if input.filter.is_some() {
        scoring.filtered_min_candidates
    } else {
        scoring.min_candidates
    };
    let candidate_limit = (input.limit * scoring.oversample_factor).max(floor);
    let (hnsw_candidates, rotated_queries) = hnsw_search_all_spaces(
        index,
        rotation,
        &search_spaces,
        &query_embedding,
        candidate_limit,
    )
    .await?;

    // 4. Branch on strategy to produce final candidate list
    let mut all_candidates = match input.search_strategy {
        SearchStrategy::CosineOnly => hnsw_candidates,
        SearchStrategy::HybridBm25Inject => {
            bm25_inject(
                store,
                &hnsw_candidates,
                &rotated_queries,
                query,
                &search_spaces,
                candidate_limit,
            )
            .await?
        }
        SearchStrategy::HybridRrf => {
            rrf_merge(
                store,
                &hnsw_candidates,
                query,
                &search_spaces,
                candidate_limit,
            )
            .await?
        }
    };

    // 5. Load metadata for all candidates (batch)
    let mut memories: HashMap<Uuid, Memory> = HashMap::new();
    for c in &all_candidates {
        if !memories.contains_key(&c.memory_id) {
            match store.get_memory(c.memory_id).await {
                Ok(m) => {
                    memories.insert(c.memory_id, m);
                }
                Err(StoreError::NotFound(_)) => continue, // tombstoned
                Err(e) => return Err(e.into()),
            }
        }
    }

    // 6. Apply recency boost
    for c in &mut all_candidates {
        if let Some(mem) = memories.get(&c.memory_id) {
            let recency = recency_score(mem.created_at, now);
            c.final_score = blend_score(c.cosine_score, recency, scoring.recency_weight);
        }
    }

    // Sort by final score descending
    all_candidates.sort_by(|a, b| {
        b.final_score
            .partial_cmp(&a.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 7. Exclude superseded memories (always, unless caller explicitly requests a belief_state)
    let caller_wants_superseded = input
        .filter
        .as_ref()
        .map_or(false, |f| f.belief_state == Some(BeliefState::Superseded));

    if !caller_wants_superseded {
        all_candidates.retain(|c| {
            if let Some(mem) = memories.get(&c.memory_id) {
                mem.belief_state != Some(BeliefState::Superseded)
            } else {
                false
            }
        });
    }

    // 8. Apply remaining filters
    if let Some(ref filter) = input.filter {
        all_candidates.retain(|c| {
            if let Some(mem) = memories.get(&c.memory_id) {
                passes_filter(mem, filter)
            } else {
                false
            }
        });
    }

    // 9. Deduplicate
    let deduped = deduplicate(&all_candidates, &memories);

    // 10. Truncate and build results
    let mut results = Vec::new();
    for entry in deduped.into_iter().take(input.limit) {
        let mem = match memories.get(&entry.primary.memory_id) {
            Some(m) => m,
            None => continue,
        };

        let (content, parent_content, parent_id, chunk_index, chunk_count) = if mem.is_chunk {
            resolve_chunk_content(store, mem, input.content_mode).await?
        } else {
            (mem.content.clone(), None, None, None, None)
        };

        results.push(RecallResult {
            memory_id: entry.primary.memory_id,
            content,
            parent_content,
            metadata: RecallResultMetadata {
                kind: mem.kind,
                source: mem.source,
                tags: mem.tags.clone(),
                created_at: mem.created_at,
                importance: mem.importance,
                confidence: mem.confidence,
                belief_state: mem.belief_state,
                source_agent: mem.source_agent.clone(),
            },
            score: entry.primary.final_score,
            cosine_score: entry.primary.cosine_score,
            space_id: entry.primary.space_id.clone(),
            parent_id,
            chunk_index,
            chunk_count,
            duplicates: entry.duplicates, // set below
        });
    }

    // A contradiction check used to run here on every recall, against a table where 1 in 4
    // live memories was flagged and nothing was ever resolvable. Removed 2026-07-26.

    Ok(RecallOutput { results })
}

// ── HNSW Search (shared) ──────────────────────────────────────────

/// Run HNSW search across all spaces, returning candidates and the per-space rotated queries.
async fn hnsw_search_all_spaces(
    index: &dyn VectorIndex,
    rotation: &RotationManager,
    search_spaces: &[String],
    query_embedding: &[f32],
    candidate_limit: usize,
) -> Result<(Vec<ScoredCandidate>, HashMap<String, Vec<f32>>), anyhow::Error> {
    let mut candidates = Vec::new();
    let mut rotated_queries = HashMap::new();

    for space_id in search_spaces {
        let rotated_query = rotation.rotate(space_id, query_embedding)?;

        let results = match index
            .search(space_id, &rotated_query, candidate_limit)
            .await
        {
            Ok(r) => r,
            Err(crate::index::IndexError::SpaceNotIndexed(_)) => {
                rotated_queries.insert(space_id.clone(), rotated_query);
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        for r in results {
            let cosine = r.cosine_score.clamp(0.0, 1.0);
            candidates.push(ScoredCandidate {
                memory_id: r.memory_id,
                space_id: space_id.clone(),
                cosine_score: cosine,
                final_score: 0.0,
            });
        }

        rotated_queries.insert(space_id.clone(), rotated_query);
    }

    Ok((candidates, rotated_queries))
}

// ── Strategy A: BM25 Inject ───────────────────────────────────────

/// BM25 as candidate injector with score boosting.
///
/// For candidates found in BOTH HNSW and FTS: boost their cosine score by a BM25 factor.
/// For candidates found ONLY in FTS: use normalized BM25 as their content score.
/// This ensures that strong keyword matches surface even when embedding similarity is low.
async fn bm25_inject(
    store: &dyn MemoryStore,
    hnsw_candidates: &[ScoredCandidate],
    _rotated_queries: &HashMap<String, Vec<f32>>,
    query: &str,
    search_spaces: &[String],
    limit: usize,
) -> Result<Vec<ScoredCandidate>, anyhow::Error> {
    let mut all = hnsw_candidates.to_vec();

    // Run FTS5 search
    let fts_results = store.fts_search(query, search_spaces, limit).await?;

    if fts_results.is_empty() {
        return Ok(all);
    }

    // Normalize BM25 scores to [0, 1]
    let max_bm25 = fts_results
        .iter()
        .map(|r| r.bm25_score)
        .fold(0.0f32, f32::max);
    let bm25_boost = 0.3; // How much BM25 boosts existing HNSW candidates

    // Get the max HNSW cosine for calibrating FTS-only scores
    let max_hnsw_cosine = hnsw_candidates
        .iter()
        .map(|c| c.cosine_score)
        .fold(0.0f32, f32::max);

    // Conditional FTS override: only inject FTS-only candidates when cosine is "weak."
    // When cosine already has a strong match (>0.55), the embedding model understood
    // the query — don't let keyword matches override it.
    // When cosine is weak (<0.55), the model failed — trust keywords.
    let cosine_is_weak = max_hnsw_cosine < 0.55;

    for fts in &fts_results {
        let normalized_bm25 = if max_bm25 > 0.0 {
            fts.bm25_score / max_bm25
        } else {
            0.0
        };

        if let Some(candidate) = all.iter_mut().find(|c| c.memory_id == fts.memory_id) {
            // Already in HNSW: boost cosine score with BM25 signal (always safe)
            candidate.cosine_score =
                candidate.cosine_score * (1.0 - bm25_boost) + normalized_bm25 * bm25_boost;
        } else if cosine_is_weak {
            // FTS-only + weak cosine: HNSW failed, trust keyword match.
            // Score at 30% above the best HNSW cosine (floor 0.6) so FTS rank 1
            // clearly beats HNSW rank 1 even after recency tiebreaking.
            let fts_ceiling = (max_hnsw_cosine * 1.3).max(0.6);
            let fts_score = normalized_bm25 * fts_ceiling;
            all.push(ScoredCandidate {
                memory_id: fts.memory_id,
                space_id: fts.space_id.clone(),
                cosine_score: fts_score,
                final_score: 0.0,
            });
        }
        // else: FTS-only + strong cosine → skip injection (cosine already found good results)
    }

    Ok(all)
}

// ── Strategy B: Reciprocal Rank Fusion ────────────────────────────

/// RRF merge: combine HNSW and FTS5 ranked lists via reciprocal rank fusion.
///
/// Candidates appearing in both lists get boosted (dual-source signal).
/// The key insight: max_possible = 1/(K+1) per source = 2/(K+1) for dual-source.
/// Normalizing by max_possible (not max_observed) preserves the dual-source advantage.
async fn rrf_merge(
    store: &dyn MemoryStore,
    hnsw_candidates: &[ScoredCandidate],
    query: &str,
    search_spaces: &[String],
    limit: usize,
) -> Result<Vec<ScoredCandidate>, anyhow::Error> {
    const K: f32 = 60.0;

    // HNSW ranked list (sorted by cosine desc)
    let mut hnsw_sorted = hnsw_candidates.to_vec();
    hnsw_sorted.sort_by(|a, b| {
        b.cosine_score
            .partial_cmp(&a.cosine_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // FTS5 ranked list
    let fts_results = store.fts_search(query, search_spaces, limit).await?;

    // Compute RRF scores
    let mut rrf_scores: HashMap<Uuid, (f32, String, bool, bool)> = HashMap::new(); // (score, space, in_hnsw, in_fts)

    // HNSW contribution
    for (rank, c) in hnsw_sorted.iter().enumerate() {
        let rrf_contrib = 1.0 / (K + rank as f32 + 1.0);
        let entry =
            rrf_scores
                .entry(c.memory_id)
                .or_insert((0.0, c.space_id.clone(), false, false));
        entry.0 += rrf_contrib;
        entry.2 = true; // in_hnsw
    }

    // FTS contribution
    for (rank, fts) in fts_results.iter().enumerate() {
        let rrf_contrib = 1.0 / (K + rank as f32 + 1.0);
        let entry =
            rrf_scores
                .entry(fts.memory_id)
                .or_insert((0.0, fts.space_id.clone(), false, false));
        entry.0 += rrf_contrib;
        entry.3 = true; // in_fts
    }

    // Normalize by single-source max: 1/(K+1)
    // Then boost FTS-only candidates: when HNSW completely misses a memory
    // but FTS finds it, the embedding model has failed — trust keyword match.
    let single_source_max = 1.0 / (K + 1.0);

    let candidates: Vec<ScoredCandidate> = rrf_scores
        .into_iter()
        .map(|(memory_id, (rrf_score, space_id, in_hnsw, in_fts))| {
            let mut normalized = rrf_score / single_source_max;

            // FTS-only boost: HNSW missed this entirely, keyword match should dominate.
            // Boost by 30% so FTS rank 1 clearly beats HNSW rank 1.
            if in_fts && !in_hnsw {
                normalized *= 1.3;
            }

            ScoredCandidate {
                memory_id,
                space_id,
                cosine_score: normalized,
                final_score: 0.0,
            }
        })
        .collect();

    Ok(candidates)
}

// ── Chunk Content Resolution ──────────────────────────────────────

async fn resolve_chunk_content(
    store: &dyn MemoryStore,
    mem: &Memory,
    content_mode: ContentMode,
) -> Result<
    (
        String,
        Option<String>,
        Option<Uuid>,
        Option<u16>,
        Option<u16>,
    ),
    anyhow::Error,
> {
    let parent_id = mem.parent_id;
    let chunk_index = mem.chunk_index;

    match content_mode {
        ContentMode::Chunk => {
            let chunk_count = if let Some(pid) = parent_id {
                store.get_chunks(pid).await.map(|c| c.len() as u16).ok()
            } else {
                None
            };
            Ok((
                mem.content.clone(),
                None,
                parent_id,
                chunk_index,
                chunk_count,
            ))
        }
        ContentMode::Parent => {
            if let Some(pid) = parent_id {
                let parent = store.get_memory(pid).await?;
                let chunk_count = store.get_chunks(pid).await.map(|c| c.len() as u16).ok();
                Ok((parent.content, None, Some(pid), chunk_index, chunk_count))
            } else {
                Ok((mem.content.clone(), None, None, chunk_index, None))
            }
        }
        ContentMode::Both => {
            if let Some(pid) = parent_id {
                let parent = store.get_memory(pid).await?;
                let chunk_count = store.get_chunks(pid).await.map(|c| c.len() as u16).ok();
                Ok((
                    mem.content.clone(),
                    Some(parent.content),
                    Some(pid),
                    chunk_index,
                    chunk_count,
                ))
            } else {
                Ok((mem.content.clone(), None, None, chunk_index, None))
            }
        }
    }
}

// ── Direct Fetch ──────────────────────────────────────────────────

/// Direct fetch by memory ID.
async fn direct_fetch(
    store: &dyn MemoryStore,
    memory_id: Uuid,
    content_mode: ContentMode,
) -> Result<RecallOutput, anyhow::Error> {
    let mem = store.get_memory(memory_id).await?;

    let (content, parent_content, parent_id, chunk_index, chunk_count) = if mem.is_chunk {
        resolve_chunk_content(store, &mem, content_mode).await?
    } else if store.is_parent(memory_id).await.unwrap_or(false) {
        let chunks = store.get_chunks(memory_id).await?;
        (
            mem.content.clone(),
            None,
            None,
            None,
            Some(chunks.len() as u16),
        )
    } else {
        (mem.content.clone(), None, None, None, None)
    };

    let result = RecallResult {
        memory_id: mem.id,
        content,
        parent_content,
        metadata: RecallResultMetadata {
            kind: mem.kind,
            source: mem.source,
            tags: mem.tags.clone(),
            created_at: mem.created_at,
            importance: mem.importance,
            confidence: mem.confidence,
            belief_state: mem.belief_state,
            source_agent: mem.source_agent.clone(),
        },
        score: 1.0,
        cosine_score: 1.0,
        space_id: mem.space_id.clone(),
        parent_id,
        chunk_index,
        chunk_count,
        duplicates: Vec::new(),
    };

    Ok(RecallOutput {
        results: vec![result],
    })
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Resolve which space IDs to search based on scope and key ring.
fn resolve_spaces(key_ring: &[KeyRingEntry], scope: &SpaceScope, agent_id: &str) -> Vec<String> {
    match scope {
        SpaceScope::All => key_ring.iter().map(|e| e.space_id.clone()).collect(),
        SpaceScope::Private => {
            let private_id = format!("agent:{agent_id}");
            key_ring
                .iter()
                .filter(|e| e.space_id == private_id)
                .map(|e| e.space_id.clone())
                .collect()
        }
        SpaceScope::Commons => key_ring
            .iter()
            .filter(|e| e.space_id == "commons")
            .map(|e| e.space_id.clone())
            .collect(),
        SpaceScope::Specific(ids) => key_ring
            .iter()
            .filter(|e| ids.contains(&e.space_id))
            .map(|e| e.space_id.clone())
            .collect(),
    }
}

/// Check if a memory passes a filter.
fn passes_filter(mem: &Memory, filter: &RecallFilter) -> bool {
    if let Some(kind) = filter.kind {
        if mem.kind != kind {
            return false;
        }
    }
    if let Some(source) = filter.source {
        if mem.source != source {
            return false;
        }
    }
    if !filter.tags.is_empty() && !filter.tags.iter().any(|t| mem.tags.contains(t)) {
        return false;
    }
    if let Some(start) = filter.date_start {
        if mem.created_at < start {
            return false;
        }
    }
    if let Some(end) = filter.date_end {
        if mem.created_at > end {
            return false;
        }
    }
    if let Some(min) = filter.importance_min {
        if mem.importance.unwrap_or(0.0) < min {
            return false;
        }
    }
    if let Some(min) = filter.confidence_min {
        if mem.confidence.unwrap_or(0.0) < min {
            return false;
        }
    }
    if let Some(state) = filter.belief_state {
        if mem.belief_state != Some(state) {
            return false;
        }
    }
    true
}

/// Internal scored candidate during retrieval.
#[derive(Debug, Clone)]
struct ScoredCandidate {
    memory_id: Uuid,
    space_id: String,
    cosine_score: f32,
    final_score: f32,
}

/// A deduplicated entry with provenance.
struct DedupEntry {
    primary: ScoredCandidate,
    duplicates: Vec<DuplicateInfo>,
}

/// Deduplicate candidates. Filter must have already run.
fn deduplicate(
    candidates: &[ScoredCandidate],
    memories: &HashMap<Uuid, Memory>,
) -> Vec<DedupEntry> {
    let mut groups: HashMap<DedupKey, Vec<&ScoredCandidate>> = HashMap::new();

    for c in candidates {
        let mem = match memories.get(&c.memory_id) {
            Some(m) => m,
            None => continue,
        };

        let key = if let Some(source_mem) = mem.source_memory {
            DedupKey::SourceMemory(source_mem)
        } else if mem.is_chunk {
            if let Some(pid) = mem.parent_id {
                // Key on the parent ALONE. Including chunk_index gave every fragment of a
                // document its own group, so a single long memory could fill all ten
                // result slots with consecutive slices of itself — observed doing exactly
                // that in the worst probe of the 2026-07-25 audit. Grouping by parent
                // surfaces the best-matching fragment and returns the rest as duplicates.
                DedupKey::ParentChunk(pid)
            } else {
                DedupKey::ContentHash(mem.content_hash)
            }
        } else {
            DedupKey::ContentHash(mem.content_hash)
        };

        groups.entry(key).or_default().push(c);
    }

    let mut entries: Vec<DedupEntry> = Vec::new();

    for (_key, mut group) in groups {
        group.sort_by(|a, b| {
            b.final_score
                .partial_cmp(&a.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let a_private = a.space_id.starts_with("agent:");
                    let b_private = b.space_id.starts_with("agent:");
                    b_private.cmp(&a_private)
                })
        });

        let primary = group[0].clone();
        let duplicates: Vec<DuplicateInfo> = group[1..]
            .iter()
            .map(|c| DuplicateInfo {
                space_id: c.space_id.clone(),
                memory_id: c.memory_id,
                score: c.final_score,
            })
            .collect();

        entries.push(DedupEntry {
            primary,
            duplicates,
        });
    }

    entries.sort_by(|a, b| {
        b.primary
            .final_score
            .partial_cmp(&a.primary.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    entries
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum DedupKey {
    SourceMemory(Uuid),
    ContentHash([u8; 32]),
    /// All chunks of one parent share a group, so a single document cannot dominate.
    ParentChunk(Uuid),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(id: Uuid, parent: Option<Uuid>, chunk_index: Option<u16>) -> Memory {
        Memory {
            id,
            content: format!("chunk {chunk_index:?} of {parent:?}"),
            embedding: None,
            space_id: "agent:demo".to_string(),
            kind: MemoryKind::Observation,
            source: MemorySource::Agent,
            tags: vec![],
            created_at: Utc::now(),
            content_hash: content_hash(&id.to_string()),
            importance: Some(0.5),
            confidence: None,
            belief_state: Some(BeliefState::Active),
            parent_id: parent,
            chunk_index,
            is_chunk: parent.is_some(),
            source_agent: None,
            source_memory: None,
            source_space: None,
            shared_at: None,
        }
    }

    fn candidate(id: Uuid, score: f32) -> ScoredCandidate {
        ScoredCandidate {
            memory_id: id,
            space_id: "agent:demo".to_string(),
            cosine_score: score,
            final_score: score,
        }
    }

    /// Regression: every fragment of one document used to form its own dedup group, so a
    /// single long memory could fill all ten result slots with consecutive slices of
    /// itself. Observed doing exactly that in the worst probe of the 2026-07-25 audit.
    #[test]
    fn chunk_siblings_collapse_into_one_entry() {
        let parent = Uuid::new_v4();
        let other = Uuid::new_v4();

        let mut memories = HashMap::new();
        let mut candidates = Vec::new();

        // Five chunks of the same document, descending score.
        for i in 0..5u16 {
            let id = Uuid::new_v4();
            memories.insert(id, mem(id, Some(parent), Some(i)));
            candidates.push(candidate(id, 0.9 - (i as f32) * 0.01));
        }
        // One chunk of a different document, scoring below all of them.
        let other_chunk = Uuid::new_v4();
        memories.insert(other_chunk, mem(other_chunk, Some(other), Some(0)));
        candidates.push(candidate(other_chunk, 0.5));

        let deduped = deduplicate(&candidates, &memories);

        assert_eq!(
            deduped.len(),
            2,
            "expected one entry per document, got {}",
            deduped.len()
        );
        // The winning fragment is the best-scoring one, and its siblings are retained
        // as duplicates rather than discarded.
        let top = &deduped[0];
        assert!((top.primary.final_score - 0.9).abs() < 1e-6);
        assert_eq!(
            top.duplicates.len(),
            4,
            "siblings should be reported, not dropped"
        );
    }

    /// Distinct documents must still occupy distinct slots -- collapsing by parent must
    /// not over-merge.
    #[test]
    fn distinct_documents_keep_distinct_slots() {
        let mut memories = HashMap::new();
        let mut candidates = Vec::new();
        for i in 0..4 {
            let parent = Uuid::new_v4();
            let id = Uuid::new_v4();
            memories.insert(id, mem(id, Some(parent), Some(0)));
            candidates.push(candidate(id, 0.9 - (i as f32) * 0.05));
        }

        assert_eq!(deduplicate(&candidates, &memories).len(), 4);
    }

    /// Regression: recency used to outrank relevance. The measured median cosine spread
    /// across a whole top-10 is 0.054, while a 1-day-vs-90-day age gap was worth 0.0722 --
    /// so age decided rankings outright. Seen live: cosine 0.716 ranked fourth behind
    /// cosine 0.691.
    #[test]
    fn recency_breaks_ties_without_overturning_a_better_match() {
        let w = ScoringConfig::default().recency_weight;
        let now = Utc::now();
        let fresh = recency_score(now, now);
        let old = recency_score(now - chrono::Duration::days(90), now);

        // A clearly better match that is 90 days old must still beat a fresh weaker one.
        let better_but_old = blend_score(0.716, old, w);
        let worse_but_fresh = blend_score(0.691, fresh, w);
        assert!(
            better_but_old > worse_but_fresh,
            "recency overturned a 0.025 cosine advantage: {better_but_old} vs {worse_but_fresh}"
        );

        // But recency still decides between genuinely comparable matches.
        let tie_fresh = blend_score(0.700, fresh, w);
        let tie_old = blend_score(0.700, old, w);
        assert!(tie_fresh > tie_old, "recency should still break exact ties");
    }

    /// The age gap must be worth less than the typical cosine spread, or it dominates.
    #[test]
    fn age_is_worth_less_than_the_median_cosine_spread() {
        let w = ScoringConfig::default().recency_weight;
        let now = Utc::now();
        let gap =
            (recency_score(now, now) - recency_score(now - chrono::Duration::days(90), now)) * w;
        assert!(
            gap < 0.054,
            "a 90-day age gap is worth {gap} cosine-equivalent, exceeding the measured \
             median top-10 spread of 0.054"
        );
    }
}
