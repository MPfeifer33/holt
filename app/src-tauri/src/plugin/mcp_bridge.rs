use std::sync::Arc;

use super::host::PluginHost;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

/// Adapter that bridges an MCP plugin tool into the built-in Tool trait.
pub struct McpToolBridge {
    plugin_id: String,
    tool_name: String,
    description: String,
    parameters_schema: serde_json::Value,
    host: Arc<PluginHost>,
}

impl McpToolBridge {
    pub fn new(
        plugin_id: String,
        tool_name: String,
        description: String,
        parameters_schema: serde_json::Value,
        host: Arc<PluginHost>,
    ) -> Self {
        Self {
            plugin_id,
            tool_name,
            description,
            parameters_schema,
            host,
        }
    }
}

#[async_trait::async_trait]
impl Tool for McpToolBridge {
    fn name(&self) -> &str {
        &self.tool_name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn parameters_schema(&self) -> serde_json::Value {
        self.parameters_schema.clone()
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        match self
            .host
            .handle_tool_call(&self.plugin_id, &self.tool_name, arguments)
            .await
        {
            Ok(result) => {
                // MCP tools/call returns { content: [{ type, text }] }
                let text = result
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_else(|| serde_json::to_string_pretty(&result).unwrap_or_default());

                Ok(ToolResult {
                    content: serde_json::Value::String(text),
                    truncated: false,
                    trace_id: None,
                    image_content: None,
                })
            }
            Err(e) => Err(ToolError {
                code: ToolErrorCode::InternalError,
                message: format!(
                    "MCP plugin '{}' tool '{}' error: {}",
                    self.plugin_id, self.tool_name, e
                ),
                retryable: true,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_mcp_content_extraction() {
        let result = serde_json::json!({
            "content": [
                {"type": "text", "text": "Search found 3 results"},
                {"type": "text", "text": "Result 1: foo"}
            ]
        });
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        assert_eq!(text, "Search found 3 results\nResult 1: foo");
    }

    #[test]
    fn test_mcp_content_fallback_no_content_array() {
        let result = serde_json::json!({"status": "ok"});
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| serde_json::to_string_pretty(&result).unwrap_or_default());
        assert!(text.contains("\"status\": \"ok\""));
    }
}
