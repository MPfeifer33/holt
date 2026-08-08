use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::config::agent_config::AgentToolsBlock;
use crate::plugin::host::PluginHost;
use crate::runtime::connection::AgentProtocol;

use super::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolResult,
    ToolSuccessBehavior,
};

/// Core tools for engine agents — always sent as native function definitions.
/// Extended tools are registered but not part of the core definition set.
const ENGINE_CORE_TOOL_NAMES: &[&str] = &[
    // Filesystem (6)
    "edit_file",
    "read_file",
    "write_file",
    "list_directory",
    "search_files",
    "find_files",
    // Execution (1)
    "bash",
    // Memory (3)
    "remember",
    "recall",
    "memory_filter",
    // Orientation (4)
    "get_agent_info",
    "list_agents",
    "context_status",
    "list_available_tools",
    // Communication (3)
    "send_message_to_agent",
    "alert_user",
    "ask_hil",
];

/// Core tools for SDK/Codex agents — filesystem and bash come from the host SDK.
const SDK_CORE_TOOL_NAMES: &[&str] = &[
    // Memory (3)
    "remember",
    "recall",
    "memory_filter",
    // Orientation (4)
    "get_agent_info",
    "list_agents",
    "context_status",
    "list_available_tools",
    // Communication (3)
    "send_message_to_agent",
    "alert_user",
    "ask_hil",
];

