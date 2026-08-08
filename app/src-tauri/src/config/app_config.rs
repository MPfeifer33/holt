use crate::config::agent_sdk_config::AgentSdkBlock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Top-level application config, deserialized from config.toml (PRD Section 11.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct AppConfig {
    pub app: AppBlock,
    pub ui: UiBlock,
    pub workspace: WorkspaceRootBlock,
    pub persistence: PersistenceBlock,
    pub network: NetworkBlock,
    pub context: ContextBlock,
    pub limits: LimitsBlock,
    pub checkpoints: CheckpointsBlock,
    pub approval: ApprovalBlock,
    pub traces: TracesBlock,
    pub file_locking: FileLockingBlock,
    pub subagents: Option<SubagentsGlobalBlock>,
    pub communication: CommunicationBlock,
    #[serde(rename = "tools")]
    pub tools: ToolsBlock,
    pub plugins: PluginsBlock,
    pub keybindings: KeybindingsBlock,
    #[serde(default)]
    pub pricing: PricingBlock,
    #[serde(default)]
    pub memory: MemoryBlock,
    /// Persisted connections (API keys, local endpoints) — independent of agents.
    #[serde(default)]
    pub connections: Vec<SavedConnection>,
    #[serde(default)]
    pub attention: AttentionBlock,
    #[serde(default, alias = "claude_sdk")]
    pub agent_sdk: AgentSdkBlock,
    #[serde(default)]
    pub cron: CronBlock,
}

/// A persisted API connection or local model endpoint, stored independently of agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedConnection {
    pub id: String,
    pub provider: String,
    pub endpoint: String,
    pub key_ref: String,
    pub is_local: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppBlock {
    pub theme: String,
    pub default_agent_count: u32,
    pub auto_detect_local_models: bool,
    pub auto_detect_interval_seconds: u32,
    pub minimize_to_tray: bool,
    pub offline_mode: bool,
}

impl Default for AppBlock {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            default_agent_count: 3,
            auto_detect_local_models: true,
            auto_detect_interval_seconds: 30,
            minimize_to_tray: true,
            offline_mode: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiBlock {
    pub constellation_physics_enabled: bool,
    pub constellation_node_glow: bool,
    pub animation_duration_ms: u32,
    pub glass_opacity: f64,
    pub thought_log_visible: bool,
    pub tools_panel_collapsed: bool,
    pub power_user_mode: bool,
    pub gfx_mode: bool,
    #[serde(default)]
    pub gfx: GfxBlock,
    #[serde(default)]
    pub constellation: ConstellationBlock,
}

impl Default for UiBlock {
    fn default() -> Self {
        Self {
            constellation_physics_enabled: true,
            constellation_node_glow: true,
            animation_duration_ms: 250,
            glass_opacity: 0.65,
            thought_log_visible: true,
            tools_panel_collapsed: false,
            power_user_mode: false,
            gfx_mode: false,
            gfx: GfxBlock::default(),
            constellation: ConstellationBlock::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GfxBlock {
    pub quality_preset: String,
    pub bloom_strength: f64,
    pub chromatic_aberration: bool,
    pub particle_density: String,
    pub auto_rotate_speed: f64,
    pub camera_attention: bool,
}

impl Default for GfxBlock {
    fn default() -> Self {
        Self {
            quality_preset: "auto".to_string(),
            bloom_strength: 0.8,
            chromatic_aberration: true,
            particle_density: "medium".to_string(),
            auto_rotate_speed: 1.0,
            camera_attention: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConstellationBlock {
    pub theme: String,
    pub nebula_primary: String,
    pub nebula_secondary: String,
    pub nebula_intensity: f64,
    pub color_cycling: bool,
    pub color_cycling_speed: f64,
    pub starfield_density: String,
    pub orbital_trails: bool,
    pub orbital_trail_opacity: f64,
    pub bloom_enabled: bool,
    pub mouse_proximity_glow: bool,
    pub star_flash_on_status: bool,
    pub node_base_size: u32,
    pub orbit_speed_multiplier: f64,
}

impl Default for ConstellationBlock {
    fn default() -> Self {
        Self {
            theme: "deep_space".to_string(),
            nebula_primary: "#8b5cf6".to_string(),
            nebula_secondary: "#06b6d4".to_string(),
            nebula_intensity: 0.04,
            color_cycling: false,
            color_cycling_speed: 0.5,
            starfield_density: "medium".to_string(),
            orbital_trails: true,
            orbital_trail_opacity: 0.5,
            bloom_enabled: true,
            mouse_proximity_glow: true,
            star_flash_on_status: true,
            node_base_size: 32,
            orbit_speed_multiplier: 1.0,
        }
    }
}

/// Global workspace root — scopes all agent working directories.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceRootBlock {
    pub root: PathBuf,
    pub show_hidden_folders: bool,
}

impl Default for WorkspaceRootBlock {
    fn default() -> Self {
        Self {
            root: dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
            show_hidden_folders: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistenceBlock {
    pub auto_save_interval_seconds: u32,
    pub restore_on_launch: bool,
    pub session_directory: String,
}

impl Default for PersistenceBlock {
    fn default() -> Self {
        Self {
            auto_save_interval_seconds: 30,
            restore_on_launch: true,
            session_directory: "sessions".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkBlock {
    pub request_timeout_seconds: u64,
    pub stream_timeout_seconds: u64,
    pub tool_timeout_seconds: u64,
    pub retry_on_network_error: u32,
    pub retry_backoff_ms: u64,
    pub rate_limit_max_retries: u32,
}

impl Default for NetworkBlock {
    fn default() -> Self {
        Self {
            request_timeout_seconds: 120,
            stream_timeout_seconds: 300,
            tool_timeout_seconds: 30,
            retry_on_network_error: 1,
            retry_backoff_ms: 1000,
            rate_limit_max_retries: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextBlock {
    pub response_reserve_tokens: u32,
    pub pinned_top_max_tokens: u32,
    pub pinned_bottom_max_tokens: u32,
    pub max_tool_result_tokens: u32,
    pub truncation_strategy: String,
    pub truncation_notice: bool,
    pub eviction_summarize: bool,
    pub compact_button: bool,
    pub char_ratio: f32,
    pub compaction_threshold_percent: u32,
    pub compaction_keep_recent: usize,
    pub compaction_summary_tokens: u32,
    pub compaction_model_override: String,
    pub auto_checkpoint_on_compaction: bool,
    pub pre_compaction_threshold_percent: u32,
    /// Compaction mode: "model" (default) uses a separate summarization prompt,
    /// "self_summary" asks the agent itself to summarize using its full context
    /// and persona for higher quality summaries.
    #[serde(default = "default_compaction_mode")]
    pub compaction_mode: String,
}

fn default_compaction_mode() -> String {
    "model".to_string()
}

impl Default for ContextBlock {
    fn default() -> Self {
        Self {
            response_reserve_tokens: 35_000,
            pinned_top_max_tokens: 16000,
            pinned_bottom_max_tokens: 12000,
            max_tool_result_tokens: 2000,
            truncation_strategy: "head_tail".to_string(),
            truncation_notice: true,
            eviction_summarize: true,
            compact_button: true,
            char_ratio: 4.0,
            compaction_threshold_percent: 80,
            compaction_keep_recent: 5,
            compaction_summary_tokens: 2000,
            compaction_model_override: "none".to_string(),
            auto_checkpoint_on_compaction: true,
            pre_compaction_threshold_percent: 70,
            compaction_mode: default_compaction_mode(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LimitsBlock {
    pub max_tool_calls_per_turn: u32,
    pub max_tool_calls_per_session: u32,
    pub response_timeout_seconds: u64,
    pub max_response_tokens: u32,
    pub max_tool_retries: u32,
}

impl Default for LimitsBlock {
    fn default() -> Self {
        Self {
            max_tool_calls_per_turn: 25,
            max_tool_calls_per_session: 200,
            response_timeout_seconds: 300,
            max_response_tokens: 16384,
            max_tool_retries: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckpointsBlock {
    pub max_per_agent: u32,
    pub auto_on_compaction: bool,
    pub auto_on_destructive_approval: bool,
    pub checkpoint_directory: String,
}

impl Default for CheckpointsBlock {
    fn default() -> Self {
        Self {
            max_per_agent: 10,
            auto_on_compaction: true,
            auto_on_destructive_approval: true,
            checkpoint_directory: "sessions/checkpoints".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalBlock {
    pub default: String,
    pub overrides: HashMap<String, String>,
    pub veto_timeout_seconds: u32,
    /// Maximum time (seconds) to wait for RequireApproval responses before denying.
    /// Default: 300 (5 minutes). Set to 0 for no timeout (not recommended).
    pub require_approval_timeout_seconds: u32,
    #[serde(default)]
    pub bash_escalation: BashEscalationBlock,
}

impl Default for ApprovalBlock {
    fn default() -> Self {
        let mut overrides = HashMap::new();
        overrides.insert("delete_file".to_string(), "require_approval".to_string());
        overrides.insert("bash".to_string(), "auto".to_string());
        overrides.insert("write_file".to_string(), "auto".to_string());
        overrides.insert("fetch_url".to_string(), "notify_unless_veto".to_string());
        overrides.insert("web_search".to_string(), "auto".to_string());
        overrides.insert(
            "start_process".to_string(),
            "notify_unless_veto".to_string(),
        );
        overrides.insert("spawn_subagent".to_string(), "auto".to_string());
        overrides.insert("kill_process".to_string(), "notify_unless_veto".to_string());
        Self {
            default: "auto".to_string(),
            overrides,
            veto_timeout_seconds: 15,
            require_approval_timeout_seconds: 300,
            bash_escalation: BashEscalationBlock::default(),
        }
    }
}

/// Bash command pattern escalation config — dangerous commands trigger a higher approval tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashEscalationBlock {
    pub tier: String,
    pub patterns: Vec<String>,
}

impl Default for BashEscalationBlock {
    fn default() -> Self {
        Self {
            tier: "require_approval".to_string(),
            patterns: vec![
                "rm -rf".into(),
                "rm -r".into(),
                "rm ".into(),
                "sudo".into(),
                "chmod".into(),
                "chown".into(),
                "mkfs".into(),
                "dd if=".into(),
                "kill -9".into(),
                "pkill".into(),
                "shutdown".into(),
                "reboot".into(),
                "> /dev/".into(),
                "curl.*| sh".into(),
                "wget.*| sh".into(),
                "git push --force".into(),
                "git reset --hard".into(),
                "docker rm".into(),
                "docker system prune".into(),
                "npm publish".into(),
                "cargo publish".into(),
            ],
        }
    }
}

/// Cron scheduler config — global settings for scheduled jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CronBlock {
    pub enabled: bool,
    pub min_interval_seconds: u64,
    pub max_jobs: u32,
}

impl Default for CronBlock {
    fn default() -> Self {
        Self {
            enabled: true,
            min_interval_seconds: 60,
            max_jobs: 50,
        }
    }
}

/// Attention / notification config — when to flash the title bar or play audio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionBlock {
    pub title_flash_on: String,
    pub audio_on: String,
}

impl Default for AttentionBlock {
    fn default() -> Self {
        Self {
            title_flash_on: "critical".to_string(),
            audio_on: "critical".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TracesBlock {
    pub enabled: bool,
    pub retention_days: u32,
    pub max_db_size_mb: u32,
    pub export_format: String,
}

impl Default for TracesBlock {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 90,
            max_db_size_mb: 500,
            export_format: "json".to_string(),
        }
    }
}

/// Virtual File Lock configuration (PRD Section 7.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileLockingBlock {
    pub enabled: bool,
    pub max_hold_seconds: u32,
    pub stale_timeout_seconds: u32,
    pub queue_timeout_seconds: u32,
}

impl Default for FileLockingBlock {
    fn default() -> Self {
        Self {
            enabled: true,
            max_hold_seconds: 60,
            stale_timeout_seconds: 30,
            queue_timeout_seconds: 120,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentsGlobalBlock {
    /// Max concurrent subagents per parent agent.
    pub max_concurrent: u32,
    /// Max total subagents across all parents (system-wide cap).
    pub max_total: u32,
    pub auto_spawn_enabled: bool,
}

impl Default for SubagentsGlobalBlock {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            max_total: 16,
            auto_spawn_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CommunicationBlock {
    pub a2a_default_enabled: bool,
    pub broadcast_enabled: bool,
}

impl Default for CommunicationBlock {
    fn default() -> Self {
        Self {
            a2a_default_enabled: true,
            broadcast_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct ToolsBlock {
    pub web: WebToolsBlock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebToolsBlock {
    pub search_provider: String,
    pub search_endpoint: String,
    pub fetch_max_bytes: u64,
    pub fetch_rate_limit_per_minute: u32,
    pub search_rate_limit_per_minute: u32,
}

impl Default for WebToolsBlock {
    fn default() -> Self {
        Self {
            search_provider: "searxng".to_string(),
            search_endpoint: "http://localhost:8888".to_string(),
            fetch_max_bytes: 1_048_576,
            fetch_rate_limit_per_minute: 10,
            search_rate_limit_per_minute: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsBlock {
    pub directory: String,
    pub max_ipc_payload_bytes: usize,
    pub health_check_interval_seconds: u32,
    #[serde(default)]
    pub registry: HashMap<String, PluginEntry>,
}

impl Default for PluginsBlock {
    fn default() -> Self {
        Self {
            directory: "plugins".to_string(),
            max_ipc_payload_bytes: crate::plugin::types::MAX_IPC_PAYLOAD_BYTES,
            health_check_interval_seconds: 60,
            registry: HashMap::new(),
        }
    }
}

/// Type of plugin — native (custom JSON-RPC) or MCP (Model Context Protocol).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PluginType {
    #[default]
    Native,
    Mcp,
}

/// Configuration for a single plugin (lives under [plugins.registry.<id>]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    pub enabled: bool,
    /// Plugin type: "native" (default) or "mcp"
    #[serde(default)]
    pub plugin_type: PluginType,
    /// Binary name for native plugins (matched against plugins directory)
    #[serde(default)]
    pub binary: String,
    /// Command to spawn MCP plugins (e.g. "python3")
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments for MCP plugin command (e.g. ["/path/to/shim.py"])
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub settings: HashMap<String, toml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindingsBlock {
    /// Action ID -> key binding string (e.g. "Ctrl+K", "Ctrl+Shift+A", "F11")
    /// Supports modifiers: Ctrl, Shift, Alt, Super
    /// Supports chords: "Ctrl+K 1" (first combo, then second key)
    pub bindings: HashMap<String, String>,
}

impl Default for KeybindingsBlock {
    fn default() -> Self {
        let mut bindings = HashMap::new();
        // Tier 1: Core shortcuts
        bindings.insert("command_center.toggle".to_string(), "Ctrl+K".to_string());
        bindings.insert("terminal.new".to_string(), "Ctrl+`".to_string());
        bindings.insert("a2a_panel.toggle".to_string(), "Ctrl+Shift+A".to_string());
        bindings.insert("window.focus_next".to_string(), "Ctrl+Tab".to_string());
        bindings.insert(
            "window.focus_prev".to_string(),
            "Ctrl+Shift+Tab".to_string(),
        );
        bindings.insert("window.close".to_string(), "Ctrl+W".to_string());
        bindings.insert("window.minimize".to_string(), "Ctrl+M".to_string());
        bindings.insert("app.minimize".to_string(), "Ctrl+Shift+M".to_string());
        bindings.insert("app.toggle_fullscreen".to_string(), "F11".to_string());
        // Tier 2: Navigation
        bindings.insert("command_center.tab.1".to_string(), "Ctrl+K 1".to_string());
        bindings.insert("command_center.tab.2".to_string(), "Ctrl+K 2".to_string());
        bindings.insert("command_center.tab.3".to_string(), "Ctrl+K 3".to_string());
        bindings.insert("command_center.tab.4".to_string(), "Ctrl+K 4".to_string());
        bindings.insert("command_center.tab.5".to_string(), "Ctrl+K 5".to_string());
        bindings.insert("agent.new".to_string(), "Ctrl+N".to_string());
        bindings.insert("canvas.zoom_reset".to_string(), "Ctrl+0".to_string());
        // Global
        bindings.insert("focus.escape".to_string(), "Escape".to_string());
        Self { bindings }
    }
}

impl KeybindingsBlock {
    /// Get binding for an action, returns None if action is not mapped.
    pub fn get(&self, action_id: &str) -> Option<&str> {
        self.bindings.get(action_id).map(|s| s.as_str())
    }

    /// Set a binding for an action. Returns the previous binding if any.
    pub fn set(&mut self, action_id: String, binding: String) -> Option<String> {
        self.bindings.insert(action_id, binding)
    }

    /// Check for conflicts (multiple actions mapped to same binding).
    /// Returns pairs of (binding, vec of action_ids) where conflicts exist.
    pub fn find_conflicts(&self) -> Vec<(String, Vec<String>)> {
        let mut reverse: HashMap<&str, Vec<&str>> = HashMap::new();
        for (action, binding) in &self.bindings {
            reverse
                .entry(binding.as_str())
                .or_default()
                .push(action.as_str());
        }
        reverse
            .into_iter()
            .filter(|(_, actions)| actions.len() > 1)
            .map(|(binding, actions)| {
                (
                    binding.to_string(),
                    actions.into_iter().map(|s| s.to_string()).collect(),
                )
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PricingBlock {
    pub models: HashMap<String, ModelPricing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

impl PricingBlock {
    /// Find pricing for a model name using longest-match-first substring matching.
    /// Prevents "gpt-4o" from matching before "gpt-4o-mini".
    pub fn lookup(&self, model_name: &str) -> Option<&ModelPricing> {
        let name_lower = model_name.to_lowercase();
        let mut patterns: Vec<_> = self.models.keys().collect();
        patterns.sort_by(|a, b| b.len().cmp(&a.len()));
        for pattern in patterns {
            if name_lower.contains(&pattern.to_lowercase()) {
                return self.models.get(pattern);
            }
        }
        None
    }
}

/// Memory system configuration (Hillock-backed persistent memory).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryBlock {
    pub enabled: bool,
    pub embedding_model: String,
    /// Path to the Hillock SQLite database (embedded mode).
    /// XDG fallback when empty: ~/.local/share/holt/hillock.db.
    #[serde(default)]
    pub hillock_db_path: String,
    /// Master key for rotation key derivation (embedded mode).
    /// Default: "default-dev-key"
    #[serde(default)]
    pub hillock_master_key: String,
    /// Namespace aliases for collapsing sibling agent identities into a
    /// canonical namespace. Public Holt defaults to no implicit aliases;
    /// projects can opt in by setting this map explicitly in config.toml.
    #[serde(default = "default_namespace_aliases")]
    pub memory_namespace_aliases: HashMap<String, String>,
    /// Hillock manages hot tier cap and TTL now — kept for backward compat
    pub hot_tier_cap: u32,
    pub hot_ttl_hours: u32,
    /// Disabled — replaced by LLM extraction at compaction (Phase 3)
    pub auto_capture: bool,
    pub nightly_interval_hours: u32,
    pub min_score_threshold: f64,
    /// Hillock handles dedup now — kept for backward compat
    pub dedup: DedupBlock,
    /// Context token drop % that triggers compaction detection (0.0-1.0)
    #[serde(default = "default_context_drop_threshold")]
    pub context_drop_threshold: f64,
    /// Whether cognitive intake (per-turn passive memory extraction) is enabled
    #[serde(default = "default_true")]
    pub cognitive_intake_enabled: bool,
    /// Novelty gate similarity threshold (0.0-1.0). Content above this similarity
    /// to recent submissions is rejected as redundant.
    #[serde(default = "default_novelty_threshold")]
    pub cognitive_intake_novelty_threshold: f64,
    /// Number of recent embeddings to keep in the novelty ring buffer
    #[serde(default = "default_novelty_window")]
    pub cognitive_intake_novelty_window: u32,
}

fn default_true() -> bool {
    true
}

fn default_gate_cache_ttl() -> u64 {
    300
}
fn default_context_drop_threshold() -> f64 {
    0.50
}
fn default_novelty_threshold() -> f64 {
    0.85
}
fn default_novelty_window() -> u32 {
    10
}

fn default_namespace_aliases() -> HashMap<String, String> {
    HashMap::new()
}

impl Default for MemoryBlock {
    fn default() -> Self {
        Self {
            enabled: true,
            embedding_model: "BGEBaseENV15".to_string(),
            hillock_db_path: String::new(),
            hillock_master_key: String::new(),
            memory_namespace_aliases: default_namespace_aliases(),
            hot_tier_cap: 20,
            hot_ttl_hours: 24,
            auto_capture: false,
            nightly_interval_hours: 24,
            min_score_threshold: 0.3,
            dedup: DedupBlock::default(),
            context_drop_threshold: 0.50,
            cognitive_intake_enabled: true,
            cognitive_intake_novelty_threshold: 0.85,
            cognitive_intake_novelty_window: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DedupBlock {
    pub episode_threshold: f64,
    pub concept_threshold: f64,
    pub policy_threshold: f64,
}

impl Default for DedupBlock {
    fn default() -> Self {
        Self {
            episode_threshold: 0.92,
            concept_threshold: 0.95,
            policy_threshold: 0.97,
        }
    }
}

// --- Utility functions ---

/// Returns the platform-idiomatic base directory for Holt config/data.
/// Windows: %APPDATA%/holt/
/// Linux:   ~/.config/holt/
/// macOS:   ~/Library/Application Support/holt/
pub fn base_config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("holt")
}

/// Per-agent directory: ~/.config/holt/agents/{agent_id}/
pub fn agent_dir(agent_id: &str) -> PathBuf {
    base_config_dir().join("agents").join(agent_id)
}

/// Ensures the full directory structure exists under the base config dir.
pub fn ensure_directories() -> std::io::Result<()> {
    let base = base_config_dir();
    std::fs::create_dir_all(base.join("agents"))?;
    std::fs::create_dir_all(base.join("sessions").join("checkpoints"))?;
    std::fs::create_dir_all(base.join("plugins"))?;
    std::fs::create_dir_all(base.join("logs"))?;
    Ok(())
}

impl AppConfig {
    /// Load config from TOML file, or return defaults if the file doesn't exist.
    pub fn load(path: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let config_path = path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| base_config_dir().join("config.toml"));

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: AppConfig = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(AppConfig::default())
        }
    }

    /// Clamp config values to valid ranges after deserialization.
    pub fn validate(&mut self) {
        self.ui.glass_opacity = self.ui.glass_opacity.clamp(0.0, 1.0);
        self.ui.animation_duration_ms = self.ui.animation_duration_ms.max(50);
        self.network.request_timeout_seconds = self.network.request_timeout_seconds.max(5);
        self.network.stream_timeout_seconds = self.network.stream_timeout_seconds.max(10);
        self.network.retry_backoff_ms = self.network.retry_backoff_ms.max(100);
        self.context.response_reserve_tokens =
            self.context.response_reserve_tokens.clamp(1000, 200_000);
        self.context.char_ratio = self.context.char_ratio.clamp(1.0, 10.0);
        self.context.compaction_threshold_percent =
            self.context.compaction_threshold_percent.clamp(50, 100);
        self.limits.max_tool_calls_per_turn = self.limits.max_tool_calls_per_turn.max(1);
        self.limits.max_tool_calls_per_session = self.limits.max_tool_calls_per_session.max(1);
        self.limits.max_response_tokens = self.limits.max_response_tokens.clamp(256, 128_000);
        self.limits.response_timeout_seconds = self.limits.response_timeout_seconds.max(10);
        self.file_locking.max_hold_seconds = self.file_locking.max_hold_seconds.max(5);
        self.file_locking.stale_timeout_seconds = self.file_locking.stale_timeout_seconds.max(5);
        self.traces.retention_days = self.traces.retention_days.max(1);
        self.traces.max_db_size_mb = self.traces.max_db_size_mb.max(10);
        self.checkpoints.max_per_agent = self.checkpoints.max_per_agent.max(1);
        self.persistence.auto_save_interval_seconds =
            self.persistence.auto_save_interval_seconds.max(5);
        self.memory.hot_tier_cap = self.memory.hot_tier_cap.clamp(5, 100);
        self.memory.hot_ttl_hours = self.memory.hot_ttl_hours.max(1);
        self.memory.min_score_threshold = self.memory.min_score_threshold.clamp(0.0, 1.0);
    }

    /// Save config to TOML file.
    pub fn save(&self, path: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| base_config_dir().join("config.toml"));

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.app.theme, "dark");
        assert_eq!(config.app.default_agent_count, 3);
        assert_eq!(config.ui.glass_opacity, 0.65);
        assert_eq!(config.ui.animation_duration_ms, 250);
        assert_eq!(config.limits.max_tool_calls_per_turn, 25);
        assert_eq!(config.network.stream_timeout_seconds, 300);
        assert_eq!(config.context.char_ratio, 4.0);
        assert_eq!(config.context.compaction_threshold_percent, 80);
        assert_eq!(config.context.compaction_keep_recent, 5);
        assert_eq!(config.traces.retention_days, 90);
    }

    #[test]
    fn test_parse_full_config() {
        let toml_str = r#"
[app]
theme = "dark"
default_agent_count = 5
auto_detect_local_models = true
auto_detect_interval_seconds = 60
minimize_to_tray = false
offline_mode = true

[ui]
constellation_physics_enabled = false
glass_opacity = 0.8
power_user_mode = true

[persistence]
auto_save_interval_seconds = 15
restore_on_launch = false

[network]
request_timeout_seconds = 60
stream_timeout_seconds = 120

[context]
char_ratio = 3.5
compaction_threshold_percent = 70

[limits]
max_tool_calls_per_turn = 50
max_tool_calls_per_session = 500

[checkpoints]
max_per_agent = 20

[traces]
retention_days = 30
max_db_size_mb = 200

[communication]
a2a_default_enabled = true
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.app.default_agent_count, 5);
        assert!(config.app.offline_mode);
        assert!(!config.ui.constellation_physics_enabled);
        assert_eq!(config.ui.glass_opacity, 0.8);
        assert!(config.ui.power_user_mode);
        assert_eq!(config.persistence.auto_save_interval_seconds, 15);
        assert_eq!(config.network.request_timeout_seconds, 60);
        assert_eq!(config.context.char_ratio, 3.5);
        assert_eq!(config.limits.max_tool_calls_per_turn, 50);
        assert_eq!(config.checkpoints.max_per_agent, 20);
        assert_eq!(config.traces.retention_days, 30);
        assert!(config.communication.a2a_default_enabled);
    }

    #[test]
    fn test_parse_minimal_config() {
        // Only [app] block — everything else defaults
        let toml_str = r#"
[app]
theme = "dark"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.app.theme, "dark");
        // Defaults for unspecified sections
        assert_eq!(config.limits.max_tool_calls_per_turn, 25);
        assert_eq!(config.ui.glass_opacity, 0.65);
        assert_eq!(config.network.stream_timeout_seconds, 300);
    }

    #[test]
    fn test_round_trip() {
        let config = AppConfig::default();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: AppConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config.app.theme, deserialized.app.theme);
        assert_eq!(
            config.limits.max_tool_calls_per_turn,
            deserialized.limits.max_tool_calls_per_turn
        );
        assert_eq!(config.context.char_ratio, deserialized.context.char_ratio);
    }

    #[test]
    fn test_load_nonexistent_returns_defaults() {
        let config = AppConfig::load(Some(Path::new("/nonexistent/path/config.toml"))).unwrap();
        assert_eq!(config.app.theme, "dark");
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let mut config = AppConfig::default();
        config.app.default_agent_count = 7;
        config.save(Some(&path)).unwrap();

        let loaded = AppConfig::load(Some(&path)).unwrap();
        assert_eq!(loaded.app.default_agent_count, 7);
    }

    #[test]
    fn test_validate_clamps_values() {
        let mut config = AppConfig::default();
        config.ui.glass_opacity = 2.0;
        config.ui.animation_duration_ms = 0;
        config.limits.max_tool_calls_per_turn = 0;
        config.limits.max_response_tokens = 0;
        config.context.char_ratio = -1.0;
        config.context.compaction_threshold_percent = 200;
        config.network.stream_timeout_seconds = 0;
        config.traces.retention_days = 0;

        config.validate();

        assert_eq!(config.ui.glass_opacity, 1.0);
        assert_eq!(config.ui.animation_duration_ms, 50);
        assert_eq!(config.limits.max_tool_calls_per_turn, 1);
        assert_eq!(config.limits.max_response_tokens, 256);
        assert_eq!(config.context.char_ratio, 1.0);
        assert_eq!(config.context.compaction_threshold_percent, 100);
        assert_eq!(config.network.stream_timeout_seconds, 10);
        assert_eq!(config.traces.retention_days, 1);
    }

    #[test]
    fn test_parse_mcp_plugin_entry() {
        let toml_str = r#"
[plugins.registry.example-mcp]
enabled = true
plugin_type = "mcp"
command = "python3"
args = ["/home/user/example-mcp-shim.py"]

[plugins.registry.example-mcp.settings]
url = "http://localhost:9000"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        let entry = config.plugins.registry.get("example-mcp").unwrap();
        assert!(entry.enabled);
        assert_eq!(entry.plugin_type, PluginType::Mcp);
        assert_eq!(entry.command.as_deref(), Some("python3"));
        assert_eq!(entry.args.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_bash_escalation_block_defaults() {
        let block = BashEscalationBlock::default();
        assert_eq!(block.tier, "require_approval");
        assert!(!block.patterns.is_empty());
        assert!(block.patterns.contains(&"rm -rf".to_string()));
    }

    #[test]
    fn test_cron_block_defaults() {
        let toml_str = "";
        let block: CronBlock = toml::from_str(toml_str).expect("deserialize");
        assert!(block.enabled);
        assert_eq!(block.min_interval_seconds, 60);
        assert_eq!(block.max_jobs, 50);
    }

    #[test]
    fn test_parse_native_plugin_entry_default_type() {
        let toml_str = r#"
[plugins.registry.my-plugin]
enabled = true
binary = "my-plugin"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        let entry = config.plugins.registry.get("my-plugin").unwrap();
        assert_eq!(entry.plugin_type, PluginType::Native);
    }

    #[test]
    fn test_config_with_agent_sdk_block() {
        let toml_str = r#"
[agent_sdk]
enabled = true
model = "claude-opus-4-6"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert!(config.agent_sdk.enabled);
        assert_eq!(config.agent_sdk.model, "claude-opus-4-6");
    }

    #[test]
    fn test_config_old_claude_sdk_alias() {
        let toml_str = r#"
[claude_sdk]
enabled = true
node_path = "node"
model = "claude-opus-4-6"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert!(config.agent_sdk.enabled);
        assert_eq!(config.agent_sdk.node_path, "node");
    }

    #[test]
    fn test_config_without_agent_sdk_defaults() {
        let toml_str = r#"
[app]
theme = "dark"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert!(config.agent_sdk.enabled);
        assert_eq!(config.agent_sdk.model, "claude-opus-4-6");
    }
}
