//! maintenance — decay, prune, and health operations.
//!
//! This is the "forgetting" engine. It runs on a schedule (triggered externally),
//! decays importance of aging memories and prunes those below threshold.
//!
//! It does NOT delete superseded memories. Supersession means "no longer current" —
//! recall excludes them already — not "destroy this soon".
//!
//! Design principles:
//! - Pinned memories are immune to all maintenance.
//! - No self-reinforcing loops — retrieval does not boost future retrieval.
//! - Decay is continuous, not step-wise. No tiers.
//! - SQLite is source of truth.

use chrono::Utc;
use uuid::Uuid;

use crate::index::VectorIndex;
use crate::model::*;
use crate::store::MemoryStore;

/// Run the full maintenance pipeline for an agent's accessible spaces.
pub async fn run(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    agent_id: &str,
    config: MaintenanceConfig,
) -> Result<MaintenanceOutput, anyhow::Error> {
    let start = std::time::Instant::now();

    // Resolve agent's accessible spaces
    let key_ring = store.get_key_ring(agent_id).await?;
    let space_ids: Vec<String> = key_ring.iter().map(|k| k.space_id.clone()).collect();

    if space_ids.is_empty() {
        return Ok(MaintenanceOutput {
            memories_decayed: 0,
            memories_pruned: 0,
            tombstones_cleaned: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    // Get pinned IDs upfront — these are exempt from everything
    let pinned_ids = store.get_pinned(agent_id).await?;

    // Stage 1: Decay
    let memories_decayed = run_decay(store, &space_ids, &pinned_ids, &config).await?;

    // Stage 2: Prune
    let memories_pruned = run_prune(store, index, &space_ids, &pinned_ids, &config).await?;

    // Superseded cleanup was Stage 3 until 2026-07-26. Removed, not disabled: `superseded`
    // meant both "no longer current" and "destroy in 14 days", and recall already honours
    // the first by excluding superseded memories. The deletion half added only risk — it is
    // what put 104 memories on a single-day cliff, including constraints about how to work
    // after the retention-policy decision. Superseded now means hidden and kept.

    // Stage 3: Index GC (rebuild dirty indices)
    //
    // Skipped on a dry run: an index only goes dirty because of deletions, and this run
    // performed none. Rebuilding here would be doing real work to clean up after work that
    // never happened.
    let tombstones_cleaned = if config.dry_run {
        0
    } else {
        run_index_gc(store, index, &space_ids).await?
    };

    Ok(MaintenanceOutput {
        memories_decayed,
        memories_pruned,
        tombstones_cleaned,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Stage 1: Importance decay.
///
/// For each eligible memory: `new_importance = max(importance * decay_factor, decay_floor)`
async fn run_decay(
    store: &dyn MemoryStore,
    space_ids: &[String],
    pinned_ids: &[Uuid],
    config: &MaintenanceConfig,
) -> Result<u32, anyhow::Error> {
    // Gather the full candidate set BEFORE writing anything.
    //
    // Offset pagination is only correct over a result set that isn't changing underneath
    // it. Updating a batch stamps `last_decayed_at`, which removes those rows from the
    // filter — so advancing the offset afterwards steps past rows that shifted down into
    // the range just consumed, silently skipping them. Reading first keeps the set stable;
    // writing after keeps each memory decayed exactly once per run.
    let mut candidates = Vec::new();
    let mut offset = 0usize;

    loop {
        let page = store
            .decay_candidates(
                space_ids,
                pinned_ids,
                config.decay_min_age_days,
                config.decay_floor,
                config.decay_min_interval_hours,
                config.batch_size,
                offset,
            )
            .await?;

        let page_len = page.len();
        candidates.extend(page);

        if page_len < config.batch_size {
            break;
        }
        offset += config.batch_size;
    }

    if candidates.is_empty() {
        return Ok(0);
    }

    // The candidate set IS the projection: every row selected here would be decayed.
    if config.dry_run {
        return Ok(candidates.len() as u32);
    }

    let mut total_decayed = 0u32;
    for chunk in candidates.chunks(config.batch_size) {
        let updates: Vec<(Uuid, f32)> = chunk
            .iter()
            .map(|c| {
                let new_imp = (c.importance * config.decay_factor).max(config.decay_floor);
                (c.memory_id, new_imp)
            })
            .collect();

        total_decayed += store.batch_update_importance(&updates).await?;
    }

    Ok(total_decayed)
}

/// Stage 2: Prune memories below threshold.
async fn run_prune(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    space_ids: &[String],
    pinned_ids: &[Uuid],
    config: &MaintenanceConfig,
) -> Result<u32, anyhow::Error> {
    let candidates = store
        .prune_candidates(
            space_ids,
            pinned_ids,
            config.prune_threshold,
            config.prune_min_age_days,
            config.batch_size,
        )
        .await?;

    if config.dry_run {
        return Ok(candidates.len() as u32);
    }

    let mut pruned = 0u32;

    for memory_id in &candidates {
        // Get memory for audit log
        let memory = match store.get_memory(*memory_id).await {
            Ok(m) => m,
            Err(_) => continue, // already gone
        };

        // Write audit log
        let log_entry = ForgetLogEntry {
            memory_id: *memory_id,
            content_hash: memory.content_hash,
            deleted_at: Utc::now(),
            reason: ForgetReason::DecayedBelowThreshold,
            // No actor: a threshold was crossed, nobody asked.
            actor: None,
        };
        let _ = store.insert_forget_log(&log_entry).await;

        // Delete (cascade handles chunks)
        if store.is_parent(*memory_id).await.unwrap_or(false) {
            store.delete_parent_and_chunks(*memory_id).await?;
        } else {
            store.delete_memory(*memory_id).await?;
        }

        // Remove from index
        let _ = index.delete(&memory.space_id, *memory_id).await;

        pruned += 1;
    }

    Ok(pruned)
}

/// Stage 3: Rebuild dirty HNSW indices.
async fn run_index_gc(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    space_ids: &[String],
) -> Result<u32, anyhow::Error> {
    let mut rebuilt = 0u32;

    for space_id in space_ids {
        if index.is_dirty(space_id) {
            let vectors = store.all_vectors_in_space(space_id).await?;
            index.build_space(space_id, vectors).await?;
            rebuilt += 1;
        }
    }

    Ok(rebuilt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decay_math() {
        let result = (0.5f32 * 0.98).max(0.05);
        assert!((result - 0.49).abs() < 0.001);

        let result = (0.05f32 * 0.98).max(0.05);
        assert!((result - 0.05).abs() < 0.001);

        let result = (0.9f32 * 0.98).max(0.05);
        assert!((result - 0.882).abs() < 0.001);
    }

    #[test]
    fn test_decay_over_time() {
        let mut importance = 0.5f32;
        for _ in 0..83 {
            importance = (importance * 0.98).max(0.05);
        }
        assert!(importance < 0.10);
        assert!(importance > 0.05);

        for _ in 0..200 {
            importance = (importance * 0.98).max(0.05);
        }
        assert!((importance - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_default_config() {
        let config = MaintenanceConfig::default();
        assert!((config.decay_factor - 0.98).abs() < 0.001);
        assert_eq!(config.decay_min_age_days, 7);
        assert!((config.decay_floor - 0.05).abs() < 0.001);
        assert!((config.prune_threshold - 0.05).abs() < 0.001);
        assert_eq!(config.prune_min_age_days, 30);
        assert_eq!(config.batch_size, 500);
    }
}
