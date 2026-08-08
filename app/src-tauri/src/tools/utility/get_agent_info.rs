use serde_json::json;

use crate::runtime::connection::AgentProtocol;
use crate::tools::shell::shell_config::ShellConfig;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolResult};

pub struct GetAgentInfoTool;

#[async_trait::async_trait]
impl Tool for GetAgentInfoTool {
    fn name(&self) -> &'static str {
        "get_agent_info"
    }

    fn description(&self) -> &'static str {
        "Get your own agent profile: name, model, role, working directory, connection status, and current configuration. Use this on cold start to orient yourself."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(
        &self,
        _arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let shell_config = ShellConfig::for_current_os();

        // Count active processes for this agent
        let active_processes = {
            let manager = context.shell_manager.read().await;
            manager
                .processes
                .values()
                .filter(|p| p.agent_id == context.agent_id)
                .count() as u32
        };

        let protocol = {
            let agents = context.agents.read().await;
            agents
                .iter()
                .find(|agent| agent.id == context.agent_id)
                .map(|agent| agent.protocol.clone())
                .unwrap_or(AgentProtocol::ChatCompletions)
        };

        let lane = match protocol {
            AgentProtocol::AgentSdk => "sdk",
            AgentProtocol::Codex => "codex",
            _ => "engine",
        };

        let active_subagents = context
            .subagent_manager
            .active_count(&context.agent_id)
            .await as u32;

        Ok(ToolResult {
            content: json!({
                "agent_id": context.agent_id,
                "working_directory": context.working_directory.display().to_string(),
                "os": ShellConfig::os_name(),
                "shell": shell_config.shell_binary.display().to_string(),
                "active_processes": active_processes,
                "active_subagents": active_subagents,
                "lane": lane,
                "tool_discovery_hint": "Use list_available_tools to inspect the Holt-native tools available in this lane right now.",
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
