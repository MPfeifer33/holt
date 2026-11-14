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

        let agent_ns = crate::memory::MemoryEngine::agent_namespace(&context.agent_id);

        let was_pinned = memory_engine
            .unpin(memory_id, &agent_ns)
            .await
            .map_err(unpin_error)?;

        // Unpinning something that was not pinned changes nothing and is
        // not a failure; the result says which of the two happened.
        Ok(ToolResult {
            content: json!({
                "success": true,
                "memory_id": memory_id,
                "pinned": false,
                "was_pinned": was_pinned,
                "message": if was_pinned { UNPINNED_MESSAGE } else { NOT_PINNED_MESSAGE }
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

/// Unpinning removes the pin row and nothing else. There are no tiers to demote to.
const UNPINNED_MESSAGE: &str =
    "Memory unpinned: the slot is free, and the memory decays and ranks like any other again";

/// Said when there was no pin to release.
const NOT_PINNED_MESSAGE: &str = "Nothing to unpin: the memory was not pinned, so nothing changed";

fn unpin_error(e: crate::memory::MemoryError) -> ToolError {
    use crate::memory::MemoryError;
    match e {
        MemoryError::Validation(_) | MemoryError::NotFound(_) => ToolError {
            code: ToolErrorCode::InvalidInput,
            message: format!("Memory not unpinned: {e}"),
            retryable: false,
        },
        e => ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to unpin memory: {e}"),
            retryable: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_result_says_what_unpinning_did() {
        for text in [UNPINNED_MESSAGE, NOT_PINNED_MESSAGE] {
            assert!(!text.contains("tier"), "there are no tiers: {text}");
        }
        assert!(UNPINNED_MESSAGE.contains("unpinned"));
        assert!(NOT_PINNED_MESSAGE.contains("not pinned"));
    }

    #[test]
    fn a_malformed_id_is_the_callers_error() {
        let err = unpin_error(crate::memory::MemoryError::Validation(
            "Invalid memory ID".into(),
        ));
        assert!(matches!(err.code, ToolErrorCode::InvalidInput));
    }
}
