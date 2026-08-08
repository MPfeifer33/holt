use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use super::error::CommandError;
use crate::config::agent_config::{
    AgentBlock, AgentConfigFile, AgentLimitsBlock, AgentToolsBlock, BuildSandboxBlock,
    ConnectionBlock, SubagentsBlock, WorkspaceBlock,
};
use crate::config::agent_sdk_config::AgentSdkBlock;
use crate::config::app_config::{agent_dir, AttentionBlock};
use crate::runtime::agent::{
    AgentColor, AgentMetadata, AgentSlot, AgentStatus, ConversationHistory,
};
use crate::runtime::connection::{AgentProtocol, ConnectionConfig, DetectedModel};
use crate::runtime::protocol::preferred_protocol;
use crate::AppState;

#[path = "agent_commands/a2a.rs"]
pub(crate) mod a2a;
#[path = "agent_commands/codex_thread.rs"]
pub(crate) mod codex_thread;
#[path = "agent_commands/context.rs"]
pub(crate) mod context;
#[path = "agent_commands/lifecycle.rs"]
pub(crate) mod lifecycle;
#[path = "agent_commands/messaging.rs"]
pub(crate) mod messaging;
#[path = "agent_commands/misc.rs"]
pub(crate) mod misc;

pub use a2a::{
    approve_a2a_collaboration, get_a2a_visibility, list_a2a_pairs, load_orbital_layout,
    rebuild_all_orbital_membership, save_orbital_layout, set_a2a_visibility,
    update_orbital_membership, A2APairEntry,
};
pub use codex_thread::{
    codex_fork_thread, codex_rollback, codex_set_approval, codex_set_effort, codex_set_goal,
    codex_shell_command, codex_switch_model,
};
pub use context::{compact_agent_context, get_context_status, trigger_extraction, ContextStatus};
pub use lifecycle::{
    clear_conversation, create_agent, get_agent_details, list_agents, preview_cognitive_profile,
    remove_agent, scan_local_models, store_api_key, update_agent, update_agent_traits,
    AgentSummary, FullAgentConfig,
};
pub use messaging::{
    broadcast_message, cancel_subagent, check_vision_support, get_conversation, interrupt_agent,
    list_subagents, respond_to_approval, respond_to_hil, run_codex_review, send_message,
    tag_message_outcome, trigger_agent_turn, ConversationMessageSummary,
};
pub use misc::{get_app_metadata, AppMetadata};

/// Clean up cross-references to a deleted agent across all subsystems.
/// Called after the agent is removed from the agents vec.
async fn cleanup_agent_references(state: &AppState, agent_id: &str) {
    // 1. Session state (dedup IDs)
    state.session_state.reset(agent_id).await;

    // 1b. Cancel and remove any in-flight engine token for the deleted agent
    if let Some(token) = state.cancellation_tokens.write().await.remove(agent_id) {
        token.cancel();
    }

    // 1c. Remove any pending blocking HIL sender for the deleted agent
    {
        let mut shell_manager = state.shell_manager.write().await;
        shell_manager.hil_senders.remove(agent_id);
    }

    // 2. A2A pairs (Manual + Auto)
    let pairs_removed = state
        .a2a_registry
        .remove_all_pairs_for_agent(agent_id)
        .await;
    if pairs_removed > 0 {
        tracing::debug!(agent_id, pairs_removed, "Cleaned up A2A pairs");
    }

    // 3. A2A pending messages
    state
        .a2a_registry
        .drain_pending_messages_for_cleanup(agent_id)
        .await;

    // 4. Subagent pending results
    state
        .subagent_manager
        .cleanup_pending_for_parent(agent_id)
        .await;

    // 5. Approval manager (sends false to dangling oneshot senders)
    state.approval_manager.cancel_all_for_agent(agent_id).await;

    // 6. Memory injection token cache (prevents stale entries accumulating)
    state.last_injection_tokens.write().await.remove(agent_id);

    // 8. Claude TUI locks (prevents DashMap entries accumulating for deleted agents)
    state.claude_tui_locks.remove(agent_id);
}

