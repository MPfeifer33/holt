pub mod commands;
pub mod config;
pub mod memory;
pub mod persistence;
pub mod plugin;
pub mod providers;
pub mod runtime;
pub mod security;
pub mod terminal;
pub mod tools;
pub mod trace;

use std::sync::Arc;
use tauri::Manager;
use tokio::sync::RwLock;

use crate::config::app_config::{base_config_dir, ensure_directories, AppConfig};
use crate::plugin::host::PluginHost;
use crate::runtime::a2a::A2ARegistry;
use crate::runtime::approval::ApprovalManager;
use crate::runtime::connection::AutoDetector;
use crate::runtime::rate_limiter::RateLimiter;
use crate::runtime::skills::SkillRegistry;
use crate::runtime::subagent::SubagentManager;
use crate::runtime::turn_manager::TurnManager;
use crate::security::keychain::KeychainManager;
use crate::tools::shell::ShellManager;
use crate::tools::vfl::VirtualFileLockRegistry;
use crate::trace::engine::TraceEngine;

/// Central application state, shared across all Tauri commands and background tasks.
pub struct AppState {
    pub agents: Arc<RwLock<Vec<runtime::agent::AgentSlot>>>,
    pub trace_engine: Arc<TraceEngine>,
    pub config: RwLock<AppConfig>,
    pub keychain: Arc<KeychainManager>,
    pub auto_detector: Arc<AutoDetector>,
    pub rate_limiter: RwLock<RateLimiter>,
    pub background_rate_limiter: RwLock<RateLimiter>,
    pub http_client: reqwest::Client,
    pub shell_manager: Arc<RwLock<ShellManager>>,
    pub approval_manager: Arc<ApprovalManager>,
    pub vfl_registry: Arc<VirtualFileLockRegistry>,
    pub subagent_manager: Arc<SubagentManager>,
    pub a2a_registry: Arc<A2ARegistry>,
    pub a2a_logger: Arc<runtime::a2a_logger::A2ALogger>,
    pub plugin_host: Arc<PluginHost>,
    pub skill_registry: Arc<SkillRegistry>,
    pub trigger_registry: Arc<crate::runtime::triggers::TriggerRegistry>,
    pub file_watch_manager: Arc<crate::runtime::file_watch::FileWatchManager>,
    pub memory_engine: std::sync::RwLock<Option<Arc<crate::memory::MemoryEngine>>>,
    pub session_state: Arc<crate::memory::session::SessionState>,
    pub cancellation_tokens:
        RwLock<std::collections::HashMap<String, tokio_util::sync::CancellationToken>>,
    pub turn_manager: Arc<TurnManager>,
    pub codex_mcp_url: RwLock<Option<String>>,
    pub shutdown_token: tokio_util::sync::CancellationToken,
    /// Last memory injection token count per agent (for panel display)
    pub last_injection_tokens: RwLock<std::collections::HashMap<String, usize>>,
    /// Per-agent mutexes serializing `claude_tui_start` so concurrent project
    /// switches for the same agent can't race on PTY spawn or `config.toml`
    /// writes. Lazily populated via `entry().or_insert_with`.
    pub claude_tui_locks: dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

impl AppState {
    /// Get the memory engine if available. Cheap (Arc clone = refcount bump).
    pub fn get_memory_engine(&self) -> Option<Arc<crate::memory::MemoryEngine>> {
        self.memory_engine.read().unwrap().clone()
    }

    /// Check if the memory engine is available.
    pub fn has_memory_engine(&self) -> bool {
        self.memory_engine.read().unwrap().is_some()
    }
}

/// System resource pressure info returned to frontend.
#[derive(serde::Serialize)]
pub struct SystemPressure {
    pub cpu_percent: f32,
    pub available_memory_mb: u64,
    pub total_memory_mb: u64,
}

/// Persistent system monitor — sysinfo needs a living System instance for accurate CPU readings.
static SYSTEM_MONITOR: std::sync::LazyLock<std::sync::Mutex<sysinfo::System>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(sysinfo::System::new()));

#[tauri::command]
fn get_system_pressure() -> SystemPressure {
    let mut sys = SYSTEM_MONITOR.lock().unwrap_or_else(|e| e.into_inner());
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    let cpu_percent = sys.global_cpu_usage();
    let available_memory_mb = sys.available_memory() / (1024 * 1024);
    let total_memory_mb = sys.total_memory() / (1024 * 1024);

    SystemPressure {
        cpu_percent,
        available_memory_mb,
        total_memory_mb,
    }
}

#[tauri::command]
fn window_minimize(window: tauri::WebviewWindow) {
    let _ = window.minimize();
}

#[tauri::command]
fn window_close(window: tauri::WebviewWindow) {
    let _ = window.close();
}

#[tauri::command]
fn window_toggle_fullscreen(window: tauri::WebviewWindow) {
    if let Ok(is_full) = window.is_fullscreen() {
        let _ = window.set_fullscreen(!is_full);
    }
}

