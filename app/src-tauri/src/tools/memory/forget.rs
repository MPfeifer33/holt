use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct ForgetTool;

#[async_trait::async_trait]
impl Tool for ForgetTool {
    fn name(&self) -> &'static str {
        "forget"
    }

    fn description(&self) -> &'static str {
        "Delete a memory by ID. The row is soft-deleted: excluded from all recall and retained on \
         disk, not erased. The deletion is recorded in the audit log against your agent id, and \
         you can only delete memories in spaces you can read."
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
                    "description": "The ID of the memory to delete"
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

        let agent_ns = crate::memory::MemoryEngine::agent_namespace(&context.agent_id);

        memory_engine
            .delete(memory_id, &agent_ns)
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to delete memory: {e}"),
                retryable: false,
            })?;

        Ok(ToolResult {
            content: json!({
                "deleted": true,
                "memory_id": memory_id,
                "message": "Memory deleted permanently"
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
