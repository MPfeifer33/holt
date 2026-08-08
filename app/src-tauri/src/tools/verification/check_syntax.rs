use serde_json::json;

use crate::tools::sandbox::validate_path_with_root;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct CheckSyntaxTool;

#[async_trait::async_trait]
impl Tool for CheckSyntaxTool {
    fn name(&self) -> &'static str {
        "check_syntax"
    }

    fn description(&self) -> &'static str {
        "Check a file for syntax errors without running it. Detects language by file extension."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"path": "src/main.rs"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to check" }
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

        let ext = resolved.extension().and_then(|e| e.to_str()).unwrap_or("");

        let (language, check_command) = match ext {
            "rs" => (
                "rust",
                // Use cargo check with message-format=short for targeted output.
                // Full project check is unavoidable for Rust (imports, macros, etc.)
                // but short format keeps output focused.
                "cargo check --message-format=short 2>&1".to_string(),
            ),
            "ts" | "tsx" => (
                "typescript",
                format!("tsc --noEmit '{}' 2>&1", resolved.display()),
            ),
            "js" | "jsx" => (
                "javascript",
                format!("node --check '{}' 2>&1", resolved.display()),
            ),
            "py" => (
                "python",
                format!("python -m py_compile '{}' 2>&1", resolved.display()),
            ),
            "go" => ("go", format!("go vet '{}' 2>&1", resolved.display())),
            other => {
                return Err(ToolError {
                    code: ToolErrorCode::InvalidInput,
                    message: format!(
                        "No syntax checker for .{} files. Use bash tool to run a checker directly.",
                        other
                    ),
                    retryable: false,
                });
            }
        };

        // Run the check command via bash
        let bash_args = json!({
            "command": check_command,
            "timeout_seconds": 30,
        });

        let bash_tool = crate::tools::shell::bash::BashTool;
        let result = bash_tool.execute(bash_args, context).await?;

        let output = result
            .content
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let exit_code = result
            .content
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        let valid = exit_code == 0;

        let errors = parse_syntax_errors(output, language);

        Ok(ToolResult {
            content: json!({
                "valid": valid,
                "errors": errors,
                "language": language,
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

fn parse_syntax_errors(output: &str, _language: &str) -> Vec<serde_json::Value> {
    let mut errors = Vec::new();

    for line in output.lines() {
        // Common pattern: "file:line:col: message" or "file(line,col): message"
        if line.contains("error")
            || line.contains("Error")
            || line.contains("warning")
            || line.contains("Warning")
        {
            let severity = if line.to_lowercase().contains("warning") {
                "warning"
            } else {
                "error"
            };

            // Try to extract line number
            let line_num = extract_line_number(line);

            errors.push(json!({
                "line": line_num,
                "column": serde_json::Value::Null,
                "message": line.trim(),
                "severity": severity,
            }));
        }
    }

    errors
}

fn extract_line_number(line: &str) -> serde_json::Value {
    // Try pattern: ":NUM:" or "(NUM,"
    for part in line.split(':') {
        if let Ok(n) = part.trim().parse::<u32>() {
            return json!(n);
        }
    }
    serde_json::Value::Null
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_line_number_colon_format() {
        // Rust-style: "src/main.rs:42:5: error[E0308]"
        let result = extract_line_number("src/main.rs:42:5: error[E0308]");
        assert_eq!(result, json!(42));
    }

    #[test]
    fn test_extract_line_number_no_number() {
        let result = extract_line_number("general error message");
        assert_eq!(result, serde_json::Value::Null);
    }

    #[test]
    fn test_parse_syntax_errors_catches_errors() {
        let output = "src/lib.rs:10:5: error[E0308]: mismatched types\nwarning: unused variable";
        let errors = parse_syntax_errors(output, "rust");
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0]["severity"], "error");
        assert_eq!(errors[1]["severity"], "warning");
    }

    #[test]
    fn test_parse_syntax_errors_empty_output() {
        let errors = parse_syntax_errors("", "rust");
        assert!(errors.is_empty());
    }

    #[test]
    fn test_parse_syntax_errors_clean_output() {
        let errors = parse_syntax_errors("Compiling app v0.1.0\nFinished dev", "rust");
        assert!(errors.is_empty());
    }
}