/// Phase 1: Environment, logging, filesystem migrations.
/// Panics on critical directory creation failure.
fn init_infrastructure() {
    // Disable WebKit GPU compositing to prevent SIGABRT crashes from Mesa/WebKit
    // heap corruption on Linux (Fedora 43 + RDNA 4 + Mesa 25.3.6).
    // The Holt UI is 2D — no GPU acceleration needed.
    std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");

    // Initialize structured logging (controlled via RUST_LOG env var)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("holt_lib=info".parse().unwrap()),
        )
        .init();

    // Ensure directory structure exists
    ensure_directories().expect("Failed to create config directories");

    // Migrate flat agent files to per-agent directories (idempotent)
    if let Err(e) = persistence::migration::migrate_to_per_agent_dirs() {
        tracing::warn!("Agent directory migration failed: {e}");
    }
}

/// Phase 2: Load config, create trace DB, keychain, auto-detector.
/// Panics on config load failure or trace DB creation failure.
// If this return tuple grows past 4, consider a CoreServices struct.
fn init_core_services() -> (AppConfig, TraceEngine, KeychainManager, AutoDetector) {
    let mut config = AppConfig::load(None).expect("Failed to load config");
    config.validate();

    let trace_db_path = base_config_dir().join("traces.db");
    let trace_engine =
        TraceEngine::new(&trace_db_path).expect("Failed to initialize trace database");

    let keychain = KeychainManager::new();
    let auto_detector = AutoDetector::new();

    (config, trace_engine, keychain, auto_detector)
}

/// Migrate existing agents: if an agent has archetype_base but no [agent.traits],
/// resolve traits from the registry and persist them. Idempotent — skips agents
/// that already have traits. Logs but does not fail on errors.
fn migrate_agent_traits(
    agents_dir: &std::path::Path,
    configs: &mut [crate::config::agent_config::AgentConfigFile],
) {
    // Try to load registry once for all agents
    let registry = crate::runtime::persona_registry::resolve_registry_dir(None)
        .ok()
        .and_then(|dir| crate::runtime::persona_registry::load_registry(&dir).ok());

    for ac in configs.iter_mut() {
        // Skip if traits already populated
        if ac.agent.traits.is_some() {
            continue;
        }

        // Skip if no archetype to migrate from
        let base_id = match &ac.agent.archetype_base {
            Some(id) if !id.is_empty() => id.clone(),
            _ => continue,
        };

        let registry = match &registry {
            Some(r) => r,
            None => {
                tracing::warn!(
                    agent = ac.agent.id.as_str(),
                    "Cannot migrate traits — registry not available"
                );
                continue;
            }
        };

        let primary_base = registry.bases.get(base_id.as_str());
        let spec = ac
            .agent
            .archetype_specialization
            .as_deref()
            .and_then(|id| registry.specializations.get(id));

        let overrides = std::collections::HashMap::new();
        let traits = crate::runtime::persona_registry::resolve_traits(
            primary_base,
            None,
            1.0,
            spec,
            &overrides,
        );

        ac.agent.traits = Some(traits);

        // Persist to disk
        let config_path = agents_dir.join(&ac.agent.id).join("config.toml");
        if let Err(e) = ac.save(&config_path) {
            tracing::warn!(
                agent = ac.agent.id.as_str(),
                "Failed to persist migrated traits: {}",
                e
            );
        } else {
            tracing::info!(
                agent = ac.agent.id.as_str(),
                "Migrated archetype traits to [agent.traits] block"
            );
            // Log to changelog
            crate::runtime::persona::append_changelog(
                &ac.agent.id,
                "config.toml [agent.traits]",
                "Traits migrated from archetype IDs to resolved values",
                &crate::runtime::persona::ChangeInitiator::System("startup migration"),
            );
        }
    }
}

