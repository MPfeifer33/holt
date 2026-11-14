use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};
use serde_json::json;

pub struct CrystallizeTool;

#[async_trait::async_trait]
impl Tool for CrystallizeTool {
    fn name(&self) -> &'static str {
        "memory_crystallize"
    }

    fn description(&self) -> &'static str {
        "Combine multiple episode memories into a single concept memory that captures the shared pattern. Requires at least 2 source memories."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(
            json!({"summary": "Config always uses port 8080", "source_memory_ids": ["id-1", "id-2"]}),
        )
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string", "description": "Distilled concept — what pattern do these episodes share?" },
                "source_memory_ids": { "type": "array", "items": { "type": "string" }, "description": "IDs of the episode memories to crystallize (minimum 2)" },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tags for provenance or categorization on the crystallized memory"
                }
            },
            "required": ["summary", "source_memory_ids"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let app_state = context.app_state.as_ref().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "AppState not available".into(),
            retryable: false,
        })?;
        let engine = app_state.get_memory_engine().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "Memory system not available".into(),
            retryable: false,
        })?;

        let summary = arguments
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing: summary".into(),
                retryable: false,
            })?;
        let source_ids: Vec<String> = arguments
            .get("source_memory_ids")
            .map(|v| match v {
                serde_json::Value::Array(arr) => arr
                    .iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect(),
                serde_json::Value::String(s) if s.contains(',') => s
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect(),
                serde_json::Value::String(s) if !s.is_empty() => vec![s.clone()],
                _ => vec![],
            })
            .unwrap_or_default();
        let tags: Vec<String> = arguments
            .get("tags")
            .map(|v| match v {
                serde_json::Value::Array(arr) => arr
                    .iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect(),
                serde_json::Value::String(s) => s
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect(),
                _ => vec![],
            })
            .unwrap_or_default();

        if source_ids.len() < 2 {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Need at least 2 source memory IDs".into(),
                retryable: false,
            });
        }

        // Crystallize as the calling agent, so the concept is written to the
        // caller's own memory space.
        let agent_ns = crate::memory::MemoryEngine::agent_namespace(&context.agent_id);
        let result = engine
            .bridge_crystallize(summary, &source_ids, &agent_ns, &tags)
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Crystallize failed: {e}"),
                retryable: true,
            })?;

        // The engine supersedes the sources: they stay stored and leave
        // default recall. Report what it did, nothing more.
        let superseded = result
            .get("superseded")
            .and_then(|s| s.as_array())
            .map(|s| s.len())
            .unwrap_or(0);

        Ok(ToolResult {
            content: json!({
                "crystallized": result,
                "sources_superseded": superseded,
                "total_sources": source_ids.len(),
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

    #[test]
    fn crystallize_schema_exposes_optional_tags() {
        let schema = CrystallizeTool.parameters_schema();
        let tags = schema
            .get("properties")
            .and_then(|v| v.get("tags"))
            .and_then(|v| v.get("type"))
            .and_then(|v| v.as_str());
        assert_eq!(tags, Some("array"));
    }
}
