use serde_json::json;

use crate::tools::sandbox::validate_path_with_root;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct FindFilesTool;

#[async_trait::async_trait]
impl Tool for FindFilesTool {
    fn name(&self) -> &'static str {
        "find_files"
    }

    fn description(&self) -> &'static str {
        "Find files by name using glob patterns (e.g., '*.rs', 'src/**/*.test.ts'). Returns file paths sorted by modification time. Use this to locate files when you know the naming pattern."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"pattern": "**/*.test.ts"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern (e.g., '*.rs', 'src/**/*.test.ts')" },
                "path": { "type": "string", "description": "Directory to search (default: working directory)" },
                "max_results": { "type": "integer", "description": "Max results (default: 200)" }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let pattern = arguments
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: pattern".to_string(),
                retryable: false,
            })?;

        let search_path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let max_results = arguments
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(200) as usize;

        let resolved = validate_path_with_root(
            search_path,
            &context.working_directory,
            &context.workspace_root,
        )?;

        // Build walker with glob override
        let mut walker = ignore::WalkBuilder::new(&resolved);
        walker.hidden(true).git_ignore(true).git_global(true);

        let mut override_builder = ignore::overrides::OverrideBuilder::new(&resolved);
        override_builder.add(pattern).map_err(|e| ToolError {
            code: ToolErrorCode::InvalidInput,
            message: format!("Invalid glob pattern: {}", e),
            retryable: false,
        })?;
        let overrides = override_builder.build().map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to build glob: {}", e),
            retryable: false,
        })?;
        walker.overrides(overrides);

        // Collect matching files with modification times
        let mut files: Vec<(String, std::time::SystemTime)> = Vec::new();
        let mut total_found = 0u32;

        for entry in walker.build().flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            total_found += 1;
            let relative = path
                .strip_prefix(&resolved)
                .unwrap_or(path)
                .display()
                .to_string();

            let mtime = path
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);

            files.push((relative, mtime));
        }

        // Sort by modification time, newest first
        files.sort_by(|a, b| b.1.cmp(&a.1));

        let truncated = files.len() > max_results;
        files.truncate(max_results);

        let file_list: Vec<String> = files.into_iter().map(|(path, _)| path).collect();

        Ok(ToolResult {
            content: json!({
                "files": file_list,
                "total_found": total_found,
                "truncated": truncated,
            }),
            truncated,
            trace_id: None,
            image_content: None,
        })
    }
}