/// Write starter persona template files into a new agent's persona directory.
/// Best-effort — logs warnings on failure but doesn't block agent creation.
fn write_persona_templates(persona_dir: &std::path::Path, agent_name: &str, model: &str) {
    let soul = format!(
        "# Soul\n\n\
        You are {agent_name}.\n\n\
        <!-- Define your core personality, tone, and values here.\n\
        What kind of assistant are you? What principles guide your responses?\n\
        How do you approach problems? What's your communication style? -->\n"
    );

    let identity = format!(
        "# Identity\n\n\
        **Name:** {agent_name}\n\
        **Model:** {model}\n\n\
        <!-- How do you introduce yourself? What is your role?\n\
        What are your areas of expertise? -->\n"
    );

    let user = "# User\n\n\
        <!-- What do you know about the user you're working with?\n\
        Their role, preferences, projects, communication style.\n\
        This file helps you tailor your responses. -->\n"
        .to_string();

    let tools = "# Tools\n\n\
        <!-- Your tools will be auto-generated. This placeholder will be overwritten. -->\n"
        .to_string();

    let agents = "# Agents\n\n\
        <!-- Rules for interacting with other agents in the system.\n\
        Who are the other agents? How should you collaborate?\n\
        What are the boundaries between your responsibilities? -->\n"
        .to_string();

    let preamble = "MANDATORY: ORIENT BEFORE YOUR FIRST RESPONSE\n\n\
        On your FIRST turn in any session, you MUST orient before generating any reply to the user. \
        This is non-negotiable.\n\n\
        1. recall(\"session-summary\") -- what happened recently\n\
        2. memory_filter(tags: [\"session_memory\"], sort: \"newest\", limit: 3) -- recent auto-captured session artifacts\n\
        3. Review your persona context (already provided above in this prompt)\n\
        4. Only AFTER completing orientation may you respond to the human\n\n\
        Do NOT skip this step. Do NOT respond with a greeting first. Orient, THEN respond.\n"
        .to_string();

    let templates: &[(&str, &str)] = &[
        ("PREAMBLE.md", &preamble),
        ("SOUL.md", &soul),
        ("IDENTITY.md", &identity),
        ("USER.md", &user),
        ("TOOLS.md", &tools),
        ("AGENTS.md", &agents),
    ];

    for (filename, content) in templates {
        let path = persona_dir.join(filename);
        if !path.exists() {
            if let Err(e) = std::fs::write(&path, content) {
                tracing::warn!("Failed to write persona template {}: {e}", filename);
            }
        }
    }
}