/// Registry of tools available to a specific agent.
///
/// Built per-invocation from agent config flags. Maps tool names to implementations
/// and provides OpenAI-compatible function schemas for model requests.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    core_tools: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct ToolInventoryEntry {
    pub name: String,
    pub description: String,
    pub authority_class: ToolAuthorityClass,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            core_tools: HashSet::new(),
        }
    }

    /// Return the core tool names for a given agent protocol.
    pub fn core_tool_names_for_protocol(protocol: &AgentProtocol) -> &'static [&'static str] {
        match protocol {
            AgentProtocol::AgentSdk | AgentProtocol::Codex => SDK_CORE_TOOL_NAMES,
            _ => ENGINE_CORE_TOOL_NAMES,
        }
    }

    /// Set the core tool names for this registry based on protocol.
    fn set_core_tools(&mut self, protocol: &AgentProtocol) {
        let names = Self::core_tool_names_for_protocol(protocol);
        self.core_tools = names.iter().map(|s| s.to_string()).collect();
    }

    /// Register a single tool.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Build a registry with tools enabled per agent config.
    /// If a plugin_host is provided, plugin-provided tools are also registered.
    ///
    /// `sandbox_level` controls tool availability for restricted agents:
    /// - `Some("restricted")`: No shell, no write tools — read-only + analysis only
    /// - `Some("sandboxed")` / `Some("unrestricted")` / `None`: Full tool set
    pub async fn build_for_agent(
        tools_config: &AgentToolsBlock,
        plugin_host: Option<&Arc<PluginHost>>,
        coordinator_mode: bool,
        sandbox_level: Option<&str>,
    ) -> Self {
        let mut registry = Self::new();
        let is_restricted = sandbox_level == Some("restricted");

        // Filesystem tools (default: enabled)
        if tools_config.filesystem {
            use super::filesystem;
            // Read-only tools always available
            registry.register(Box::new(filesystem::read_file::ReadFileTool));
            registry.register(Box::new(filesystem::list_directory::ListDirectoryTool));
            registry.register(Box::new(filesystem::search_files::SearchFilesTool));
            registry.register(Box::new(filesystem::find_files::FindFilesTool));

            // Write tools only for non-restricted agents
            if !is_restricted {
                registry.register(Box::new(filesystem::write_file::WriteFileTool));
                registry.register(Box::new(filesystem::edit_file::EditFileTool));
                registry.register(Box::new(filesystem::delete_file::DeleteFileTool));
            }
        }

        // Shell & process tools (default: enabled)
        // Restricted agents get NO shell access.
        if tools_config.code_execution && !is_restricted {
            use super::shell;
            registry.register(Box::new(shell::bash::BashTool));
            registry.register(Box::new(shell::process::StartProcessTool));
            registry.register(Box::new(shell::process::ReadProcessTool));
            registry.register(Box::new(shell::process::WriteProcessTool));
            registry.register(Box::new(shell::process::KillProcessTool));
            registry.register(Box::new(shell::process::ListProcessesTool));

            use super::verification;
            registry.register(Box::new(verification::execute_tests::ExecuteTestsTool));
            registry.register(Box::new(verification::check_syntax::CheckSyntaxTool));
            registry.register(Box::new(verification::diff_file::DiffFileTool));
        } else if tools_config.code_execution && is_restricted {
            // Restricted agents still get read-only verification tools
            use super::verification;
            registry.register(Box::new(verification::check_syntax::CheckSyntaxTool));
            registry.register(Box::new(verification::diff_file::DiffFileTool));

            // Read-only process tools
            use super::shell;
            registry.register(Box::new(shell::process::ReadProcessTool));
            registry.register(Box::new(shell::process::ListProcessesTool));
        }

        // Web tools (default: disabled)
        if tools_config.web_access {
            use super::web;
            registry.register(Box::new(web::fetch_url::FetchUrlTool));
            if !is_restricted {
                registry.register(Box::new(web::web_search::WebSearchTool));
                registry.register(Box::new(web::screenshot_url::ScreenshotUrlTool));
            }
        }

        // Subagent tools (default: enabled)
        {
            use super::subagent;
            registry.register(Box::new(subagent::spawn_subagent::SpawnSubagentTool));
            registry.register(Box::new(subagent::check_subagent::CheckSubagentTool));
            registry.register(Box::new(subagent::cancel_subagent::CancelSubagentTool));
            registry.register(Box::new(subagent::list_subagents::ListSubagentsTool));
        }

        // A2A discovery is always available. Messaging/workspace tools honor
        // the per-agent A2A capability toggle.
        {
            use super::a2a;
            registry.register(Box::new(a2a::list_agents::ListAgentsTool));
            if tools_config.a2a_messaging {
                registry.register(Box::new(a2a::send_message::SendMessageToAgentTool));
                registry.register(Box::new(a2a::drain_inbox::DrainInboxTool));
                registry.register(Box::new(a2a::read_workspace::ReadAgentWorkspaceTool));
                registry.register(Box::new(
                    a2a::request_collaboration::RequestCollaborationTool,
                ));
            }
        }

        // Utility tools (always enabled)
        {
            use super::utility;
            registry.register(Box::new(utility::read_full_output::ReadFullOutputTool));
            registry.register(Box::new(utility::ask_hil::AskHilTool));
            registry.register(Box::new(utility::get_agent_info::GetAgentInfoTool));
            registry.register(Box::new(
                utility::list_available_tools::ListAvailableToolsTool,
            ));
            registry.register(Box::new(utility::context_status::ContextStatusTool));
            registry.register(Box::new(utility::alert_user::AlertUserTool));
        }

        // Memory tools (always enabled)
        registry.register_memory_tools();

        // Draft tools (always enabled)
        registry.register_draft_tools();

        // File watch tools
        registry.register_watch_tools();

        // Schedule/cron tools
        registry.register_schedule_tools();

        // Appearance tool (always available)
        registry.register(Box::new(super::appearance::CustomizeAppearanceTool));

        // Skill tools (read_skill for reference-mode skill loading)
        registry.register(Box::new(super::skills::read_skill::ReadSkillTool));

        // Plugin-provided tools
        if let Some(host) = plugin_host {
            let plugin_tools = crate::plugin::bridge::build_plugin_tools(host).await;
            for tool in plugin_tools {
                registry.register(tool);
            }
        }

        // Coordinator mode: remove spawn_subagent so the model cannot spawn
        // its own sub-agents (Holt handles orchestration externally).
        if coordinator_mode {
            registry.tools.remove("spawn_subagent");
        }

        registry.set_core_tools(&AgentProtocol::ChatCompletions);
        registry
    }

    /// Build a registry for subagents — same as `build_for_agent` but WITHOUT
    /// the `spawn_subagent` tool, enforcing depth=1 (subagents cannot spawn sub-subagents).
    pub async fn build_for_subagent(
        tools_config: &AgentToolsBlock,
        plugin_host: Option<&Arc<PluginHost>>,
        sandbox_level: Option<&str>,
    ) -> Self {
        let mut registry =
            Self::build_for_agent(tools_config, plugin_host, false, sandbox_level).await;
        registry.tools.remove("spawn_subagent");
        registry
    }

    /// Build a registry for session memory extraction forks — edit_file only,
    /// restricted to the specified session memory file path.
    pub fn build_for_session_memory(allowed_path: std::path::PathBuf) -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(
            super::filesystem::session_edit_file::SessionEditFileTool::new(allowed_path),
        ));
        registry
    }

    /// Build a registry for Agent SDK agents — intelligence tools only.
    ///
    /// SDK agents get native file/code/search tools from Claude Code itself.
    /// Holt only provides lane-native capabilities here: memory,
    /// collaboration, introspection, persona management, drafts, watches,
    /// appearance, and subagent orchestration.
    pub async fn build_for_sdk_agent_with_tools(
        tools_config: &AgentToolsBlock,
        plugin_host: Option<&Arc<PluginHost>>,
    ) -> Self {
        let mut registry = Self::new();

        // Memory tools
        registry.register_memory_tools();

        // A2A discovery is always available. Messaging/workspace tools honor
        // the per-agent A2A capability toggle.
        {
            use super::a2a;
            registry.register(Box::new(a2a::list_agents::ListAgentsTool));
            if tools_config.a2a_messaging {
                registry.register(Box::new(a2a::send_message::SendMessageToAgentTool));
                registry.register(Box::new(a2a::drain_inbox::DrainInboxTool));
                registry.register(Box::new(a2a::read_workspace::ReadAgentWorkspaceTool));
                registry.register(Box::new(
                    a2a::request_collaboration::RequestCollaborationTool,
                ));
            }
        }

        // Subagent tools
        {
            use super::subagent;
            registry.register(Box::new(subagent::spawn_subagent::SpawnSubagentTool));
        }

        // Utility tools (subset: alert, ask_hil, agent info, context status, persona)
        {
            use super::utility;
            registry.register(Box::new(utility::alert_user::AlertUserTool));
            registry.register(Box::new(utility::ask_hil::AskHilTool));
            registry.register(Box::new(utility::get_agent_info::GetAgentInfoTool));
            registry.register(Box::new(
                utility::list_available_tools::ListAvailableToolsTool,
            ));
            registry.register(Box::new(utility::context_status::ContextStatusTool));
            registry.register(Box::new(
                utility::set_persona_anchors::SetPersonaAnchorsTool,
            ));
            registry.register(Box::new(utility::update_persona::UpdatePersonaTool));
        }

        // Draft tools
        registry.register_draft_tools();

        // File watch tools — Holt-native capability, no overlap with Claude Code
        registry.register_watch_tools();

        // Schedule/cron tools — Holt-native scheduling
        registry.register_schedule_tools();

        // Appearance tool — agent self-expression, available to all agents
        registry.register(Box::new(super::appearance::CustomizeAppearanceTool));

        // Skill tools (read_skill for reference-mode skill loading)
        registry.register(Box::new(super::skills::read_skill::ReadSkillTool));

        // Plugin-provided tools (plugins may add intelligence tools)
        if let Some(host) = plugin_host {
            let plugin_tools = crate::plugin::bridge::build_plugin_tools(host).await;
            for tool in plugin_tools {
                registry.register(tool);
            }
        }

        registry.set_core_tools(&AgentProtocol::AgentSdk);
        registry
    }

    /// Build a registry for Agent SDK agents using default tool flags.
    pub async fn build_for_sdk_agent(plugin_host: Option<&Arc<PluginHost>>) -> Self {
        Self::build_for_sdk_agent_with_tools(&AgentToolsBlock::default(), plugin_host).await
    }

    /// Build a registry for Codex agents — Holt-native tools only.
    ///
    /// Codex already owns filesystem, shell, and code-editing capabilities in its
    /// own lane. Holt layers on its non-overlapping collaboration, memory,
    /// orchestration, persona, drafts, watches, and appearance tools here.
    pub async fn build_for_codex_agent_with_tools(
        tools_config: &AgentToolsBlock,
        plugin_host: Option<&Arc<PluginHost>>,
    ) -> Self {
        let mut registry = Self::new();

        // Memory tools
        registry.register_memory_tools();

        // A2A discovery is always available. Messaging/workspace tools honor
        // the per-agent A2A capability toggle.
        {
            use super::a2a;
            registry.register(Box::new(a2a::list_agents::ListAgentsTool));
            if tools_config.a2a_messaging {
                registry.register(Box::new(a2a::send_message::SendMessageToAgentTool));
                registry.register(Box::new(a2a::drain_inbox::DrainInboxTool));
                registry.register(Box::new(a2a::read_workspace::ReadAgentWorkspaceTool));
                registry.register(Box::new(
                    a2a::request_collaboration::RequestCollaborationTool,
                ));
            }
        }

        // Holt orchestration tools
        {
            use super::subagent;
            registry.register(Box::new(subagent::spawn_subagent::SpawnSubagentTool));
            registry.register(Box::new(subagent::check_subagent::CheckSubagentTool));
            registry.register(Box::new(subagent::cancel_subagent::CancelSubagentTool));
            registry.register(Box::new(subagent::list_subagents::ListSubagentsTool));
        }

        // Utility tools relevant to a host-managed coding lane.
        {
            use super::utility;
            registry.register(Box::new(utility::ask_hil::AskHilTool));
            registry.register(Box::new(utility::alert_user::AlertUserTool));
            registry.register(Box::new(utility::get_agent_info::GetAgentInfoTool));
            registry.register(Box::new(
                utility::list_available_tools::ListAvailableToolsTool,
            ));
            registry.register(Box::new(utility::context_status::ContextStatusTool));
            registry.register(Box::new(
                utility::set_persona_anchors::SetPersonaAnchorsTool,
            ));
            registry.register(Box::new(utility::update_persona::UpdatePersonaTool));
        }

        // Draft tools
        registry.register_draft_tools();

        // File watch tools
        registry.register_watch_tools();

        // Schedule/cron tools
        registry.register_schedule_tools();

        // Appearance tool
        registry.register(Box::new(super::appearance::CustomizeAppearanceTool));

        // Skill tools (read_skill for reference-mode skill loading)
        registry.register(Box::new(super::skills::read_skill::ReadSkillTool));

        // Plugin-provided tools
        if let Some(host) = plugin_host {
            let plugin_tools = crate::plugin::bridge::build_plugin_tools(host).await;
            for tool in plugin_tools {
                registry.register(tool);
            }
        }

        registry.set_core_tools(&AgentProtocol::Codex);
        registry
    }

    /// Build a registry for Codex agents using default tool flags.
    pub async fn build_for_codex_agent(plugin_host: Option<&Arc<PluginHost>>) -> Self {
        Self::build_for_codex_agent_with_tools(&AgentToolsBlock::default(), plugin_host).await
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Retain only the named tools in this registry.
    pub fn retain_only<'a, I>(&mut self, names: I)
    where
        I: IntoIterator<Item = &'a str>,
    {
        let keep: std::collections::HashSet<&str> = names.into_iter().collect();
        self.tools.retain(|name, _| keep.contains(name.as_str()));
    }

    /// Get a tool's description by name.
    pub fn get_description(&self, name: &str) -> Option<String> {
        self.tools.get(name).map(|t| t.description().to_string())
    }

    /// Get a tool's engine-level success behavior by name.
    pub fn get_success_behavior(&self, name: &str) -> Option<ToolSuccessBehavior> {
        self.tools.get(name).map(|t| t.success_behavior())
    }

    /// Get a tool's authority class by name.
    pub fn get_authority_class(&self, name: &str) -> Option<ToolAuthorityClass> {
        self.tools.get(name).map(|t| t.authority_class())
    }

    /// Register the full set of memory tools.
    pub(crate) fn register_memory_tools(&mut self) {
        use super::memory;
        self.register(Box::new(memory::remember::RememberTool));
        self.register(Box::new(memory::recall::RecallTool));
        self.register(Box::new(memory::query::MemoryQueryTool));
        self.register(Box::new(memory::pin::PinTool));
        self.register(Box::new(memory::unpin::UnpinMemoryTool));
        self.register(Box::new(memory::forget::ForgetTool));
        self.register(Box::new(memory::crystallize::CrystallizeTool));
        // Cluster tools hidden — Hillock manages clustering internally via dream-lite.
        // See MEMORY_SYSTEM_PHILOSOPHY.md: "No Visible Clustering"
        self.register(Box::new(memory::cleanup::CleanupTool));
    }

    /// Register draft tools.
    fn register_draft_tools(&mut self) {
        use super::drafts;
        self.register(Box::new(drafts::write::DraftWriteTool));
        self.register(Box::new(drafts::read::DraftReadTool));
        self.register(Box::new(drafts::list::DraftListTool));
        self.register(Box::new(drafts::promote::DraftPromoteTool));
        self.register(Box::new(drafts::delete::DraftDeleteTool));
    }

    /// Register file watch tools.
    fn register_watch_tools(&mut self) {
        use super::watch;
        self.register(Box::new(watch::WatchPathTool));
        self.register(Box::new(watch::UnwatchPathTool));
        self.register(Box::new(watch::ListWatchesTool));
    }

    /// Register schedule/cron tools.
    fn register_schedule_tools(&mut self) {
        use super::schedule;
        self.register(Box::new(schedule::CronCreateTool));
        self.register(Box::new(schedule::CronListTool));
        self.register(Box::new(schedule::CronDeleteTool));
    }

    /// Return OpenAI-compatible function tool definitions for all registered tools.
    /// Used by list_available_tools for the full catalog.
    pub fn definitions(&self) -> Vec<serde_json::Value> {
        let mut entries: Vec<_> = self.tools.iter().collect();
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));
        entries
            .into_iter()
            .map(|(_, tool)| tool.as_ref())
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters_schema(),
                    }
                })
            })
            .collect()
    }

    /// Return OpenAI-compatible function tool definitions for core tools only.
    /// Used in API requests to reduce token overhead — extended tools are
    /// listed in the catalog for awareness only.
    pub fn core_definitions(&self) -> Vec<serde_json::Value> {
        let mut entries: Vec<_> = self
            .tools
            .iter()
            .filter(|(name, _)| self.core_tools.contains(name.as_str()))
            .collect();
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));
        entries
            .into_iter()
            .map(|(_, tool)| tool.as_ref())
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters_schema(),
                    }
                })
            })
            .collect()
    }

    /// Check if a tool name is in the core set.
    pub fn is_core(&self, name: &str) -> bool {
        self.core_tools.contains(name)
    }

    /// Sorted inventory of registered tools.
    pub fn inventory(&self) -> Vec<ToolInventoryEntry> {
        let mut entries: Vec<_> = self.tools.iter().collect();
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));
        entries
            .into_iter()
            .map(|(name, tool)| ToolInventoryEntry {
                name: name.clone(),
                description: tool.description().to_string(),
                authority_class: tool.authority_class(),
            })
            .collect()
    }

    /// Human-facing awareness block for system prompts and docs.
    pub fn awareness_block(&self, intro: &str) -> String {
        let lines: Vec<String> = self
            .inventory()
            .into_iter()
            .map(|tool| format!("- {}: {}", tool.name, tool.description))
            .collect();
        if lines.is_empty() {
            String::new()
        } else {
            format!("{intro}\n{}", lines.join("\n"))
        }
    }

    /// Execute a tool by name with the given arguments and context.
    pub async fn execute(
        &self,
        name: &str,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let tool = self.tools.get(name).ok_or_else(|| ToolError {
            code: ToolErrorCode::InvalidInput,
            message: format!("Unknown tool: '{}'", name),
            retryable: false,
        })?;

        tool.execute(arguments, context).await
    }
}