/// Phase 3: Load agent configs from disk, convert to AgentSlots, restore sessions.
/// Does not panic — malformed configs produce an empty list, restore failures are logged per-agent.
fn init_agents(config: &AppConfig) -> Vec<runtime::agent::AgentSlot> {
    let agents_dir = base_config_dir().join("agents");
    let mut agent_configs =
        crate::config::agent_config::AgentConfigFile::load_all(&agents_dir).unwrap_or_default();

    // Migration: resolve traits for existing agents that have archetype IDs but no traits block
    migrate_agent_traits(&agents_dir, &mut agent_configs);

    let mut agents = Vec::new();
    for ac in agent_configs {
        let model_name = match &ac.connection {
            crate::config::agent_config::ConnectionBlock::Api { model, .. } => model.clone(),
            crate::config::agent_config::ConnectionBlock::Local { model, .. } => {
                model.clone().unwrap_or_default()
            }
            crate::config::agent_config::ConnectionBlock::McpStdio { .. } => String::new(),
            crate::config::agent_config::ConnectionBlock::OAuth { model, .. } => model.clone(),
            crate::config::agent_config::ConnectionBlock::AgentSdk { model, .. } => model
                .clone()
                .unwrap_or_else(|| "claude-sonnet-4-6-20250725".to_string()),
        };

        let connection = match &ac.connection {
            crate::config::agent_config::ConnectionBlock::Api {
                endpoint,
                model,
                key_ref,
            } => runtime::connection::ConnectionConfig::Api {
                endpoint: endpoint.clone(),
                api_key_ref: key_ref.clone().unwrap_or_else(|| ac.agent.id.clone()),
                model: model.clone(),
            },
            crate::config::agent_config::ConnectionBlock::Local { host, port, model } => {
                runtime::connection::ConnectionConfig::Local {
                    host: host.clone(),
                    port: *port,
                    model: model.clone(),
                }
            }
            crate::config::agent_config::ConnectionBlock::McpStdio { command, args } => {
                runtime::connection::ConnectionConfig::McpStdio {
                    command: command.clone(),
                    args: args.clone(),
                    env: std::collections::HashMap::new(),
                }
            }
            crate::config::agent_config::ConnectionBlock::OAuth {
                provider,
                credentials_path,
                model,
            } => runtime::connection::ConnectionConfig::OAuth {
                provider: provider.clone(),
                credentials_path: credentials_path.clone(),
                model: model.clone(),
            },
            crate::config::agent_config::ConnectionBlock::AgentSdk { .. } => {
                runtime::connection::ConnectionConfig::AgentSdk
            }
        };

        let endpoint_hint = match &ac.connection {
            crate::config::agent_config::ConnectionBlock::Api { endpoint, .. } => {
                Some(endpoint.as_str())
            }
            crate::config::agent_config::ConnectionBlock::OAuth { .. } => {
                Some("https://api.anthropic.com")
            }
            _ => None,
        };
        let context_window_size = runtime::context::resolve_context_size_with_endpoint(
            ac.limits.context_tokens,
            &model_name,
            endpoint_hint,
            125_000,
            None,
        );

        let protocol = crate::runtime::protocol::preferred_protocol(&connection);

        agents.push(runtime::agent::AgentSlot {
            id: ac.agent.id,
            name: ac.agent.name,
            color: ac.agent.color,
            protected: ac.agent.protected,
            connection,
            protocol,
            working_directory: ac.workspace.working_directory,
            status: runtime::agent::AgentStatus::Idle,
            subagents: vec![],
            conversation: runtime::agent::ConversationHistory::default(),
            tools: vec![],
            system_prompt: ac.workspace.system_prompt,
            metadata: runtime::agent::AgentMetadata::default(),
            limits: runtime::limits::AgentLimits {
                max_tool_calls_per_turn: ac.limits.max_tool_calls_per_turn,
                max_tool_calls_per_session: ac.limits.max_tool_calls_per_session,
                response_timeout_seconds: ac.limits.response_timeout_seconds,
                max_response_tokens: ac.limits.max_response_tokens,
                max_tool_retries: ac.limits.max_tool_retries,
                ..Default::default()
            },
            context_window_size,
            context_tokens_override: ac.limits.context_tokens,
            context_probed: false,
            sandbox_config: ac.sandbox.clone(),
            sandbox: None,
            session_prompt_tokens: 0,
            session_completion_tokens: 0,
            current_context_tokens: 0,
            session_start: None,
            is_dirty: false,
            anthropic_settings: None,
            skills: vec![],
            user_notes: vec![],
            agent_session_id: None,
            system_prompt_hash: None,
            coordinator_mode: ac.coordinator_mode,
            autonomous: false,
            extraction_pending_done: false,
            provider: None,
            cached_params: None,
            multi_agent_model: false,
            archetype_base: None,
            archetype_specialization: None,
            session_memory: Default::default(),
            cognitive_intake: Default::default(),
            appearance: None,
            openai_responses_settings: None,
            xai_settings: None,
            gemini_settings: None,
            local_model_settings: None,
        });
    }

    if config.persistence.restore_on_launch {
        for agent in &mut agents {
            if let Err(e) = persistence::session::restore_agent_session(agent) {
                tracing::error!(agent_id = %agent.id, agent_name = %agent.name, "Failed to restore session: {e}");
            }
        }
    }

    agents
}

