//! Tool call processing — from approval to completion.
//!
//! For each tool call in the model's response:
//! 1. Log tool call trace
//! 2. Check approval tier (Auto/NotifyUnlessVeto/RequireApproval/Blocked)
//! 3. Execute via ToolRegistry
//! 4. Log tool result trace
//! 5. Emit frontend events
//! 6. Check for sandbox entry/exit (build/test failure detection)
//! 7. Handle shell session resets

use chrono::Utc;
use std::collections::HashSet;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use crate::config::app_config::AppConfig;
use crate::providers::types::{AssembledResponse, StreamEvent, StreamEventType};
use crate::runtime::a2a::A2ARegistry;
use crate::runtime::agent::{AgentSlot, AgentStatus, ConversationMessage, MessageRole};
use crate::runtime::approval::{check_approval, ApprovalDecision, ApprovalManager};
use crate::runtime::subagent::SubagentManager;
use crate::tools::registry::ToolRegistry;
use crate::tools::shell::ShellManager;
use crate::tools::types::{
    ToolAuthorityClass, ToolContext, ToolExecutionReceipt, ToolSuccessBehavior,
    ToolVerificationStatus,
};
use crate::tools::vfl::VirtualFileLockRegistry;
use crate::trace::engine::{TraceEngine, TraceInput};
use crate::trace::truncation::{TraceOutcome, TraceType};

use super::helpers;
use super::EngineError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionOutcome {
    ContinueLoop,
    EndTurn,
}

fn parse_tool_status(result_json: &serde_json::Value) -> &'static str {
    match result_json
        .get("tool_result_status")
        .and_then(|v| v.as_str())
        .unwrap_or("success")
    {
        "partial" => "partial",
        "failed" => "failed",
        _ => "done",
    }
}

fn parse_trace_outcome(result_json: &serde_json::Value) -> TraceOutcome {
    match parse_tool_status(result_json) {
        "partial" => {
            let details = result_json
                .get("receipt")
                .and_then(|v| v.get("summary"))
                .and_then(|v| v.as_str())
                .unwrap_or("Tool completed partially")
                .to_string();
            TraceOutcome::Partial { details }
        }
        "failed" => TraceOutcome::Failure {
            reason: result_json
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("Tool failed")
                .to_string(),
        },
        _ => TraceOutcome::Success,
    }
}

fn normalize_tool_receipt(
    result_json: &mut serde_json::Value,
    authority_class: ToolAuthorityClass,
    tool_name: &str,
    tool_call_id: &str,
    execution_id: &str,
    tool_call_trace_id: &str,
    tool_result_trace_id: &str,
    executed: bool,
    default_execution_status: &str,
    default_summary: Option<String>,
) {
    let mut receipt = result_json
        .get("receipt")
        .cloned()
        .and_then(|value| serde_json::from_value::<ToolExecutionReceipt>(value).ok())
        .unwrap_or_else(|| ToolExecutionReceipt {
            authority_class,
            executed,
            execution_status: default_execution_status.to_string(),
            verified: matches!(authority_class, ToolAuthorityClass::EffectfulVerified),
            verification_status: if matches!(authority_class, ToolAuthorityClass::EffectfulVerified)
            {
                ToolVerificationStatus::VerificationSkipped
            } else {
                ToolVerificationStatus::NotRequired
            },
            execution_id: None,
            tool_name: None,
            tool_call_id: None,
            tool_call_trace_id: None,
            tool_result_trace_id: None,
            summary: default_summary.clone(),
        });

    receipt.authority_class = authority_class;
    receipt.executed = executed;
    if receipt.execution_status.trim().is_empty() {
        receipt.execution_status = default_execution_status.to_string();
    }
    if receipt.summary.is_none() {
        receipt.summary = default_summary;
    }
    receipt.execution_id = Some(execution_id.to_string());
    receipt.tool_name = Some(tool_name.to_string());
    receipt.tool_call_id = Some(tool_call_id.to_string());
    receipt.tool_call_trace_id = Some(tool_call_trace_id.to_string());
    receipt.tool_result_trace_id = Some(tool_result_trace_id.to_string());

    if let serde_json::Value::Object(map) = result_json {
        map.insert(
            "receipt".to_string(),
            serde_json::to_value(receipt).unwrap_or(serde_json::Value::Null),
        );
    }
}

