use crate::config::agent_projects::ProjectEntry;
use crate::config::agent_sdk_config::AgentSdkBlock;
use crate::config::app_config::AttentionBlock;
use crate::runtime::agent::AgentColor;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Per-agent config file (PRD Section 11.3).
/// Stored at {base_config_dir}/agents/{agent_id}.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfigFile {
    pub agent: AgentBlock,
    pub connection: ConnectionBlock,
    pub workspace: WorkspaceBlock,
    pub tools: AgentToolsBlock,
    #[serde(default)]
    pub subagents: SubagentsBlock,
    #[serde(default)]
    pub limits: AgentLimitsBlock,
    #[serde(default)]
    pub sandbox: BuildSandboxBlock,
    #[serde(default)]
    pub execution_sandbox: Option<ExecutionSandboxBlock>,
    #[serde(default)]
    pub attention: AttentionBlock,
    #[serde(default)]
    #[serde(alias = "claude_sdk")]
    pub agent_sdk: AgentSdkBlock,
    #[serde(default)]
    pub coordinator_mode: bool,
    /// Permission profile name (matches docs/specs/PERMISSIONS_PROFILES_SPEC.md).
    /// Resolves to a Profile definition that filters the agent's tool surface.
    /// Defaults to "sandboxed" when missing.
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Switchable project list for TUI lanes. Each entry pairs a display name
    /// with a directory and (once spawned) a claude session UUID for resume.
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
}

fn default_profile() -> String {
    "sandboxed".to_string()
}

/// Per-agent execution limits (PRD Section 5.11).
/// Overrides the global defaults when set.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentLimitsBlock {
    pub max_tool_calls_per_turn: u32,
    pub max_tool_calls_per_session: u32,
    pub response_timeout_seconds: u64,
    pub max_response_tokens: u32,
    pub max_tool_retries: u32,
    #[serde(default)]
    pub context_tokens: Option<u32>,
}

impl Default for AgentLimitsBlock {
    fn default() -> Self {
        Self {
            max_tool_calls_per_turn: 50,
            max_tool_calls_per_session: 500,
            response_timeout_seconds: 300,
            max_response_tokens: 16384,
            max_tool_retries: 3,
            context_tokens: None,
        }
    }
}

/// Per-agent sandbox configuration for the test-fix loop.
/// (Renamed from SandboxBlock to avoid collision with execution sandbox.)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildSandboxBlock {
    pub enabled: bool,
    pub max_iterations: u32,
    pub auto_verify_command: String,
}

impl Default for BuildSandboxBlock {
    fn default() -> Self {
        Self {
            enabled: true,
            max_iterations: 4,
            auto_verify_command: String::new(),
        }
    }
}

/// Per-agent execution sandbox configuration.
/// Controls OS-level isolation for shell commands and filesystem access.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecutionSandboxBlock {
    /// Trust level: "unrestricted", "sandboxed", "restricted"
    pub level: String,

    /// Resource limits (only apply to sandboxed level)
    pub limits: SandboxLimitsBlock,

    /// Additional filesystem mounts (only apply to sandboxed level)
    pub mounts: SandboxMountsBlock,

    /// Network policy (only apply to sandboxed level)
    pub network: SandboxNetworkBlock,
}

impl Default for ExecutionSandboxBlock {
    fn default() -> Self {
        Self {
            level: "sandboxed".to_string(),
            limits: SandboxLimitsBlock::default(),
            mounts: SandboxMountsBlock::default(),
            network: SandboxNetworkBlock::default(),
        }
    }
}

impl ExecutionSandboxBlock {
    /// Default for existing agents that have no execution_sandbox in their config.
    /// Preserves backward compatibility — existing agents stay unrestricted.
    pub fn legacy_default() -> Self {
        Self {
            level: "unrestricted".to_string(),
            ..Default::default()
        }
    }

    pub fn is_restricted(&self) -> bool {
        self.level == "restricted"
    }

    pub fn is_sandboxed(&self) -> bool {
        self.level == "sandboxed"
    }