/// Generate a TOOLS.md file listing an agent's tools.
///
/// ⚠️ These lists are **hardcoded string literals**, one block per tool-config flag, with no
/// link to `tools::registry`. Nothing cross-checks them, so they drift: on 2026-07-26 this
/// advertised `create_memory_cluster`, `merge_memory_clusters` and `archive_memory_cluster`
/// — all three commented out of the registry — while omitting `memory_filter`,
/// `unpin_memory` and `extract_context`, which were live at the time. Existing agents had been
/// carrying that list since 2026-07-06, and subagents inherit it via `SUBAGENT_ALLOWLIST`.
///
/// The lists are corrected, but the structural fix is to generate them FROM the registry so
/// they cannot drift again. Until then, adding or removing a tool means editing this too.
fn generate_tools_md(
    tools_config: &crate::config::agent_config::AgentToolsBlock,
    memory_available: bool,
) -> String {
    let mut sections = Vec::new();

    sections.push("# Tools\n".to_string());
    sections.push("_Hand-maintained per tool-config flag. NOT derived from the tool registry — if you add or remove a tool, update this too._\n".to_string());

    if tools_config.filesystem {
        sections.push("## Filesystem\n- **read_file** — Read contents of a file\n- **write_file** — Write content to a file\n- **edit_file** — Edit a file with search/replace\n- **delete_file** — Delete a file\n- **list_directory** — List contents of a directory\n- **search_files** — Search for files by name pattern\n- **find_files** — Find files matching criteria\n".to_string());
    }

    if tools_config.code_execution {
        sections.push("## Code Execution\n- **bash** — Execute shell commands\n- **start_process** — Start a background process\n- **read_process** — Read output from a process\n- **write_process** — Write input to a process\n- **kill_process** — Kill a running process\n- **list_processes** — List running processes\n- **execute_tests** — Run test commands\n- **check_syntax** — Check code syntax\n- **diff_file** — Show file differences\n".to_string());
    }

    if tools_config.web_access {
        sections.push("## Web Access\n- **fetch_url** — Fetch content from a URL\n- **web_search** — Search the web\n- **screenshot_url** — Take a screenshot of a URL\n".to_string());
    }

    if memory_available {
        sections.push("## Memory (Hillock)\n\nYour pinned memories and highest-importance memories are loaded into context at session start (Living Persona). Beyond that, retrieval is deliberate — use recall.\n\n- **remember** — Store a memory explicitly\n- **recall** — Search memories by natural-language query\n- **memory_filter** — Query by metadata (tags, category, importance) with no semantic search\n- **pin_memory** — Protect a memory from decay/pruning and pack it first into session-start context (max 5)\n- **unpin_memory** — Release a pinned slot\n- **forget** — Delete a memory (soft-delete, audited, limited to spaces you can read)\n- **memory_crystallize** — Synthesize episodes into a concept\n- **memory_cleanup** — Consolidate duplicates + maintenance (supports dry_run)\n".to_string());
    }

    sections.push("## Utility\n- **read_full_output** — Read truncated tool output in full\n- **ask_hil** — Ask the user a question\n- **get_agent_info** — Get info about this agent\n- **context_status** — Check context window usage\n- **alert_user** — Send an alert to the user\n".to_string());

    if tools_config.code_execution {
        sections.push(
            "## Agent Toolchain (14 Rust CLIs)\n\n\
            All tools are symlinked to `~/.local/bin/` and callable directly by name. Run `toolbox list` for overview, `toolbox workflow` for usage patterns.\n\
            All tools support `--repo <path>` (defaults to cwd or git root) and `--format json|text`.\n\n\
            | Tool | Purpose | Key commands |\n\
            |------|---------|-------------|\n\
            | **probe** | Preflight scanner | `probe scan`, `probe doctor`, `probe snapshot`, `probe diff` |\n\
            | **sieve** | Test impact analyzer | `sieve analyze`, `sieve analyze --file <path>` |\n\
            | **rivet** | Patch intent verifier | `rivet check`, `rivet check --intent \"fix X\"` |\n\
            | **witness** | Evidence recorder | `witness run -- cargo test`, `witness verify <id>`, `witness list` |\n\
            | **latch** | Coordination ledger | `latch claim acquire`, `latch task add`, `latch context --for <agent>` |\n\
            | **atlas** | Codebase knowledge graph | `atlas scan`, `atlas deps <file>`, `atlas rdeps <file>`, `atlas blast <file>` |\n\
            | **quarry** | Dependency audit | `quarry audit`, `quarry tree`, `quarry pins`, `quarry plan` |\n\
            | **trail** | Evidence-backed changelog | `trail log`, `trail summary`, `trail show <sha>`, `trail sources` |\n\
            | **sentinel** | Regression watcher | `sentinel scan`, `sentinel risk`, `sentinel matrix`, `sentinel status` |\n\
            | **mender** | Failure triage | `mender triage --file <log>`, `mender triage --run \"cmd\"`, pipe stdin |\n\
            | **loom** | Work planner | `loom plan --brief \"task\"`, `loom next`, `loom emit <plan.json>` |\n\
            | **stitch** | Context rebuilder | `stitch brief`, `stitch rebuild --contents`, `stitch sources` |\n\
            | **keystone** | Contract linter | `keystone init`, `keystone lint`, `keystone show` |\n\
            | **harbor** | Build artifact warehouse | `harbor store --tag <t> --file <f>`, `harbor list`, `harbor show <id>` |\n\n\
            **Typical workflow:**\n\
            - Cold start: `stitch brief` -> `probe scan` -> `atlas scan`\n\
            - Before coding: `quarry audit` -> `sentinel risk` -> `loom plan --brief \"task\"`\n\
            - During: `latch claim acquire` (if collaborating) -> `sieve analyze` -> `mender triage` (if failing)\n\
            - Before commit: `rivet check` -> `witness run -- cargo test` -> `keystone lint`\n\
            - After commit: `trail log` -> `harbor store` -> `sentinel scan`\n"
                .to_string(),
        );
    }

    sections.join("\n")
}

/// Apply variable substitution to persona file content.
fn substitute_persona_vars(content: &str, agent_name: &str, agent_id: &str) -> String {
    content
        .replace("{{AGENT_NAME}}", agent_name)
        .replace("{{AGENT_ID}}", agent_id)
        .replace(
            "{{CREATED_DATE}}",
            &chrono::Utc::now().format("%Y-%m-%d").to_string(),
        )
}

