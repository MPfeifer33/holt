use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolResult};

pub struct ListSubagentsTool;

#[async_trait::async_trait]
impl Tool for ListSubagentsTool {
    fn name(&self) -> &'static str {
        "list_subagents"
    }

    fn description(&self) -> &'static str {
        "List all subagents you've spawned with their status and elapsed time."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
        })
    }

    async fn execute(
        &self,
        _arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let subagents = context.subagent_manager.list(&context.agent_id).await;

        let entries: Vec<serde_json::Value> = subagents
            .iter()
            .map(|s| {
                json!({
                    "job_id": s.job_id,
                    "name": s.name,
                    "task": s.task,
                    "status": format!("{:?}", s.status),
                    "elapsed_ms": s.elapsed_ms,
                })
            })
            .collect();

        Ok(ToolResult {
            content: json!({
                "count": entries.len(),
                "subagents": entries,
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
