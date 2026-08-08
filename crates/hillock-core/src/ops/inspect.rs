//! The `inspect` operation — V1 diagnostics.
//!
//! V1 returns: memory counts, index health, accessible spaces, growth, recent activity.
//! Post-V1 adds: corpus complexity, semantic gaps, duplicate clusters, density assessment.

use crate::index::VectorIndex;
use crate::model::*;
use crate::store::MemoryStore;

/// Execute the inspect operation (V1).
pub async fn inspect(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    agent_id: &str,
) -> Result<InspectOutput, anyhow::Error> {
    let key_ring = store.get_key_ring(agent_id).await?;
    let accessible_spaces: Vec<String> = key_ring.iter().map(|e| e.space_id.clone()).collect();

    let private_space = format!("agent:{agent_id}");
    let private_count = store.count_memories(&private_space).await.unwrap_or(0);
    let commons_count = store.count_memories("commons").await.unwrap_or(0);
    let tombstone_count = store.count_tombstones().await.unwrap_or(0);

    // Check index health
    let index_healthy = accessible_spaces
        .iter()
        .all(|s| index.has_space(s) && !index.is_dirty(s));
    let index_dirty = accessible_spaces.iter().any(|s| index.is_dirty(s));

    // Growth history (last 30 days)
    let recent_growth = store.growth_history(30).await.unwrap_or_default();

    Ok(InspectOutput {
        memory_count: private_count + commons_count,
        private_count,
        commons_count,
        index_healthy,
        tombstone_count,
        index_dirty,
        accessible_spaces,
        recent_growth,
    })
}
