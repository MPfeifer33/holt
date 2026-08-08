use serde_json::json;

use crate::runtime::agent::AgentStatus;
use crate::runtime::connection::ConnectionConfig;
use crate::tools::types::{Tool, ToolContext, ToolResult};

pub struct ListAgentsTool;

#[async_trait::async_trait]
impl Tool for ListAgentsTool {
    fn name(&self) -> &'static str {
        "list_agents"
    }

    fn description(&self) -> &'static str {
        "List all other agents in the system with their name, status, model, and current task. Use this to discover who's available for collaboration or to route work to the right agent."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(
        &self,
        _arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, crate::tools::types::ToolError> {
        let agents = context.agents.read().await;

        let agent_list: Vec<serde_json::Value> = agents
            .iter()
            .filter(|a| a.id != context.agent_id)
            .map(|agent| {
                // Determine model: prefer anthropic_settings override, fall back to connection config
                let model = agent
                    .anthropic_settings
                    .as_ref()
                    .map(|s| s.model.clone())
                    .or_else(|| match &agent.connection {
                        ConnectionConfig::Api { model, .. } => Some(model.clone()),
                        ConnectionConfig::Local { model, .. } => model.clone(),
                        ConnectionConfig::OAuth { model, .. } => Some(model.clone()),
                        ConnectionConfig::McpStdio { .. } => None,
                        ConnectionConfig::AgentSdk => None,
                    });

                // First line of system_prompt as role snippet
                let role = agent
                    .system_prompt
                    .lines()
                    .next()
                    .map(|line| line.trim())
                    .filter(|line| !line.is_empty())
                    .unwrap_or("(no role set)");

                let status = match &agent.status {
                    AgentStatus::Idle => "idle".to_string(),
                    AgentStatus::Working => "working".to_string(),
                    AgentStatus::WaitingForHil => "waiting_for_hil".to_string(),
                    AgentStatus::WaitingForAgent(id) => format!("waiting_for_agent:{}", id),
                    AgentStatus::Error(msg) => format!("error: {}", msg),
                };

                // Current task: last assistant message first sentence, truncated.
                // Only populated when the agent is actively working.
                let current_task = if matches!(agent.status, AgentStatus::Working) {
                    agent
                        .conversation
                        .messages
                        .iter()
                        .rev()
                        .find(|m| {
                            m.role == crate::runtime::agent::MessageRole::Assistant
                                && !m.content.is_empty()
                        })
                        .map(|m| {
                            let first_sentence = m
                                .content
                                .split(&['.', '\n'][..])
                                .next()
                                .unwrap_or(&m.content)
                                .trim();
                            if first_sentence.len() > 100 {
                                format!(
                                    "{}...",
                                    &first_sentence[..first_sentence
                                        .char_indices()
                                        .nth(100)
                                        .map(|(i, _)| i)
                                        .unwrap_or(first_sentence.len())]
                                )
                            } else {
                                first_sentence.to_string()
                            }
                        })
                } else {
                    None
                };

                json!({
                    "id": agent.id,
                    "name": agent.name,
                    "status": status,
                    "model": model,
                    "working_directory": agent.working_directory.to_string_lossy(),
                    "role": role,
                    "current_task": current_task,
                })
            })
            .collect();

        Ok(ToolResult {
            content: json!({
                "agents": agent_list,
                "count": agent_list.len(),
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