/// Phase 4: Initialize all subsystems and construct AppState.
/// Pure construction — no spawned tasks.
fn init_subsystems(
    config: AppConfig,
    trace_engine: TraceEngine,
    keychain: KeychainManager,
    auto_detector: AutoDetector,
    agents: Vec<runtime::agent::AgentSlot>,
) -> Arc<AppState> {
    let vfl_registry = Arc::new(VirtualFileLockRegistry::new(config.file_locking.clone()));

    let plugins_dir = base_config_dir().join(&config.plugins.directory);
    let max_ipc_payload = config.plugins.max_ipc_payload_bytes;
    let plugin_host = Arc::new(PluginHost::new(plugins_dir, max_ipc_payload));

    let skills_dir = base_config_dir().join("skills");
    std::fs::create_dir_all(&skills_dir).expect("Failed to create skills directory");
    if std::fs::read_dir(&skills_dir)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true)
    {
        seed_starter_skills(&skills_dir);
    }
    let skill_registry = Arc::new(SkillRegistry::new(skills_dir));

    let memory_engine = {
        let mem_config = config.memory.clone();
        std::sync::RwLock::new(
            tauri::async_runtime::block_on(crate::memory::MemoryEngine::try_init(&mem_config))
                .map(Arc::new),
        )
    };
    let session_state = Arc::new(crate::memory::session::SessionState::new(
        config.memory.context_drop_threshold,
    ));

    let trigger_registry = Arc::new(crate::runtime::triggers::TriggerRegistry::load(
        base_config_dir(),
    ));

    let a2a_default_enabled = config.communication.a2a_default_enabled;

    Arc::new(AppState {
        agents: Arc::new(RwLock::new(agents)),
        trace_engine: Arc::new(trace_engine),
        config: RwLock::new(config),
        keychain: Arc::new(keychain),
        auto_detector: Arc::new(auto_detector),
        rate_limiter: RwLock::new(RateLimiter::new()),
        background_rate_limiter: RwLock::new(RateLimiter::new()),
        http_client: reqwest::Client::new(),
        shell_manager: Arc::new(RwLock::new(ShellManager::new())),
        approval_manager: Arc::new(ApprovalManager::new()),
        vfl_registry,
        subagent_manager: Arc::new(SubagentManager::new()),
        a2a_registry: Arc::new(A2ARegistry::load(base_config_dir(), a2a_default_enabled)),
        a2a_logger: Arc::new(runtime::a2a_logger::A2ALogger::new()),
        plugin_host,
        skill_registry,
        trigger_registry,
        file_watch_manager: Arc::new(crate::runtime::file_watch::FileWatchManager::new()),
        memory_engine,
        session_state,
        cancellation_tokens: RwLock::new(std::collections::HashMap::new()),
        turn_manager: Arc::new(TurnManager::new()),
        codex_mcp_url: RwLock::new(None),
        shutdown_token: tokio_util::sync::CancellationToken::new(),
        last_injection_tokens: RwLock::new(std::collections::HashMap::new()),
        claude_tui_locks: dashmap::DashMap::new(),
    })
}

