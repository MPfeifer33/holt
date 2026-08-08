//! `pack_context` — assemble a context payload for injection.
//!
//! Two modes:
//! - Session start (no query): pinned first, then importance-ordered fill.
//! - Turn injection (with query): pinned first, then KNN results.
//!
//! Returns an ordered list of memories fitted to a token budget.

use std::collections::HashSet;

use crate::embed::Embedder;
use crate::index::VectorIndex;
use crate::model::*;
use crate::rotation::RotationManager;
use crate::store::MemoryStore;

/// Estimate token count from content. Simple heuristic: chars / 4.
fn estimate_tokens(content: &str) -> usize {
    content.len() / 4
}

/// Pack context for an agent.
pub async fn pack_context(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    embedder: &dyn Embedder,
    rotation: &RotationManager,
    agent_id: &str,
    scoring: &ScoringConfig,
    input: PackContextInput,
) -> Result<PackContextOutput, anyhow::Error> {
    let mut memories: Vec<PackedMemory> = Vec::new();
    let mut used_tokens = 0usize;
    let mut seen_ids: HashSet<uuid::Uuid> = input.exclude_ids.iter().copied().collect();
    let truncated;

    // Phase 1: Pinned memories (always included, never excluded by budget)
    let pinned_ids = store.get_pinned(agent_id).await?;
    for pid in &pinned_ids {
        if seen_ids.contains(pid) {
            continue;
        }
        if let Ok(mem) = store.get_memory(*pid).await {
            // If it's a parent, reassemble content from chunks
            let content = if store.is_parent(*pid).await.unwrap_or(false) {
                let chunks = store.get_chunks(*pid).await?;
                chunks
                    .into_iter()
                    .map(|c| c.content)
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                mem.content.clone()
            };

            let tokens = estimate_tokens(&content);
            used_tokens += tokens;
            seen_ids.insert(*pid);

            memories.push(PackedMemory {
                memory_id: *pid,
                content,
                kind: mem.kind,
                source: mem.source,
                tags: mem.tags,
                created_at: mem.created_at,
                importance: mem.importance,
                pinned: true,
                score: None,
                space_id: mem.space_id,
            });
        }
    }

    // Phase 2: Fill remaining budget
    let remaining_budget = input.token_budget.saturating_sub(used_tokens);

    if let Some(query) = &input.query {
        // KNN mode: use recall pipeline
        let recall_input = RecallInput {
            query: Some(query.clone()),
            memory_id: None,
            limit: 20, // Oversample, we'll cut by budget
            filter: None,
            spaces: input.spaces.clone(),
            content_mode: ContentMode::Parent, // Get full content for parents
            search_strategy: SearchStrategy::default(),
        };

        let output = crate::ops::recall::recall(
            store,
            index,
            embedder,
            rotation,
            agent_id,
            scoring,
            recall_input,
        )
        .await?;

        let mut fill_tokens = 0usize;
        truncated = pack_recall_results(
            &output.results,
            remaining_budget,
            &mut seen_ids,
            &mut memories,
            &mut fill_tokens,
        );
        used_tokens += fill_tokens;
    } else {
        // Importance mode: query non-chunk memories ordered by importance
        let space_ids = resolve_spaces(store, agent_id, &input.spaces).await?;
        let filter = FilterInput {
            sort_by: SortField::Importance,
            sort_order: SortOrder::Desc,
            limit: 50, // Generous oversample
            ..Default::default()
        };

        let (results, _) = store.filter_memories(&space_ids, &filter, 50, 0).await?;

        let mut fill_tokens = 0usize;
        truncated = pack_filter_results(
            &results,
            remaining_budget,
            &mut seen_ids,
            &mut memories,
            &mut fill_tokens,
        );
        used_tokens += fill_tokens;
    }

    Ok(PackContextOutput {
        memories,
        total_tokens: used_tokens,
        truncated,
    })
}

/// Pack recall results into the memory list, respecting budget.
fn pack_recall_results(
    results: &[RecallResult],
    budget: usize,
    seen: &mut HashSet<uuid::Uuid>,
    out: &mut Vec<PackedMemory>,
    tokens_used: &mut usize,
) -> bool {
    for result in results {
        if seen.contains(&result.memory_id) {
            continue;
        }
        // Use parent_content if available (for chunked results), else content
        let content = result.parent_content.as_deref().unwrap_or(&result.content);
        let tokens = estimate_tokens(content);
        if *tokens_used + tokens > budget {
            return true; // Truncated
        }
        *tokens_used += tokens;
        seen.insert(result.memory_id);

        out.push(PackedMemory {
            memory_id: result.memory_id,
            content: content.to_string(),
            kind: result.metadata.kind,
            source: result.metadata.source,
            tags: result.metadata.tags.clone(),
            created_at: result.metadata.created_at,
            importance: result.metadata.importance,
            pinned: false,
            score: Some(result.score),
            space_id: result.space_id.clone(),
        });
    }
    false
}

/// Pack filter results into the memory list, respecting budget.
fn pack_filter_results(
    results: &[FilterResult],
    budget: usize,
    seen: &mut HashSet<uuid::Uuid>,
    out: &mut Vec<PackedMemory>,
    tokens_used: &mut usize,
) -> bool {
    for result in results {
        if seen.contains(&result.memory_id) {
            continue;
        }
        let tokens = estimate_tokens(&result.content);
        if *tokens_used + tokens > budget {
            return true;
        }
        *tokens_used += tokens;
        seen.insert(result.memory_id);

        out.push(PackedMemory {
            memory_id: result.memory_id,
            content: result.content.clone(),
            kind: result.kind,
            source: result.source,
            tags: result.tags.clone(),
            created_at: result.created_at,
            importance: result.importance,
            pinned: result.pinned,
            score: None,
            space_id: result.space_id.clone(),
        });
    }
    false
}

/// Resolve SpaceScope to concrete space IDs.
async fn resolve_spaces(
    store: &dyn MemoryStore,
    agent_id: &str,
    scope: &SpaceScope,
) -> Result<Vec<String>, anyhow::Error> {
    match scope {
        SpaceScope::All => {
            let ring = store.get_key_ring(agent_id).await?;
            Ok(ring.into_iter().map(|e| e.space_id).collect())
        }
        SpaceScope::Private => Ok(vec![format!("agent:{agent_id}")]),
        SpaceScope::Commons => Ok(vec!["commons".to_string()]),
        SpaceScope::Specific(ids) => Ok(ids.clone()),
    }
}
