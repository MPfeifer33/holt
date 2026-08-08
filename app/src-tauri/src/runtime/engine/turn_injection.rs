//! Turn-boundary message injection.
//!
//! At the start of each loop iteration, drain pending subagent results and
//! A2A messages, build A2A system prompt section, and retrieve relevant memories.
//! A2A messages are injected into the system prompt (not the user message channel)
//! so models can structurally distinguish them from human input.
//! This is the extension point for future OS-level integrations.

use chrono::Utc;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use crate::providers::types::{StreamEvent, StreamEventType};
use crate::runtime::a2a::A2ARegistry;
use crate::runtime::agent::{AgentSlot, ConversationMessage, MessageRole};
use crate::runtime::memory_pipeline::{write_memory_pipeline_trace, MemoryPipelineReport};
use crate::runtime::subagent::SubagentManager;
use crate::trace::engine::{TraceEngine, TraceInput};
use crate::trace::truncation::{TraceOutcome, TraceType};

use super::EngineError;

/// Result of turn-boundary injection.
pub struct InjectionResult {
    /// Memory context to include in the next payload (may be empty).
    pub memory_section: String,
    /// A2A messages formatted as a system prompt section (empty string if none).
    /// Injected into the augmented system prompt, NOT the user message channel.
    pub a2a_section: String,
}

/// Format A2A messages as a system prompt section for injection into the augmented
/// system prompt. This ensures models structurally distinguish agent messages from
/// human input -- the system prompt is the identity/context channel, not conversation.
///
/// Replaces the old `format_a2a_section` which used box-drawing characters and was
/// injected as `role: User` content (causing models to confuse A2A with human input).
pub fn format_a2a_system_section(messages: &[crate::runtime::a2a::A2AMessage]) -> String {
    if messages.is_empty() {
        return String::new();
    }

    let mut section = String::new();
    section.push_str(
        "\n\n# Direct Messages from Other Agents\n\n\
         You have received direct messages from other agents. These are NOT from the human user.\n\
         Reply to each using the send_message_to_agent tool. Do NOT address these in your chat response to the human.\n",
    );

    for msg in messages {
        section.push_str(&format!(
            "\n---\nFROM: {} (agent-id: {})\nSENT: {}\n\n{}\n",
            msg.from_name,
            msg.from_id,
            msg.timestamp.format("%Y-%m-%dT%H:%M:%SZ"),
            msg.content
        ));
    }

    section.push_str(
        "---\n\n\
         Reply to agents via send_message_to_agent(to=\"<agent_id>\", content=\"<reply>\").\n\
         Do NOT include agent replies in your response to the human.",
    );

    section
}

/// Legacy A2A formatting with box-drawing characters.
/// Used only by SDK and Codex lanes pending their migration to system prompt injection.
#[allow(dead_code)]
pub fn format_a2a_section(messages: &[crate::runtime::a2a::A2AMessage]) -> String {
    let mut section = String::new();
    section.push_str(
        "═══════════════════════════════════════════════════════════════════════════════\n\
         INTER-AGENT MESSAGES — Reply via send_message_to_agent tool, NOT in user chat\n\
         ═══════════════════════════════════════════════════════════════════════════════\n\n",
    );

    for msg in messages {
        section.push_str(&format!(
            "┌─ FROM: {} (id: {}) ─────────────────────────────────────────────\n",
            msg.from_name, msg.from_id
        ));
        for line in msg.content.lines() {
            section.push_str(&format!("│ {}\n", line));
        }
        section.push_str(
            "└──────────────────────────────────────────────────────────────────────────────\n\n",
        );
    }

    section.push_str(
        "PROTOCOL:\n\
         • These messages are from OTHER AGENTS, not from the human user.\n\
         • Reply using: send_message_to_agent(to=\"<agent_id>\", content=\"<your reply>\")\n\
         • Do NOT address these messages in your response to the human.\n\
         • If the human's message AND agent messages both arrived, handle separately:\n\
         \x20 1. Reply to agents via send_message_to_agent tool calls\n\
         \x20 2. Reply to human in your chat response\n\
         ═══════════════════════════════════════════════════════════════════════════════",
    );

    section
}

