use serde_json::json;
use std::collections::BTreeMap;

use crate::config::agent_config::{AgentConfigFile, AgentToolsBlock};
use crate::runtime::connection::AgentProtocol;
use crate::tools::registry::ToolRegistry;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolResult};

pub struct ListAvailableToolsTool;

fn load_tools_config(agent_id: &str) -> AgentToolsBlock {
    let config_path = crate::config::app_config::agent_dir(agent_id).join("config.toml");
    if config_path.exists() {
        AgentConfigFile::load(&config_path)
            .map(|cfg| cfg.tools)
            .unwrap_or_default()
    } else {
        AgentToolsBlock::default()
    }
}

fn load_sandbox_level(agent_id: &str) -> String {
    let config_path = crate::config::app_config::agent_dir(agent_id).join("config.toml");
    if config_path.exists() {
        AgentConfigFile::load(&config_path)
            .map(|cfg| cfg.resolved_execution_sandbox().level)
            .unwrap_or_else(|_| "unrestricted".to_string())
    } else {
        "unrestricted".to_string()
    }
}

/// Map a tool name to its domain category.
fn tool_domain(name: &str) -> &'static str {
    match name {
        "read_file" | "edit_file" | "write_file" | "list_directory" | "search_files"
        | "find_files" | "delete_file" => "Filesystem",
        "bash" => "Execution",
        "remember" | "recall" | "memory_filter" | "pin_memory" | "unpin_memory" | "forget"
        | "memory_crystallize" | "memory_cleanup" => "Memory",
        "get_agent_info" | "list_agents" | "context_status" | "list_available_tools" => {
            "Orientation"
        }
        "send_message_to_agent"
        | "alert_user"
        | "ask_hil"
        | "read_agent_workspace"
        | "request_collaboration"
        | "drain_a2a_inbox" => "Communication",
        "draft_write" | "draft_read" | "draft_list" | "draft_promote" | "draft_delete" => "Drafts",
        "watch_path" | "unwatch_path" | "list_watches" => "Watches",
        "cron_create" | "cron_list" | "cron_delete" => "Schedule",
        "update_persona" | "set_persona_anchors" | "customize_appearance" => "Persona",
        "execute_tests" | "check_syntax" | "diff_file" => "Verification",
        "spawn_subagent" | "check_subagent" | "cancel_subagent" | "list_subagents" => "Subagents",
        "fetch_url" | "web_search" | "screenshot_url" => "Web",
        "read_skill" => "Skills",
        "start_process" | "read_process" | "write_process" | "kill_process" | "list_processes" => {
            "Process Management"
        }
        "read_full_output" => "Utility",
        "session_edit_file" => "Internal",
        n if n.starts_with("browser_") => "Browser",
        _ => "Other",
    }
}

/// Fixed domain ordering for deterministic output.
fn domain_order(domain: &str) -> u32 {
    match domain {
        "Filesystem" => 0,
        "Execution" => 1,
        "Memory" => 2,
        "Orientation" => 3,
        "Communication" => 4,
        "Meta" => 5,
        "Drafts" => 10,
        "Watches" => 11,
        "Schedule" => 12,
        "Persona" => 13,
        "Verification" => 14,
        "Process Management" => 15,
        "Subagents" => 16,
        "Web" => 17,
        "Browser" => 18,
        "Skills" => 19,
        "Utility" => 20,
        _ => 99,
    }
}