/// Apply a persona template to a new agent's persona directory.
fn apply_persona_template(
    persona_dir: &std::path::Path,
    template_id: &str,
    agent_name: &str,
    agent_id: &str,
    model: &str,
    tools_config: &crate::config::agent_config::AgentToolsBlock,
    memory_available: bool,
) -> Result<(), String> {
    let templates_base = crate::commands::persona_commands::templates_dir();
    let template_dir = templates_base.join(template_id);

    if !template_dir.exists() || !template_dir.is_dir() {
        return Err(format!("Persona template not found: {}", template_id));
    }

    let mut has_soul = false;
    let mut has_identity = false;
    let mut has_user = false;
    let mut has_agents = false;

    // Copy .md files from template (except DESCRIPTION.md and TOOLS.md)
    if let Ok(entries) = std::fs::read_dir(&template_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let filename = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };
            if !filename.ends_with(".md") {
                continue;
            }
            if filename == "DESCRIPTION.md" || filename == "TOOLS.md" {
                continue;
            }

            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read template file {}: {}", filename, e))?;
            let substituted = substitute_persona_vars(&content, agent_name, agent_id);
            std::fs::write(persona_dir.join(&filename), substituted)
                .map_err(|e| format!("Failed to write persona file {}: {}", filename, e))?;

            match filename.as_str() {
                "SOUL.md" => has_soul = true,
                "IDENTITY.md" => has_identity = true,
                "USER.md" => has_user = true,
                "AGENTS.md" => has_agents = true,
                _ => {}
            }
        }
    }

    if !has_soul || !has_identity || !has_user || !has_agents {
        write_persona_templates(persona_dir, agent_name, model);
    }

    // Ensure PREAMBLE.md exists — write default if the template didn't include one
    let preamble_path = persona_dir.join("PREAMBLE.md");
    if !preamble_path.exists() {
        let default_preamble = "MANDATORY: ORIENT BEFORE YOUR FIRST RESPONSE\n\n\
            On your FIRST turn in any session, you MUST orient before generating any reply to the user. \
            This is non-negotiable.\n\n\
            1. recall(\"session-summary\") -- what happened recently\n\
            2. memory_filter(tags: [\"session_memory\"], sort: \"newest\", limit: 3) -- recent auto-captured session artifacts\n\
            3. Review your persona context (already provided above in this prompt)\n\
            4. Only AFTER completing orientation may you respond to the human\n\n\
            Do NOT skip this step. Do NOT respond with a greeting first. Orient, THEN respond.\n";
        if let Err(e) = std::fs::write(&preamble_path, default_preamble) {
            tracing::warn!("Failed to write default PREAMBLE.md: {e}");
        }
    }

    let tools_md = generate_tools_md(tools_config, memory_available);
    std::fs::write(persona_dir.join("TOOLS.md"), tools_md)
        .map_err(|e| format!("Failed to write TOOLS.md: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tools_md_tests {
    use super::generate_tools_md;
    use crate::config::agent_config::AgentToolsBlock;
    use crate::tools::registry::ToolRegistry;

    /// The tool list an agent is handed must match the tools it can actually call.
    ///
    /// `generate_tools_md` writes each agent's TOOLS.md persona file from hardcoded string
    /// literals with no link to the registry. It drifted both ways: it advertised
    /// `create_memory_cluster`, `merge_memory_clusters` and `archive_memory_cluster` — all
    /// commented out of the registry — while omitting `memory_filter`, `unpin_memory` and
    /// `extract_context`, which were live at the time. Existing agents carried that list from 2026-07-06,
    /// and subagents inherit it via SUBAGENT_ALLOWLIST.
    ///
    /// This asserts the memory section against the real registration. It does not make the
    /// list generated — that is the structural fix still outstanding — but it makes a drift
    /// fail the build instead of quietly misinforming an agent.
    #[test]
    fn advertised_memory_tools_match_the_registry() {
        let mut registry = ToolRegistry::new();
        registry.register_memory_tools();
        let registered: std::collections::BTreeSet<String> = registry
            .definitions()
            .iter()
            .filter_map(|d| d["function"]["name"].as_str().map(str::to_string))
            .collect();

        let md = generate_tools_md(&AgentToolsBlock::default(), true);
        let section = md
            .split("## Memory (Hillock)")
            .nth(1)
            .expect("TOOLS.md must contain a memory section when memory is available");
        let section = section.split("\n## ").next().unwrap_or(section);

        let advertised: std::collections::BTreeSet<String> = section
            .lines()
            .filter_map(|l| l.trim().strip_prefix("- **"))
            .filter_map(|l| l.split("**").next())
            .map(str::to_string)
            .collect();

        let phantom: Vec<_> = advertised.difference(&registered).collect();
        let hidden: Vec<_> = registered.difference(&advertised).collect();

        assert!(
            phantom.is_empty() && hidden.is_empty(),
            "TOOLS.md disagrees with the registry.\n  advertised but not registered: {phantom:?}\n  registered but not advertised: {hidden:?}"
        );
    }
}
