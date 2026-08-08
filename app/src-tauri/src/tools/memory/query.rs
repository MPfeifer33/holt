use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct MemoryQueryTool;

#[async_trait::async_trait]
impl Tool for MemoryQueryTool {
    fn name(&self) -> &'static str {
        "memory_filter"
    }

    fn description(&self) -> &'static str {
        "Query memories by metadata (tags, category, importance) without semantic search. Use this when you know the exact tag or category you want, rather than searching by meaning."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"tags": ["architecture"], "sort": "newest", "limit": 10}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Filter by tags (all must match)"
                },
                "category": {
                    "type": "string",
                    "enum": ["episode", "concept", "policy", "identity"],
                    "description": "Filter by category"
                },
                "importance_min": {
                    "type": ["number", "string"],
                    "minimum": 0.0,
                    "maximum": 1.0,
                    "description": "Minimum importance threshold"
                },
                "pinned": {
                    "type": "boolean",
                    "description": "Filter to only pinned (true) or only unpinned (false) memories"
                },
                "limit": {
                    "type": ["integer", "string"],
                    "description": "Max results (default 20, max 50)"
                },
                "sort": {
                    "type": "string",
                    "enum": ["newest", "oldest", "importance"],
                    "description": "Sort order. Default: newest"
                },
                "spaces": {
                    "type": "string",
                    "enum": ["private", "commons", "all"],
                    "description": "Which memory spaces to search. Default: private (your own memories only). Use 'all' to include shared commons memories."
                }
            },
            "required": []
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

        let memory_engine = app_state.get_memory_engine().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "Memory system not available".into(),
            retryable: false,
        })?;

        let agent_ns = crate::memory::MemoryEngine::agent_namespace(&context.agent_id);

        // Build the bridge query args, injecting the agent namespace
        let mut args = arguments.clone();
        if let Some(obj) = args.as_object_mut() {
            obj.insert("source".to_string(), json!(agent_ns));
        }

        let result = memory_engine
            .query_by_metadata(args)
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Query failed: {e}"),
                retryable: true,
            })?;

        Ok(ToolResult {
            content: result,
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
