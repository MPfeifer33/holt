//! The `share` operation — copy memories between spaces.
//!
//! Two modes:
//! - From private: unrotate from source space, re-rotate into target, store with provenance.
//! - Direct: embed new content directly into the target space.
//!
//! Chunked sharing: share(parent_id) copies parent + all chunks. share(chunk_id) is rejected.

use crate::embed::Embedder;
use crate::index::VectorIndex;
use crate::model::*;
use crate::rotation::RotationManager;
use crate::store::{MemoryStore, StoreError};
use chrono::Utc;
use uuid::Uuid;

/// Execute the share operation.
pub async fn share(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    embedder: &dyn Embedder,
    rotation: &RotationManager,
    agent_id: &str,
    input: ShareInput,
) -> Result<ShareOutput, anyhow::Error> {
    let now = Utc::now();

    if let Some(memory_id) = input.memory_id {
        // ── Share existing memory ──
        share_existing(
            store,
            index,
            rotation,
            agent_id,
            memory_id,
            &input.target_space,
            now,
        )
        .await
    } else if let Some(content) = input.content {
        // ── Direct share (new content) ──
        share_direct(
            store,
            index,
            embedder,
            rotation,
            agent_id,
            &content,
            input.source,
            &input.target_space,
            now,
        )
        .await
    } else {
        Err(anyhow::anyhow!(
            "share requires either memory_id or content"
        ))
    }
}

/// Share an existing memory from the agent's private space into a target space.
async fn share_existing(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    rotation: &RotationManager,
    agent_id: &str,
    memory_id: Uuid,
    target_space: &str,
    now: chrono::DateTime<Utc>,
) -> Result<ShareOutput, anyhow::Error> {
    let mem = store.get_memory(memory_id).await?;

    // Reject sharing individual chunks
    if mem.is_chunk {
        return Err(StoreError::ChunkShareRejected.into());
    }

    // Check if this is a parent (chunked memory)
    let is_parent = store.is_parent(memory_id).await?;

    if is_parent {
        // ── Share parent + all chunks ──
        let chunks = store.get_chunks(memory_id).await?;
        let new_parent_id = Uuid::new_v4();

        // Create new parent (no embedding, not indexed)
        let new_parent = Memory {
            id: new_parent_id,
            content: mem.content.clone(),
            embedding: None,
            space_id: target_space.to_string(),
            kind: mem.kind,
            source: mem.source,
            tags: mem.tags.clone(),
            created_at: mem.created_at, // preserve original assertion time
            content_hash: mem.content_hash,
            importance: mem.importance,
            confidence: mem.confidence,
            belief_state: mem.belief_state,
            parent_id: None,
            chunk_index: None,
            is_chunk: false,
            source_agent: Some(agent_id.to_string()),
            source_memory: Some(memory_id),
            source_space: Some(mem.space_id.clone()),
            shared_at: Some(now),
        };
        store.insert_memory(&new_parent).await?;

        // Re-embed and rotate each chunk for the target space
        for chunk in &chunks {
            let new_chunk_id = Uuid::new_v4();

            // Unrotate from source space, re-rotate into target
            let embedding = chunk
                .embedding
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("chunk {0} has no embedding", chunk.id))?;
            let unrotated = rotation.unrotate(&mem.space_id, embedding)?;
            let re_rotated = rotation.rotate(target_space, &unrotated)?;

            let new_chunk = Memory {
                id: new_chunk_id,
                content: chunk.content.clone(),
                embedding: Some(re_rotated.clone()),
                space_id: target_space.to_string(),
                kind: chunk.kind,
                source: chunk.source,
                tags: chunk.tags.clone(),
                created_at: chunk.created_at,
                content_hash: chunk.content_hash,
                importance: chunk.importance,
                confidence: chunk.confidence,
                belief_state: chunk.belief_state,
                parent_id: Some(new_parent_id),
                chunk_index: chunk.chunk_index,
                is_chunk: true,
                source_agent: Some(agent_id.to_string()),
                source_memory: Some(chunk.id),
                source_space: Some(mem.space_id.clone()),
                shared_at: Some(now),
            };

            store.insert_memory(&new_chunk).await?;
            index
                .insert(target_space, new_chunk_id, &re_rotated)
                .await?;
        }

        Ok(ShareOutput {
            memory_id: new_parent_id,
        })
    } else {
        // ── Share standalone memory ──
        let embedding = mem
            .embedding
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("memory has no embedding"))?;

        let unrotated = rotation.unrotate(&mem.space_id, embedding)?;
        let re_rotated = rotation.rotate(target_space, &unrotated)?;

        let new_id = Uuid::new_v4();
        let new_memory = Memory {
            id: new_id,
            content: mem.content.clone(),
            embedding: Some(re_rotated.clone()),
            space_id: target_space.to_string(),
            kind: mem.kind,
            source: mem.source,
            tags: mem.tags.clone(),
            created_at: mem.created_at, // preserve original assertion time
            content_hash: mem.content_hash,
            importance: mem.importance,
            confidence: mem.confidence,
            belief_state: mem.belief_state,
            parent_id: None,
            chunk_index: None,
            is_chunk: false,
            source_agent: Some(agent_id.to_string()),
            source_memory: Some(memory_id),
            source_space: Some(mem.space_id.clone()),
            shared_at: Some(now),
        };

        store.insert_memory(&new_memory).await?;
        index.insert(target_space, new_id, &re_rotated).await?;

        Ok(ShareOutput { memory_id: new_id })
    }
}

/// Share new content directly into a target space.
async fn share_direct(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    embedder: &dyn Embedder,
    rotation: &RotationManager,
    agent_id: &str,
    content: &str,
    source: Option<MemorySource>,
    target_space: &str,
    now: chrono::DateTime<Utc>,
) -> Result<ShareOutput, anyhow::Error> {
    let embedding = embedder.embed(content).await?;
    let rotated = rotation.rotate(target_space, &embedding)?;
    let hash = content_hash(content);
    let tags = Vec::new();

    let id = Uuid::new_v4();
    let memory = Memory {
        id,
        content: content.to_string(),
        embedding: Some(rotated.clone()),
        space_id: target_space.to_string(),
        kind: MemoryKind::Observation,
        source: source.unwrap_or(MemorySource::Human),
        tags,
        created_at: now,
        content_hash: hash,
        importance: None,
        confidence: None,
        belief_state: Some(BeliefState::Active),
        parent_id: None,
        chunk_index: None,
        is_chunk: false,
        source_agent: Some(agent_id.to_string()),
        source_memory: None,
        source_space: None,
        shared_at: Some(now),
    };

    store.insert_memory(&memory).await?;
    index.insert(target_space, id, &rotated).await?;

    Ok(ShareOutput { memory_id: id })
}
