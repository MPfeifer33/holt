//! The `forget` operation — deletion with cascade rules.
//!
//! - Standalone: delete memory, remove from index.
//! - Parent: delete parent AND all chunks, remove all from index.
//! - Individual chunk: REJECTED. Forget the parent instead.
//!
//! Every successful deletion writes one `forget_log` row naming the agent that asked.
//! Until 2026-07-26 it wrote none, so the audit trail covered scheduled pruning and
//! missed deliberate deletion — the path that actually has someone behind it.

use chrono::Utc;

use crate::index::VectorIndex;
use crate::model::*;
use crate::store::{MemoryStore, StoreError};

/// Execute the forget operation on behalf of `agent_id`, which is recorded as the actor.
pub async fn forget(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    agent_id: &str,
    input: ForgetInput,
) -> Result<ForgetOutput, anyhow::Error> {
    // Authorization first: the memory's space must be on the caller's key ring.
    //
    // Same predicate `recall` resolves through, so there is exactly one notion of access —
    // you can delete what you can see. This is checked before the chunk test so an
    // unauthorized caller learns nothing about the memory, not even its shape.
    //
    // Until 2026-07-26 there was no check at all: `forget` took a bare UUID and deleted
    // whatever it named, in any space, for any caller. Deletion was the only memory
    // operation without space scoping — the destructive one.
    let target = store.get_memory(input.memory_id).await?;
    let key_ring = store.get_key_ring(agent_id).await?;
    if !key_ring.iter().any(|k| k.space_id == target.space_id) {
        return Err(StoreError::NotAuthorized {
            agent: agent_id.to_string(),
            space: target.space_id.clone(),
        }
        .into());
    }

    // Check if it's a chunk — reject if so.
    //
    // Rejected before anything is logged: a call that deletes nothing must leave no audit
    // row, or the log overstates what was removed.
    if store.is_chunk(input.memory_id).await? {
        return Err(StoreError::ChunkForgetRejected.into());
    }

    // Check if it's a parent (has chunks)
    if store.is_parent(input.memory_id).await? {
        // Get all chunks first so we can remove them from the index
        let chunks = store.get_chunks(input.memory_id).await?;
        let chunk_count = chunks.len() as u32;

        // Get the space_id from the parent
        let parent = store.get_memory(input.memory_id).await?;
        let space_id = &parent.space_id;

        // Remove all chunks from index
        for chunk in &chunks {
            index.delete(space_id, chunk.id).await?;
        }

        // One row for the parent, not one per chunk: a parent and its chunks are one
        // memory to the caller, and the prune path already logs it that way.
        write_audit_row(store, input.memory_id, parent.content_hash, agent_id).await;

        // Delete parent and all chunks from store
        let deleted = store.delete_parent_and_chunks(input.memory_id).await?;

        Ok(ForgetOutput {
            deleted_count: deleted,
            delete_type: format!("parent ({chunk_count} chunks)"),
        })
    } else {
        // Standalone memory
        let mem = store.get_memory(input.memory_id).await?;

        // Remove from index (if it has an embedding — parents don't)
        if mem.embedding.is_some() {
            index.delete(&mem.space_id, input.memory_id).await?;
        }

        write_audit_row(store, input.memory_id, mem.content_hash, agent_id).await;

        // Delete from store
        store.delete_memory(input.memory_id).await?;

        Ok(ForgetOutput {
            deleted_count: 1,
            delete_type: "standalone".to_string(),
        })
    }
}

/// Record a deliberate deletion.
///
/// Best-effort, matching the prune path: a failure to write the audit row must not leave
/// the caller believing the memory survived when it is about to be deleted anyway. Logged
/// loudly so a silently-degrading audit trail is visible rather than inferred.
async fn write_audit_row(
    store: &dyn MemoryStore,
    memory_id: uuid::Uuid,
    content_hash: [u8; 32],
    agent_id: &str,
) {
    let entry = ForgetLogEntry {
        memory_id,
        content_hash,
        deleted_at: Utc::now(),
        reason: ForgetReason::Requested,
        actor: Some(agent_id.to_string()),
    };
    if let Err(e) = store.insert_forget_log(&entry).await {
        tracing::error!(%memory_id, "failed to write forget_log row: {e}");
    }
}
