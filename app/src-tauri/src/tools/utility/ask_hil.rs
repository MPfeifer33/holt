use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct AskHilTool;

#[async_trait::async_trait]
impl Tool for AskHilTool {
    fn name(&self) -> &'static str {
        "ask_hil"
    }

    fn description(&self) -> &'static str {
        "Ask the human a question and pause until they respond. Use this when you need explicit input, approval, or a decision before proceeding. Prefer this over guessing."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(
            json!({"question": "Should I proceed with the migration?", "options": ["yes", "no", "defer"]}),
        )
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "What you need from the human" },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of choices for the human"
                },
                "blocking": { "type": "boolean", "description": "Wait for response (default: true)" }
            },
            "required": ["question"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let question = arguments
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: question".to_string(),
                retryable: false,
            })?;

        let options: Option<Vec<String>> = arguments
            .get("options")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let blocking = arguments
            .get("blocking")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        // Emit HIL question event to frontend
        if let Some(ref app_handle) = context.app_handle {
            use tauri::Emitter;
            let _ = app_handle.emit(
                "agent-hil-question",
                json!({
                    "agent_id": context.agent_id,
                    "question": question,
                    "options": options,
                    "blocking": blocking,
                }),
            );
        }

        if blocking {
            // Create a oneshot channel and store sender in the shell manager
            // (using shell_manager as a shared state holder for HIL responses too)
            let (tx, rx) = tokio::sync::oneshot::channel::<serde_json::Value>();

            // Store the sender keyed by agent_id for the respond_to_hil command to resolve
            {
                let mut manager = context.shell_manager.write().await;
                manager.hil_senders.insert(context.agent_id.clone(), tx);
            }

            // Wait for the response (with configurable timeout)
            let hil_timeout = context.config.read().await.limits.response_timeout_seconds;
            let response =
                tokio::time::timeout(std::time::Duration::from_secs(hil_timeout), rx).await;

            // Always clear the sender entry after the wait completes. The normal
            // response path already removes it in `respond_to_hil`; this is the
            // timeout/channel-drop cleanup path.
            {
                let mut manager = context.shell_manager.write().await;
                manager.hil_senders.remove(&context.agent_id);
            }

            let response = response
                .map_err(|_| ToolError {
                    code: ToolErrorCode::CommandTimeout,
                    message: format!("HIL response timed out after {}s", hil_timeout),
                    retryable: false,
                })?
                .map_err(|_| ToolError {
                    code: ToolErrorCode::InternalError,
                    message: "HIL response channel was dropped".to_string(),
                    retryable: false,
                })?;

            Ok(ToolResult {
                content: response,
                truncated: false,
                trace_id: None,
                image_content: None,
            })
        } else {
            Ok(ToolResult {
                content: json!({
                    "response": null,
                    "selected_option": null,
                    "status": "pending",
                }),
                truncated: false,
                trace_id: None,
                image_content: None,
            })
        }
    }
}
