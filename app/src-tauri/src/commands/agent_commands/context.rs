use super::*;

/// Get context window usage status for an agent.
/// Uses API-reported token data only — no estimation.
#[derive(serde::Serialize)]
pub struct ContextStatus {
    pub context_tokens: u64,
    pub context_window_size: u32,
    pub message_count: usize,
    pub needs_compaction: bool,
}

#[tauri::command]
pub async fn get_context_status(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<ContextStatus, CommandError> {
    let agents = state.agents.read().await;
    let agent = agents
        .iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    let context_tokens = agent.current_context_tokens;
    let context_window = agent.context_window_size;

    let needs_compaction = if context_tokens > 0 && context_window > 0 {
        let percent = (context_tokens as f64 / context_window as f64 * 100.0) as u32;
        let config = state.config.read().await;
        percent >= config.context.compaction_threshold_percent
    } else {
        false
    };

    Ok(ContextStatus {
        context_tokens,
        context_window_size: context_window,
        message_count: agent.conversation.messages.len(),
        needs_compaction,
    })
}

#[tauri::command]
pub async fn compact_agent_context(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<String, CommandError> {
    // Single write lock for both checkpoint and compact to prevent
    // agent state from changing between the two operations.
    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    if agent.conversation.messages.is_empty() {
        return Err(CommandError::validation("Conversation is already empty"));
    }

    // Checkpoint (if configured) before compaction
    let config = state.config.read().await;
    let checkpoint_id = if config.context.auto_checkpoint_on_compaction {
        let trace_count = state.trace_engine.count_traces().unwrap_or(0) as u32;
        match crate::runtime::checkpoint::create_checkpoint(
            agent,
            "pre-compaction (manual)",
            trace_count,
        ) {
            Ok(cp) => cp.id,
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };

    // Compact
    let working_dir = agent.working_directory.to_string_lossy().to_string();
    let keep_recent = config.context.compaction_keep_recent;
    drop(config);

    let compacted = crate::runtime::context::compact_messages(
        &agent.conversation.messages,
        &working_dir,
        keep_recent,
        None,
    );
    agent.conversation.messages = compacted;
    agent.is_dirty = true;

    Ok(checkpoint_id)
}

#[tauri::command]
pub async fn trigger_extraction(
    agent_id: String,
    state: State<'_, Arc<crate::AppState>>,
) -> Result<serde_json::Value, CommandError> {
    use crate::memory::compaction::{
        build_active_work_md, llm_extract_context, save_fifo_summary, write_active_work,
    };
    use crate::memory::store::{MemoryCategory, MemorySource, MemoryTier, StoreInput};
    use crate::memory::MemoryEngine;

    let messages = {
        let agents = state.agents.read().await;
        let agent = agents
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;
        agent.conversation.messages.clone()
    };

    if messages.is_empty() {
        return Ok(
            serde_json::json!({"status": "skipped", "reason": "No conversation to extract from"}),
        );
    }

    let (endpoint, api_key, model, summary_tokens, keep_recent) = {
        let config = state.config.read().await;
        let ctx_config = &config.context;
        let compaction_override = &ctx_config.compaction_model_override;
        let (ep, key, mdl) = if !compaction_override.is_empty() && compaction_override != "none" {
            if let Some(conn) = config
                .connections
                .iter()
                .find(|c| c.id == *compaction_override)
            {
                (
                    conn.endpoint.clone(),
                    Some(conn.key_ref.clone()),
                    conn.provider.clone(),
                )
            } else {
                let agents = state.agents.read().await;
                let agent = agents.iter().find(|a| a.id == agent_id).ok_or_else(|| {
                    CommandError::not_found(format!("Agent not found: {}", agent_id))
                })?;
                let (ep, mdl) = crate::providers::extract_endpoint_and_model(&agent.connection)
                    .map_err(|e| {
                        CommandError::internal(format!("Endpoint resolution failed: {e}"))
                    })?;
                (ep, None, mdl)
            }
        } else {
            let agents = state.agents.read().await;
            let agent = agents
                .iter()
                .find(|a| a.id == agent_id)
                .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;
            let (ep, mdl) = crate::providers::extract_endpoint_and_model(&agent.connection)
                .map_err(|e| CommandError::internal(format!("Endpoint resolution failed: {e}")))?;
            (ep, None, mdl)
        };
        (
            ep,
            key,
            mdl,
            ctx_config.compaction_summary_tokens,
            ctx_config.compaction_keep_recent,
        )
    };

    let client = state.http_client.clone();
    let result = llm_extract_context(
        &messages,
        &client,
        &endpoint,
        api_key.as_deref(),
        &model,
        summary_tokens,
        keep_recent,
    )
    .await
    .map_err(|e| CommandError::internal(format!("Extraction failed: {e}")))?;

    let active_work = build_active_work_md(&result);
    write_active_work(&agent_id, &active_work).await;

    let _ = save_fifo_summary(&agent_id, &result);

    if let Some(mem_engine) = state.get_memory_engine() {
        let agent_ns = MemoryEngine::agent_namespace(&agent_id);
        let extraction_json = serde_json::to_string(&result).unwrap_or_default();
        let store_input = StoreInput {
            content: format!("[Pre-Compaction Extraction] {}", extraction_json),
            category: MemoryCategory::Episode,
            importance: 0.9,
            source: MemorySource::AutoCaptured,
            agent_ns,
            tier: MemoryTier::Warm,
            tags: vec!["extraction".to_string()],
        };
        if let Err(e) = mem_engine.store(store_input).await {
            tracing::warn!(agent_id, "Failed to push extraction to bridge: {e}");
        }
    }

    Ok(serde_json::json!({
        "status": "extracted",
        "active_task": result.active_task,
        "decisions_count": result.decisions.len(),
        "files_count": result.files_modified.len(),
        "summary": result.summary,
    }))
}
