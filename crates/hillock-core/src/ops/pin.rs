//! `pin` / `unpin` — mark memories for always-inject.
//!
//! Pinned memories appear in every `pack_context` call regardless of relevance.
//! Hard cap of [`MAX_PINS`] pins per agent. Pinning chunks is rejected.

use uuid::Uuid;

use crate::model::*;
use crate::store::MemoryStore;

/// Maximum pinned memories per agent.
/// Pin slots per agent.
///
/// Five, decided after the tool contract advertised "Max 5 pins" while the
/// engine enforced 3. The advertised number is the correct one: agents get
/// five slots.
///
/// A pin exempts a memory from decay and pruning **and** places the memory first in the
/// session-start context pack (`ops::pack_context`), ahead of the importance-ordered fill.
/// That pack is what the host harness can inject after static persona files,
/// so a pinned memory can be loaded into an agent's context each session —
/// within that pack's token budget, not unconditionally.
pub const MAX_PINS: u8 = 5;

/// Pin a memory. Returns error info if at cap or memory is a chunk.
pub async fn pin(
    store: &dyn MemoryStore,
    agent_id: &str,
    memory_id: Uuid,
) -> Result<PinOutput, anyhow::Error> {
    // Check memory exists
    let _mem = store.get_memory(memory_id).await?;

    // Reject chunks
    if store.is_chunk(memory_id).await? {
        return Ok(PinOutput {
            pinned: false,
            pin_count: store.get_pinned(agent_id).await?.len() as u8,
            error: Some("cannot pin a chunk — pin the parent instead".to_string()),
        });
    }

    // Check already pinned
    if store.is_pinned(agent_id, memory_id).await? {
        let count = store.get_pinned(agent_id).await?.len() as u8;
        return Ok(PinOutput {
            pinned: false,
            pin_count: count,
            error: Some("memory is already pinned".to_string()),
        });
    }

    // Check cap
    let current = store.get_pinned(agent_id).await?;
    if current.len() as u8 >= MAX_PINS {
        return Ok(PinOutput {
            pinned: false,
            pin_count: current.len() as u8,
            error: Some(format!("max {MAX_PINS} pins reached — unpin one first")),
        });
    }

    store.pin_memory(agent_id, memory_id).await?;

    let new_count = store.get_pinned(agent_id).await?.len() as u8;
    Ok(PinOutput {
        pinned: true,
        pin_count: new_count,
        error: None,
    })
}

/// Unpin a memory.
pub async fn unpin(
    store: &dyn MemoryStore,
    agent_id: &str,
    memory_id: Uuid,
) -> Result<UnpinOutput, anyhow::Error> {
    store.unpin_memory(agent_id, memory_id).await?;
    let count = store.get_pinned(agent_id).await?.len() as u8;
    Ok(UnpinOutput {
        unpinned: true,
        pin_count: count,
    })
}