    pub fn is_unrestricted(&self) -> bool {
        self.level == "unrestricted"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxLimitsBlock {
    /// Memory limit in MB (default: 1024)
    pub memory_mb: u32,
    /// CPU time limit per command in seconds (default: 300)
    pub cpu_seconds_per_command: u32,
    /// Maximum number of processes (default: 64)
    pub max_processes: u32,
}

impl Default for SandboxLimitsBlock {
    fn default() -> Self {
        Self {
            memory_mb: 1024,
            cpu_seconds_per_command: 300,
            max_processes: 64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxMountsBlock {
    /// Additional read-only paths beyond system defaults
    pub extra_ro_paths: Vec<String>,
    /// Additional read-write paths beyond workspace_root
    pub extra_rw_paths: Vec<String>,
}

impl Default for SandboxMountsBlock {
    fn default() -> Self {
        Self {
            extra_ro_paths: Vec::new(),
            extra_rw_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxNetworkBlock {
    /// Allow outbound network access (default: true)
    pub allow_outbound: bool,
    /// Log network connections (default: true)
    pub log_connections: bool,
    /// Explicit host blocklist
    pub blocked_hosts: Vec<String>,
}

impl Default for SandboxNetworkBlock {
    fn default() -> Self {
        Self {
            allow_outbound: true,
            log_connections: true,
            blocked_hosts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBlock {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: AgentColor,
    /// Protected agents cannot be deleted through the UI or API.
    /// Used for dedicated lanes or other user-defined resident agents.
    #[serde(default)]
    pub protected: bool,
    /// When true, this agent runs a multi-agent model (e.g., Grok 4.20 swarm).
    /// Forking is rejected for such agents.
    #[serde(default)]
    pub multi_agent_model: bool,
    /// Cognitive base archetype (e.g., "scout", "sentinel"). Retained as provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archetype_base: Option<String>,
    /// Role specialization (e.g., "code_reviewer", "red_teamer"). Retained as provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archetype_specialization: Option<String>,
    /// Resolved personality traits. Source of truth for profile generation.
    /// When present, COGNITIVE_PROFILE.md is generated from these values.
    /// When absent (legacy agents), traits are resolved from archetype_base + archetype_specialization on demand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traits: Option<ResolvedTraits>,
}

/// Resolved personality traits — the authoritative source for profile generation.
///
/// Provenance fields record where starting values came from.
/// OCEAN fields are the actual personality values used for prose generation.
/// Communication fields control how the agent frames its output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedTraits {
    // ── Provenance (informational — where starting values came from) ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend_weight: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specialization: Option<String>,

    // ── OCEAN (authoritative — profile generates from these) ──
    pub openness: f32,
    pub conscientiousness: f32,
    pub extraversion: f32,
    pub agreeableness: f32,
    pub neuroticism: f32,

    // ── Communication ──
    #[serde(default = "default_verbosity")]
    pub verbosity: String,
    #[serde(default = "default_initiative")]
    pub initiative: String,
    #[serde(default = "default_tone")]
    pub tone: String,

    // ── Manual overrides — which traits were hand-adjusted vs computed ──
    #[serde(default)]
    pub manual_overrides: Vec<String>,

    // ── Custom agent onboarding flag ──
    #[serde(default)]
    pub directives_pending: bool,
}

fn default_verbosity() -> String {
    "balanced".to_string()
}
fn default_initiative() -> String {
    "measured".to_string()
}
fn default_tone() -> String {
    "balanced".to_string()
}

impl Default for ResolvedTraits {
    fn default() -> Self {
        Self {
            primary_base: None,
            secondary_base: None,
            blend_weight: None,
            specialization: None,
            openness: 0.5,
            conscientiousness: 0.5,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.5,
            verbosity: default_verbosity(),
            initiative: default_initiative(),
            tone: default_tone(),
            manual_overrides: Vec::new(),
            directives_pending: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConnectionBlock {
    #[serde(rename = "api")]
    Api {
        endpoint: String,
        model: String,
        /// Keychain reference — saved connection key_ref or agent ID
        #[serde(default)]
        key_ref: Option<String>,
    },
    #[serde(rename = "local")]
    Local {
        host: String,
        port: u16,
        model: Option<String>,
    },
    #[serde(rename = "mcp_stdio")]
    McpStdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    #[serde(rename = "oauth")]
    OAuth {
        provider: String,
        credentials_path: std::path::PathBuf,
        model: String,
    },
    #[serde(rename = "agent_sdk", alias = "claude_code")]
    AgentSdk {
        #[serde(default)]
        model: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceBlock {
    pub working_directory: PathBuf,
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
}

fn default_system_prompt() -> String {
    "You are a capable AI assistant with full access to the local filesystem and shell. You should use your tools to accomplish tasks directly — read files, write code, run commands — rather than just describing steps. When asked to do something, take action using the appropriate tool.".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentToolsBlock {
    pub filesystem: bool,
    pub code_execution: bool,
    pub web_access: bool,
    pub verification: bool,
    pub subagent: bool,
    pub a2a_messaging: bool,
}

impl Default for AgentToolsBlock {
    fn default() -> Self {
        Self {
            filesystem: true,
            code_execution: true,
            web_access: false,
            verification: true,
            subagent: true,
            a2a_messaging: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentsBlock {
    pub max_concurrent: u32,
    pub auto_spawn_enabled: bool,
    pub inherit_connection: bool,
}

impl Default for SubagentsBlock {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            auto_spawn_enabled: true,
            inherit_connection: true,
        }
    }
}

impl AgentConfigFile {
    /// Resolve execution sandbox config.
    /// - If explicitly set in the config file, use that.
    /// - If missing (legacy config), default to unrestricted for backward compatibility.
    /// - New agents should explicitly set `execution_sandbox` to `Some(ExecutionSandboxBlock::default())`
    ///   which gives them `sandboxed` level.
    pub fn resolved_execution_sandbox(&self) -> ExecutionSandboxBlock {
        self.execution_sandbox
            .clone()
            .unwrap_or_else(ExecutionSandboxBlock::legacy_default)
    }

    /// Load a single agent config from TOML file.
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: AgentConfigFile = toml::from_str(&content)?;
        Ok(config)
    }

    /// Save agent config to TOML file.
    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Load all agent configs from a directory.
    pub fn load_all(agents_dir: &Path) -> Result<Vec<Self>, Box<dyn std::error::Error>> {
        let mut configs = Vec::new();
        if !agents_dir.exists() {
            return Ok(configs);
        }

        for entry in std::fs::read_dir(agents_dir)? {
            let entry = entry?;
            let path = entry.path();
            // New format: agents/{id}/config.toml
            if path.is_dir() {
                let config_path = path.join("config.toml");
                if config_path.exists() {
                    match Self::load(&config_path) {
                        Ok(cfg) => configs.push(cfg),
                        Err(e) => {
                            tracing::warn!("Failed to load agent config {:?}: {e}", config_path)
                        }
                    }
                }
            }
        }
        Ok(configs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_api_agent() {
        let toml_str = r#"
[agent]
id = "agent-001"
name = "TestAgent"
color = { r = 100, g = 149, b = 237 }

[connection]
type = "api"
endpoint = "https://api.example.com/v1"
model = "test-model-1"

[workspace]
working_directory = "/home/mark/projects/harness"
system_prompt = "You are a helpful coding assistant."

[tools]
filesystem = true
code_execution = true
web_access = false
"#;
        let config: AgentConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.agent.id, "agent-001");
        assert_eq!(config.agent.name, "TestAgent");
        assert_eq!(config.agent.color.r, 100);
        match &config.connection {
            ConnectionBlock::Api {
                endpoint, model, ..
            } => {
                assert_eq!(endpoint, "https://api.example.com/v1");
                assert_eq!(model, "test-model-1");
            }
            _ => panic!("Expected Api connection"),
        }
        assert!(config.tools.filesystem);
        assert!(!config.tools.web_access);
    }

    #[test]
    fn test_parse_local_agent() {
        let toml_str = r#"
[agent]
id = "agent-002"
name = "Local Qwen"

[connection]
type = "local"
host = "192.168.50.107"
port = 8080

[workspace]
working_directory = "/tmp/test"

[tools]
filesystem = true
code_execution = false
web_access = false
"#;
        let config: AgentConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.agent.name, "Local Qwen");
        match &config.connection {
            ConnectionBlock::Local { host, port, model } => {
                assert_eq!(host, "192.168.50.107");
                assert_eq!(*port, 8080);
                assert!(model.is_none());
            }
            _ => panic!("Expected Local connection"),
        }
    }

    #[test]
    fn test_parse_mcp_stdio_agent() {
        let toml_str = r#"
[agent]
id = "agent-003"
name = "MCP Filesystem"

[connection]
type = "mcp_stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem"]

[workspace]
working_directory = "/home/mark/projects"

[tools]
filesystem = true
code_execution = false
web_access = false
"#;
        let config: AgentConfigFile = toml::from_str(toml_str).unwrap();
        match &config.connection {
            ConnectionBlock::McpStdio { command, args } => {
                assert_eq!(command, "npx");
                assert_eq!(args.len(), 2);
            }
            _ => panic!("Expected McpStdio connection"),
        }
    }

    #[test]
    fn test_save_and_load_agent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-test.toml");

        let config = AgentConfigFile {
            agent: AgentBlock {
                id: "test-001".to_string(),
                name: "Test Agent".to_string(),
                color: AgentColor::default(),
                protected: false,
                multi_agent_model: false,
                archetype_base: None,
                archetype_specialization: None,
                traits: None,
            },
            connection: ConnectionBlock::Local {
                host: "localhost".to_string(),
                port: 11434,
                model: Some("llama3".to_string()),
            },
            workspace: WorkspaceBlock {
                working_directory: PathBuf::from("/tmp/test"),
                system_prompt: "Test prompt".to_string(),
            },
            tools: AgentToolsBlock::default(),
            subagents: SubagentsBlock::default(),
            limits: AgentLimitsBlock::default(),
            sandbox: BuildSandboxBlock::default(),
            execution_sandbox: None,
            attention: AttentionBlock::default(),
            agent_sdk: AgentSdkBlock::default(),
            coordinator_mode: false,
            profile: "sandboxed".to_string(),
            projects: Vec::new(),
        };

        config.save(&path).unwrap();
        let loaded = AgentConfigFile::load(&path).unwrap();
        assert_eq!(loaded.agent.id, "test-001");
        assert_eq!(loaded.agent.name, "Test Agent");
    }

    #[test]
    fn test_execution_sandbox_default_is_sandboxed() {
        let block = ExecutionSandboxBlock::default();
        assert_eq!(block.level, "sandboxed");
        assert!(block.is_sandboxed());
        assert!(!block.is_unrestricted());
        assert!(!block.is_restricted());
    }

    #[test]
    fn test_execution_sandbox_legacy_default_is_unrestricted() {
        let block = ExecutionSandboxBlock::legacy_default();
        assert_eq!(block.level, "unrestricted");
        assert!(block.is_unrestricted());
        assert!(!block.is_sandboxed());
    }

    #[test]
    fn test_missing_execution_sandbox_resolves_to_unrestricted() {
        // Simulates loading an existing config that has no [execution_sandbox] section.
        let toml_str = r#"
[agent]
id = "legacy-agent"
name = "Legacy"

[connection]
type = "api"
endpoint = "https://api.example.com/v1"
model = "test-model"

[workspace]
working_directory = "/tmp/test"

[tools]
filesystem = true
code_execution = true
web_access = false
"#;
        let config: AgentConfigFile = toml::from_str(toml_str).unwrap();
        assert!(config.execution_sandbox.is_none());
        let resolved = config.resolved_execution_sandbox();
        assert_eq!(resolved.level, "unrestricted");
    }

    #[test]
    fn test_explicit_execution_sandbox_round_trips() {
        let toml_str = r#"
[agent]
id = "new-agent"
name = "New"

[connection]
type = "api"
endpoint = "https://api.example.com/v1"
model = "test-model"

[workspace]
working_directory = "/tmp/test"

[tools]
filesystem = true
code_execution = true
web_access = false

[execution_sandbox]
level = "sandboxed"

[execution_sandbox.limits]
memory_mb = 2048
cpu_seconds_per_command = 600
max_processes = 128

[execution_sandbox.mounts]
extra_ro_paths = ["/opt/tools"]
extra_rw_paths = ["/tmp/shared"]

[execution_sandbox.network]
allow_outbound = true
log_connections = false
blocked_hosts = ["evil.com"]
"#;
        let config: AgentConfigFile = toml::from_str(toml_str).unwrap();
        let es = config.execution_sandbox.unwrap();
        assert_eq!(es.level, "sandboxed");
        assert_eq!(es.limits.memory_mb, 2048);
        assert_eq!(es.limits.cpu_seconds_per_command, 600);
        assert_eq!(es.limits.max_processes, 128);
        assert_eq!(es.mounts.extra_ro_paths, vec!["/opt/tools"]);
        assert_eq!(es.mounts.extra_rw_paths, vec!["/tmp/shared"]);
        assert!(es.network.allow_outbound);
        assert!(!es.network.log_connections);
        assert_eq!(es.network.blocked_hosts, vec!["evil.com"]);
    }

    #[test]
    fn test_restricted_level_helpers() {
        let block = ExecutionSandboxBlock {
            level: "restricted".to_string(),
            ..Default::default()
        };
        assert!(block.is_restricted());
        assert!(!block.is_sandboxed());
        assert!(!block.is_unrestricted());
    }

    #[test]
    fn test_new_agent_gets_sandboxed_via_explicit_default() {
        // New agents should set execution_sandbox = Some(Default) which is sandboxed
        let config = AgentConfigFile {
            agent: AgentBlock {
                id: "new-001".to_string(),
                name: "New Agent".to_string(),
                color: AgentColor::default(),
                protected: false,
                multi_agent_model: false,
                archetype_base: None,
                archetype_specialization: None,
                traits: None,
            },
            connection: ConnectionBlock::Local {
                host: "localhost".to_string(),
                port: 11434,
                model: None,
            },
            workspace: WorkspaceBlock {
                working_directory: PathBuf::from("/tmp/test"),
                system_prompt: "Test".to_string(),
            },
            tools: AgentToolsBlock::default(),
            subagents: SubagentsBlock::default(),
            limits: AgentLimitsBlock::default(),
            sandbox: BuildSandboxBlock::default(),
            execution_sandbox: Some(ExecutionSandboxBlock::default()),
            attention: AttentionBlock::default(),
            agent_sdk: AgentSdkBlock::default(),
            coordinator_mode: false,
            profile: "sandboxed".to_string(),
            projects: Vec::new(),
        };
        let resolved = config.resolved_execution_sandbox();
        assert_eq!(resolved.level, "sandboxed");
    }

    #[test]
    fn profile_defaults_to_sandboxed_when_missing() {
        let toml_text = r#"
[agent]
id = "test"
name = "Test"

[agent.color]
r = 100
g = 100
b = 100

[connection]
type = "local"
host = "localhost"
port = 11434

[workspace]
working_directory = "/tmp/test"
system_prompt = "Test"

[tools]
"#;
        let parsed: AgentConfigFile = toml::from_str(toml_text).unwrap();
        assert_eq!(parsed.profile, "sandboxed");
    }

    #[test]
    fn profile_field_round_trips() {
        let toml_text = r#"
profile = "unrestricted"

[agent]
id = "agent-terminal"
name = "Agent Terminal"

[agent.color]
r = 249
g = 115
b = 22

[connection]
type = "agent_sdk"

[workspace]
working_directory = "/tmp/test"
system_prompt = "Test"

[tools]
"#;
        let parsed: AgentConfigFile = toml::from_str(toml_text).unwrap();
        assert_eq!(parsed.profile, "unrestricted");
    }
}
