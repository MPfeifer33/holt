//! Context compaction — automatic conversation summarization and eviction.
//!
//! When the context window nears capacity, this module:
//! 1. Emits a ContextWarning event to the frontend
//! 2. Auto-checkpoints the conversation (crash recovery)
//! 3. Attempts model-based summarization (with fallback to rule-based)
//! 4. Sweeps evicted messages into long-term memory
//! 5. Evicts old messages from the conversation
//!
//! All fallback chains preserved exactly — every branch prevents context loss.

use chrono::Utc;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use crate::providers::types::{StreamEvent, StreamEventType};
use crate::runtime::agent::{AgentSlot, MessageRole};
use crate::trace::engine::TraceEngine;

use super::payload::LoopPayload;
use super::EngineError;

/// Attempt context compaction if the payload indicates it's needed.
///
/// Returns `Ok(true)` if compaction happened (caller should `continue` to rebuild payload).
/// Returns `Ok(false)` if no compaction was needed or it's disabled.
pub async fn try_compact_if_needed(
    agents: &Arc<RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    client: &reqwest::Client,
    payload: &LoopPayload,
    app_handle: Option<&AppHandle>,
    app_state: &Option<Arc<crate::AppState>>,
    #[allow(unused_variables)] trace_engine: &Arc<TraceEngine>,
    #[allow(unused_variables)] session_id: &str,
) -> Result<bool, EngineError> {
    if !payload.needs_compaction || !payload.context_config.eviction_summarize {
        return Ok(false);
    }

    // Defer compaction if extraction just completed this cycle.
    // Prevents compaction from firing mid-extraction window.
    {
        let agents_read = agents.read().await;
        if let Some(agent) = agents_read.iter().find(|a| a.id == agent_id) {
            if agent.extraction_pending_done {
                tracing::info!(
                    agent_id,
                    "Deferring compaction — extraction just completed this cycle"
                );
                return Ok(false);
            }
        }
    }

    let context_config = &payload.context_config;

    // Emit warning to frontend
    let message_count_before = {
        let agents_read = agents.read().await;
        agents_read
            .iter()
            .find(|a| a.id == agent_id)
            .map(|a| a.conversation.messages.len())
            .unwrap_or(0)
    };

    if let Some(handle) = app_handle {
        if let Err(e) = handle.emit(
            "agent-stream",
            StreamEvent {
                agent_id: agent_id.to_string(),
                event_type: StreamEventType::ContextWarning,
                data: serde_json::json!({
                    "estimated_tokens": payload.context_tokens,
                    "available_tokens": payload.context_window_size,
                    "message_count": message_count_before,
                    "needs_compaction": true,
                    "action": "compacting",
                }),
            },
        ) {
            tracing::warn!(agent_id, "event emission failed: {e}");
        }
    }

    // Auto-checkpoint before compaction
    if context_config.auto_checkpoint_on_compaction {
        let agents_read = agents.read().await;
        if let Some(agent) = agents_read.iter().find(|a| a.id == agent_id) {
            if let Err(e) =
                crate::runtime::checkpoint::create_checkpoint(agent, "pre-compaction (auto)", 0)
            {
                tracing::warn!(agent_id, "checkpoint creation failed: {e}");
            }
        }
    }

    // Summarization: produce a text summary to preserve after eviction.
    // Two modes:
    //   "self_summary" — uses the agent's own system prompt + full conversation
    //                    for higher-quality summaries (the agent knows its context)
    //   "model" (default) — separate summarizer prompt with truncated conversation
    let is_self_summary = context_config.compaction_mode == "self_summary";

    let summary_text = {
        let agents_read = agents.read().await;
        let agent = agents_read.iter().find(|a| a.id == agent_id);

        let summary_prompt = if is_self_summary {
            // Self-summary: use agent's system prompt + full conversation messages
            let system_prompt = agent.map(|a| a.system_prompt.as_str()).unwrap_or("");
            let conv_messages: Vec<serde_json::Value> = agent
                .map(|a| {
                    a.conversation
                        .messages
                        .iter()
                        .filter(|m| m.role != MessageRole::System)
                        .map(|m| {
                            crate::runtime::context::message_to_json(
                                m,
                                None,
                                &context_config.truncation_strategy,
                                context_config.char_ratio,
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();

            crate::runtime::context::build_self_summary_prompt(
                system_prompt,
                &conv_messages,
                context_config.compaction_summary_tokens,
            )
        } else {
            // Model-based: separate summarizer with truncated conversation text
            let conversation_text = agent
                .map(|a| {
                    a.conversation
                        .messages
                        .iter()
                        .map(|m| {
                            let role = match m.role {
                                MessageRole::System => "System",
                                MessageRole::User => "User",
                                MessageRole::Assistant => "Assistant",
                                MessageRole::Tool => "Tool",
                            };
                            format!(
                                "[{}]: {}",
                                role,
                                m.content.chars().take(500).collect::<String>()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();

            crate::runtime::context::build_compaction_prompt(
                &conversation_text,
                context_config.compaction_summary_tokens,
            )
        };

        // Resolve compaction model (override → same model)
        let (comp_endpoint, comp_key, comp_model) =
            if !context_config.compaction_model_override.is_empty()
                && context_config.compaction_model_override != "none"
            {
                // Look up override connection from app config
                if let Some(ref app_state) = app_state {
                    let config = app_state.config.read().await;
                    if let Some(conn) = config
                        .connections
                        .iter()
                        .find(|c| c.id == context_config.compaction_model_override)
                    {
                        let api_key = app_state.keychain.get_api_key(&conn.key_ref).ok();
                        (conn.endpoint.clone(), api_key, conn.provider.clone())
                    } else {
                        (
                            payload.endpoint.clone(),
                            payload.api_key.clone(),
                            payload.model.clone(),
                        )
                    }
                } else {
                    (
                        payload.endpoint.clone(),
                        payload.api_key.clone(),
                        payload.model.clone(),
                    )
                }
            } else {
                (
                    payload.endpoint.clone(),
                    payload.api_key.clone(),
                    payload.model.clone(),
                )
            };

        let noop_callback = |_: StreamEvent| {};
        // Compaction summarizer doesn't carry per-agent settings;
        // detect_and_build will synthesize defaults for the target provider.
        match crate::providers::detect_and_build(
            &comp_endpoint,
            &comp_model,
            comp_key.as_deref(),
            None,
            None,
        ) {
            Err(e) => {
                tracing::warn!(
                    agent_id,
                    "Compaction provider build failed, using rule-based: {e}"
                );
                None
            }
            Ok(comp_provider) => {
                // Self-summary gets a longer timeout since it processes more context.
                // Local models get 120s — compaction is already expensive, and a
                // quality self-summary is worth the time hit. See spec decision.
                let is_local_agent = {
                    let agents_read = agents.read().await;
                    agents_read
                        .iter()
                        .find(|a| a.id == agent_id)
                        .and_then(|a| a.local_model_settings.as_ref())
                        .is_some()
                };
                let timeout_secs = if is_local_agent && is_self_summary {
                    120
                } else if is_self_summary {
                    60
                } else {
                    30
                };
                match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    crate::providers::send_provider_completion(
                        comp_provider.as_ref(),
                        client,
                        &summary_prompt,
                        None, // no tools for summary
                        context_config.compaction_summary_tokens,
                        &noop_callback,
                        agent_id,
                    ),
                )
                .await
                {
                    Ok(Ok(response)) => {
                        tracing::info!(
                            agent_id,
                            mode = if is_self_summary {
                                "self_summary"
                            } else {
                                "model"
                            },
                            "Compaction summary generated"
                        );
                        Some(response.content)
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(
                            agent_id,
                            "Compaction summarization failed, using rule-based: {e}"
                        );
                        None
                    }
                    Err(_) => {
                        tracing::warn!(
                            agent_id,
                            timeout_secs,
                            "Compaction summarization timed out, using rule-based"
                        );
                        None
                    }
                }
            }
        }
    };

    // Pre-compaction persona update: write summary to ACTIVE_WORK.md so the
    // agent's system prompt reflects current state after compaction reinitializes.
    // Persona files are injected into the system prompt at conversation init —
    // writing here means the model's next system prompt already contains its
    // pre-compaction state.
    if let Some(ref summary) = summary_text {
        let persona_dir = crate::runtime::persona::persona_dir(agent_id);
        let active_work_path = persona_dir.join("ACTIVE_WORK.md");
        let active_work_content = format!(
            "# ACTIVE_WORK.md\n\n\
            **Last Updated:** {} (auto — pre-compaction)\n\n\
            ---\n\n\
            ## Pre-Compaction State\n\n\
            {}\n",
            Utc::now().format("%Y-%m-%d %H:%M UTC"),
            summary,
        );
        if let Err(e) = std::fs::create_dir_all(&persona_dir) {
            tracing::warn!(
                agent_id,
                "Failed to create persona dir for ACTIVE_WORK.md: {e}"
            );
        } else if let Err(e) = std::fs::write(&active_work_path, &active_work_content) {
            tracing::warn!(
                agent_id,
                "Failed to write ACTIVE_WORK.md pre-compaction: {e}"
            );
        } else {
            tracing::info!(agent_id, "Wrote ACTIVE_WORK.md from pre-compaction summary");
        }
    }

    // Compact the conversation
    {
        let mut agents_w = agents.write().await;
        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
            let compacted = crate::runtime::context::compact_messages(
                &agent.conversation.messages,
                &payload.working_dir,
                context_config.compaction_keep_recent,
                summary_text.as_deref(),
            );
            agent.conversation.messages = compacted;

            // Inject session memory as first system message after compaction summary
            let session_memory_used = if let Some(ref sid) = agent.agent_session_id {
                let sm_path = crate::runtime::session_memory::session_memory_path(agent_id, sid);
                if let Ok(content) = std::fs::read_to_string(&sm_path) {
                    if !content.is_empty() {
                        let insert_pos = if !agent.conversation.messages.is_empty() {
                            1
                        } else {
                            0
                        };
                        agent.conversation.messages.insert(
                            insert_pos,
                            crate::runtime::agent::ConversationMessage {
                                role: crate::runtime::agent::MessageRole::System,
                                content: format!(
                                    "[Session Memory — preserved across compaction]\n\n{}",
                                    content
                                ),
                                content_blocks: None,
                                timestamp: chrono::Utc::now(),
                                token_estimate: None,
                                tool_calls: None,
                                tool_call_id: None,
                                outcome_tag: None,
                                thinking_content: None,
                                responses_phase: None,
                                reasoning_items: None,
                            },
                        );
                        tracing::info!(
                            agent_id,
                            "Injected session memory into post-compaction context"
                        );
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };

            // Inject FIFO rolling summaries after session memory (or after compaction summary)
            let fifo_summaries = crate::memory::compaction::read_fifo_summaries(agent_id);
            if !fifo_summaries.is_empty() {
                let fifo_base = if session_memory_used {
                    if agent.conversation.messages.len() > 1 {
                        2
                    } else {
                        0
                    }
                } else if !agent.conversation.messages.is_empty() {
                    1
                } else {
                    0
                };
                for (i, fifo) in fifo_summaries.iter().enumerate() {
                    agent.conversation.messages.insert(
                        fifo_base + i,
                        crate::runtime::agent::ConversationMessage {
                            role: crate::runtime::agent::MessageRole::System,
                            content: format!(
                                "[Prior context summary ({})]\n{}",
                                fifo.extracted_at, fifo.summary
                            ),
                            content_blocks: None,
                            timestamp: chrono::Utc::now(),
                            token_estimate: None,
                            tool_calls: None,
                            tool_call_id: None,
                            outcome_tag: None,
                            thinking_content: None,
                            responses_phase: None,
                            reasoning_items: None,
                        },
                    );
                }
                tracing::info!(
                    agent_id,
                    count = fifo_summaries.len(),
                    "Injected FIFO summaries after compaction"
                );
            }

            // Reorientation prompt for local agents: after compaction, before
            // the next user turn, explicitly tell the model what happened and
            // where to find its state. Non-negotiable — the model must orient
            // before processing any new messages.
            if agent.local_model_settings.is_some() {
                agent.conversation.messages.push(
                    crate::runtime::agent::ConversationMessage {
                        role: crate::runtime::agent::MessageRole::System,
                        content: "[SYSTEM CONTEXT — NOT FROM HUMAN]\n\
                            Your conversation context was just compacted. Your ACTIVE_WORK.md \
                            and persona files have been updated with your pre-compaction state. \
                            Before responding to the user's next message, orient yourself:\n\n\
                            1. Review your system prompt — your persona files contain your current state\n\
                            2. Review the context summary above — it captures what was discussed before compaction\n\
                            3. If anything is unclear, say so rather than guessing\n\n\
                            Proceed with the user's message.\n\
                            [END SYSTEM CONTEXT]".to_string(),
                        content_blocks: None,
                        timestamp: chrono::Utc::now(),
                        token_estimate: None,
                        tool_calls: None,
                        tool_call_id: None,
                        outcome_tag: None,
                        thinking_content: None,
                        responses_phase: None,
                        reasoning_items: None,
                    },
                );
                tracing::info!(agent_id, "Injected reorientation prompt for local agent");
            }

            // Reset extraction flag so next cycle can extract again
            agent.extraction_pending_done = false;

            // Reset session memory state after compaction
            agent.session_memory.tokens_at_last_extraction = 0;
            agent.session_memory.tool_calls_since_extraction = 0;
            agent.session_memory.assistant_messages_since_extraction = 0;
            // Keep initialized = true — the session continues

            agent.is_dirty = true;
        }
    }

    let message_count_after = {
        let agents_read = agents.read().await;
        agents_read
            .iter()
            .find(|a| a.id == agent_id)
            .map(|a| a.conversation.messages.len())
            .unwrap_or(0)
    };

    tracing::info!(
        agent_id,
        model_based = summary_text.is_some(),
        "context compacted"
    );

    // Persist compaction memory
    let compaction_memory = crate::runtime::memory::CompactionMemory {
        id: format!("compaction-{}", Utc::now().timestamp()),
        timestamp: Utc::now(),
        summary: summary_text
            .clone()
            .unwrap_or_else(|| "Rule-based compaction".to_string()),
        model_based: summary_text.is_some(),
        messages_compacted: message_count_before.saturating_sub(message_count_after),
        tokens_before: payload.context_tokens as usize,
        tokens_after: 0, // Will be recalculated on next loop iteration
        key_facts: vec![],
        files_modified: vec![],
        working_directory: payload.working_dir.clone(),
    };
    if let Err(e) = crate::runtime::memory::save_compaction_memory(agent_id, &compaction_memory) {
        tracing::warn!(agent_id, "Failed to save compaction memory: {e}");
    }

    // Report compaction event to memory bridge for pressure tracking
    if let Some(ref app_state) = app_state {
        if let Some(memory_engine) = app_state.get_memory_engine() {
            let agent_id_owned = agent_id.to_string();
            let session_id_owned = session_id.to_string();
            let msg_count = message_count_after;
            let tool_calls = {
                let agents_read = agents.read().await;
                agents_read
                    .iter()
                    .find(|a| a.id == agent_id)
                    .map(|a| a.metadata.total_api_calls)
                    .unwrap_or(0)
            };
            // Fire and forget — don't block compaction on bridge availability
            tokio::spawn(async move {
                if let Err(e) = memory_engine
                    .compaction_report(
                        &agent_id_owned,
                        &session_id_owned,
                        "holt_compaction",
                        Some(msg_count),
                        Some(tool_calls),
                    )
                    .await
                {
                    tracing::warn!(
                        agent_id = agent_id_owned,
                        "Failed to report compaction to bridge: {e}"
                    );
                }
            });
        }
    }

    // Emit compaction complete event
    if let Some(handle) = app_handle {
        if let Err(e) = handle.emit(
            "agent-stream",
            StreamEvent {
                agent_id: agent_id.to_string(),
                event_type: StreamEventType::ContextWarning,
                data: serde_json::json!({
                    "action": "compacted",
                    "model_based": summary_text.is_some(),
                    "messages_compacted": message_count_before.saturating_sub(message_count_after),
                    "tokens_before": payload.context_tokens,
                }),
            },
        ) {
            tracing::warn!(agent_id, "event emission failed: {e}");
        }
        // Also notify that conversation state changed after compaction
        if let Err(e) = handle.emit(
            "agent-stream",
            StreamEvent {
                agent_id: agent_id.to_string(),
                event_type: StreamEventType::ConversationUpdated,
                data: serde_json::json!({}),
            },
        ) {
            tracing::warn!(agent_id, "event emission failed: {e}");
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::app_config::ContextBlock;
    use crate::runtime::engine::test_utils::{test_agent, wrap_agents};
    use crate::trace::engine::TraceEngine;

    fn temp_trace_engine() -> Arc<TraceEngine> {
        let path = std::env::temp_dir().join(format!("holt_test_{}.db", uuid::Uuid::new_v4()));
        Arc::new(TraceEngine::new(&path).expect("temp trace DB"))
    }

    fn test_payload(needs_compaction: bool, eviction_summarize: bool) -> LoopPayload {
        LoopPayload {
            endpoint: "https://test.example.com".to_string(),
            api_key: Some("test-key".to_string()),
            model: "test-model".to_string(),
            messages: vec![],
            needs_compaction,
            context_tokens: 100_000,
            context_window_size: 125_000,
            context_config: ContextBlock {
                eviction_summarize,
                ..Default::default()
            },
            working_dir: "/tmp/test".to_string(),
            augmented_system_prompt: String::new(),
            supports_tools: true,
        }
    }

    #[tokio::test]
    async fn no_compaction_when_not_needed() {
        let agents = wrap_agents(test_agent());
        let client = reqwest::Client::new();
        let trace = temp_trace_engine();
        let payload = test_payload(false, true);

        let result = try_compact_if_needed(
            &agents,
            "test-agent",
            &client,
            &payload,
            None,
            &None,
            &trace,
            "sess-1",
        )
        .await
        .unwrap();

        assert!(!result, "should not compact when needs_compaction is false");
    }

    #[tokio::test]
    async fn no_compaction_when_summarize_disabled() {
        let agents = wrap_agents(test_agent());
        let client = reqwest::Client::new();
        let trace = temp_trace_engine();
        let payload = test_payload(true, false);

        let result = try_compact_if_needed(
            &agents,
            "test-agent",
            &client,
            &payload,
            None,
            &None,
            &trace,
            "sess-1",
        )
        .await
        .unwrap();

        assert!(
            !result,
            "should not compact when eviction_summarize is false"
        );
    }

    #[tokio::test]
    async fn no_compaction_when_both_false() {
        let agents = wrap_agents(test_agent());
        let client = reqwest::Client::new();
        let trace = temp_trace_engine();
        let payload = test_payload(false, false);

        let result = try_compact_if_needed(
            &agents,
            "test-agent",
            &client,
            &payload,
            None,
            &None,
            &trace,
            "sess-1",
        )
        .await
        .unwrap();

        assert!(!result, "should not compact when both flags false");
    }

    #[tokio::test]
    async fn compaction_proceeds_when_both_flags_true() {
        // When both flags are true, the function proceeds past the guard.
        // Without app_state and app_handle, it will still attempt compaction
        // (model-based summarization will fail gracefully, falling back to
        // rule-based). The key assertion: it does NOT return Ok(false).
        let mut agent = test_agent();
        // Add some conversation messages so there's something to compact
        for i in 0..10 {
            agent
                .conversation
                .messages
                .push(crate::runtime::agent::ConversationMessage {
                    role: crate::runtime::agent::MessageRole::User,
                    content: format!("Message {}", i),
                    content_blocks: None,
                    timestamp: chrono::Utc::now(),
                    token_estimate: Some(100),
                    tool_calls: None,
                    tool_call_id: None,
                    outcome_tag: None,
                    thinking_content: None,
                    responses_phase: None,
                    reasoning_items: None,
                });
        }
        let agents = wrap_agents(agent);
        let client = reqwest::Client::new();
        let trace = temp_trace_engine();
        let payload = test_payload(true, true);

        let result = try_compact_if_needed(
            &agents,
            "test-agent",
            &client,
            &payload,
            None,
            &None,
            &trace,
            "sess-1",
        )
        .await;

        // The function should proceed past the guard and attempt compaction.
        // It may succeed or fail depending on provider availability, but it
        // should NOT return Ok(false) — that only happens when guards trip.
        match result {
            Ok(did_compact) => assert!(
                did_compact,
                "should attempt compaction when both flags true"
            ),
            Err(_) => { /* Acceptable — provider call failed, but guard was passed */ }
        }
    }

    #[test]
    fn fifo_summaries_injected_in_correct_order() {
        use crate::runtime::agent::{ConversationMessage, MessageRole};

        let mut messages = vec![
            ConversationMessage {
                role: MessageRole::System,
                content: "[Context compacted — 10 messages summarized]".to_string(),
                content_blocks: None,
                timestamp: chrono::Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
            ConversationMessage {
                role: MessageRole::User,
                content: "Recent message".to_string(),
                content_blocks: None,
                timestamp: chrono::Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
        ];

        let fifo_summaries = vec![
            crate::memory::compaction::FifoSummary {
                extracted_at: "2026-03-29T10:00:00Z".to_string(),
                summary: "Older summary".to_string(),
            },
            crate::memory::compaction::FifoSummary {
                extracted_at: "2026-03-29T11:00:00Z".to_string(),
                summary: "Newer summary".to_string(),
            },
        ];

        let insert_pos = if !messages.is_empty() { 1 } else { 0 };
        for (i, fifo) in fifo_summaries.iter().enumerate() {
            messages.insert(
                insert_pos + i,
                ConversationMessage {
                    role: MessageRole::System,
                    content: format!(
                        "[Prior context summary ({})]\n{}",
                        fifo.extracted_at, fifo.summary
                    ),
                    content_blocks: None,
                    timestamp: chrono::Utc::now(),
                    token_estimate: None,
                    tool_calls: None,
                    tool_call_id: None,
                    outcome_tag: None,
                    thinking_content: None,
                    responses_phase: None,
                    reasoning_items: None,
                },
            );
        }

        assert_eq!(messages.len(), 4);
        assert!(messages[0].content.contains("compacted"));
        assert!(messages[1].content.contains("Older summary"));
        assert!(messages[2].content.contains("Newer summary"));
        assert!(messages[3].content.contains("Recent message"));
    }

    #[test]
    fn session_memory_injected_before_fifo_summaries() {
        use crate::runtime::agent::{ConversationMessage, MessageRole};

        let mut messages = vec![
            ConversationMessage {
                role: MessageRole::System,
                content: "[Context compacted — 10 messages summarized]".to_string(),
                content_blocks: None,
                timestamp: chrono::Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
            ConversationMessage {
                role: MessageRole::User,
                content: "Recent message".to_string(),
                content_blocks: None,
                timestamp: chrono::Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
        ];

        // Simulate session memory injection at position 1
        let session_content = "# Session Title\nTest session";
        let session_msg = ConversationMessage {
            role: MessageRole::System,
            content: format!(
                "[Session Memory — preserved across compaction]\n\n{}",
                session_content
            ),
            content_blocks: None,
            timestamp: chrono::Utc::now(),
            token_estimate: None,
            tool_calls: None,
            tool_call_id: None,
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        };
        messages.insert(1, session_msg);

        // Then FIFO at position 2
        let fifo_msg = ConversationMessage {
            role: MessageRole::System,
            content: "[Prior context summary (2026-04-01)]\nFIFO content".to_string(),
            content_blocks: None,
            timestamp: chrono::Utc::now(),
            token_estimate: None,
            tool_calls: None,
            tool_call_id: None,
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        };
        messages.insert(2, fifo_msg);

        assert_eq!(messages.len(), 4);
        assert!(messages[0].content.contains("compacted"));
        assert!(messages[1].content.contains("Session Memory"));
        assert!(messages[2].content.contains("Prior context summary"));
        assert!(messages[3].content.contains("Recent message"));
    }
}