/// Phase 5: Spawn all background tasks.
/// Groups: recovery, agent lifecycle, scheduled maintenance, subsystem housekeeping.
fn start_background_tasks(state: Arc<AppState>, app_handle: tauri::AppHandle) {
    // Read config values for scheduled tasks
    let (auto_save_interval, plugin_registry, health_check_interval, retention_days) = {
        let config = state.config.blocking_read();
        (
            config.persistence.auto_save_interval_seconds as u64,
            config.plugins.registry.clone(),
            config.plugins.health_check_interval_seconds,
            config.traces.retention_days,
        )
    };

    crate::runtime::codex_mcp::spawn_server(state.clone(), app_handle.clone());

    // --- Recovery ---

    // Recover sandbox sessions from previous run
    {
        let shell_manager_clone = Arc::clone(&state.shell_manager);
        let agents_clone = Arc::clone(&state.agents);
        tauri::async_runtime::spawn(async move {
            let agent_ids: Vec<String> = {
                let agents_read = agents_clone.read().await;
                agents_read
                    .iter()
                    .filter(|a| a.sandbox.is_some())
                    .map(|a| a.id.clone())
                    .collect()
            };
            for id in agent_ids {
                runtime::sandbox::recover(&agents_clone, &id, &shell_manager_clone).await;
            }
        });
    }

    // Orphan worktree detection
    {
        let agents_clone = Arc::clone(&state.agents);
        tauri::async_runtime::spawn(async move {
            let agents_read = agents_clone.read().await;
            for agent in agents_read.iter() {
                if let Ok(entries) = std::fs::read_dir(&agent.working_directory) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with(".sandbox-") && entry.path().is_dir() {
                            if agent.sandbox.is_none() {
                                tracing::warn!(
                                    agent_id = %agent.id,
                                    path = %entry.path().display(),
                                    "Orphaned sandbox worktree detected — run 'git worktree prune' to clean up"
                                );
                            }
                        }
                    }
                }
            }
        });
    }

    // Spawn PTY sessions for all loaded agents
    {
        let pty_state = state.clone();
        tauri::async_runtime::spawn(async move {
            let agents = pty_state.agents.read().await;
            let mut sm = pty_state.shell_manager.write().await;
            for agent in agents.iter() {
                // Load agent config to get execution sandbox settings.
                // Existing agents without execution_sandbox default to unrestricted.
                let config_path =
                    crate::config::app_config::agent_dir(&agent.id).join("config.toml");
                let sandbox_config = if config_path.exists() {
                    crate::config::agent_config::AgentConfigFile::load(&config_path)
                        .map(|cfg| cfg.resolved_execution_sandbox())
                        .unwrap_or_else(|_| {
                            crate::config::agent_config::ExecutionSandboxBlock::legacy_default()
                        })
                } else {
                    crate::config::agent_config::ExecutionSandboxBlock::legacy_default()
                };

                if let Err(e) = sm
                    .spawn_session(
                        agent.id.clone(),
                        agent.working_directory.clone(),
                        &sandbox_config,
                        &agent.working_directory,
                    )
                    .await
                {
                    eprintln!("Failed to spawn PTY for agent {}: {}", agent.id, e);
                }
            }
        });
    }

    // Register TurnManager workers for all loaded agents
    {
        let tm_state = state.clone();
        let tm_app = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            let agent_ids: Vec<String> = {
                let agents = tm_state.agents.read().await;
                agents.iter().map(|a| a.id.clone()).collect()
            };
            for id in agent_ids {
                crate::commands::agent_commands::messaging::register_turn_worker(
                    &tm_state, &tm_app, &id,
                )
                .await;
            }
        });
    }

    // --- Scheduled maintenance ---

    // Auto-save loop
    {
        let save_state = state.clone();
        tauri::async_runtime::spawn(async move {
            auto_save_loop(save_state, auto_save_interval).await;
        });
    }

    // Trigger scheduler + file watch manager
    state
        .trigger_registry
        .spawn_scheduler(state.clone(), app_handle.clone());
    state
        .file_watch_manager
        .spawn(state.trigger_registry.clone(), state.clone(), app_handle);

    // Backfill persona watch for existing agents
    {
        let state_clone = state.clone();
        tauri::async_runtime::spawn(async move {
            let agents_r = state_clone.agents.read().await;
            for agent in agents_r.iter() {
                let is_sdk = matches!(
                    agent.connection,
                    crate::runtime::connection::ConnectionConfig::AgentSdk
                );
                if !is_sdk {
                    let has_persona_watch = state_clone
                        .trigger_registry
                        .file_watch_jobs_for_agent(&agent.id)
                        .await
                        .iter()
                        .any(|j| j.name == "persona-watch");
                    if !has_persona_watch {
                        let persona_dir = base_config_dir()
                            .join("agents")
                            .join(&agent.id)
                            .join("persona");
                        if persona_dir.exists() {
                            let watch_config = crate::runtime::triggers::WatchConfig {
                                path: persona_dir,
                                recursive: true,
                                glob_filter: None,
                                events: vec![
                                    crate::runtime::triggers::WatchEventType::Create,
                                    crate::runtime::triggers::WatchEventType::Modify,
                                    crate::runtime::triggers::WatchEventType::Delete,
                                ],
                            };
                            let _ = state_clone
                                .trigger_registry
                                .create_file_watch(
                                    "persona-watch".to_string(),
                                    agent.id.clone(),
                                    watch_config,
                                    true,
                                )
                                .await;
                            tracing::info!(agent_id = %agent.id, "Backfilled persona watch");
                        }
                    }
                }
            }
        });
    }

    // Memory engine lazy-init retry — if bridge wasn't reachable at startup,
    // keep retrying every 30s until it becomes available.
    if !state.has_memory_engine() {
        let retry_state = state.clone();
        let shutdown = state.shutdown_token.clone();
        tauri::async_runtime::spawn(async move {
            let retry_interval = std::time::Duration::from_secs(30);
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        tracing::info!("Memory engine retry task shutting down");
                        break;
                    }
                    _ = tokio::time::sleep(retry_interval) => {
                        if retry_state.has_memory_engine() {
                            tracing::info!("Memory engine now available, stopping retry task");
                            break;
                        }
                        let config = retry_state.config.read().await;
                        let mem_config = config.memory.clone();
                        drop(config);
                        if let Some(engine) = crate::memory::MemoryEngine::try_init(&mem_config).await {
                            *retry_state.memory_engine.write().unwrap() = Some(Arc::new(engine));
                            tracing::info!("Memory engine connected on retry — bridge is now available");
                            break;
                        }
                        tracing::debug!("Memory engine retry: bridge still not reachable");
                    }
                }
            }
        });
    }

    // Trace retention cleanup
    if retention_days > 0 {
        let te = state.trace_engine.clone();
        let days = retention_days;
        let shutdown = state.shutdown_token.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        tracing::info!("Trace retention task shutting down");
                        break;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(86400)) => {
                        let _ = te.cleanup_retention(days);
                    }
                }
            }
        });
    }

    // Trace WAL checkpoint — traces are written in WAL mode for concurrent
    // read/write performance, but reads see stale data until the WAL is
    // checkpointed. Run a PASSIVE checkpoint every 2s so TraceTile (3s poll)
    // and ActivityTile backfill see recent writes without waiting for
    // shutdown's flush_wal.
    {
        let te = state.trace_engine.clone();
        let shutdown = state.shutdown_token.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
            // Skip the immediate first tick — nothing to checkpoint yet
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        tracing::info!("Trace WAL checkpoint task shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        if let Err(e) = te.checkpoint() {
                            tracing::warn!("Trace WAL checkpoint failed: {e}");
                        }
                    }
                }
            }
        });
    }

    // --- Subsystem housekeeping ---

    // VFL stale lock reaper
    state
        .vfl_registry
        .spawn_reaper(state.shutdown_token.clone());

    // Plugin initialization
    {
        let plugin_host_init = state.plugin_host.clone();
        tauri::async_runtime::spawn(async move {
            let (ok, failed) = plugin_host_init.init_all(&plugin_registry).await;
            if failed > 0 {
                tracing::error!("Plugin init: {ok} succeeded, {failed} failed");
            } else if ok > 0 {
                tracing::info!("Plugin init: {ok} plugins initialized");
            }
        });
    }

    // Plugin health checks
    if health_check_interval > 0 {
        let ph = state.plugin_host.clone();
        let shutdown = state.shutdown_token.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(health_check_interval as u64));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        tracing::info!("Plugin health check task shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        let plugins = ph.list_plugins().await;
                        for info in &plugins {
                            ph.health_check(&info.id).await;
                        }
                    }
                }
            }
        });
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_infrastructure();
    let (config, trace_engine, keychain, auto_detector) = init_core_services();
    let agents = init_agents(&config);
    let state = init_subsystems(config, trace_engine, keychain, auto_detector, agents);

    let tray_state = state.clone();

    let terminal_manager = Arc::new(terminal::manager::TerminalManager::new(4));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(state.clone())
        .manage(terminal_manager)
        .setup(move |app| {
            setup_tray(app, tray_state)?;
            start_background_tasks(state, app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::agent_commands::lifecycle::create_agent,
            commands::agent_commands::lifecycle::remove_agent,
            commands::agent_commands::lifecycle::list_agents,
            commands::agent_commands::lifecycle::store_api_key,
            commands::agent_commands::lifecycle::scan_local_models,
            commands::agent_commands::lifecycle::update_agent_traits,
            commands::agent_commands::lifecycle::preview_cognitive_profile,
            commands::agent_commands::lifecycle::get_persona_changelog,
            commands::agent_commands::messaging::get_conversation,
            commands::agent_commands::messaging::send_message,
            commands::agent_commands::messaging::run_codex_review,
            commands::agent_commands::messaging::trigger_agent_turn,
            commands::agent_commands::messaging::check_vision_support,
            commands::agent_commands::messaging::respond_to_approval,
            commands::agent_commands::messaging::respond_to_hil,
            commands::agent_commands::messaging::interrupt_agent,
            commands::agent_commands::messaging::tag_message_outcome,
            commands::agent_commands::messaging::list_subagents,
            commands::agent_commands::messaging::cancel_subagent,
            // Codex thread operations
            commands::agent_commands::codex_thread::codex_switch_model,
            commands::agent_commands::codex_thread::codex_set_effort,
            commands::agent_commands::codex_thread::codex_set_approval,
            commands::agent_commands::codex_thread::codex_set_goal,
            commands::agent_commands::codex_thread::codex_fork_thread,
            commands::agent_commands::codex_thread::codex_rollback,
            commands::agent_commands::codex_thread::codex_shell_command,
            commands::agent_commands::a2a::set_a2a_visibility,
            commands::agent_commands::a2a::approve_a2a_collaboration,
            commands::agent_commands::a2a::get_a2a_visibility,
            commands::agent_commands::a2a::list_a2a_pairs,
            commands::agent_commands::a2a::update_orbital_membership,
            commands::agent_commands::a2a::rebuild_all_orbital_membership,
            commands::agent_commands::messaging::broadcast_message,
            // Orbital layout persistence
            commands::agent_commands::a2a::save_orbital_layout,
            commands::agent_commands::a2a::load_orbital_layout,
            // Workspace root
            commands::workspace_commands::get_workspace_root,
            commands::workspace_commands::set_workspace_root,
            commands::workspace_commands::list_directory,
            commands::workspace_commands::read_file_preview,
            commands::workspace_commands::pick_folder,
            // Connections
            commands::connection_commands::list_connections,
            commands::connection_commands::add_cloud_api,
            commands::connection_commands::test_connection,
            commands::connection_commands::remove_connection,
            commands::connection_commands::delete_api_key,
            // Agent CRUD
            commands::agent_commands::lifecycle::get_agent_details,
            commands::agent_commands::lifecycle::update_agent,
            // Settings
            commands::settings_commands::update_ui_settings,
            commands::settings_commands::update_system_settings,
            commands::settings_commands::update_trace_settings,
            // Keybindings
            commands::settings_commands::get_keybindings,
            commands::settings_commands::update_keybinding,
            commands::settings_commands::reset_keybindings,
            // App settings
            commands::settings_commands::get_app_settings,
            // Web & Search
            commands::settings_commands::get_search_config,
            commands::settings_commands::update_search_config,
            commands::settings_commands::update_fetch_settings,
            // Traces management
            commands::trace_commands::export_traces,
            commands::trace_commands::clear_traces,
            // Chat management
            commands::agent_commands::lifecycle::clear_conversation,
            // Context compaction
            commands::agent_commands::context::get_context_status,
            commands::agent_commands::context::compact_agent_context,
            // Checkpoints
            commands::checkpoint_commands::create_checkpoint,
            commands::checkpoint_commands::list_checkpoints,
            commands::checkpoint_commands::restore_checkpoint,
            commands::checkpoint_commands::delete_checkpoint,
            commands::checkpoint_commands::extend_limit,
            commands::checkpoint_commands::reset_limit,
            // Trace queries
            commands::trace_commands::get_ui_config,
            commands::trace_commands::query_recent_traces,
            // Plugins
            commands::plugin_commands::list_plugins,
            commands::plugin_commands::enable_plugin,
            commands::plugin_commands::disable_plugin,
            commands::plugin_commands::restart_plugin,
            // Usage & Pricing
            commands::usage_commands::get_agent_usage,
            commands::usage_commands::get_all_usage,
            commands::usage_commands::get_daily_usage,
            commands::usage_commands::get_pricing_config,
            commands::settings_commands::update_anthropic_settings,
            commands::settings_commands::add_user_note,
            commands::settings_commands::clear_user_notes,
            // Memory
            commands::memory_commands::get_memory_stats,
            commands::memory_commands::get_memory_entries,
            commands::memory_commands::search_memories,
            // Contradictions
            // Attention
            commands::memory_commands::acknowledge_alert,
            // Agent SDK
            commands::connection_commands::is_claude_code_available,
            // Skills
            commands::skill_commands::list_skills,
            commands::skill_commands::get_skill,
            commands::skill_commands::create_skill,
            commands::skill_commands::update_skill,
            commands::skill_commands::delete_skill,
            commands::skill_commands::assign_skill_to_agent,
            commands::skill_commands::unassign_skill_from_agent,
            commands::skill_commands::get_active_skills,
            commands::skill_commands::get_skill_file,
            commands::skill_commands::write_skill_file,
            commands::skill_commands::delete_skill_file,
            commands::skill_commands::create_skill_directory,
            commands::skill_commands::fetch_skill_preview,
            commands::skill_commands::install_imported_skill,
            // Cron Jobs
            commands::trigger_commands::create_trigger,
            commands::trigger_commands::list_triggers,
            commands::trigger_commands::get_trigger,
            commands::trigger_commands::update_trigger,
            commands::trigger_commands::delete_trigger,
            commands::trigger_commands::toggle_trigger,
            commands::trigger_commands::fire_trigger,
            // Agent Appearance
            commands::appearance_commands::get_agent_appearance,
            commands::appearance_commands::update_agent_appearance,
            // Persona templates & archetypes
            commands::persona_commands::list_persona_templates,
            commands::persona_commands::list_archetypes,
            // Window controls
            window_minimize,
            window_close,
            window_toggle_fullscreen,
            // Terminal
            terminal::commands::terminal_create,
            terminal::commands::terminal_destroy,
            terminal::commands::terminal_write,
            terminal::commands::terminal_resize,
            terminal::commands::terminal_list,
            // Claude TUI (per-agent spawn with dynamic MCP config)
            commands::claude_tui_commands::claude_tui_start,
            // TUI project switcher
            commands::agent_project_commands::list_projects,
            commands::agent_project_commands::add_project,
            commands::agent_project_commands::remove_project,
            commands::dialog_commands::pick_directory,
            // System
            get_system_pressure,
            commands::agent_commands::misc::get_app_metadata,
            commands::agent_commands::context::trigger_extraction,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            match event {
                tauri::RunEvent::Exit => {
                    // Shutdown sequence:
                    // 1. Shut down all plugins (on a dedicated thread to avoid nested runtime panic)
                    // 2. Save all agent sessions
                    // 3. Flush trace DB WAL
                    run_shutdown(app_handle);
                }
                tauri::RunEvent::WindowEvent {
                    event: tauri::WindowEvent::CloseRequested { api, .. },
                    ..
                } => {
                    // Prevent the default WebKit close (which crashes on Linux)
                    api.prevent_close();
                    // Run clean shutdown then exit via Tauri (not process::exit which skips destructors)
                    run_shutdown(app_handle);
                    app_handle.exit(0);
                }
                _ => {}
            }
        });
}

