use serde_json::json;

use crate::tools::sandbox::validate_path_with_root;
use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};

pub struct SearchFilesTool;

#[async_trait::async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &'static str {
        "search_files"
    }

    fn description(&self) -> &'static str {
        "Search file contents for text or regex patterns across the codebase. Returns matching lines with file paths. Use this to find where symbols, strings, or patterns appear in code."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"pattern": "fn main", "glob": "*.rs"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regex pattern to search for" },
                "path": { "type": "string", "description": "Directory to search (default: working directory)" },
                "glob": { "type": "string", "description": "File glob filter (e.g., '*.rs', '**/*.ts')" },
                "case_sensitive": { "type": "boolean", "description": "Case-sensitive search (default: true)" },
                "max_results": { "type": "integer", "description": "Max matching lines (default: 100)" }
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

        let glob_filter = arguments.get("glob").and_then(|v| v.as_str());
        let case_sensitive = arguments
            .get("case_sensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let max_results = arguments
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(100) as usize;

        let resolved = validate_path_with_root(
            search_path,
            &context.working_directory,
            &context.workspace_root,
        )?;

        // Build regex
        let regex = if case_sensitive {
            regex::Regex::new(pattern)
        } else {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
        }
        .map_err(|e| ToolError {
            code: ToolErrorCode::InvalidInput,
            message: format!("Invalid regex pattern: {}", e),
            retryable: false,
        })?;

        // Walk directory with .gitignore awareness
        let mut walker = ignore::WalkBuilder::new(&resolved);
        walker.hidden(true).git_ignore(true).git_global(true);

        if let Some(glob_pat) = glob_filter {
            let mut override_builder = ignore::overrides::OverrideBuilder::new(&resolved);
            override_builder.add(glob_pat).map_err(|e| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("Invalid glob pattern: {}", e),
                retryable: false,
            })?;
            let overrides = override_builder.build().map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to build glob filter: {}", e),
                retryable: false,
            })?;
            walker.overrides(overrides);
        }

        // Per-file size cap: skip files over 10MB to avoid OOM on large binaries/data files
        const MAX_SEARCH_FILE_BYTES: u64 = 10 * 1024 * 1024; // 10MB

        let mut matches = Vec::new();
        let mut total_matches = 0u32;
        let mut files_searched = 0u32;
        let mut files_skipped_size = 0u32;

        for entry in walker.build().flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            // File size guard: skip oversized files
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.len() > MAX_SEARCH_FILE_BYTES {
                    files_skipped_size += 1;
                    continue;
                }
            }

            files_searched += 1;

            // Read file, skip binary
            let content = match std::fs::read(path) {
                Ok(bytes) => {
                    // Binary check
                    let check_len = bytes.len().min(512);
                    if bytes[..check_len].contains(&0) {
                        continue;
                    }
                    String::from_utf8_lossy(&bytes).to_string()
                }
                Err(_) => continue,
            };

            let lines: Vec<&str> = content.lines().collect();
            let relative_path = path
                .strip_prefix(&resolved)
                .unwrap_or(path)
                .display()
                .to_string();

            for (line_idx, line) in lines.iter().enumerate() {
                if regex.is_match(line) {
                    total_matches += 1;

                    if matches.len() < max_results {
                        // Gather context (2 lines before/after)
                        let start = line_idx.saturating_sub(2);
                        let end = (line_idx + 3).min(lines.len());

                        let context_before: Vec<String> = lines[start..line_idx]
                            .iter()
                            .map(|s| s.to_string())
                            .collect();
                        let context_after: Vec<String> = lines[(line_idx + 1)..end]
                            .iter()
                            .map(|s| s.to_string())
                            .collect();

                        matches.push(json!({
                            "file": relative_path,
                            "line_number": line_idx + 1,
                            "line_content": line,
                            "context_before": context_before,
                            "context_after": context_after,
                        }));
                    }
                }
            }
        }

        let truncated = total_matches as usize > max_results;

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "matches": matches,
                "total_matches": total_matches,
                "files_searched": files_searched,
                "files_skipped_size": files_skipped_size,
                "truncated": truncated,
                "receipt": ToolExecutionReceipt {
                    authority_class: ToolAuthorityClass::Informational,
                    executed: true,
                    execution_status: "success".to_string(),
                    verified: false,
                    verification_status: ToolVerificationStatus::NotRequired,
                    execution_id: None,
                    tool_name: None,
                    tool_call_id: None,
                    tool_call_trace_id: None,
                    tool_result_trace_id: None,
                    summary: Some(format!(
                        "Searched {} file{} for pattern '{}' and found {} match{}",
                        files_searched,
                        if files_searched == 1 { "" } else { "s" },
                        pattern,
                        total_matches,
                        if total_matches == 1 { "" } else { "es" }
                    )),
                },
            }),
            truncated,
            trace_id: None,
            image_content: None,
        })
    }
}
