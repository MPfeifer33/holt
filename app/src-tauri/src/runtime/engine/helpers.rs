//! Utility functions and constants for the engine.

/// Preamble prepended to the system prompt when coordinator mode is active.
/// Instructs models with internal multi-agent capabilities to defer orchestration to Holt.
pub const COORDINATOR_PREAMBLE: &str = "\
You have powerful internal multi-agent capabilities. However, you are running \
inside Holt's multi-agent orchestration system. Do NOT spawn your own \
sub-agents or use internal multi-agent mode. Focus on deep reasoning, planning, \
critique, and tool calling. Let Holt handle all subagent spawning, A2A \
messaging, and War Room coordination.\n\n";

/// Check if a tool result indicates a build/test failure that should trigger sandbox mode.
/// Returns (trigger_command, failure_output) if a failure is detected.
pub fn is_build_test_failure(
    tool_name: &str,
    result_json: &serde_json::Value,
) -> Option<(String, String)> {
    match tool_name {
        "execute_tests" => {
            let exit_code = result_json
                .get("exit_code")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let failed = result_json
                .get("failed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if exit_code != 0 || failed > 0 {
                let output = result_json
                    .get("output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let command = result_json
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("test")
                    .to_string();
                Some((command, output))
            } else {
                None
            }
        }
        "check_syntax" => {
            let valid = result_json
                .get("valid")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if !valid {
                let errors = result_json
                    .get("errors")
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                Some(("check_syntax".to_string(), errors))
            } else {
                None
            }
        }
        "bash" => {
            let exit_code = result_json
                .get("exit_code")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if exit_code != 0 {
                let output = result_json
                    .get("output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Some((String::new(), output)) // command filled in by caller from tc.arguments
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn execute_tests_nonzero_exit_code_is_failure() {
        let result = is_build_test_failure(
            "execute_tests",
            &json!({
                "exit_code": 1,
                "output": "FAILED test_foo",
                "command": "cargo test"
            }),
        );
        assert!(result.is_some());
        let (cmd, output) = result.unwrap();
        assert_eq!(cmd, "cargo test");
        assert!(output.contains("FAILED"));
    }

    #[test]
    fn execute_tests_zero_exit_but_failed_count_is_failure() {
        let result = is_build_test_failure(
            "execute_tests",
            &json!({
                "exit_code": 0,
                "failed": 2,
                "output": "2 tests failed",
                "command": "pytest"
            }),
        );
        assert!(result.is_some());
    }

    #[test]
    fn execute_tests_all_pass_is_not_failure() {
        let result = is_build_test_failure(
            "execute_tests",
            &json!({
                "exit_code": 0,
                "failed": 0,
                "output": "all passed"
            }),
        );
        assert!(result.is_none());
    }

    #[test]
    fn check_syntax_invalid_is_failure() {
        let result = is_build_test_failure(
            "check_syntax",
            &json!({
                "valid": false,
                "errors": ["line 10: unexpected token"]
            }),
        );
        assert!(result.is_some());
        let (cmd, _errors) = result.unwrap();
        assert_eq!(cmd, "check_syntax");
    }

    #[test]
    fn check_syntax_valid_is_not_failure() {
        let result = is_build_test_failure(
            "check_syntax",
            &json!({
                "valid": true
            }),
        );
        assert!(result.is_none());
    }

    #[test]
    fn bash_nonzero_exit_is_failure() {
        let result = is_build_test_failure(
            "bash",
            &json!({
                "exit_code": 127,
                "output": "command not found"
            }),
        );
        assert!(result.is_some());
        let (cmd, output) = result.unwrap();
        assert!(cmd.is_empty());
        assert!(output.contains("not found"));
    }

    #[test]
    fn bash_zero_exit_is_not_failure() {
        let result = is_build_test_failure(
            "bash",
            &json!({
                "exit_code": 0,
                "output": "ok"
            }),
        );
        assert!(result.is_none());
    }

    #[test]
    fn unknown_tool_is_not_failure() {
        let result = is_build_test_failure(
            "read_file",
            &json!({
                "content": "hello world"
            }),
        );
        assert!(result.is_none());
    }

    #[test]
    fn execute_tests_missing_fields_defaults_to_pass() {
        let result = is_build_test_failure("execute_tests", &json!({}));
        assert!(result.is_none());
    }
}
