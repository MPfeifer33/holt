use serde_json::json;
use tauri::Emitter;

use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolSuccessBehavior, ToolVerificationStatus,
};

pub struct SendMessageToAgentTool;

#[async_trait::async_trait]
impl Tool for SendMessageToAgentTool {
    fn name(&self) -> &'static str {
        "send_message_to_agent"
    }

    fn description(&self) -> &'static str {
        "Send a message to another agent for coordination, handoff, or sharing findings. The target agent will be woken up automatically if idle. Use the agent ID from list_agents or the FROM header in received A2A messages — do not guess from names."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(
            json!({"target_agent_id": "agent-name-or-id", "content": "Can you review this change?"}),
        )
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "target_agent_id": { "type": "string", "description": "ID or name of the agent to send the message to" },
                "content": { "type": "string", "description": "Message content to send" }
            },
            "required": ["target_agent_id", "content"]
        })
    }

    fn success_behavior(&self) -> ToolSuccessBehavior {
        ToolSuccessBehavior::EndTurnAfterSuccess
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        super::ensure_agent_a2a_enabled(context)?;

        let target_id = arguments
            .get("target_agent_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: target_agent_id".to_string(),
                retryable: false,
            })?;

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: content".to_string(),
                retryable: false,
            })?;

        // Get sender's name for the message
        let sender_name = {
            let agents = context.agents.read().await;
            agents
                .iter()
                .find(|a| a.id == context.agent_id)
                .map(|a| a.name.clone())
                .unwrap_or_else(|| context.agent_id.clone())
        };

        // Resolve target agent by ID or name (case-insensitive)
        let (resolved_target_id, resolved_target_name) = {
            let agents = context.agents.read().await;
            if let Some(agent) = agents.iter().find(|a| a.id == target_id) {
                (agent.id.clone(), agent.name.clone())
            } else if let Some(agent) = agents
                .iter()
                .find(|a| a.name.eq_ignore_ascii_case(target_id))
            {
                (agent.id.clone(), agent.name.clone())
            } else {
                return Err(ToolError {
                    code: ToolErrorCode::InvalidInput,
                    message: format!(
                        "Target agent not found: {}. Available agents: {}",
                        target_id,
                        agents
                            .iter()
                            .filter(|a| a.id != context.agent_id)
                            .map(|a| format!("'{}' ({})", a.name, a.id))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    retryable: false,
                });
            }
        };

        if let Some(ref guard) = context.a2a_send_guard {
            let already_sent = {
                let sent_targets = guard.read().await;
                sent_targets.contains(&resolved_target_id)
            };
            if already_sent {
                return Ok(ToolResult {
                    content: json!({
                        "tool_result_status": "partial",
                        "status": "suppressed_duplicate",
                        "target_agent_id": target_id,
                        "message_length": content.len(),
                        "note": "A message to this target was already sent during the current agentic loop. Do not resend unless a new turn begins.",
                        "receipt": ToolExecutionReceipt {
                            authority_class: ToolAuthorityClass::Effectful,
                            executed: false,
                            execution_status: "suppressed_duplicate".to_string(),
                            verified: false,
                            verification_status: ToolVerificationStatus::NotRequired,
                            execution_id: None,
                            tool_name: None,
                            tool_call_id: None,
                            tool_call_trace_id: None,
                            tool_result_trace_id: None,
                            summary: Some("Duplicate outbound A2A send suppressed for this loop".to_string()),
                        },
                    }),
                    truncated: false,
                    trace_id: None,
                    image_content: None,
                });
            }
        }

        context
            .a2a_registry
            .send_message(
                &context.agent_id,
                &sender_name,
                &resolved_target_id,
                content,
            )
            .await
            .map_err(|reason| ToolError {
                code: ToolErrorCode::A2ANotEnabled,
                message: format!(
                    "{}. This agent pair is currently closed. Call request_collaboration with the target agent and a short reason so the user can approve messaging for this pair.",
                    reason
                ),
                retryable: false,
            })?;

        if let Some(ref guard) = context.a2a_send_guard {
            guard.write().await.insert(resolved_target_id.clone());
        }

        // Emit event for frontend stardust pulse
        if let Some(ref handle) = context.app_handle {
            let _ = handle.emit(
                "agent-a2a-message",
                json!({
                    "from_id": context.agent_id,
                    "to_id": resolved_target_id,
                }),
            );

            // Emit rich relay event for A2A visibility panel
            let relay_timestamp = chrono::Utc::now().to_rfc3339();
            let _ = handle.emit(
                "a2a-relay",
                json!({
                    "from_id": context.agent_id,
                    "from_name": sender_name,
                    "to_id": resolved_target_id,
                    "to_name": resolved_target_name,
                    "content": content,
                    "timestamp": relay_timestamp,
                }),
            );

            // Write to persistent gzip log
            if let Some(ref app_state) = context.app_state {
                app_state
                    .a2a_logger
                    .log_message(&sender_name, &resolved_target_name, content);
            }

            // Emit wake event so frontend can update UI for idle targets
            let _ = handle.emit(
                "agent-a2a-wake",
                json!({
                    "target_agent_id": resolved_target_id,
                    "from_agent_id": context.agent_id,
                    "from_agent_name": sender_name,
                }),
            );

            // Server-side auto-wake for non-SDK agents.
            // SDK agents have their own auto-wake loop (agent_sdk.rs:1062-1079).
            // Externally-driven slots (e.g. the TUI lane's placeholder connection)
            // have their turns submitted by an external process — Holt must
            // never submit an engine turn on their behalf.
            let (target_protocol, externally_driven) = {
                let agents = context.agents.read().await;
                agents
                    .iter()
                    .find(|a| a.id == resolved_target_id)
                    .map(|a| (Some(a.protocol.clone()), a.is_externally_driven()))
                    .unwrap_or((None, false))
            };

            if externally_driven {
                tracing::info!(
                    target = %resolved_target_id,
                    "A2A: target is externally driven; message queued, no wake submitted"
                );
            }

            if let Some(protocol) = target_protocol {
                if protocol != crate::runtime::connection::AgentProtocol::AgentSdk
                    && !externally_driven
                {
                    if let Some(ref app_state) = context.app_state {
                        let state = app_state.clone();
                        let wake_handle = handle.clone();
                        let target = resolved_target_id.clone();
                        tauri::async_runtime::spawn(async move {
                            // Respect cooldown to prevent circular wake storms
                            if !state.a2a_registry.check_wake_cooldown(&target).await {
                                tracing::debug!(
                                    target = target,
                                    "A2A auto-wake: cooldown active, skipping"
                                );
                                return;
                            }
                            if let Err(e) =
                                crate::commands::agent_commands::messaging::a2a_wake_agent(
                                    &state,
                                    &wake_handle,
                                    &target,
                                )
                                .await
                            {
                                tracing::warn!(
                                    target = target,
                                    error = %e,
                                    "A2A auto-wake failed"
                                );
                            }
                        });
                    }
                }
            }
        }

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "status": "sent",
                "target_agent_id": target_id,
                "message_length": content.len(),
                "receipt": ToolExecutionReceipt {
                    authority_class: ToolAuthorityClass::Effectful,
                    executed: true,
                    execution_status: "success".to_string(),
                    verified: false,
                    verification_status: ToolVerificationStatus::NotRequired,
                    execution_id: None,
                    tool_name: None,
                    tool_call_id: None,
                    tool_call_trace_id: None,
                    tool_result_trace_id: None,
                    summary: Some(format!("Direct message sent to {}", target_id)),
                },
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
