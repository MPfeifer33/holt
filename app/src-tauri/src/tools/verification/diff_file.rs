use serde_json::json;

use crate::tools::sandbox::validate_path_with_root;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct DiffFileTool;

#[async_trait::async_trait]
impl Tool for DiffFileTool {
    fn name(&self) -> &'static str {
        "diff_file"
    }

    fn description(&self) -> &'static str {
        "Show the diff between a file's current state and git HEAD, staged changes, or another file."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"path": "src/main.rs", "against": "head"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to diff" },
                "against": { "type": "string", "description": "'head' (default), 'staged', or a file path for file-vs-file diff" }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: path".to_string(),
                retryable: false,
            })?;

        let against = arguments
            .get("against")
            .and_then(|v| v.as_str())
            .unwrap_or("head");

        let resolved = validate_path_with_root(
            path_str,
            &context.working_directory,
            &context.workspace_root,
        )?;

        if !resolved.exists() {
            return Err(ToolError {
                code: ToolErrorCode::FileNotFound,
                message: format!("File not found: {}", path_str),
                retryable: false,
            });
        }

        let diff_command = match against {
            "head" | "HEAD" => {
                format!("git diff HEAD -- '{}' 2>&1", resolved.display())
            }
            "staged" => {
                format!("git diff --cached -- '{}' 2>&1", resolved.display())
            }
            other_path => {
                let other_resolved = validate_path_with_root(
                    other_path,
                    &context.working_directory,
                    &context.workspace_root,
                )?;
                format!(
                    "diff -u '{}' '{}' 2>&1",
                    resolved.display(),
                    other_resolved.display()
                )
            }
        };

        let bash_args = json!({
            "command": diff_command,
            "timeout_seconds": 15,
        });

        let bash_tool = crate::tools::shell::bash::BashTool;
        let result = bash_tool.execute(bash_args, context).await?;

        let output = result
            .content
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let has_changes = !output.trim().is_empty();

        let (additions, deletions) = count_diff_lines(output);

        Ok(ToolResult {
            content: json!({
                "diff": output,
                "has_changes": has_changes,
                "additions": additions,
                "deletions": deletions,
            }),
            truncated: result.truncated,
            trace_id: None,
            image_content: None,
        })
    }
}

fn count_diff_lines(diff: &str) -> (u32, u32) {
    let mut additions = 0u32;
    let mut deletions = 0u32;

    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
        }
    }

    (additions, deletions)
}