#[cfg(test)]
mod tests {
    use super::ToolRegistry;

    #[test]
    fn definitions_export_updated_tool_descriptions() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(crate::tools::filesystem::read_file::ReadFileTool));
        registry.register(Box::new(crate::tools::utility::alert_user::AlertUserTool));

        let defs = registry.definitions();

        let read_file = defs.iter().find(|def| {
            def["function"]["name"]
                .as_str()
                .map(|name| name == "read_file")
                .unwrap_or(false)
        });
        let alert_user = defs.iter().find(|def| {
            def["function"]["name"]
                .as_str()
                .map(|name| name == "alert_user")
                .unwrap_or(false)
        });

        let read_desc = read_file
            .and_then(|def| def["function"]["description"].as_str())
            .unwrap_or_default();
        let alert_desc = alert_user
            .and_then(|def| def["function"]["description"].as_str())
            .unwrap_or_default();

        assert!(read_desc.contains("inspect code, config, or documentation"));
        assert!(alert_desc.contains("notification"));
    }

    #[tokio::test]
    async fn sdk_registry_curates_out_redundant_coding_tools() {
        let mut tools = crate::config::agent_config::AgentToolsBlock::default();
        tools.a2a_messaging = true;
        let registry = ToolRegistry::build_for_sdk_agent_with_tools(&tools, None).await;

        assert!(registry.get("read_file").is_none());
        assert!(registry.get("write_file").is_none());
        assert!(registry.get("edit_file").is_none());
        assert!(registry.get("search_files").is_none());
        assert!(registry.get("bash").is_none());

        assert!(registry.get("remember").is_some());
        assert!(registry.get("recall").is_some());
        assert!(registry.get("list_agents").is_some());
        assert!(registry.get("list_available_tools").is_some());
        assert!(registry.get("send_message_to_agent").is_some());
        assert!(registry.get("drain_a2a_inbox").is_some());
        assert!(registry.get("read_agent_workspace").is_some());
        assert!(registry.get("request_collaboration").is_some());
        assert!(registry.get("set_persona_anchors").is_some());
        assert!(registry.get("update_persona").is_some());
        assert!(registry.get("draft_write").is_some());
        assert!(registry.get("watch_path").is_some());
    }

    #[tokio::test]
    async fn codex_registry_curates_out_redundant_coding_tools() {
        let mut tools = crate::config::agent_config::AgentToolsBlock::default();
        tools.a2a_messaging = true;
        let registry = ToolRegistry::build_for_codex_agent_with_tools(&tools, None).await;

        assert!(registry.get("read_file").is_none());
        assert!(registry.get("write_file").is_none());
        assert!(registry.get("edit_file").is_none());
        assert!(registry.get("search_files").is_none());
        assert!(registry.get("bash").is_none());

        assert!(registry.get("remember").is_some());
        assert!(registry.get("recall").is_some());
        assert!(registry.get("list_agents").is_some());
        assert!(registry.get("list_available_tools").is_some());
        assert!(registry.get("send_message_to_agent").is_some());
        assert!(registry.get("drain_a2a_inbox").is_some());
        assert!(registry.get("read_agent_workspace").is_some());
        assert!(registry.get("request_collaboration").is_some());
        assert!(registry.get("set_persona_anchors").is_some());
        assert!(registry.get("update_persona").is_some());
        assert!(registry.get("draft_write").is_some());
        assert!(registry.get("watch_path").is_some());
        assert!(registry.get("spawn_subagent").is_some());
        assert!(registry.get("check_subagent").is_some());
        assert!(registry.get("cancel_subagent").is_some());
        assert!(registry.get("list_subagents").is_some());
    }

    #[tokio::test]
    async fn agent_registry_hides_a2a_tools_when_disabled() {
        let tools = crate::config::agent_config::AgentToolsBlock::default();
        let registry = ToolRegistry::build_for_agent(&tools, None, false, None).await;

        assert!(registry.get("list_agents").is_some());
        assert!(registry.get("send_message_to_agent").is_none());
        assert!(registry.get("drain_a2a_inbox").is_none());
        assert!(registry.get("read_agent_workspace").is_none());
        assert!(registry.get("request_collaboration").is_none());
    }

    #[tokio::test]
    async fn restricted_agent_has_no_bash() {
        let tools = crate::config::agent_config::AgentToolsBlock::default();
        let registry = ToolRegistry::build_for_agent(&tools, None, false, Some("restricted")).await;

        assert!(
            registry.get("bash").is_none(),
            "restricted should not have bash"
        );
        assert!(
            registry.get("start_process").is_none(),
            "restricted should not have start_process"
        );
        assert!(
            registry.get("write_process").is_none(),
            "restricted should not have write_process"
        );
        assert!(
            registry.get("kill_process").is_none(),
            "restricted should not have kill_process"
        );
        assert!(
            registry.get("execute_tests").is_none(),
            "restricted should not have execute_tests"
        );
    }

    #[tokio::test]
    async fn restricted_agent_has_no_write_tools() {
        let tools = crate::config::agent_config::AgentToolsBlock::default();
        let registry = ToolRegistry::build_for_agent(&tools, None, false, Some("restricted")).await;

        assert!(registry.get("write_file").is_none());
        assert!(registry.get("edit_file").is_none());
        assert!(registry.get("delete_file").is_none());
    }

    #[tokio::test]
    async fn restricted_agent_has_read_tools() {
        let tools = crate::config::agent_config::AgentToolsBlock::default();
        let registry = ToolRegistry::build_for_agent(&tools, None, false, Some("restricted")).await;

        assert!(registry.get("read_file").is_some());
        assert!(registry.get("search_files").is_some());
        assert!(registry.get("list_directory").is_some());
        assert!(registry.get("find_files").is_some());
        // Read-only verification tools
        assert!(registry.get("check_syntax").is_some());
        assert!(registry.get("diff_file").is_some());
        // Read-only process tools
        assert!(registry.get("read_process").is_some());
        assert!(registry.get("list_processes").is_some());
    }

    #[tokio::test]
    async fn sandboxed_agent_has_all_tools() {
        let tools = crate::config::agent_config::AgentToolsBlock::default();
        let registry = ToolRegistry::build_for_agent(&tools, None, false, Some("sandboxed")).await;

        assert!(registry.get("bash").is_some());
        assert!(registry.get("write_file").is_some());
        assert!(registry.get("edit_file").is_some());
        assert!(registry.get("read_file").is_some());
        assert!(registry.get("start_process").is_some());
    }

    #[tokio::test]
    async fn unrestricted_agent_has_all_tools() {
        let tools = crate::config::agent_config::AgentToolsBlock::default();
        let registry =
            ToolRegistry::build_for_agent(&tools, None, false, Some("unrestricted")).await;

        assert!(registry.get("bash").is_some());
        assert!(registry.get("write_file").is_some());
        assert!(registry.get("edit_file").is_some());
    }

    #[tokio::test]
    async fn none_sandbox_level_gives_all_tools() {
        let tools = crate::config::agent_config::AgentToolsBlock::default();
        let registry = ToolRegistry::build_for_agent(&tools, None, false, None).await;

        assert!(registry.get("bash").is_some());
        assert!(registry.get("write_file").is_some());
    }

    #[test]
    fn inventory_is_sorted_and_contains_descriptions() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(crate::tools::utility::alert_user::AlertUserTool));
        registry.register(Box::new(
            crate::tools::utility::get_agent_info::GetAgentInfoTool,
        ));

        let inventory = registry.inventory();
        assert_eq!(inventory.len(), 2);
        assert_eq!(inventory[0].name, "alert_user");
        assert_eq!(inventory[1].name, "get_agent_info");
        assert!(inventory[0].description.contains("notification"));
    }

    #[tokio::test]
    async fn core_definitions_returns_only_core_tools() {
        let tools = crate::config::agent_config::AgentToolsBlock::default();
        let registry = ToolRegistry::build_for_agent(&tools, None, false, None).await;

        let all_defs = registry.definitions();
        let core_defs = registry.core_definitions();

        // Core should be a strict subset of all
        assert!(core_defs.len() < all_defs.len());

        // Core should contain exactly the engine core tools that are registered
        let core_names: Vec<String> = core_defs
            .iter()
            .filter_map(|d| d["function"]["name"].as_str().map(|s| s.to_string()))
            .collect();

        // Verify key core tools are present
        assert!(core_names.contains(&"read_file".to_string()));
        assert!(core_names.contains(&"bash".to_string()));
        assert!(core_names.contains(&"remember".to_string()));
        assert!(core_names.contains(&"ask_hil".to_string()));
        assert!(core_names.contains(&"alert_user".to_string()));

        // Verify extended tools are NOT in core
        assert!(!core_names.contains(&"pin_memory".to_string()));
        assert!(!core_names.contains(&"draft_write".to_string()));
        assert!(!core_names.contains(&"watch_path".to_string()));
        assert!(!core_names.contains(&"customize_appearance".to_string()));
    }

    #[tokio::test]
    async fn sdk_core_definitions_excludes_filesystem() {
        let mut tools = crate::config::agent_config::AgentToolsBlock::default();
        tools.a2a_messaging = true;
        let registry = ToolRegistry::build_for_sdk_agent_with_tools(&tools, None).await;

        let core_defs = registry.core_definitions();
        let core_names: Vec<String> = core_defs
            .iter()
            .filter_map(|d| d["function"]["name"].as_str().map(|s| s.to_string()))
            .collect();

        // SDK core should NOT have filesystem tools
        assert!(!core_names.contains(&"read_file".to_string()));
        assert!(!core_names.contains(&"bash".to_string()));
        assert!(!core_names.contains(&"edit_file".to_string()));

        // SDK core SHOULD have memory, orientation, communication
        assert!(core_names.contains(&"remember".to_string()));
        assert!(core_names.contains(&"recall".to_string()));
        assert!(core_names.contains(&"ask_hil".to_string()));
        assert!(core_names.contains(&"send_message_to_agent".to_string()));
    }

    #[tokio::test]
    async fn is_core_distinguishes_core_from_extended() {
        let tools = crate::config::agent_config::AgentToolsBlock::default();
        let registry = ToolRegistry::build_for_agent(&tools, None, false, None).await;

        assert!(registry.is_core("bash"));
        assert!(registry.is_core("remember"));

        assert!(!registry.is_core("pin_memory"));
        assert!(!registry.is_core("draft_write"));
        assert!(!registry.is_core("customize_appearance"));
    }
}
