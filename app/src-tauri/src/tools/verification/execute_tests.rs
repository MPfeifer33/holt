use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct ExecuteTestsTool;

#[async_trait::async_trait]
impl Tool for ExecuteTestsTool {
    fn name(&self) -> &'static str {
        "execute_tests"
    }

    fn description(&self) -> &'static str {
        "Run the project's test suite. Auto-detects the test runner (cargo test, npm test, pytest, etc.)."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"filter": "test_core"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "test_command": { "type": "string", "description": "Override test command (default: auto-detect)" },
                "filter": { "type": "string", "description": "Run only tests matching this pattern" },
                "timeout_seconds": { "type": "integer", "description": "Timeout in seconds (default: 120)" }
            }
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let timeout = arguments
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(120);

        let test_command = if let Some(cmd) = arguments.get("test_command").and_then(|v| v.as_str())
        {
            cmd.to_string()
        } else {
            auto_detect_test_command(&context.working_directory)?
        };

        let filter = arguments.get("filter").and_then(|v| v.as_str());
        let full_command = if let Some(f) = filter {
            // Sanitize filter to prevent shell injection — only allow characters
            // valid in test names: alphanumeric, underscores, colons, dots, hyphens, slashes
            if !f
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_:.-/".contains(c))
            {
                return Err(ToolError {
                    code: ToolErrorCode::InvalidInput,
                    message: "Test filter contains invalid characters. Only alphanumeric, _, :, ., -, / are allowed.".to_string(),
                    retryable: false,
                });
            }
            format!("{} {}", test_command, f)
        } else {
            test_command
        };

        // Execute via bash tool infrastructure
        let bash_args = json!({
            "command": full_command,
            "timeout_seconds": timeout,
        });

        let bash_tool = crate::tools::shell::bash::BashTool;
        let result = bash_tool.execute(bash_args, context).await?;

        // Parse the bash result and augment with test-specific fields
        let output = result
            .content
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let exit_code = result
            .content
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1) as i32;
        let duration_ms = result
            .content
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Basic pass/fail parsing
        let (passed, failed, skipped) = parse_test_counts(output);
        let total = passed + failed + skipped;

        let failures = parse_failures(output);

        Ok(ToolResult {
            content: json!({
                "passed": passed,
                "failed": failed,
                "skipped": skipped,
                "total": total,
                "output": output,
                "exit_code": exit_code,
                "duration_ms": duration_ms,
                "failures": failures,
            }),
            truncated: result.truncated,
            trace_id: None,
            image_content: None,
        })
    }
}

fn auto_detect_test_command(working_dir: &std::path::Path) -> Result<String, ToolError> {
    if working_dir.join("Cargo.toml").exists() {
        return Ok("cargo test".to_string());
    }
    if working_dir.join("package.json").exists() {
        return Ok("npm test".to_string());
    }
    if working_dir.join("pyproject.toml").exists() || working_dir.join("pytest.ini").exists() {
        return Ok("pytest".to_string());
    }
    if working_dir.join("go.mod").exists() {
        return Ok("go test ./...".to_string());
    }

    Err(ToolError {
        code: ToolErrorCode::InvalidInput,
        message: "Could not auto-detect test runner. Please provide test_command.".to_string(),
        retryable: false,
    })
}

fn parse_test_counts(output: &str) -> (u32, u32, u32) {
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut skipped = 0u32;

    // Cargo test format: "test result: ok. X passed; Y failed; Z ignored"
    if let Some(line) = output.lines().find(|l| l.contains("test result:")) {
        for part in line.split(';') {
            let part = part.trim();
            if let Some(n) = extract_number_before(part, "passed") {
                passed = n;
            }
            if let Some(n) = extract_number_before(part, "failed") {
                failed = n;
            }
            if let Some(n) = extract_number_before(part, "ignored") {
                skipped = n;
            }
        }
    }

    // pytest format: "X passed, Y failed, Z skipped"
    if passed == 0 && failed == 0 {
        for line in output.lines().rev() {
            if line.contains("passed") || line.contains("failed") {
                if let Some(n) = extract_number_before(line, "passed") {
                    passed = n;
                }
                if let Some(n) = extract_number_before(line, "failed") {
                    failed = n;
                }
                if let Some(n) = extract_number_before(line, "skipped") {
                    skipped = n;
                }
                break;
            }
        }
    }

    (passed, failed, skipped)
}

fn extract_number_before(text: &str, keyword: &str) -> Option<u32> {
    if let Some(pos) = text.find(keyword) {
        let before = text[..pos].trim();
        before
            .rsplit(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|s| s.parse().ok())
    } else {
        None
    }
}

fn parse_failures(output: &str) -> Vec<serde_json::Value> {
    let mut failures = Vec::new();

    // Cargo test: lines starting with "---- <test_name> stdout ----" followed by failure
    let lines: Vec<&str> = output.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("---- ") && line.ends_with(" stdout ----") {
            let name = line
                .trim_start_matches("---- ")
                .trim_end_matches(" stdout ----")
                .to_string();
            let message = lines.get(i + 1).map(|s| s.to_string()).unwrap_or_default();
            failures.push(json!({
                "name": name,
                "message": message,
                "location": serde_json::Value::Null,
            }));
        }

        // Also catch "FAILED" lines from various frameworks
        if line.contains("FAILED") && line.contains("::") {
            failures.push(json!({
                "name": line.trim(),
                "message": "",
                "location": serde_json::Value::Null,
            }));
        }
    }

    failures
}
