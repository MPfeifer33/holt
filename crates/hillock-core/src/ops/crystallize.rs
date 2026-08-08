//! `crystallize` — synthesize episodes into a concept memory.
//!
//! Creates a new memory from the agent-authored summary, embeds and indexes it,
//! then marks all source memories as superseded.

use crate::embed::Embedder;
use crate::index::VectorIndex;
use crate::model::*;
use crate::rotation::RotationManager;
use crate::store::MemoryStore;

/// Minimum source memories required for crystallization.
pub const MIN_SOURCES: usize = 2;

/// Crystallize multiple episodes into a single concept.
pub async fn crystallize(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    embedder: &dyn Embedder,
    rotation: &RotationManager,
    agent_id: &str,
    space_id: &str,
    input: CrystallizeInput,
) -> Result<CrystallizeOutput, anyhow::Error> {
    // Validate minimum sources
    if input.source_ids.len() < MIN_SOURCES {
        anyhow::bail!(
            "crystallize requires at least {MIN_SOURCES} source memories, got {}",
            input.source_ids.len()
        );
    }

    // Validate all sources exist and belong to this agent's spaces
    let mut max_importance: Option<f32> = None;
    for sid in &input.source_ids {
        let mem = store.get_memory(*sid).await?;
        if let Some(imp) = mem.importance {
            max_importance = Some(max_importance.map_or(imp, |m: f32| m.max(imp)));
        }
    }

    // Determine importance: explicit > max of sources > None
    let importance = input.importance.or(max_importance);
    let kind = input.kind.unwrap_or(MemoryKind::Fact);

    // Create the crystallized memory using the standard remember pipeline
    let remember_input = RememberInput {
        content: input.summary,
        metadata: RememberMetadata {
            kind: Some(kind),
            source: Some(MemorySource::Agent),
            tags: input.tags,
            importance,
            confidence: Some(1.0), // Crystallized knowledge is high-confidence
        },
        supersedes: input.source_ids.clone(),
        created_at: None, // Now
    };

    let output = crate::ops::remember::remember(
        store,
        index,
        embedder,
        rotation,
        agent_id,
        space_id,
        remember_input,
    )
    .await?;

    Ok(CrystallizeOutput {
        memory_id: output.memory_id,
        superseded: input.source_ids,
        chunk_count: output.chunk_count,
    })
}
