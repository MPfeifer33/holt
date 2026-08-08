use std::sync::Arc;

use crate::runtime::agent::AgentStatus;
use crate::runtime::approval::ApprovalManager;
use crate::runtime::subagent::SubagentManager;
use crate::runtime::turn_manager::TurnManager;
use crate::tools::vfl::VirtualFileLockRegistry;
use crate::trace::engine::{TraceEngine, TraceInput};
use crate::trace::truncation::{TraceOutcome, TraceType};

/// Perform the full interrupt cleanup chain for an agent (PRD Section 7.4).
///
/// 1. Cancel the CancellationToken (via TurnManager, fallback to legacy HashMap)
/// 2. Release all VFL locks for agent
/// 3. Cancel all subagents
/// 4. Cancel pending approvals
/// 5. Log trace
/// 6. Set agent status to Idle
pub async fn interrupt_agent(
    agent_id: &str,
    cancellation_tokens: &tokio::sync::RwLock<
        std::collections::HashMap<String, tokio_util::sync::CancellationToken>,
    >,
    vfl_registry: &Arc<VirtualFileLockRegistry>,
    subagent_manager: &Arc<SubagentManager>,
    trace_engine: &Arc<TraceEngine>,
    agents: &Arc<tokio::sync::RwLock<Vec<crate::runtime::agent::AgentSlot>>>,
    approval_manager: Option<&Arc<ApprovalManager>>,
    turn_manager: Option<&Arc<TurnManager>>,
) {
    // 1. Cancel the CancellationToken — prefer TurnManager, fallback to legacy HashMap
    if let Some(tm) = turn_manager {
        tm.cancel_current(agent_id).await;
    }
    // Also check the legacy HashMap (dual-write from worker ensures this works during migration,
    // and non-TurnManager code paths still write here)
    {
        let tokens = cancellation_tokens.read().await;
        if let Some(token) = tokens.get(agent_id) {
            token.cancel();
        }
    }

    // 2. Release all VFL locks for this agent
    vfl_registry.release_all_for_agent(agent_id).await;

    // 3. Cancel all subagents
    subagent_manager.cancel_all_for_parent(agent_id).await;

    // 4. Cancel pending approvals
    if let Some(approval_mgr) = approval_manager {
        approval_mgr.cancel_all_for_agent(agent_id).await;
    }

    // 5. Log interrupt trace
    if let Err(e) = trace_engine.write_trace(TraceInput {
        agent_id: agent_id.to_string(),
        trace_type: TraceType::Error,
        content: serde_json::json!({
            "event": "interrupt",
            "reason": "HIL interrupt",
        }),
        outcome: Some(TraceOutcome::Failure {
            reason: "HIL interrupt".to_string(),
        }),
        duration_ms: None,
        token_count: None,
        parent_trace_id: None,
        session_id: String::new(),
        prompt_tokens: None,
        completion_tokens: None,
        cost_usd_cents: None,
    }) {
        tracing::warn!(agent_id, "trace write failed during interrupt: {e}");
    }

    // 5. Set agent status to Idle
    {
        let mut agents_w = agents.write().await;
        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
            agent.status = AgentStatus::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn test_interrupt_cancels_token() {
        let tokens = tokio::sync::RwLock::new(HashMap::new());
        let token = CancellationToken::new();
        tokens
            .write()
            .await
            .insert("agent-1".to_string(), token.clone());

        let vfl = Arc::new(VirtualFileLockRegistry::new(
            crate::config::app_config::FileLockingBlock::default(),
        ));
        let subagent_mgr = Arc::new(SubagentManager::new());
        let trace = Arc::new(TraceEngine::new(std::path::Path::new(":memory:")).unwrap());
        let agents = Arc::new(tokio::sync::RwLock::new(vec![]));

        assert!(!token.is_cancelled());
        interrupt_agent(
            "agent-1",
            &tokens,
            &vfl,
            &subagent_mgr,
            &trace,
            &agents,
            None,
            None,
        )
        .await;
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn test_interrupt_sets_idle() {
        let tokens = tokio::sync::RwLock::new(HashMap::new());
        let vfl = Arc::new(VirtualFileLockRegistry::new(
            crate::config::app_config::FileLockingBlock::default(),
        ));
        let subagent_mgr = Arc::new(SubagentManager::new());
        let trace = Arc::new(TraceEngine::new(std::path::Path::new(":memory:")).unwrap());

        let agent = crate::runtime::agent::AgentSlot {
            id: "agent-1".to_string(),
            name: "Test".to_string(),
            color: crate::runtime::agent::AgentColor::default(),
            protected: false,
            connection: crate::runtime::connection::ConnectionConfig::Local {
                host: "localhost".to_string(),
                port: 8080,
                model: None,
            },
            protocol: crate::runtime::connection::AgentProtocol::default(),
            working_directory: std::path::PathBuf::from("/tmp"),
            status: AgentStatus::Working,
            subagents: vec![],
            conversation: crate::runtime::agent::ConversationHistory::default(),
            tools: vec![],
            system_prompt: String::new(),
            metadata: crate::runtime::agent::AgentMetadata::default(),
            limits: crate::runtime::limits::AgentLimits::default(),
            context_window_size: 125_000,
            context_tokens_override: None,
            context_probed: false,
            sandbox_config: crate::config::agent_config::BuildSandboxBlock::default(),
            sandbox: None,
            session_prompt_tokens: 0,
            session_completion_tokens: 0,
            current_context_tokens: 0,
            session_start: None,
            is_dirty: false,
            anthropic_settings: None,
            skills: vec![],
            user_notes: vec![],
            agent_session_id: None,
            system_prompt_hash: None,
            coordinator_mode: false,
            autonomous: false,
            extraction_pending_done: false,
            provider: None,
            cached_params: None,
            multi_agent_model: false,
            archetype_base: None,
            archetype_specialization: None,
            session_memory: Default::default(),
            cognitive_intake: Default::default(),
            appearance: None,
            openai_responses_settings: None,
            xai_settings: None,
            gemini_settings: None,
            local_model_settings: None,
        };

        let agents = Arc::new(tokio::sync::RwLock::new(vec![agent]));

        interrupt_agent(
            "agent-1",
            &tokens,
            &vfl,
            &subagent_mgr,
            &trace,
            &agents,
            None,
            None,
        )
        .await;

        let agents_r = agents.read().await;
        assert_eq!(agents_r[0].status, AgentStatus::Idle);
    }

    #[tokio::test]
    async fn test_interrupt_via_turn_manager() {
        use crate::runtime::turn_manager::{TurnManager, TurnResult};
        use std::time::Duration;

        let tm = Arc::new(TurnManager::new());

        let started = Arc::new(tokio::sync::Notify::new());
        let s = started.clone();

        tm.ensure_worker("agent-1".to_string(), move |_aid, _req, token| {
            let s = s.clone();
            async move {
                s.notify_one();
                tokio::select! {
                    _ = token.cancelled() => TurnResult::Cancelled,
                    _ = tokio::time::sleep(Duration::from_secs(60)) => TurnResult::Completed(String::new()),
                }
            }
        })
        .await;

        // Submit a turn
        let handle = tm
            .submit_turn(
                "agent-1",
                crate::runtime::turn_manager::TurnRequest::new(
                    crate::runtime::turn_manager::TurnProtocol::ApiDirect {
                        message: "test".into(),
                        images: None,
                        content_blocks: None,
                        transient_prompt: None,
                        max_turns: None,
                    },
                    crate::runtime::turn_manager::TurnSource::SendMessage,
                    crate::runtime::turn_manager::TurnPriority::User,
                ),
            )
            .await
            .unwrap();

        started.notified().await;

        // Empty HashMap — interrupt should work via TurnManager
        let tokens = tokio::sync::RwLock::new(HashMap::new());
        let vfl = Arc::new(VirtualFileLockRegistry::new(
            crate::config::app_config::FileLockingBlock::default(),
        ));
        let subagent_mgr = Arc::new(SubagentManager::new());
        let trace = Arc::new(TraceEngine::new(std::path::Path::new(":memory:")).unwrap());
        let agents = Arc::new(tokio::sync::RwLock::new(vec![]));

        interrupt_agent(
            "agent-1",
            &tokens,
            &vfl,
            &subagent_mgr,
            &trace,
            &agents,
            None,
            Some(&tm),
        )
        .await;

        // Turn should have been cancelled
        assert!(matches!(handle.result().await, TurnResult::Cancelled));
    }
}