/// Drain pending A2A messages and subagent results, surface A2A as transient
/// prompt overlays, and retrieve relevant memories for the upcoming model call.
pub async fn inject_turn_messages(
    agents: &Arc<RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    subagent_manager: &Arc<SubagentManager>,
    a2a_registry: &Arc<A2ARegistry>,
    app_state: &Option<Arc<crate::AppState>>,
    app_handle: Option<&AppHandle>,
    trace_engine: &Arc<TraceEngine>,
    session_id: &str,
    suppress_memory_pipeline: bool,
) -> Result<InjectionResult, EngineError> {
    let (subagent_results_injected, a2a_messages_injected);
    let mut a2a_section = String::new();

    // Drain subagent results and A2A messages
    {
        let completed_subagents = subagent_manager.drain_results(agent_id).await;
        let a2a_messages = a2a_registry.drain_messages(agent_id).await;
        subagent_results_injected = completed_subagents.len();
        a2a_messages_injected = a2a_messages.len();

        if !completed_subagents.is_empty() || !a2a_messages.is_empty() {
            // Acquire write lock ONLY for message injection, release before I/O
            {
                let mut agents_w = agents.write().await;
                if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                    for result in &completed_subagents {
                        let content = format!(
                            "[Subagent '{}' (job: {}) completed: {}]",
                            result.name, result.job_id, result.summary
                        );
                        agent.add_message(ConversationMessage {
                            role: MessageRole::System,
                            content,
                            content_blocks: None,
                            timestamp: Utc::now(),
                            token_estimate: None,
                            tool_calls: None,
                            tool_call_id: None,
                            outcome_tag: None,
                            thinking_content: None,
                            responses_phase: None,
                            reasoning_items: None,
                        });
                    }
                }
            } // Write lock released here — event emission and traces don't need it

            if !a2a_messages.is_empty() {
                a2a_section = format_a2a_system_section(&a2a_messages);
            }

            // Emit A2A events to frontend + write traces (lock-free)
            for msg in &a2a_messages {
                let content_preview: String = msg.content.chars().take(200).collect();
                if let Some(handle) = app_handle {
                    if let Err(e) = handle.emit(
                        "agent-stream",
                        StreamEvent {
                            agent_id: agent_id.to_string(),
                            event_type: StreamEventType::A2AMessage,
                            data: serde_json::json!({
                                "from_id": msg.from_id,
                                "from_name": msg.from_name,
                                "content_preview": content_preview,
                            }),
                        },
                    ) {
                        tracing::warn!(agent_id, "event emission failed: {e}");
                    }
                }
                if let Err(e) = trace_engine.write_trace(TraceInput {
                    agent_id: agent_id.to_string(),
                    trace_type: TraceType::A2AMessage,
                    content: serde_json::json!({
                        "from_id": msg.from_id,
                        "from_name": msg.from_name,
                        "to_id": agent_id,
                        "content": msg.content,
                    }),
                    outcome: Some(TraceOutcome::Success),
                    duration_ms: None,
                    token_count: None,
                    parent_trace_id: None,
                    session_id: session_id.to_string(),
                    prompt_tokens: None,
                    completion_tokens: None,
                    cost_usd_cents: None,
                }) {
                    tracing::warn!(agent_id, "trace write failed: {e}");
                }
            }
        }
    }

    // Memory injection with relevance gate + session dedup.
    let (current_msg, recent_msgs) = {
        let agents_r = agents.read().await;
        agents_r
            .iter()
            .find(|a| a.id == agent_id)
            .map(|a| {
                let user_msgs: Vec<String> = a
                    .conversation
                    .messages
                    .iter()
                    .rev()
                    .filter(|m| m.role == MessageRole::User)
                    .take(3)
                    .map(|m| m.content.clone())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let current = user_msgs.last().cloned().unwrap_or_default();
                let recent = if user_msgs.len() > 1 {
                    user_msgs[..user_msgs.len() - 1].to_vec()
                } else {
                    vec![]
                };
                (current, recent)
            })
            .unwrap_or_default()
    };

    // NOTE: A2A messages are intentionally NOT merged into current_msg.
    // current_msg drives memory relevance scoring -- polluting it with
    // inter-agent chatter would bias memory retrieval away from the
    // actual user request. A2A content is delivered separately via the
    // system prompt (InjectionResult.a2a_section).

    let mut memory_report = None;
    let mut memory_status = "disabled".to_string();
    let mut memory_error = None;
    let memory_section = if let Some(ref state) = app_state {
        if let Some(mem_engine) = state.get_memory_engine() {
            memory_status = "reported".to_string();
            let agent_ns = crate::memory::MemoryEngine::agent_namespace(agent_id);
            // Injection is retired; the budget is reported as 0, never spent.
            let budget = 0usize;
            match crate::memory::injection::build_memory_context(
                &mem_engine,
                &state.session_state,
                &mem_engine.embedder,
                agent_id,
                &agent_ns,
                &current_msg,
                &recent_msgs,
                budget,
            )
            .await
            {
                Ok(result) => {
                    // Cache tokens used for panel display
                    if let Some(ref state) = app_state {
                        state
                            .last_injection_tokens
                            .write()
                            .await
                            .insert(agent_id.to_string(), result.report.tokens_used);
                    }
                    memory_report = Some(result.report);
                    let text = result.text;
                    if !text.is_empty() {
                        format!("\n\n{}", text)
                    } else {
                        String::new()
                    }
                }
                Err(e) => {
                    memory_status = "error".to_string();
                    memory_error = Some(e.to_string());
                    tracing::warn!(agent_id, "Memory injection failed: {e}");
                    String::new()
                }
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    if !suppress_memory_pipeline {
        let pipeline_report = MemoryPipelineReport {
            source: "engine_turn".to_string(),
            agent_id: agent_id.to_string(),
            agent_ns: app_state
                .as_ref()
                .map(|_| crate::memory::MemoryEngine::agent_namespace(agent_id)),
            session_id: session_id.to_string(),
            current_message: current_msg,
            recent_messages: recent_msgs,
            memory_enabled: app_state
                .as_ref()
                .map(|state| state.has_memory_engine())
                .unwrap_or(false),
            memory_status,
            memory_error,
            a2a_messages_injected,
            subagent_results_injected,
            memory_context: memory_report,
        };
        if let Err(e) = write_memory_pipeline_trace(trace_engine, &pipeline_report) {
            tracing::warn!(agent_id, "memory pipeline trace write failed: {e}");
        }
    }

    Ok(InjectionResult {
        memory_section,
        a2a_section,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::a2a::{A2ARegistry, VisibilityConfig};
    use crate::runtime::agent::ConversationHistory;
    use crate::runtime::connection::ConnectionConfig;
    use crate::runtime::engine::test_utils::{test_agent, wrap_agents};
    use crate::runtime::subagent::{SubagentManager, SubagentSlot, SubagentStatus};
    use crate::trace::engine::TraceEngine;
    use chrono::Utc;

    fn temp_trace_engine() -> Arc<TraceEngine> {
        let path = std::env::temp_dir().join(format!("holt_test_{}.db", uuid::Uuid::new_v4()));
        Arc::new(TraceEngine::new(&path).expect("temp trace DB"))
    }

    fn test_subagent_slot(job_id: &str, parent_id: &str, name: &str) -> SubagentSlot {
        SubagentSlot {
            job_id: job_id.to_string(),
            parent_id: parent_id.to_string(),
            name: name.to_string(),
            connection: ConnectionConfig::Local {
                host: "localhost".to_string(),
                port: 8080,
                model: None,
            },
            status: SubagentStatus::Working,
            conversation: ConversationHistory::default(),
            task: "test task".to_string(),
            model_override: None,
            created_at: Utc::now(),
            result: None,
        }
    }

    #[tokio::test]
    async fn empty_drains_produce_no_messages() {
        let agents = wrap_agents(test_agent());
        let subagent_mgr = Arc::new(SubagentManager::new());
        let a2a = Arc::new(A2ARegistry::new(false));
        let trace = temp_trace_engine();

        let result = inject_turn_messages(
            &agents,
            "test-agent",
            &subagent_mgr,
            &a2a,
            &None,
            None,
            &trace,
            "sess-1",
            false,
        )
        .await
        .unwrap();

        let agents_r = agents.read().await;
        let agent = agents_r.iter().find(|a| a.id == "test-agent").unwrap();
        assert!(agent.conversation.messages.is_empty());
        assert!(result.memory_section.is_empty());
    }

    #[tokio::test]
    async fn subagent_results_injected_as_system_messages() {
        let agents = wrap_agents(test_agent());
        let subagent_mgr = Arc::new(SubagentManager::new());
        let a2a = Arc::new(A2ARegistry::new(false));
        let trace = temp_trace_engine();

        // Seed a subagent result
        let slot = test_subagent_slot("job-1", "test-agent", "helper-bot");
        let handle = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
        subagent_mgr.register(slot, handle).await;
        subagent_mgr
            .complete("job-1", "Found 3 bugs".to_string())
            .await;

        let _result = inject_turn_messages(
            &agents,
            "test-agent",
            &subagent_mgr,
            &a2a,
            &None,
            None,
            &trace,
            "sess-1",
            false,
        )
        .await
        .unwrap();

        let agents_r = agents.read().await;
        let agent = agents_r.iter().find(|a| a.id == "test-agent").unwrap();
        assert_eq!(agent.conversation.messages.len(), 1);
        let msg = &agent.conversation.messages[0];
        assert!(matches!(msg.role, MessageRole::System));
        assert!(msg.content.contains("helper-bot"));
        assert!(msg.content.contains("Found 3 bugs"));
    }

    #[tokio::test]
    async fn a2a_messages_returned_as_system_prompt_section() {
        let agents = wrap_agents(test_agent());
        let subagent_mgr = Arc::new(SubagentManager::new());
        let a2a = Arc::new(A2ARegistry::new(false));
        let trace = temp_trace_engine();

        // Enable messaging and send
        a2a.set_visibility(
            "sender-1",
            "test-agent",
            VisibilityConfig {
                messaging_enabled: true,
                workspace_visibility: false,
                source: Default::default(),
            },
        )
        .await;
        a2a.send_message("sender-1", "Grok", "test-agent", "Check this out")
            .await
            .unwrap();

        let result = inject_turn_messages(
            &agents,
            "test-agent",
            &subagent_mgr,
            &a2a,
            &None,
            None,
            &trace,
            "sess-1",
            false,
        )
        .await
        .unwrap();

        let agents_r = agents.read().await;
        let agent = agents_r.iter().find(|a| a.id == "test-agent").unwrap();
        // A2A messages should NOT be in conversation history
        assert!(agent.conversation.messages.is_empty());
        // A2A should be in the system prompt section, not a transient user message
        assert!(!result.a2a_section.is_empty());
        assert!(result
            .a2a_section
            .contains("Direct Messages from Other Agents"));
        assert!(result.a2a_section.contains("FROM: Grok"));
        assert!(result.a2a_section.contains("Check this out"));
        assert!(result.a2a_section.contains("send_message_to_agent"));
    }

    #[tokio::test]
    async fn multiple_results_all_injected_in_order() {
        let agents = wrap_agents(test_agent());
        let subagent_mgr = Arc::new(SubagentManager::new());
        let a2a = Arc::new(A2ARegistry::new(false));
        let trace = temp_trace_engine();

        // Two subagent results
        let slot_a = test_subagent_slot("job-a", "test-agent", "bot-a");
        let handle_a = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
        subagent_mgr.register(slot_a, handle_a).await;
        subagent_mgr.complete("job-a", "Result A".to_string()).await;

        let slot_b = test_subagent_slot("job-b", "test-agent", "bot-b");
        let handle_b = tauri::async_runtime::JoinHandle::Tokio(tokio::spawn(async {}));
        subagent_mgr.register(slot_b, handle_b).await;
        subagent_mgr.complete("job-b", "Result B".to_string()).await;

        // One A2A message
        a2a.set_visibility(
            "other",
            "test-agent",
            VisibilityConfig {
                messaging_enabled: true,
                workspace_visibility: false,
                source: Default::default(),
            },
        )
        .await;
        a2a.send_message("other", "Other", "test-agent", "Hey")
            .await
            .unwrap();

        let result = inject_turn_messages(
            &agents,
            "test-agent",
            &subagent_mgr,
            &a2a,
            &None,
            None,
            &trace,
            "sess-1",
            false,
        )
        .await
        .unwrap();

        let agents_r = agents.read().await;
        let agent = agents_r.iter().find(|a| a.id == "test-agent").unwrap();
        assert_eq!(agent.conversation.messages.len(), 2);
        assert!(agent.conversation.messages[0].content.contains("bot-a"));
        assert!(agent.conversation.messages[1].content.contains("bot-b"));
        assert!(!result.a2a_section.is_empty());
        assert!(result
            .a2a_section
            .contains("Direct Messages from Other Agents"));
        assert!(result.a2a_section.contains("FROM: Other"));
    }

    #[tokio::test]
    async fn memory_section_empty_when_no_app_state() {
        let agents = wrap_agents(test_agent());
        let subagent_mgr = Arc::new(SubagentManager::new());
        let a2a = Arc::new(A2ARegistry::new(false));
        let trace = temp_trace_engine();

        let result = inject_turn_messages(
            &agents,
            "test-agent",
            &subagent_mgr,
            &a2a,
            &None,
            None,
            &trace,
            "sess-1",
            false,
        )
        .await
        .unwrap();

        assert!(result.memory_section.is_empty());
        assert!(result.a2a_section.is_empty());
    }
}
