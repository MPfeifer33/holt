use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};

pub struct AlertUserTool;

#[async_trait::async_trait]
impl Tool for AlertUserTool {
    fn name(&self) -> &'static str {
        "alert_user"
    }

    fn description(&self) -> &'static str {
        "Send a notification to the human when you need their attention and they may not be actively watching. Use this for blockers, required decisions, important findings, or completion notices that should interrupt the user. Do not use this as a substitute for your normal conversational response when the human is already engaged. Set blocking=true whenever work must pause for acknowledgement or explicit confirmation. If you describe the alert as blocked, blocking, paused pending confirmation, or awaiting acknowledgement, the call must include blocking=true."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"message": "Build complete", "priority": "medium", "blocking": false}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message to show the user"
                },
                "priority": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "critical"],
                    "description": "How urgently to alert. low=subtle, critical=audio+title flash"
                },
                "blocking": {
                    "type": "boolean",
                    "description": "If true, pause execution until user acknowledges. Required whenever the alert says work is blocked, paused, or waiting for confirmation."
                }
            },
            "required": ["message", "blocking"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let message = arguments
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: message".to_string(),
                retryable: false,
            })?
            .to_string();

        let priority = arguments
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or("high")
            .to_string();

        let blocking = arguments
            .get("blocking")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let alert_id = Uuid::new_v4().to_string();

        // Emit event to frontend
        if let Some(ref app_handle) = context.app_handle {
            use tauri::Emitter;
            let _ = app_handle.emit(
                "agent-alert",
                json!({
                    "agent_id": context.agent_id,
                    "alert_id": alert_id,
                    "message": message,
                    "priority": priority,
                    "blocking": blocking,
                }),
            );
        }

        if !blocking {
            return Ok(ToolResult {
                content: json!({
                    "tool_result_status": "success",
                    "status": "sent",
                    "blocking": false,
                    "alert_id": alert_id,
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
                        summary: Some("Alert delivered to user".to_string()),
                    },
                }),
                truncated: false,
                trace_id: None,
                image_content: None,
            });
        }

        // Blocking: wait for acknowledgement via ApprovalManager
        let app_state = context.app_state.as_ref().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "AppState not available for blocking alert".to_string(),
            retryable: false,
        })?;

        let key = format!("{}:{}", context.agent_id, alert_id);
        let rx = app_state.approval_manager.register(&key).await;

        match tokio::time::timeout(Duration::from_secs(300), rx).await {
            Ok(Ok(_)) => Ok(ToolResult {
                content: json!({
                    "tool_result_status": "success",
                    "status": "acknowledged",
                    "blocking": true,
                    "alert_id": alert_id,
                    "message": "User acknowledged. Do not send another alert_user call. Provide a short final answer now.",
                    "receipt": ToolExecutionReceipt {
                        authority_class: ToolAuthorityClass::Effectful,
                        executed: true,
                        execution_status: "acknowledged".to_string(),
                        verified: false,
                        verification_status: ToolVerificationStatus::NotRequired,
                        execution_id: None,
                        tool_name: None,
                        tool_call_id: None,
                        tool_call_trace_id: None,
                        tool_result_trace_id: None,
                        summary: Some("Blocking alert acknowledged by user".to_string()),
                    },
                }),
                truncated: false,
                trace_id: None,
                image_content: None,
            }),
            _ => {
                // Resolve as false to clean up the channel
                app_state.approval_manager.resolve(&key, false).await;
                Ok(ToolResult {
                    content: json!({
                        "tool_result_status": "partial",
                        "status": "timed_out",
                        "blocking": true,
                        "alert_id": alert_id,
                        "message": "User did not respond (timeout)",
                        "receipt": ToolExecutionReceipt {
                            authority_class: ToolAuthorityClass::Effectful,
                            executed: true,
                            execution_status: "timed_out".to_string(),
                            verified: false,
                            verification_status: ToolVerificationStatus::NotRequired,
                            execution_id: None,
                            tool_name: None,
                            tool_call_id: None,
                            tool_call_trace_id: None,
                            tool_result_trace_id: None,
                            summary: Some("Blocking alert timed out without acknowledgement".to_string()),
                        },
                    }),
                    truncated: false,
                    trace_id: None,
                    image_content: None,
                })
            }
        }
    }
}