#[async_trait::async_trait]
impl Tool for ListAvailableToolsTool {
    fn name(&self) -> &'static str {
        "list_available_tools"
    }

    fn description(&self) -> &'static str {
        "List all tools available to you, grouped by domain, with descriptions and examples. Core tools are callable directly. Extended tools are listed for awareness and are not directly callable from this lane."
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
    ) -> Result<ToolResult, ToolError> {
        let (protocol, coordinator_mode) = {
            let agents = context.agents.read().await;
            let agent = agents
                .iter()
                .find(|agent| agent.id == context.agent_id)
                .ok_or(ToolError {
                    code: crate::tools::types::ToolErrorCode::InternalError,
                    message: format!("Agent {} not found", context.agent_id),
                    retryable: false,
                })?;
            (agent.protocol.clone(), agent.coordinator_mode)
        };

        let tools_config = load_tools_config(&context.agent_id);
        let plugin_host = context.app_state.as_ref().map(|state| &state.plugin_host);

        let registry = if matches!(protocol, AgentProtocol::AgentSdk) {
            ToolRegistry::build_for_sdk_agent_with_tools(&tools_config, plugin_host).await
        } else if matches!(protocol, AgentProtocol::Codex) {
            ToolRegistry::build_for_codex_agent_with_tools(&tools_config, plugin_host).await
        } else {
            let sandbox_level = load_sandbox_level(&context.agent_id);
            ToolRegistry::build_for_agent(
                &tools_config,
                plugin_host,
                coordinator_mode,
                Some(&sandbox_level),
            )
            .await
        };

        // Build grouped output separated into core and extended
        let mut core_domains: BTreeMap<u32, (String, Vec<serde_json::Value>)> = BTreeMap::new();
        let mut extended_domains: BTreeMap<u32, (String, Vec<serde_json::Value>)> = BTreeMap::new();
        let mut core_count = 0u32;
        let mut extended_count = 0u32;

        // Get full definitions for schema info
        let all_defs = registry.definitions();
        let def_map: std::collections::HashMap<String, &serde_json::Value> = all_defs
            .iter()
            .filter_map(|d| {
                d.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(|name| (name.to_string(), d))
            })
            .collect();

        for entry in registry.inventory() {
            let domain = tool_domain(&entry.name);
            let order = domain_order(domain);
            let is_core = registry.is_core(&entry.name);

            // Get the tool for example()
            let example = registry.get(&entry.name).and_then(|t| t.example());

            // Get parameter schema from definitions
            let parameters = def_map
                .get(&entry.name)
                .and_then(|d| d.get("function"))
                .and_then(|f| f.get("parameters"))
                .cloned();

            let mut tool_entry = json!({
                "name": entry.name,
                "description": entry.description,
                "authority_class": entry.authority_class,
            });

            if let Some(ex) = example {
                tool_entry
                    .as_object_mut()
                    .unwrap()
                    .insert("example".to_string(), ex);
            }
            if let Some(params) = parameters {
                tool_entry
                    .as_object_mut()
                    .unwrap()
                    .insert("parameters".to_string(), params);
            }

            if is_core {
                core_count += 1;
                core_domains
                    .entry(order)
                    .or_insert_with(|| (domain.to_string(), Vec::new()))
                    .1
                    .push(tool_entry);
            } else {
                extended_count += 1;
                extended_domains
                    .entry(order)
                    .or_insert_with(|| (domain.to_string(), Vec::new()))
                    .1
                    .push(tool_entry);
            }
        }

        // Convert to ordered JSON objects
        let core_grouped: serde_json::Value = core_domains
            .into_values()
            .map(|(domain, tools)| (domain, serde_json::Value::Array(tools)))
            .collect::<serde_json::Map<String, serde_json::Value>>()
            .into();

        let extended_grouped: serde_json::Value = extended_domains
            .into_values()
            .map(|(domain, tools)| (domain, serde_json::Value::Array(tools)))
            .collect::<serde_json::Map<String, serde_json::Value>>()
            .into();

        let lane = match protocol {
            AgentProtocol::AgentSdk => "sdk",
            AgentProtocol::Codex => "codex",
            _ => "engine",
        };

        Ok(ToolResult {
            content: json!({
                "agent_id": context.agent_id,
                "lane": lane,
                "core_count": core_count,
                "extended_count": extended_count,
                "note": "Core tools are callable directly. Extended tools are listed for awareness and are not directly callable from this lane. All tools below include parameter schemas and usage examples.",
                "core_tools": core_grouped,
                "extended_tools": extended_grouped,
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
