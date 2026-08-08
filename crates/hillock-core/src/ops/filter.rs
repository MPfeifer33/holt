//! `memory_filter` — metadata-only query, no embeddings.
//!
//! Pure SQL filtering by kind, source, tags, date, importance, belief state, pinned status.
//! Returns non-chunk memories only, with pagination.

use crate::model::*;
use crate::store::MemoryStore;

/// Run a metadata-only filter query.
///
/// Resolves space scope to concrete space IDs using the agent's key ring,
/// then delegates to the store.
pub async fn filter(
    store: &dyn MemoryStore,
    agent_id: &str,
    input: FilterInput,
) -> Result<FilterOutput, anyhow::Error> {
    // Resolve space scope
    let space_ids = match &input.spaces {
        SpaceScope::All => {
            let ring = store.get_key_ring(agent_id).await?;
            ring.into_iter().map(|e| e.space_id).collect::<Vec<_>>()
        }
        SpaceScope::Private => vec![format!("agent:{agent_id}")],
        SpaceScope::Commons => vec!["commons".to_string()],
        SpaceScope::Specific(ids) => ids.clone(),
    };

    if space_ids.is_empty() {
        return Ok(FilterOutput {
            results: Vec::new(),
            total_count: 0,
        });
    }

    let (results, total_count) = store
        .filter_memories(&space_ids, &input, input.limit, input.offset)
        .await?;

    Ok(FilterOutput {
        results,
        total_count,
    })
}
