//! Permission profile schema. Matches docs/specs/PERMISSIONS_PROFILES_SPEC.md.
//!
//! This module ships the profile schema, built-in profiles, and family lookup.
//! Workspace scoping, env filtering, approval overrides, and the full UI/migration
//! flow are owned by the broader permission profiles work.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFamily {
    FilesystemRead,
    FilesystemWrite,
    Shell,
    Web,
    MemoryRead,
    MemoryWrite,
    Cron,
    Persona,
    Subagents,
    Browser,
    A2a,
    Drafts,
    Watches,
}

impl ToolFamily {
    pub fn all() -> &'static [ToolFamily] {
        &[
            ToolFamily::FilesystemRead,
            ToolFamily::FilesystemWrite,
            ToolFamily::Shell,
            ToolFamily::Web,
            ToolFamily::MemoryRead,
            ToolFamily::MemoryWrite,
            ToolFamily::Cron,
            ToolFamily::Persona,
            ToolFamily::Subagents,
            ToolFamily::Browser,
            ToolFamily::A2a,
            ToolFamily::Drafts,
            ToolFamily::Watches,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub tool_families: ToolFamilies,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolFamilies {
    pub filesystem_read: bool,
    pub filesystem_write: bool,
    pub shell: bool,
    pub web: bool,
    pub memory_read: bool,
    pub memory_write: bool,
    pub cron: bool,
    pub persona: bool,
    pub subagents: bool,
    pub browser: bool,
    pub a2a: bool,
    pub drafts: bool,
    pub watches: bool,
}

impl ToolFamilies {
    pub fn all_true() -> Self {
        Self {
            filesystem_read: true,
            filesystem_write: true,
            shell: true,
            web: true,
            memory_read: true,
            memory_write: true,
            cron: true,
            persona: true,
            subagents: true,
            browser: true,
            a2a: true,
            drafts: true,
            watches: true,
        }
    }

    pub fn get(&self, family: ToolFamily) -> bool {
        match family {
            ToolFamily::FilesystemRead => self.filesystem_read,
            ToolFamily::FilesystemWrite => self.filesystem_write,
            ToolFamily::Shell => self.shell,
            ToolFamily::Web => self.web,
            ToolFamily::MemoryRead => self.memory_read,
            ToolFamily::MemoryWrite => self.memory_write,
            ToolFamily::Cron => self.cron,
            ToolFamily::Persona => self.persona,
            ToolFamily::Subagents => self.subagents,
            ToolFamily::Browser => self.browser,
            ToolFamily::A2a => self.a2a,
            ToolFamily::Drafts => self.drafts,
            ToolFamily::Watches => self.watches,
        }
    }
}

impl Profile {
    pub fn allows_family(&self, family: ToolFamily) -> bool {
        self.tool_families.get(family)
    }

    pub fn lookup(name: &str) -> Option<Profile> {
        match name {
            "unrestricted" => Some(Self::unrestricted()),
            "sandboxed" => Some(Self::sandboxed()),
            "standard" => Some(Self::standard()),
            "networked" => Some(Self::networked()),
            "restricted" => Some(Self::restricted()),
            _ => None,
        }
    }

    fn unrestricted() -> Self {
        Self {
            name: "unrestricted".into(),
            display_name: "Unrestricted".into(),
            description: "Full access to all tools with no resource limits".into(),
            tool_families: ToolFamilies::all_true(),
        }
    }

    fn sandboxed() -> Self {
        let mut tf = ToolFamilies::all_true();
        tf.persona = false;
        Self {
            name: "sandboxed".into(),
            display_name: "Sandboxed".into(),
            description: "Full tool access with resource limits".into(),
            tool_families: tf,
        }
    }

    fn standard() -> Self {
        Self {
            name: "standard".into(),
            display_name: "Standard".into(),
            description: "File read/write and memory access, no shell, web, or system tools".into(),
            tool_families: ToolFamilies {
                filesystem_read: true,
                filesystem_write: true,
                memory_read: true,
                memory_write: true,
                drafts: true,
                ..ToolFamilies::default()
            },
        }
    }

    fn networked() -> Self {
        Self {
            name: "networked".into(),
            display_name: "Networked".into(),
            description: "File access and web/browser capabilities, no shell".into(),
            tool_families: ToolFamilies {
                filesystem_read: true,
                filesystem_write: true,
                web: true,
                memory_read: true,
                memory_write: true,
                browser: true,
                a2a: true,
                drafts: true,
                ..ToolFamilies::default()
            },
        }
    }

    fn restricted() -> Self {
        Self {
            name: "restricted".into(),
            display_name: "Restricted".into(),
            description: "Read-only access for observation and analysis".into(),
            tool_families: ToolFamilies {
                filesystem_read: true,
                memory_read: true,
                ..ToolFamilies::default()
            },
        }
    }
}

/// Tools that are always exposed regardless of profile. Matches the always-on set
/// in docs/specs/PERMISSIONS_PROFILES_SPEC.md.
pub fn always_on_tool_names() -> &'static [&'static str] {
    &[
        "list_agents",
        "get_agent_info",
        "alert_user",
        "ask_hil",
        "list_available_tools",
        "read_skill",
        "customize_appearance",
    ]
}

/// Map a tool name to its family. Returns None for tools not currently
/// classified (those tools pass through unfiltered — they're not yet
/// covered by the profile system).
pub fn tool_family_for(tool_name: &str) -> Option<ToolFamily> {
    use ToolFamily::*;
    Some(match tool_name {
        // filesystem_read
        "read_file" | "list_directory" | "search_files" | "find_files" | "diff_file"
        | "check_syntax" => FilesystemRead,
        // filesystem_write
        "write_file" | "edit_file" | "delete_file" | "create_directory" | "move_file"
        | "copy_file" => FilesystemWrite,
        // shell
        "bash" | "start_process" | "write_process" | "kill_process" | "read_process"
        | "list_processes" | "execute_tests" => Shell,
        // web
        "fetch_url" | "web_search" | "screenshot_url" => Web,
        // memory_read
        "recall" | "memory_filter" | "context_status" => MemoryRead,
        // memory_write
        "remember"
        | "forget"
        | "pin_memory"
        | "unpin_memory"
        | "memory_crystallize"
        | "create_memory_cluster"
        | "archive_memory_cluster"
        | "merge_memory_clusters"
        | "memory_cleanup" => MemoryWrite,
        // cron
        "cron_create" | "cron_list" | "cron_delete" => Cron,
        // persona
        "update_persona" | "set_persona_anchors" => Persona,
        // subagents
        "spawn_subagent" | "check_subagent" | "cancel_subagent" | "list_subagents" => Subagents,
        // browser
        "browser_navigate"
        | "browser_click"
        | "browser_fill_form"
        | "browser_select_option"
        | "browser_press_key"
        | "browser_hover"
        | "browser_drag"
        | "browser_drop"
        | "browser_file_upload"
        | "browser_screenshot"
        | "browser_take_screenshot"
        | "browser_snapshot"
        | "browser_evaluate"
        | "browser_wait_for"
        | "browser_resize"
        | "browser_tabs"
        | "browser_close"
        | "browser_navigate_back"
        | "browser_console_messages"
        | "browser_network_request"
        | "browser_network_requests"
        | "browser_handle_dialog"
        | "browser_run_code_unsafe"
        | "browser_type" => Browser,
        // a2a
        "send_message_to_agent"
        | "read_agent_workspace"
        | "request_collaboration"
        | "drain_a2a_inbox" => A2a,
        // drafts
        "draft_write" | "draft_read" | "draft_list" | "draft_promote" | "draft_delete" => Drafts,
        // watches
        "watch_path" | "unwatch_path" | "list_watches" => Watches,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrestricted_profile_enables_all_families() {
        let p = Profile::lookup("unrestricted").expect("unrestricted profile must exist");
        for family in ToolFamily::all() {
            assert!(
                p.allows_family(*family),
                "unrestricted must allow {family:?}"
            );
        }
    }

    #[test]
    fn restricted_profile_only_allows_filesystem_read_and_memory_read() {
        let p = Profile::lookup("restricted").expect("restricted profile must exist");
        assert!(p.allows_family(ToolFamily::FilesystemRead));
        assert!(p.allows_family(ToolFamily::MemoryRead));
        assert!(!p.allows_family(ToolFamily::FilesystemWrite));
        assert!(!p.allows_family(ToolFamily::Shell));
        assert!(!p.allows_family(ToolFamily::Web));
    }

    #[test]
    fn unknown_profile_returns_none() {
        assert!(Profile::lookup("nonexistent").is_none());
    }

    #[test]
    fn always_on_tools_listed() {
        let always_on = always_on_tool_names();
        assert!(always_on.contains(&"list_agents"));
        assert!(always_on.contains(&"get_agent_info"));
        assert!(always_on.contains(&"alert_user"));
        assert!(always_on.contains(&"list_available_tools"));
    }
}