/// Write bundled starter skill files into the skills directory on first launch.
fn seed_starter_skills(skills_dir: &std::path::Path) {
    let starters = [
        ("tdd", include_str!("starter_skills/tdd.md")),
        ("code-review", include_str!("starter_skills/code-review.md")),
        ("safe-shell", include_str!("starter_skills/safe-shell.md")),
    ];
    for (name, content) in &starters {
        let path = skills_dir.join(format!("{}.md", name));
        if let Err(e) = std::fs::write(&path, content) {
            tracing::warn!("Failed to write starter skill {}: {}", name, e);
        } else {
            tracing::info!("Seeded starter skill: {}.md", name);
        }
    }
}

fn run_shutdown(app_handle: &tauri::AppHandle) {
    let state: &Arc<AppState> = app_handle.state::<Arc<AppState>>().inner();

    tracing::info!("Graceful shutdown initiated");

    // Signal all background tasks to exit
    state.shutdown_token.cancel();

    // Plugin shutdown — uses try_write() internally to avoid deadlock.
    // tokio::sync::RwLock cannot be awaited from a different runtime, so
    // try_shutdown_sync() uses the non-blocking try_write() path.
    tracing::info!("Shutdown: plugin cleanup starting");
    if state.plugin_host.try_shutdown_sync() {
        tracing::info!("Shutdown: plugin cleanup done");
    } else {
        tracing::warn!("Shutdown: could not acquire plugin lock — skipping plugin cleanup");
    }

    // Save agent sessions — use try_read to avoid deadlock
    tracing::info!("Shutdown: saving agent sessions");
    match state.agents.try_read() {
        Ok(agents) => match persistence::session::save_all_agents(&agents) {
            Ok(count) => tracing::info!("Shutdown: saved {count} agent sessions"),
            Err(e) => tracing::error!("Shutdown: failed to save agent sessions: {e}"),
        },
        Err(_) => {
            tracing::warn!("Shutdown: could not acquire agents lock — skipping session save");
        }
    }

    // Destroy all PTY sessions — use synchronous SIGKILL to avoid block_on deadlock.
    // block_on + tokio::time::sleep inside the same runtime causes a hang.
    tracing::info!("Shutdown: destroying PTY sessions");
    match state.shell_manager.try_write() {
        Ok(mut sm) => {
            for (id, session) in sm.sessions.drain() {
                let raw_pid = session.pid() as i32;
                let pid = nix::unistd::Pid::from_raw(raw_pid);
                let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
                tracing::info!(agent_id = %id, pid = raw_pid, "Shutdown: killed PTY");
            }
            sm.processes.clear();
            tracing::info!("Shutdown: PTY sessions destroyed");
        }
        Err(_) => {
            tracing::warn!("Shutdown: could not acquire shell_manager lock — skipping PTY cleanup");
        }
    }

    // Shut down Agent SDK bridges — run on a dedicated thread to avoid
    // nested runtime panic (block_on cannot be called within tokio runtime)
    tracing::info!("Shutdown: stopping Agent SDK bridges");
    let shutdown_thread = std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("shutdown runtime");
        rt.block_on(crate::runtime::agent_sdk::shutdown_all_bridges());
    });
    if let Err(e) = shutdown_thread.join() {
        tracing::error!(
            "Shutdown: Agent SDK bridge shutdown thread panicked: {:?}",
            e
        );
    } else {
        tracing::info!("Shutdown: Agent SDK bridges stopped");
    }

    tracing::info!("Shutdown: flushing trace WAL");
    let _ = state.trace_engine.flush_wal();

    tracing::info!("Shutdown complete");
}

