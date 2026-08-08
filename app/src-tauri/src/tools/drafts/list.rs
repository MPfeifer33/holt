use serde_json::json;

use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolExecutionReceipt, ToolResult,
    ToolVerificationStatus,
};

pub struct DraftListTool;

#[async_trait::async_trait]
impl Tool for DraftListTool {
    fn name(&self) -> &'static str {
        "draft_list"
    }

    fn description(&self) -> &'static str {
        "List all your drafts with name, size, and last modified time."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "include_preview": {
                    "type": "boolean",
                    "description": "If true, include first 200 chars of each draft. Default: false"
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
        let include_preview = arguments
            .get("include_preview")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let dir = super::drafts_dir(&context.agent_id);

        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                return Ok(ToolResult {
                    content: json!({
                        "tool_result_status": "success",
                        "drafts": [],
                        "count": 0,
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
                            summary: Some("No drafts found".to_string()),
                        }
                    }),
                    truncated: false,
                    trace_id: None,
                    image_content: None,
                });
            }
        };

        let mut drafts = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let meta = std::fs::metadata(&path).ok();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta
                .and_then(|m| m.modified().ok())
                .map(|t| {
                    let dt: chrono::DateTime<chrono::Utc> = t.into();
                    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                })
                .unwrap_or_default();

            let mut draft = json!({
                "name": name,
                "size_bytes": size,
                "modified_at": modified,
            });

            if include_preview {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let preview: String = content.chars().take(200).collect();
                    draft["preview"] = json!(preview);
                }
            }

            drafts.push(draft);
        }

        // Sort by modified_at descending
        drafts.sort_by(|a, b| {
            let ta = a.get("modified_at").and_then(|v| v.as_str()).unwrap_or("");
            let tb = b.get("modified_at").and_then(|v| v.as_str()).unwrap_or("");
            tb.cmp(ta)
        });

        let count = drafts.len();

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "drafts": drafts,
                "count": count,
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
                    summary: Some(format!("Listed {} draft{}", count, if count == 1 { "" } else { "s" })),
                }
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