/// Execute all tool calls from the model's response.
///
/// For each tool call: check approval, execute, log trace, emit events,
/// check sandbox entry/exit, handle shell resets.
pub async fn execute_tool_calls(
    agents: &Arc<RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    response: &AssembledResponse,
    tool_registry: &ToolRegistry,
    approval_config: &crate::config::app_config::ApprovalBlock,
    approval_manager: &Arc<ApprovalManager>,
    app_handle: Option<&AppHandle>,
    app_config: &RwLock<AppConfig>,
    shell_manager: &Arc<RwLock<ShellManager>>,
    trace_engine: &Arc<TraceEngine>,
    session_id: &str,
    vfl_registry: &Arc<VirtualFileLockRegistry>,
    subagent_manager: &Arc<SubagentManager>,
    a2a_registry: &Arc<A2ARegistry>,
    a2a_send_guard: &Arc<RwLock<HashSet<String>>>,
    app_state: &Option<Arc<crate::AppState>>,
    working_dir: &str,
) -> Result<ToolExecutionOutcome, EngineError> {
    let working_directory = {
        let agents_read = agents.read().await;
        agents_read
            .iter()
            .find(|a| a.id == agent_id)
            .map(|a| a.working_directory.clone())
            .unwrap_or_default()
    };

    let workspace_root = {
        let cfg = app_config.read().await;
        cfg.workspace.root.clone()
    };

    let mut loop_outcome = ToolExecutionOutcome::ContinueLoop;

    for tc in &response.tool_calls {
        let tool_start = std::time::Instant::now();
        let execution_id = uuid::Uuid::new_v4().to_string();
        let tool_call_trace_id = uuid::Uuid::new_v4().to_string();
        let authority_class = tool_registry
            .get_authority_class(&tc.name)
            .unwrap_or(ToolAuthorityClass::Informational);

        // Log tool call trace
        if let Err(e) = trace_engine.write_trace_with_id(
            tool_call_trace_id.clone(),
            TraceInput {
                agent_id: agent_id.to_string(),
                trace_type: TraceType::ToolCall,
                content: serde_json::json!({
                    "execution_id": execution_id,
                    "tool_call_id": tc.id,
                    "tool": tc.name,
                    "arguments": tc.arguments,
                    "authority_class": authority_class,
                }),
                outcome: Some(TraceOutcome::Unknown),
                duration_ms: None,
                token_count: None,
                parent_trace_id: None,
                session_id: session_id.to_string(),
                prompt_tokens: None,
                completion_tokens: None,
                cost_usd_cents: None,
            },
        ) {
            tracing::warn!(agent_id, "trace write failed: {e}");
        }

        if let Some(handle) = app_handle {
            if let Err(e) = handle.emit(
                "agent-stream",
                StreamEvent {
                    agent_id: agent_id.to_string(),
                    event_type: StreamEventType::ToolCall,
                    data: serde_json::json!({
                        "tool": tc.name,
                        "arguments": tc.arguments,
                        "tool_call_id": tc.id,
                        "authority_class": authority_class,
                    }),
                },
            ) {
                tracing::warn!(agent_id, "event emission failed: {e}");
            }
        }

        // APPROVAL CHECK: Check approval tier before execution (PRD 7.4)
        // Approval happens BEFORE VFL queue entry.
        let approval_decision = check_approval(
            &tc.name,
            &tc.id,
            &tc.arguments,
            agent_id,
            approval_config,
            app_handle,
            approval_manager,
            trace_engine,
            session_id,
            &tool_registry.get_description(&tc.name).unwrap_or_default(),
            working_dir,
        )
        .await;

        // Set status back to Working after approval wait
        {
            let mut agents_w = agents.write().await;
            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                if agent.status == AgentStatus::WaitingForHil {
                    agent.status = AgentStatus::Working;
                }
            }
        }

        if let ApprovalDecision::Denied { reason } = approval_decision {
            // Approval denied — inject error as tool result, skip execution
            let mut denied_json = serde_json::json!({
                "error": reason,
                "code": "HilDenied",
                "retryable": false,
            });
            let tool_result_trace_id = uuid::Uuid::new_v4().to_string();
            normalize_tool_receipt(
                &mut denied_json,
                authority_class,
                &tc.name,
                &tc.id,
                &execution_id,
                &tool_call_trace_id,
                &tool_result_trace_id,
                false,
                "blocked",
                Some(reason.clone()),
            );

            if let Err(e) = trace_engine.write_trace_with_id(
                tool_result_trace_id,
                TraceInput {
                    agent_id: agent_id.to_string(),
                    trace_type: TraceType::ToolResult,
                    content: denied_json.clone(),
                    outcome: Some(TraceOutcome::Failure {
                        reason: reason.clone(),
                    }),
                    duration_ms: Some(tool_start.elapsed().as_millis() as u64),
                    token_count: None,
                    parent_trace_id: Some(tool_call_trace_id.clone()),
                    session_id: session_id.to_string(),
                    prompt_tokens: None,
                    completion_tokens: None,
                    cost_usd_cents: None,
                },
            ) {
                tracing::warn!(agent_id, "trace write failed: {e}");
            }

            // Add denied result to conversation
            let mut agents_write = agents.write().await;
            if let Some(agent) = agents_write.iter_mut().find(|a| a.id == agent_id) {
                agent.add_message(ConversationMessage {
                    role: MessageRole::Tool,
                    content: denied_json.to_string(),
                    content_blocks: None,
                    timestamp: Utc::now(),
                    token_estimate: None,
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    outcome_tag: None,
                    thinking_content: None,
                    responses_phase: None,
                    reasoning_items: None,
                });
            }
            continue;
        }

        // Build tool context
        let tool_ctx = ToolContext {
            agent_id: agent_id.to_string(),
            working_directory: working_directory.clone(),
            workspace_root: workspace_root.clone(),
            trace_engine: trace_engine.clone(),
            app_handle: app_handle.cloned(),
            config: Arc::new(RwLock::new(app_config.read().await.clone())),
            shell_manager: shell_manager.clone(),
            session_id: session_id.to_string(),
            vfl_registry: Arc::clone(vfl_registry),
            subagent_manager: Arc::clone(subagent_manager),
            agents: Arc::clone(agents),
            a2a_registry: Arc::clone(a2a_registry),
            a2a_send_guard: Some(Arc::clone(a2a_send_guard)),
            app_state: app_state.clone(),
            http_client: app_state
                .as_ref()
                .map(|s| s.http_client.clone())
                .unwrap_or_else(reqwest::Client::new),
        };

        tracing::info!(agent_id, tool = tc.name, "executing tool");
        // Execute tool
        let tool_result = tool_registry
            .execute(&tc.name, tc.arguments.clone(), &tool_ctx)
            .await;
        let success_behavior = tool_registry
            .get_success_behavior(&tc.name)
            .unwrap_or(ToolSuccessBehavior::ContinueLoop);

        let tool_duration_ms = tool_start.elapsed().as_millis() as u64;

        // Format result for conversation and trace
        let (mut result_json, trace_outcome, image_content, ui_status, executed, default_summary) =
            match tool_result {
                Ok(result) => {
                    if success_behavior == ToolSuccessBehavior::EndTurnAfterSuccess {
                        loop_outcome = ToolExecutionOutcome::EndTurn;
                    }
                    let trace_outcome = parse_trace_outcome(&result.content);
                    let ui_status = parse_tool_status(&result.content).to_string();
                    let summary = result
                        .content
                        .get("receipt")
                        .and_then(|v| v.get("summary"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    (
                        result.content,
                        trace_outcome,
                        result.image_content,
                        ui_status,
                        true,
                        summary,
                    )
                }
                Err(tool_error) => {
                    let mut json = serde_json::json!({
                        "error": tool_error.message,
                        "code": format!("{:?}", tool_error.code),
                        "retryable": tool_error.retryable,
                    });
                    if let serde_json::Value::Object(map) = &mut json {
                        map.insert(
                            "tool_result_status".to_string(),
                            serde_json::json!("failed"),
                        );
                    }
                    (
                        json,
                        TraceOutcome::Failure {
                            reason: tool_error.message.clone(),
                        },
                        None,
                        "failed".to_string(),
                        false,
                        Some(tool_error.message),
                    )
                }
            };
        let tool_result_trace_id = uuid::Uuid::new_v4().to_string();
        normalize_tool_receipt(
            &mut result_json,
            authority_class,
            &tc.name,
            &tc.id,
            &execution_id,
            &tool_call_trace_id,
            &tool_result_trace_id,
            executed,
            if executed { "success" } else { "failed" },
            default_summary,
        );

        // Log tool result trace
        if let Err(e) = trace_engine.write_trace_with_id(
            tool_result_trace_id,
            TraceInput {
                agent_id: agent_id.to_string(),
                trace_type: TraceType::ToolResult,
                content: result_json.clone(),
                outcome: Some(trace_outcome),
                duration_ms: Some(tool_duration_ms),
                token_count: None,
                parent_trace_id: Some(tool_call_trace_id.clone()),
                session_id: session_id.to_string(),
                prompt_tokens: None,
                completion_tokens: None,
                cost_usd_cents: None,
            },
        ) {
            tracing::warn!(agent_id, "trace write failed: {e}");
        }

        // Emit tool result event to frontend
        if let Some(handle) = app_handle {
            if let Err(e) = handle.emit(
                "agent-stream",
                StreamEvent {
                    agent_id: agent_id.to_string(),
                    event_type: StreamEventType::ToolResult,
                    data: serde_json::json!({
                        "tool": tc.name,
                        "tool_call_id": tc.id,
                        "result": result_json,
                        "duration_ms": tool_duration_ms,
                        "status": ui_status,
                        "receipt": result_json.get("receipt").cloned(),
                    }),
                },
            ) {
                tracing::warn!(agent_id, "event emission failed: {e}");
            }
        }

        // Strip receipt metadata before conversation insertion.
        // The full receipt has already been sent to the trace system and frontend
        // event above — the model doesn't need it and it wastes ~40-60% of tool
        // result tokens per call.
        if let serde_json::Value::Object(ref mut map) = result_json {
            map.remove("receipt");
        }

        // Build content_blocks if tool returned images
        let tool_content_blocks = image_content.map(|images| {
            use crate::runtime::agent::ContentBlock;
            let mut blocks = vec![ContentBlock::Text {
                text: result_json.to_string(),
            }];
            blocks.extend(images);
            blocks
        });

        // Add tool result to conversation
        {
            let mut agents_write = agents.write().await;
            if let Some(agent) = agents_write.iter_mut().find(|a| a.id == agent_id) {
                agent.add_message(ConversationMessage {
                    role: MessageRole::Tool,
                    content: result_json.to_string(),
                    content_blocks: tool_content_blocks,
                    timestamp: Utc::now(),
                    token_estimate: None,
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    outcome_tag: None,
                    thinking_content: None,
                    responses_phase: None,
                    reasoning_items: None,
                });
            }
        }

        // Notify frontend that tool result has been committed
        if let Some(handle) = app_handle {
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

        // SANDBOX: Check for build/test failure or sandbox state transitions
        {
            let (not_in_sandbox, in_sandbox) = {
                let agents_read = agents.read().await;
                let is_in = agents_read
                    .iter()
                    .find(|a| a.id == agent_id)
                    .map(|a| a.sandbox.is_some())
                    .unwrap_or(false);
                (!is_in, is_in)
            };

            let bash_command = tc
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_bash_build =
                tc.name == "bash" && crate::runtime::sandbox::is_build_test_command(bash_command);
            let is_relevant_tool =
                tc.name == "execute_tests" || tc.name == "check_syntax" || is_bash_build;

            // DEBUG: sandbox detection trace
            tracing::info!(
                agent_id,
                tool = %tc.name,
                bash_command = %bash_command,
                is_bash_build,
                is_relevant_tool,
                not_in_sandbox,
                exit_code = ?result_json.get("exit_code"),
                "SANDBOX DEBUG: tool result check"
            );

            // Entry: detect build/test failure when NOT in sandbox
            if not_in_sandbox && is_relevant_tool {
                if let Some((_trigger_cmd, failure_output)) =
                    helpers::is_build_test_failure(&tc.name, &result_json)
                {
                    let sandbox_config = {
                        let agents_read = agents.read().await;
                        agents_read
                            .iter()
                            .find(|a| a.id == agent_id)
                            .map(|a| (a.sandbox_config.enabled, a.sandbox_config.max_iterations))
                            .unwrap_or((true, 4))
                    };
                    let (sandbox_enabled, max_iter) = sandbox_config;
                    if sandbox_enabled {
                        let actual_trigger = if tc.name == "bash" {
                            bash_command.to_string()
                        } else {
                            _trigger_cmd
                        };
                        if let Err(e) = crate::runtime::sandbox::enter(
                            agents,
                            agent_id,
                            shell_manager,
                            app_handle,
                            &actual_trigger,
                            &failure_output,
                            max_iter,
                        )
                        .await
                        {
                            tracing::error!(agent_id, "Failed to enter sandbox: {e}");
                        }
                    }
                }
            }

            // In sandbox: check tool results for pass/fail
            if in_sandbox && is_relevant_tool {
                let exit_code = result_json
                    .get("exit_code")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(-1);
                let valid = result_json
                    .get("valid")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                if exit_code == 0 && valid {
                    // Looks like success — verify and exit sandbox
                    match crate::runtime::sandbox::verify_and_exit(
                        agents,
                        agent_id,
                        shell_manager,
                        app_handle,
                    )
                    .await
                    {
                        Ok(()) => {
                            tracing::info!(agent_id, "Sandbox resolved successfully");
                        }
                        Err(e) => {
                            // Verification failed — advance
                            tracing::warn!(agent_id, "Sandbox verification failed: {e}");
                            match crate::runtime::sandbox::advance(agents, agent_id, &e).await {
                                Ok(crate::runtime::sandbox::SandboxAction::Continue) => {}
                                Ok(crate::runtime::sandbox::SandboxAction::Escalate) | Err(_) => {
                                    crate::runtime::sandbox::escalate(
                                        agents,
                                        agent_id,
                                        shell_manager,
                                        app_handle,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                } else {
                    // Still failing — advance the sandbox
                    let output = result_json
                        .get("output")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(no output)")
                        .to_string();
                    match crate::runtime::sandbox::advance(agents, agent_id, &output).await {
                        Ok(crate::runtime::sandbox::SandboxAction::Continue) => {}
                        Ok(crate::runtime::sandbox::SandboxAction::Escalate) | Err(_) => {
                            crate::runtime::sandbox::escalate(
                                agents,
                                agent_id,
                                shell_manager,
                                app_handle,
                            )
                            .await;
                        }
                    }
                }
            }
        }

        // Inject system message if shell session was reset
        if tc.name == "bash" {
            if let Some(true) = result_json.get("session_reset").and_then(|v| v.as_bool()) {
                let mut agents_w = agents.write().await;
                if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                    agent.add_message(ConversationMessage {
                        role: MessageRole::System,
                        content: "Note: The shell environment crashed and was restarted. Environment variables, aliases, functions, and shell state have been lost. You may need to re-set any environment variables that were previously configured.".to_string(),
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
        }
    }

    Ok(loop_outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tool_receipt_synthesizes_receipt_when_missing() {
        let mut result_json = serde_json::json!({
            "tool_result_status": "failed",
            "error": "blocked by policy",
        });

        normalize_tool_receipt(
            &mut result_json,
            ToolAuthorityClass::Effectful,
            "send_message_to_agent",
            "toolu_123",
            "exec_123",
            "trace_call",
            "trace_result",
            false,
            "blocked",
            Some("blocked by policy".to_string()),
        );

        let receipt: ToolExecutionReceipt = serde_json::from_value(
            result_json
                .get("receipt")
                .cloned()
                .expect("receipt should exist"),
        )
        .expect("receipt should deserialize");

        assert!(!receipt.executed);
        assert_eq!(receipt.execution_status, "blocked");
        assert_eq!(receipt.execution_id.as_deref(), Some("exec_123"));
        assert_eq!(receipt.tool_call_trace_id.as_deref(), Some("trace_call"));
        assert_eq!(
            receipt.tool_result_trace_id.as_deref(),
            Some("trace_result")
        );
        assert_eq!(receipt.tool_name.as_deref(), Some("send_message_to_agent"));
        assert_eq!(receipt.tool_call_id.as_deref(), Some("toolu_123"));
    }

    #[test]
    fn normalize_tool_receipt_preserves_existing_receipt_fields() {
        let mut result_json = serde_json::json!({
            "tool_result_status": "success",
            "receipt": {
                "authority_class": "effectful_verified",
                "executed": true,
                "execution_status": "success",
                "verified": true,
                "verification_status": "verified",
                "summary": "wrote durable memory"
            }
        });

        normalize_tool_receipt(
            &mut result_json,
            ToolAuthorityClass::EffectfulVerified,
            "remember",
            "toolu_456",
            "exec_456",
            "trace_call_b",
            "trace_result_b",
            true,
            "success",
            None,
        );

        let receipt: ToolExecutionReceipt = serde_json::from_value(
            result_json
                .get("receipt")
                .cloned()
                .expect("receipt should exist"),
        )
        .expect("receipt should deserialize");

        assert!(receipt.verified);
        assert_eq!(
            receipt.verification_status,
            ToolVerificationStatus::Verified
        );
        assert_eq!(receipt.summary.as_deref(), Some("wrote durable memory"));
        assert_eq!(receipt.execution_id.as_deref(), Some("exec_456"));
    }
}