/// Set up the system tray icon with tooltip and context menu.
fn setup_tray(
    app: &mut tauri::App,
    state: Arc<AppState>,
) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::TrayIconBuilder;

    let show_item = MenuItemBuilder::with_id("show", "Show Holt").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .separator()
        .item(&quit_item)
        .build()?;

    let tray = TrayIconBuilder::new()
        .tooltip("Holt — 0 agents active")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    // Spawn background task to update tray tooltip every 5 seconds
    let tray_id = tray.id().clone();
    let app_handle = app.handle().clone();
    let shutdown = state.shutdown_token.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!("Tray tooltip update task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    let agents = state.agents.read().await;
                    let active_count = agents
                        .iter()
                        .filter(|a| !matches!(a.status, runtime::agent::AgentStatus::Idle))
                        .count();
                    let total_count = agents.len();
                    drop(agents);

                    let tooltip = if active_count > 0 {
                        format!("Holt — {} active / {} agents", active_count, total_count)
                    } else {
                        format!("Holt — {} agents", total_count)
                    };

                    if let Some(tray) = app_handle.tray_by_id(&tray_id) {
                        let _ = tray.set_tooltip(Some(&tooltip));
                    }
                }
            }
        }
    });

    Ok(())
}

/// Background auto-save loop. Saves dirty agents every `interval_seconds`.
async fn auto_save_loop(state: Arc<AppState>, interval_seconds: u64) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_seconds));
    let shutdown = state.shutdown_token.clone();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                save_dirty_agents(&state).await;
                break;
            }
            _ = interval.tick() => {
                save_dirty_agents(&state).await;
            }
        }
    }
}

