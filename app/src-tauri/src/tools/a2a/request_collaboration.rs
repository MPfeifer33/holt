use serde_json::json;
use tauri::Emitter;

use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};

pub struct RequestCollaborationTool;

#[async_trait::async_trait]
impl Tool for RequestCollaborationTool {
    fn name(&self) -> &'static str {
        "request_collaboration"
    }

    fn description(&self) -> &'static str {
        "Ask the user to enable A2A communication or workspace visibility with another agent."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(
            json!({"target_agent_id": "agent-name-or-id", "reason": "Need to coordinate on the refactor"}),
        )
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "target_agent_id": { "type": "string", "description": "ID or name of the agent you want to collaborate with" },
                "reason": { "type": "string", "description": "Why you want to collaborate with this agent" }
            },
            "required": ["target_agent_id", "reason"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        super::ensure_agent_a2a_enabled(context)?;

        let target_id_raw = arguments
            .get("target_agent_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: target_agent_id".to_string(),
                retryable: false,
            })?;

        let reason = arguments
            .get("reason")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: reason".to_string(),
                retryable: false,
            })?;

        // Resolve target by ID or name (case-insensitive), and get display names
        let (resolved_id, source_name, target_name) = {
            let agents = context.agents.read().await;
            let source = agents
                .iter()
                .find(|a| a.id == context.agent_id)
                .map(|a| a.name.clone())
                .unwrap_or_else(|| context.agent_id.clone());

            let (tid, tname) = if let Some(agent) = agents.iter().find(|a| a.id == target_id_raw) {
                (agent.id.clone(), agent.name.clone())
            } else if let Some(agent) = agents
                .iter()
                .find(|a| a.name.eq_ignore_ascii_case(target_id_raw))
            {
                (agent.id.clone(), agent.name.clone())
            } else {
                return Err(ToolError {
                    code: ToolErrorCode::InvalidInput,
                    message: format!("Target agent not found: {}", target_id_raw),
                    retryable: false,
                });
            };
            (tid, source, tname)
        };

        if resolved_id == context.agent_id {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "You cannot request collaboration with yourself.".to_string(),
                retryable: false,
            });
        }

        if context
            .a2a_registry
            .is_messaging_enabled_bidirectional(&context.agent_id, &resolved_id)
            .await
        {
            return Ok(ToolResult {
                content: json!({
                    "tool_result_status": "success",
                    "status": "already_enabled",
                    "target_agent_id": resolved_id,
                    "target_agent_name": target_name,
                    "message": "Collaboration is already enabled for this agent pair.",
                    "receipt": ToolExecutionReceipt {
                        authority_class: ToolAuthorityClass::Effectful,
                        executed: false,
                        execution_status: "already_enabled".to_string(),
                        verified: false,
                        verification_status: ToolVerificationStatus::NotRequired,
                        execution_id: None,
                        tool_name: None,
                        tool_call_id: None,
                        tool_call_trace_id: None,
                        tool_result_trace_id: None,
                        summary: Some("Collaboration pair was already enabled".to_string()),
                    }
                }),
                truncated: false,
                trace_id: None,
                image_content: None,
            });
        }

        // Emit event to frontend
        if let Some(handle) = &context.app_handle {
            let _ = handle.emit(
                "agent-a2a-request",
                json!({
                    "source_agent_id": context.agent_id,
                    "source_agent_name": source_name,
                    "target_agent_id": resolved_id,
                    "target_agent_name": target_name,
                    "reason": reason,
                }),
            );
        }

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "status": "requested",
                "target_agent_id": resolved_id,
                "target_agent_name": target_name,
                "message": "Collaboration request sent to user. They will need to enable A2A communication for this pair.",
                "receipt": ToolExecutionReceipt {
                    authority_class: ToolAuthorityClass::Effectful,
                    executed: true,
                    execution_status: "requested".to_string(),
                    verified: false,
                    verification_status: ToolVerificationStatus::NotRequired,
                    execution_id: None,
                    tool_name: None,
                    tool_call_id: None,
                    tool_call_trace_id: None,
                    tool_result_trace_id: None,
                    summary: Some(format!("Requested collaboration with {}", target_name)),
                }
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
