use std::sync::Arc;

use crate::config::app_config::PluginType;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

use super::host::PluginHost;
use super::mcp_bridge::McpToolBridge;

/// Adapter that bridges a plugin-provided tool into the built-in Tool trait.
///
/// When the agent calls a plugin tool, PluginToolBridge routes the call to the
/// appropriate plugin process via the PluginHost's JSON-RPC transport.
pub struct PluginToolBridge {
    plugin_id: String,
    tool_name: String,
    description: String,
    parameters_schema: serde_json::Value,
    host: Arc<PluginHost>,
}

#[async_trait::async_trait]
impl Tool for PluginToolBridge {
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
            Ok(result) => Ok(ToolResult {
                content: result,
                truncated: false,
                trace_id: None,
                image_content: None,
            }),
            Err(e) => Err(ToolError {
                code: ToolErrorCode::InternalError,
                message: format!(
                    "Plugin '{}' tool '{}' error: {}",
                    self.plugin_id, self.tool_name, e
                ),
                retryable: true,
            }),
        }
    }
}

/// Build Tool trait objects for all tools provided by running plugins.
pub async fn build_plugin_tools(host: &Arc<PluginHost>) -> Vec<Box<dyn Tool>> {
    let tool_defs = host.get_all_tool_defs_with_type().await;
    let mut tools: Vec<Box<dyn Tool>> = Vec::new();

    for (plugin_id, tool_def, plugin_type) in tool_defs {
        match plugin_type {
            PluginType::Native => {
                tools.push(Box::new(PluginToolBridge {
                    plugin_id,
                    tool_name: tool_def.name,
                    description: tool_def.description,
                    parameters_schema: tool_def.parameters_schema,
                    host: Arc::clone(host),
                }));
            }
            PluginType::Mcp => {
                tools.push(Box::new(McpToolBridge::new(
                    plugin_id,
                    tool_def.name,
                    tool_def.description,
                    tool_def.parameters_schema,
                    Arc::clone(host),
                )));
            }
        }
    }

    tools
}