async fn save_dirty_agents(state: &Arc<AppState>) {
    // Snapshot dirty agents under read lock (non-blocking for agent operations)
    let dirty_ids: Vec<String> = {
        let agents = state.agents.read().await;
        agents
            .iter()
            .filter(|a| a.is_dirty)
            .map(|a| a.id.clone())
            .collect()
    };

    if dirty_ids.is_empty() {
        return;
    }

    // Clone data and clear dirty flags atomically under the same write lock.
    // Any mutation that arrives after we release the lock will re-set is_dirty
    // and be picked up in the next cycle.
    let snapshots: Vec<_> = {
        let mut agents = state.agents.write().await;
        dirty_ids
            .iter()
            .filter_map(|id| {
                if let Some(agent) = agents.iter_mut().find(|a| a.id == *id) {
                    agent.is_dirty = false; // clear while still holding the lock
                    Some(agent.clone())
                } else {
                    None
                }
            })
            .collect()
    };

    // Write to disk outside the lock — if save fails, re-dirty the agent
    // so it will be retried on the next auto-save cycle.
    let mut failed_ids = Vec::new();
    for agent in &snapshots {
        if let Err(e) = persistence::session::save_agent_session(agent) {
            tracing::error!(agent_id = %agent.id, error = %e, "Auto-save failed for agent — will retry next cycle");
            failed_ids.push(agent.id.clone());
        }
    }

    // Re-dirty any agents that failed to save
    if !failed_ids.is_empty() {
        let mut agents = state.agents.write().await;
        for id in &failed_ids {
            if let Some(agent) = agents.iter_mut().find(|a| a.id == *id) {
                agent.is_dirty = true;
            }
        }
    }
}
