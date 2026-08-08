use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct PinTool;

#[async_trait::async_trait]
impl Tool for PinTool {
    fn name(&self) -> &'static str {
        "pin_memory"
    }

    fn description(&self) -> &'static str {
        "Pin a memory so it is never decayed or pruned, and is packed FIRST into your session-start \
         context — ahead of the importance-ordered fill — via the Living Persona profile. Bounded \
         by that pack's token budget, so pinning prioritises a memory rather than guaranteeing \
         it. Max 5 pins per agent. Use unpin to release. Reserve for sacred or load-bearing \
         memories."
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
                    "description": "The ID of the memory to pin"
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
            .pin(memory_id, &agent_ns)
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to pin memory: {e}"),
                retryable: false,
            })?;

        Ok(ToolResult {
            content: json!({
                "success": true,
                "memory_id": memory_id,
                "pinned": true,
                "message": "Memory pinned successfully — always injected, never decays"
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hillock_core::ops::pin::MAX_PINS;

    /// The advertised cap must be the enforced cap.
    ///
    /// `pin_memory` said "Max 5 pins" while the engine enforced 3, and nothing noticed
    /// because the description is prose and the cap is a constant in another crate. This is
    /// the template for description-vs-code: assert the text against the value it describes.
    #[test]
    fn pin_description_advertises_the_cap_the_engine_enforces() {
        let desc = PinTool.description();
        let expected = format!("Max {MAX_PINS} pins");
        assert!(
            desc.contains(&expected),
            "pin_memory advertises a cap the engine does not enforce (expected {expected:?}) in: {desc}"
        );
    }
}
