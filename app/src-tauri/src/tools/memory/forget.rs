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
            .map_err(forget_error)?;

        Ok(ToolResult {
            content: json!({
                "deleted": true,
                "memory_id": memory_id,
                "message": FORGOTTEN_MESSAGE
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

/// What the engine's forget does: marks the row (and a long memory's fragments) deleted,
/// drops it from the search indexes, removes its pins, and writes one audit row.
const FORGOTTEN_MESSAGE: &str =
    "Memory removed from recall. The row is kept on disk marked deleted, \
     its pins are removed, and the deletion is in the audit log under your agent id";

/// A forget the engine refused (no such memory, a fragment, a space the caller cannot
/// read, a malformed id) is the caller's to fix, and is reported as such.
fn forget_error(e: crate::memory::MemoryError) -> ToolError {
    use crate::memory::MemoryError;
    match e {
        MemoryError::Validation(_) | MemoryError::NotFound(_) | MemoryError::ForgetRefused(_) => {
            ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("Memory not deleted: {e}"),
                retryable: false,
            }
        }
        e => ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to delete memory: {e}"),
            retryable: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_result_agrees_with_the_description_that_nothing_is_erased() {
        assert!(
            !FORGOTTEN_MESSAGE.contains("permanent"),
            "{FORGOTTEN_MESSAGE}"
        );
        assert!(
            FORGOTTEN_MESSAGE.contains("kept on disk") && FORGOTTEN_MESSAGE.contains("audit log")
        );
        assert!(ForgetTool.description().contains("soft-deleted"));
    }

    #[test]
    fn a_refused_forget_is_the_callers_error_and_carries_the_reason() {
        let err = forget_error(crate::memory::MemoryError::ForgetRefused(
            "cannot forget individual chunk — forget the parent instead".into(),
        ));
        assert!(matches!(err.code, ToolErrorCode::InvalidInput));
        assert_eq!(
            err.message,
            "Memory not deleted: cannot forget individual chunk — forget the parent instead"
        );

        let gone = forget_error(crate::memory::MemoryError::NotFound("abc".into()));
        assert!(matches!(gone.code, ToolErrorCode::InvalidInput));
    }
}
