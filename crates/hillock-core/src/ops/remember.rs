//! The `remember` operation — ingest pipeline.
//!
//! Pipeline: normalize → hash dedup → embed → rotate → conflict scan → store → index
//! For long content: chunk → embed all → rotate → conflict scan → store parent and
//! fragments in one transaction → index

use crate::embed::Embedder;
use crate::index::VectorIndex;
use crate::model::*;
use crate::rotation::RotationManager;
use crate::store::{MemoryStore, StoreError};
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

/// Characters of a conflict candidate's content shown to the caller.
const CONFLICT_PREVIEW_CHARS: usize = 160;

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
    //
    // But "already exists" is not "nothing to do". The repeat write may carry tags, a
    // higher importance or a `supersedes` list, and the existing row may have been
    // superseded since. Returning early dropped all of that and left the row hidden, so
    // the caller was told their memory was stored and could never recall it.
    if let Some(existing_id) = store.find_by_content_hash(space_id, &hash).await? {
        tracing::debug!(%existing_id, "remember: exact duplicate, folding into existing memory");
        let existing = store.get_memory(existing_id).await?;

        store
            .merge_tags_and_importance(existing_id, &tags, input.metadata.importance)
            .await?;
        // Saying it again is asserting it again.
        if existing.belief_state == Some(BeliefState::Superseded) {
            store
                .update_belief_state(&[existing_id], BeliefState::Active)
                .await?;
        }
        // A memory cannot supersede itself: that would undo the revival above.
        let supersedes: Vec<Uuid> = input
            .supersedes
            .iter()
            .copied()
            .filter(|id| *id != existing_id)
            .collect();
        if !supersedes.is_empty() {
            store
                .update_belief_state(&supersedes, BeliefState::Superseded)
                .await?;
        }

        return Ok(RememberOutput {
            memory_id: existing_id,
            chunk_count: None,
            near_duplicates: Vec::new(),
            potential_conflicts: Vec::new(),
            deduplicated: true,
        });
    }
    let kind = input.metadata.kind.unwrap_or(MemoryKind::Observation);
    let source = input.metadata.source.unwrap_or(MemorySource::Agent);

    // Estimate token count (rough: ~4 chars per token)
    let estimated_tokens = input.content.len() / 4;

    let (memory_id, chunk_count, near_duplicates, potential_conflicts) = if estimated_tokens
        <= CHUNK_THRESHOLD_TOKENS
    {
        // ── Short path ──
        let embedding = embedder.embed(&input.content).await?;
        let rotated = rotation.rotate(space_id, &embedding)?;

        // Conflict-candidate scan (after rotation, before store)
        let (near_dupes, conflicts) = conflict_scan(store, index, space_id, &rotated).await?;

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
        //
        // Everything that can fail without touching the store happens first:
        // chunk, embed, rotate, scan. Only then are the parent and its fragments
        // written, in one transaction. The parent used to be inserted before the
        // fragments were embedded, so an embedding failure stranded a parent that
        // owned the content hash: unfindable by meaning, and every retry
        // deduplicated onto it.
        let parent_id = Uuid::new_v4();
        let chunks = chunk_content(&input.content);
        let chunk_texts: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();

        let embeddings = embedder.embed_batch(&chunk_texts).await?;
        if embeddings.len() != chunks.len() {
            anyhow::bail!(
                "embedder returned {} vectors for {} fragments",
                embeddings.len(),
                chunks.len()
            );
        }

        // Parent first: fragments reference it. No embedding, not indexed.
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

        let mut all_near_dupes = Vec::new();
        let mut all_conflicts = Vec::new();
        let mut rows = Vec::with_capacity(chunks.len() + 1);

        for (i, (chunk_text, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
            let rotated = rotation.rotate(space_id, embedding)?;

            // Conflict scan per chunk
            let (near_dupes, conflicts) = conflict_scan(store, index, space_id, &rotated).await?;
            all_near_dupes.extend(near_dupes);
            all_conflicts.extend(conflicts);

            rows.push(Memory {
                id: Uuid::new_v4(),
                content: chunk_text.clone(),
                embedding: Some(rotated),
                space_id: space_id.to_string(),
                kind,
                source,
                tags: tags.clone(),
                created_at: now,
                content_hash: content_hash(chunk_text),
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
            });
        }
        rows.insert(0, parent);

        store.insert_memories(&rows).await?;
        for row in &rows {
            if let Some(vector) = row.embedding.as_deref() {
                index.insert(space_id, row.id, vector).await?;
            }
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
        deduplicated: false,
    })
}

/// Scan for near-duplicates and conflict candidates.
async fn conflict_scan(
    store: &dyn MemoryStore,
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
        if r.cosine_score < CONFLICT_LOWER_THRESHOLD {
            continue;
        }
        // The candidate's text and date come from the store. They used to be an empty
        // string and the time of the scan, so a caller could see that something
        // conflicted and not what, or how old it was.
        let existing = match store.get_memory(r.memory_id).await {
            Ok(m) => m,
            // In the index but gone from the store: nothing to conflict with.
            Err(StoreError::NotFound(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let candidate = ConflictCandidate {
            memory_id: r.memory_id,
            content_preview: content_preview(&existing.content),
            cosine_similarity: r.cosine_score,
            created_at: existing.created_at,
        };
        if r.cosine_score >= NEAR_DUPLICATE_THRESHOLD {
            near_duplicates.push(candidate);
        } else {
            potential_conflicts.push(candidate);
        }
    }

    Ok((near_duplicates, potential_conflicts))
}

/// The first [`CONFLICT_PREVIEW_CHARS`] characters of a memory, on one line, with an
/// ellipsis when cut. Counted in characters, so a multi-byte character is never split.
fn content_preview(content: &str) -> String {
    let one_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= CONFLICT_PREVIEW_CHARS {
        return one_line;
    }
    let mut preview: String = one_line.chars().take(CONFLICT_PREVIEW_CHARS).collect();
    preview.push('…');
    preview
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
