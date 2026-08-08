//! `memory_cleanup` — consolidation and maintenance.
//!
//! Was `cluster_tools.rs`, holding `create_memory_cluster`, `merge_memory_clusters` and
//! `archive_memory_cluster` alongside this. Those three were removed 2026-07-26: they had
//! been commented out of the registry so no agent could call them, their engine methods
//! returned `{"status":"noop"}` or bare `Ok(())`, and Hillock has no notion of clusters at
//! all. They still appeared in every agent's TOOLS.md, and the traces show agents reached
//! for `create_memory_cluster` twice — advertising a capability that does not exist costs
//! real turns.

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};
use serde_json::json;
// ---------------------------------------------------------------------------
// memory_cleanup
// ---------------------------------------------------------------------------

pub struct CleanupTool;

#[async_trait::async_trait]
impl Tool for CleanupTool {
    fn name(&self) -> &'static str {
        "memory_cleanup"
    }

    fn description(&self) -> &'static str {
        "Run maintenance on your memory: decay aging entries and prune stale ones. Duplicates are \
         prevented at write time by a uniqueness constraint, not consolidated here. Set dry_run \
         to report exactly what a live run would do while changing nothing."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "dry_run": { "type": "boolean", "description": "Preview only, don't make changes (default: false)" }
            }
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

        let dry_run = arguments
            .get("dry_run")
            .and_then(|v| match v {
                serde_json::Value::Bool(b) => Some(*b),
                serde_json::Value::String(s) => match s.trim().to_lowercase().as_str() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or(false);
        let agent_ns = crate::memory::MemoryEngine::agent_namespace(&context.agent_id);

        // Run nightly maintenance scoped to this namespace
        let nightly_result = engine
            .run_nightly(Some(&agent_ns), dry_run)
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Nightly failed: {e}"),
                retryable: true,
            })?;

        Ok(ToolResult {
            content: json!({
                "dry_run": dry_run,
                "maintenance": nightly_result,
                "message": if dry_run {
                    "Dry run complete — nothing was changed. Counts are projections of what a live run would do."
                } else {
                    "Cleanup complete"
                }
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
