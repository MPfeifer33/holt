use serde_json::json;

use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};

pub struct ReadAgentWorkspaceTool;

#[async_trait::async_trait]
impl Tool for ReadAgentWorkspaceTool {
    fn name(&self) -> &'static str {
        "read_agent_workspace"
    }

    fn description(&self) -> &'static str {
        "Read another agent's recent conversation messages. Requires workspace visibility to be enabled for that agent pair."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"target_agent_id": "agent-name-or-id", "last_n": 10}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "target_agent_id": { "type": "string", "description": "ID or name of the agent whose workspace to read" },
                "last_n": { "type": "integer", "description": "Number of recent messages to read (default: 10, max: 50)" }
            },
            "required": ["target_agent_id"]
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

        let last_n = arguments
            .get("last_n")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(50) as usize;

        // Resolve target by ID or name (case-insensitive)
        let resolved_target_id = {
            let agents = context.agents.read().await;
            if let Some(agent) = agents.iter().find(|a| a.id == target_id_raw) {
                agent.id.clone()
            } else if let Some(agent) = agents
                .iter()
                .find(|a| a.name.eq_ignore_ascii_case(target_id_raw))
            {
                agent.id.clone()
            } else {
                return Err(ToolError {
                    code: ToolErrorCode::InvalidInput,
                    message: format!("Target agent not found: {}", target_id_raw),
                    retryable: false,
                });
            }
        };

        // Check workspace visibility
        if !context
            .a2a_registry
            .is_workspace_visible(&context.agent_id, &resolved_target_id)
            .await
        {
            return Err(ToolError {
                code: ToolErrorCode::A2ANotEnabled,
                message: format!(
                    "Workspace visibility not enabled from agent '{}' to '{}'. Ask the user to approve collaboration for this pair and choose the option that enables messaging + workspace visibility.",
                    context.agent_id, resolved_target_id
                ),
                retryable: false,
            });
        }

        // Read target agent's conversation
        let agents = context.agents.read().await;
        let target = agents
            .iter()
            .find(|a| a.id == resolved_target_id)
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("Target agent not found: {}", resolved_target_id),
                retryable: false,
            })?;

        let messages: Vec<serde_json::Value> = target
            .conversation
            .messages
            .iter()
            .rev()
            .take(last_n)
            .rev()
            .map(|m| {
                json!({
                    "role": format!("{:?}", m.role),
                    "content": if m.content.chars().count() > 500 {
                        format!("{}...", m.content.chars().take(500).collect::<String>())
                    } else {
                        m.content.clone()
                    },
                    "timestamp": m.timestamp.to_rfc3339(),
                })
            })
            .collect();

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "target_agent_id": resolved_target_id,
                "target_agent_name": target.name,
                "message_count": messages.len(),
                "messages": messages,
                "receipt": ToolExecutionReceipt {
                    authority_class: ToolAuthorityClass::Informational,
                    executed: true,
                    execution_status: "success".to_string(),
                    verified: false,
                    verification_status: ToolVerificationStatus::NotRequired,
                    execution_id: None,
                    tool_name: None,
                    tool_call_id: None,
                    tool_call_trace_id: None,
                    tool_result_trace_id: None,
                    summary: Some(format!(
                        "Read {} recent workspace message{} from {}",
                        last_n.min(target.conversation.messages.len()),
                        if last_n.min(target.conversation.messages.len()) == 1 { "" } else { "s" },
                        target.name
                    )),
                },
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
