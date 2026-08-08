use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct UnpinMemoryTool;

#[async_trait::async_trait]
impl Tool for UnpinMemoryTool {
    fn name(&self) -> &'static str {
        "unpin_memory"
    }

    fn description(&self) -> &'static str {
        "Unpin a memory so it resumes normal decay. Frees a pinned slot."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"memory_id": "abc-123"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "memory_id": {
                    "type": "string",
                    "description": "The ID of the memory to unpin"
                }
            },
            "required": ["memory_id"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let app_state = context.app_state.as_ref().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "AppState not available".to_string(),
            retryable: false,
        })?;

        let memory_engine = app_state.get_memory_engine().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "Memory system not available".to_string(),
            retryable: false,
        })?;

        let memory_id = arguments
            .get("memory_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: memory_id".to_string(),
                retryable: false,
            })?;

        memory_engine
            .unpin(memory_id)
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to unpin memory: {e}"),
                retryable: false,
            })?;

        Ok(ToolResult {
            content: json!({
                "success": true,
                "memory_id": memory_id,
                "pinned": false,
                "message": "Memory unpinned — demoted to warm tier, normal decay resumed"
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
