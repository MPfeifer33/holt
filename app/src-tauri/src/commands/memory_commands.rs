use std::sync::Arc;

use tauri::State;

use super::error::CommandError;
use crate::AppState;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct MemoryStats {
    pub available: bool,
    pub backend: String,
    pub tier_counts: MemoryTierCounts,
    pub total_tokens: usize,
    pub last_injection_tokens: usize,
    pub budget_max: u32,
    pub pinned_count: usize,
}

#[derive(serde::Serialize)]
pub struct MemoryTierCounts {
    pub hot: usize,
    pub warm: usize,
    pub cold: usize,
    pub archive: usize,
}

#[derive(serde::Serialize)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub tier: String,
    pub category: String,
    pub score: f64,
    pub source: String,
    pub pinned: bool,
    pub created_at: String,
    pub tokens: usize,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_memory_stats(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<MemoryStats, CommandError> {
    let engine = state
        .get_memory_engine()
        .ok_or_else(|| CommandError::internal("Memory system not available"))?;

    let agent_ns = crate::memory::MemoryEngine::agent_namespace(&agent_id);

    let results = engine
        .export(None, None, Some(&agent_ns), None)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;

    let mut counts = MemoryTierCounts {
        hot: 0,
        warm: 0,
        cold: 0,
        archive: 0,
    };
    let mut total_tokens = 0;
    let mut pinned_count = 0;
    for r in &results {
        total_tokens += crate::memory::embeddings::count_tokens(&r.content);
        if r.pinned {
            pinned_count += 1;
        }
        match r.tier.as_str() {
            "hot" => counts.hot += 1,
            "warm" => counts.warm += 1,
            "cold" => counts.cold += 1,
            "archive" => counts.archive += 1,
            _ => counts.warm += 1,
        }
    }

    let last_injection_tokens = state
        .last_injection_tokens
        .read()
        .await
        .get(&agent_id)
        .copied()
        .unwrap_or(0);

    Ok(MemoryStats {
        available: true,
        backend: "bridge".to_string(),
        tier_counts: counts,
        total_tokens,
        last_injection_tokens,
        // Injection is retired — nothing is auto-loaded into a turn.
        budget_max: 0,
        pinned_count,
    })
}

#[tauri::command]
pub async fn get_memory_entries(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    limit: Option<usize>,
) -> Result<Vec<MemoryEntry>, CommandError> {
    let engine = state
        .get_memory_engine()
        .ok_or_else(|| CommandError::internal("Memory system not available"))?;

    let agent_ns = crate::memory::MemoryEngine::agent_namespace(&agent_id);

    let results = engine
        .export(None, None, Some(&agent_ns), Some(limit.unwrap_or(50)))
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;

    Ok(results
        .into_iter()
        .map(|r| MemoryEntry {
            id: r.id.clone(),
            content: r.content.clone(),
            tier: r.tier,
            category: r.category,
            score: r.score,
            source: r.source,
            pinned: r.pinned,
            created_at: r.created_at,
            tokens: crate::memory::embeddings::count_tokens(&r.content),
        })
        .collect())
}

/// Search an agent's memories by query string via bridge retrieval.
#[tauri::command]
pub async fn search_memories(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<MemoryEntry>, CommandError> {
    let engine = state
        .get_memory_engine()
        .ok_or_else(|| CommandError::internal("Memory system not available"))?;

    let agent_ns = crate::memory::MemoryEngine::agent_namespace(&agent_id);

    let result = engine
        .retrieve(
            &query,
            limit.unwrap_or(20),
            &agent_ns,
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;

    Ok(result
        .memories
        .into_iter()
        .map(|r| MemoryEntry {
            id: r.id.clone(),
            content: r.content.clone(),
            tier: r.tier,
            category: r.category,
            score: r.score,
            source: r.source,
            pinned: r.pinned,
            created_at: r.created_at,
            tokens: crate::memory::embeddings::count_tokens(&r.content),
        })
        .collect())
}

/// Acknowledge a blocking alert from an agent, resolving the oneshot channel
/// that the alert_user tool is waiting on.
#[tauri::command]
pub async fn acknowledge_alert(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    alert_id: String,
) -> Result<(), CommandError> {
    let key = format!("{}:{}", agent_id, alert_id);
    state.approval_manager.resolve(&key, true).await;
    Ok(())
}
