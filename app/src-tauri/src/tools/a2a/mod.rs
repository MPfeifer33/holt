pub mod drain_inbox;
pub mod list_agents;
pub mod read_workspace;
pub mod request_collaboration;
pub mod send_message;

use crate::tools::types::{ToolContext, ToolError, ToolErrorCode};

pub fn agent_a2a_enabled(agent_id: &str) -> bool {
    let config_path = crate::config::app_config::agent_dir(agent_id).join("config.toml");
    if !config_path.exists() {
        return crate::config::agent_config::AgentToolsBlock::default().a2a_messaging;
    }

    crate::config::agent_config::AgentConfigFile::load(&config_path)
        .map(|cfg| cfg.tools.a2a_messaging)
        .unwrap_or_else(|_| crate::config::agent_config::AgentToolsBlock::default().a2a_messaging)
}

pub fn ensure_agent_a2a_enabled(context: &ToolContext) -> Result<(), ToolError> {
    if agent_a2a_enabled(&context.agent_id) {
        return Ok(());
    }

    Err(ToolError {
        code: ToolErrorCode::A2ANotEnabled,
        message: "A2A messaging is disabled for this agent in its tool configuration.".to_string(),
        retryable: false,
    })
}
