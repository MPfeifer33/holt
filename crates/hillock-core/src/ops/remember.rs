//! The `remember` operation — ingest pipeline.
//!
//! Pipeline: normalize → embed → rotate → conflict scan → store → index
//! For long content: chunk → embed each → rotate each → conflict scan → store → index

use crate::embed::Embedder;
use crate::index::VectorIndex;
use crate::model::*;
use crate::rotation::RotationManager;
use crate::store::MemoryStore;
use chrono::Utc;
use uuid::Uuid;

/// Threshold in tokens for chunking. Content over this is split.
const CHUNK_THRESHOLD_TOKENS: usize = 512;

/// Target chunk size in tokens.
const CHUNK_TARGET_TOKENS: usize = 400;

/// Overlap between chunks in tokens.
const CHUNK_OVERLAP_TOKENS: usize = 50;

/// Cosine threshold for near-duplicate detection.
const NEAR_DUPLICATE_THRESHOLD: f32 = 0.95;

/// Cosine threshold for conflict-candidate detection (lower bound).
const CONFLICT_LOWER_THRESHOLD: f32 = 0.80;

/// Execute the remember operation.
pub async fn remember(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    embedder: &dyn Embedder,
    rotation: &RotationManager,
    agent_id: &str,
    space_id: &str,
    input: RememberInput,
) -> Result<RememberOutput, anyhow::Error> {
    let now = input.created_at.unwrap_or_else(Utc::now);
    let tags = normalize_tags(&input.metadata.tags);
    let hash = content_hash(&input.content);

    // ── Hash dedup ──
    //
    // `content_hash` (BLAKE3 over trimmed, lowercased content) has always been computed and
    // stored, and never read. 42 exact-duplicate rows were live as of 2026-07-25, and the
    // contradiction detector was flagging some of them as *contradicting themselves* —
    // byte-identical text on both sides of a `value_conflict`.
    //
    // Returning the existing id is the honest answer to "remember this": the memory already
    // exists, and a second row for it adds nothing but a recall slot it can crowd out.
    if let Some(existing_id) = store.find_by_content_hash(space_id, &hash).await? {
        tracing::debug!(%existing_id, "remember: exact duplicate, returning existing memory");
        return Ok(RememberOutput {
            memory_id: existing_id,
            chunk_count: None,
            near_duplicates: Vec::new(),
            potential_conflicts: Vec::new(),
        });
    }
    let kind = input.metadata.kind.unwrap_or(MemoryKind::Observation);
    let source = input.metadata.source.unwrap_or(MemorySource::Agent);

    // Estimate token count (rough: ~4 chars per token)
    let estimated_tokens = input.content.len() / 4;

    let (memory_id, chunk_count, near_duplicates, potential_conflicts) =
        if estimated_tokens <= CHUNK_THRESHOLD_TOKENS {
            // ── Short path ──
            let embedding = embedder.embed(&input.content).await?;
            let rotated = rotation.rotate(space_id, &embedding)?;

            // Conflict-candidate scan (after rotation, before store)
            let (near_dupes, conflicts) = conflict_scan(index, space_id, &rotated).await?;

            let id = Uuid::new_v4();
            let memory = Memory {
                id,
                content: input.content,
                embedding: Some(rotated.clone()),
                space_id: space_id.to_string(),
                kind,
                source,
                tags,
                created_at: now,
                content_hash: hash,
                importance: input.metadata.importance,
                confidence: input.metadata.confidence,
                belief_state: Some(BeliefState::Active),
                parent_id: None,
                chunk_index: None,
                is_chunk: false,
                source_agent: Some(agent_id.to_string()),
                source_memory: None,
                source_space: None,
                shared_at: None,
            };

            store.insert_memory(&memory).await?;
            index.insert(space_id, id, &rotated).await?;

            (id, None, near_dupes, conflicts)
        } else {
            // ── Long path (chunking) ──
            let parent_id = Uuid::new_v4();

            // Store parent (no embedding, not indexed)
            let parent = Memory {
                id: parent_id,
                content: input.content.clone(),
                embedding: None,
                space_id: space_id.to_string(),
                kind,
                source,
                tags: tags.clone(),
                created_at: now,
                content_hash: hash,
                importance: input.metadata.importance,
                confidence: input.metadata.confidence,
                belief_state: Some(BeliefState::Active),
                parent_id: None,
                chunk_index: None,
                is_chunk: false,
                source_agent: Some(agent_id.to_string()),
                source_memory: None,
                source_space: None,
                shared_at: None,
            };
            store.insert_memory(&parent).await?;

            // Chunk the content
            let chunks = chunk_content(&input.content);
            let chunk_texts: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();

            // Batch embed
            let embeddings = embedder.embed_batch(&chunk_texts).await?;

            let mut all_near_dupes = Vec::new();
            let mut all_conflicts = Vec::new();

            for (i, (chunk_text, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
                let rotated = rotation.rotate(space_id, embedding)?;

                // Conflict scan per chunk
                let (near_dupes, conflicts) = conflict_scan(index, space_id, &rotated).await?;
                all_near_dupes.extend(near_dupes);
                all_conflicts.extend(conflicts);

                let chunk_id = Uuid::new_v4();
                let chunk_hash = content_hash(chunk_text);

                let chunk_memory = Memory {
                    id: chunk_id,
                    content: chunk_text.clone(),
                    embedding: Some(rotated.clone()),
                    space_id: space_id.to_string(),
                    kind,
                    source,
                    tags: tags.clone(),
                    created_at: now,
                    content_hash: chunk_hash,
                    importance: input.metadata.importance,
                    confidence: input.metadata.confidence,
                    belief_state: Some(BeliefState::Active),
                    parent_id: Some(parent_id),
                    chunk_index: Some(i as u16),
                    is_chunk: true,
                    source_agent: Some(agent_id.to_string()),
                    source_memory: None,
                    source_space: None,
                    shared_at: None,
                };

                store.insert_memory(&chunk_memory).await?;
                index.insert(space_id, chunk_id, &rotated).await?;
            }

            // Dedup conflict candidates
            all_near_dupes.sort_by(|a, b| {
                b.cosine_similarity
                    .partial_cmp(&a.cosine_similarity)
                    .unwrap()
            });
            all_near_dupes.dedup_by_key(|c| c.memory_id);
            all_conflicts.sort_by(|a, b| {
                b.cosine_similarity
                    .partial_cmp(&a.cosine_similarity)
                    .unwrap()
            });
            all_conflicts.dedup_by_key(|c| c.memory_id);

            (
                parent_id,
                Some(chunks.len() as u16),
                all_near_dupes,
                all_conflicts,
            )
        };

    // Flag superseded memories.
    if !input.supersedes.is_empty() {
        store
            .update_belief_state(&input.supersedes, BeliefState::Superseded)
            .await?;
    }

    Ok(RememberOutput {
        memory_id,
        chunk_count,
        near_duplicates,
        potential_conflicts,
    })
}

/// Scan for near-duplicates and conflict candidates.
async fn conflict_scan(
    index: &dyn VectorIndex,
    space_id: &str,
    rotated_vector: &[f32],
) -> Result<(Vec<ConflictCandidate>, Vec<ConflictCandidate>), anyhow::Error> {
    // Search for similar vectors (we need enough candidates to cover both bands)
    let results = index.search(space_id, rotated_vector, 20).await;

    let results = match results {
        Ok(r) => r,
        Err(crate::index::IndexError::SpaceNotIndexed(_)) => {
            // First memory in a space — no conflicts possible
            return Ok((Vec::new(), Vec::new()));
        }
        Err(e) => return Err(e.into()),
    };

    let mut near_duplicates = Vec::new();
    let mut potential_conflicts = Vec::new();

    for r in results {
        if r.cosine_score >= NEAR_DUPLICATE_THRESHOLD {
            near_duplicates.push(ConflictCandidate {
                memory_id: r.memory_id,
                content_preview: String::new(), // Filled in by caller if needed
                cosine_similarity: r.cosine_score,
                created_at: chrono::Utc::now(), // Placeholder — store lookup would be needed
            });
        } else if r.cosine_score >= CONFLICT_LOWER_THRESHOLD {
            potential_conflicts.push(ConflictCandidate {
                memory_id: r.memory_id,
                content_preview: String::new(),
                cosine_similarity: r.cosine_score,
                created_at: chrono::Utc::now(),
            });
        }
    }

    Ok((near_duplicates, potential_conflicts))
}

/// Split content into chunks with overlap.
///
/// Target: ~400 tokens per chunk, sentence-aligned boundaries, 50-token overlap.
/// Rough tokenization: split on sentences, accumulate until target reached.
fn chunk_content(content: &str) -> Vec<String> {
    let sentences: Vec<&str> = content
        .split_inclusive(|c: char| c == '.' || c == '!' || c == '?' || c == '\n')
        .collect();

    let mut chunks = Vec::new();
    let mut current_chunk = String::new();
    let mut current_tokens = 0;
    for sentence in sentences {
        let sentence_tokens = sentence.len() / 4; // rough estimate

        if current_tokens + sentence_tokens > CHUNK_TARGET_TOKENS && !current_chunk.is_empty() {
            chunks.push(current_chunk.clone());

            // Build overlap from the end of current chunk (char-boundary safe)
            let target_start = if current_chunk.len() > CHUNK_OVERLAP_TOKENS * 4 {
                current_chunk.len() - CHUNK_OVERLAP_TOKENS * 4
            } else {
                0
            };
            // Walk backward to find a valid char boundary
            let overlap_start = current_chunk.floor_char_boundary(target_start);
            let overlap = current_chunk[overlap_start..].to_string();

            current_chunk = overlap;
            current_tokens = current_chunk.len() / 4;
        }

        current_chunk.push_str(sentence);
        current_tokens += sentence_tokens;
    }

    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }

    // If we ended up with only one chunk, just return the whole thing
    if chunks.is_empty() {
        chunks.push(content.to_string());
    }

    chunks
}
