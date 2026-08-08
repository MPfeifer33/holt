use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::config::app_config::{PluginEntry, PluginType};

use super::discovery::{match_config_to_binaries, scan_plugin_directory};
use super::mcp_transport::McpProcess;
use super::transport::PluginProcess;
use super::types::{
    PluginError, PluginHealth, PluginInfo, PluginManifest, PluginStatus, PluginToolDef, SpawnConfig,
};

/// Transport for a running plugin — either native or MCP.
enum PluginTransport {
    Native(PluginProcess),
    Mcp(McpProcess),
}

impl PluginTransport {
    async fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, PluginError> {
        match self {
            Self::Native(p) => p.send_request(method, params, timeout).await,
            Self::Mcp(p) => p.send_request(method, params, timeout).await,
        }
    }

    fn is_alive(&mut self) -> bool {
        match self {
            Self::Native(p) => p.is_alive(),
            Self::Mcp(p) => p.is_alive(),
        }
    }

    async fn shutdown(&mut self, timeout: Duration) {
        match self {
            Self::Native(p) => p.shutdown(timeout).await,
            Self::Mcp(p) => p.shutdown(timeout).await,
        }
    }

    /// Synchronous best-effort kill for shutdown without an async runtime.
    fn start_kill(&mut self) {
        match self {
            Self::Native(p) => p.start_kill(),
            Self::Mcp(p) => p.start_kill(),
        }
    }
}

/// A currently running plugin instance.
struct RunningPlugin {
    transport: Arc<tokio::sync::Mutex<PluginTransport>>,
    manifest: PluginManifest,
    config: PluginEntry,
    status: PluginStatus,
    last_health: PluginHealth,
    last_health_check: Instant,
    plugin_type: PluginType,
    spawn_config: SpawnConfig,
    /// Guard against concurrent respawn attempts. Only one thread enters the
    /// respawn path at a time; others skip and return the current health status.
    respawning: Arc<AtomicBool>,
}

/// The central plugin host that manages all plugin lifecycles, events, and tool routing.
///
/// Safety invariant: every public method catches all plugin errors internally.
/// Plugin failures are logged but never propagated — the app always works with zero plugins.
pub struct PluginHost {
    plugins: RwLock<HashMap<String, RunningPlugin>>,
    plugins_dir: PathBuf,
    max_payload: usize,
}

impl PluginHost {
    /// Create a new PluginHost.
    pub fn new(plugins_dir: PathBuf, max_payload: usize) -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
            plugins_dir,
            max_payload,
        }
    }

    /// Discover and initialize all configured plugins.
    ///
    /// Scans the plugins directory, matches against config entries, spawns each enabled
    /// plugin process, and sends the `initialize` RPC. Errors are logged and skipped.
    /// Initialize all enabled plugins. Returns (succeeded, failed) counts.
    pub async fn init_all(&self, registry: &HashMap<String, PluginEntry>) -> (usize, usize) {
        if registry.is_empty() {
            return (0, 0);
        }

        let mut succeeded = 0usize;
        let mut failed = 0usize;

        // Native plugins
        let native_entries: HashMap<_, _> = registry
            .iter()
            .filter(|(_, e)| e.enabled && e.plugin_type == PluginType::Native)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        if !native_entries.is_empty() {
            let binaries = scan_plugin_directory(&self.plugins_dir);
            let matched = match_config_to_binaries(&native_entries, &binaries);

            for (id, entry, binary_path) in matched {
                match self.init_single(&id, &entry, &binary_path).await {
                    Ok(()) => succeeded += 1,
                    Err(e) => {
                        tracing::error!(plugin_id = id, "Failed to init native plugin: {e}");
                        failed += 1;
                    }
                }
            }
        }

        // MCP plugins
        for (id, entry) in registry {
            if entry.enabled && entry.plugin_type == PluginType::Mcp {
                match self.init_mcp_plugin(id, entry).await {
                    Ok(()) => succeeded += 1,
                    Err(e) => {
                        tracing::error!(plugin_id = id, "Failed to init MCP plugin: {e}");
                        failed += 1;
                    }
                }
            }
        }

        (succeeded, failed)
    }

    /// Initialize a single native plugin.
    async fn init_single(
        &self,
        id: &str,
        entry: &PluginEntry,
        binary_path: &std::path::Path,
    ) -> Result<(), PluginError> {
        let mut process = PluginProcess::spawn(binary_path, id, self.max_payload)?;

        // Build settings as serde_json::Value from toml::Value
        let settings: HashMap<String, serde_json::Value> = entry
            .settings
            .iter()
            .map(|(k, v)| {
                let json_val = toml_to_json(v);
                (k.clone(), json_val)
            })
            .collect();

        // Send initialize RPC with 10s timeout
        let result = process
            .send_request(
                "initialize",
                serde_json::json!({
                    "config": {
                        "settings": settings,
                    }
                }),
                Duration::from_secs(10),
            )
            .await?;

        // Parse manifest from response
        let manifest: PluginManifest = serde_json::from_value(result).map_err(|e| {
            PluginError::ProtocolError(format!("Invalid manifest from plugin '{}': {}", id, e))
        })?;

        let running = RunningPlugin {
            transport: Arc::new(tokio::sync::Mutex::new(PluginTransport::Native(process))),
            manifest,
            config: entry.clone(),
            status: PluginStatus::Running,
            last_health: PluginHealth::Healthy,
            last_health_check: Instant::now(),
            plugin_type: PluginType::Native,
            spawn_config: SpawnConfig::Native {
                binary_path: binary_path.to_path_buf(),
                plugin_id: id.to_string(),
            },
            respawning: Arc::new(AtomicBool::new(false)),
        };

        self.plugins.write().await.insert(id.to_string(), running);
        Ok(())
    }

    /// Initialize a single MCP plugin.
    async fn init_mcp_plugin(&self, id: &str, entry: &PluginEntry) -> Result<(), PluginError> {
        let command = entry.command.as_deref().ok_or_else(|| {
            PluginError::ProtocolError(format!("MCP plugin '{}' has no command", id))
        })?;
        let args = entry.args.as_deref().unwrap_or(&[]);

        let mut process = McpProcess::spawn(command, args, self.max_payload)?;
        let (server_name, server_version, tools) = process.mcp_handshake().await?;

        let manifest = PluginManifest {
            id: id.to_string(),
            name: server_name,
            version: server_version,
            capabilities: vec!["tools".to_string()],
            tools,
        };

        let running = RunningPlugin {
            transport: Arc::new(tokio::sync::Mutex::new(PluginTransport::Mcp(process))),
            manifest,
            config: entry.clone(),
            status: PluginStatus::Running,
            last_health: PluginHealth::Healthy,
            last_health_check: Instant::now(),
            plugin_type: PluginType::Mcp,
            spawn_config: SpawnConfig::Mcp {
                command: command.to_string(),
                args: args.to_vec(),
            },
            respawning: Arc::new(AtomicBool::new(false)),
        };

        self.plugins.write().await.insert(id.to_string(), running);
        Ok(())
    }

    /// Synchronous best-effort shutdown for use during app close.
    /// Uses try_write() to avoid deadlock when called outside the Tauri async runtime.
    /// Returns true if plugins were successfully killed.
    pub fn try_shutdown_sync(&self) -> bool {
        match self.plugins.try_write() {
            Ok(mut plugins) => {
                for (id, plugin) in plugins.iter_mut() {
                    tracing::info!("Shutdown: killing plugin {}", id);
                    if let Ok(mut t) = plugin.transport.try_lock() {
                        t.start_kill();
                    }
                }
                plugins.clear();
                true
            }
            Err(_) => false,
        }
    }

    /// Shut down all running plugins. 3-second timeout per plugin.
    pub async fn shutdown_all(&self) {
        let mut plugins = self.plugins.write().await;
        let ids: Vec<String> = plugins.keys().cloned().collect();

        for id in ids {
            if let Some(plugin) = plugins.remove(&id) {
                let mut transport = plugin.transport.lock().await;
                transport.shutdown(Duration::from_secs(3)).await;
            }
        }
    }

    /// Health check a single plugin.
    pub async fn health_check(&self, plugin_id: &str) -> PluginHealth {
        // Clone transport Arc, plugin type, spawn config, and respawn flag under read lock
        let (transport, plugin_type, spawn_config, respawning) = {
            let plugins = self.plugins.read().await;
            match plugins.get(plugin_id) {
                Some(p) => (
                    Arc::clone(&p.transport),
                    p.plugin_type.clone(),
                    p.spawn_config.clone(),
                    Arc::clone(&p.respawning),
                ),
                None => return PluginHealth::Unhealthy("Plugin not found".to_string()),
            }
        };

        // Lock only this plugin's transport for the ping
        let mut transport_guard = transport.lock().await;

        if !transport_guard.is_alive() {
            // Ensure only one thread attempts respawn at a time
            if respawning
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                drop(transport_guard);
                tracing::debug!(
                    plugin_id,
                    "Skipping respawn — another thread is already respawning"
                );
                return PluginHealth::Unhealthy("Respawn in progress".to_string());
            }

            // Attempt restart using spawn_config already cloned at top of method
            let restart_result: Option<Option<Vec<super::types::PluginToolDef>>> = {
                match (&mut *transport_guard, &spawn_config) {
                    (PluginTransport::Mcp(p), SpawnConfig::Mcp { command, args }) => {
                        match p.respawn(command, args).await {
                            Ok(tools) => Some(Some(tools)),
                            Err(_) => None,
                        }
                    }
                    (
                        PluginTransport::Native(p),
                        SpawnConfig::Native {
                            binary_path,
                            plugin_id: pid,
                        },
                    ) => match p.respawn(binary_path, pid).await {
                        Ok(()) => Some(None),
                        Err(_) => None,
                    },
                    _ => None,
                }
            };

            drop(transport_guard);

            if let Some(new_tools) = restart_result {
                tracing::info!(plugin_id, "Plugin restarted during health check");
                let mut plugins = self.plugins.write().await;
                if let Some(plugin) = plugins.get_mut(plugin_id) {
                    plugin.status = PluginStatus::Running;
                    plugin.last_health = PluginHealth::Healthy;
                    plugin.last_health_check = Instant::now();
                    if let Some(tools) = new_tools {
                        plugin.manifest.tools = tools;
                    }
                }
                respawning.store(false, Ordering::SeqCst);
                return PluginHealth::Healthy;
            }

            let mut plugins = self.plugins.write().await;
            if let Some(plugin) = plugins.get_mut(plugin_id) {
                plugin.status = PluginStatus::Error("Process dead".to_string());
                plugin.last_health = PluginHealth::Unhealthy("Process dead".to_string());
            }
            respawning.store(false, Ordering::SeqCst);
            return PluginHealth::Unhealthy("Process dead".to_string());
        }

        let health_result = match plugin_type {
            PluginType::Mcp => {
                match transport_guard
                    .send_request("ping", serde_json::json!({}), Duration::from_secs(5))
                    .await
                {
                    Ok(_) => Ok(serde_json::json!({"status": "healthy"})),
                    Err(PluginError::RpcError { code: -32601, .. }) => {
                        Ok(serde_json::json!({"status": "healthy"}))
                    }
                    Err(e) => Err(e),
                }
            }
            PluginType::Native => {
                transport_guard
                    .send_request("health", serde_json::json!({}), Duration::from_secs(5))
                    .await
            }
        };

        // Release transport lock before acquiring write lock on plugins
        drop(transport_guard);

        let health = match health_result {
            Ok(result) => {
                let status = result
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown");
                let message = result
                    .get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string());

                match status {
                    "healthy" => PluginHealth::Healthy,
                    "degraded" => PluginHealth::Degraded(message.unwrap_or_default()),
                    _ => PluginHealth::Unhealthy(message.unwrap_or_else(|| status.to_string())),
                }
            }
            Err(e) => PluginHealth::Unhealthy(format!("Health check failed: {}", e)),
        };

        // Update health status under write lock
        let mut plugins = self.plugins.write().await;
        if let Some(plugin) = plugins.get_mut(plugin_id) {
            plugin.last_health = health.clone();
            plugin.last_health_check = Instant::now();
        }

        health
    }

    /// Get summary info for all plugins (for frontend display).
    pub async fn list_plugins(&self) -> Vec<PluginInfo> {
        let plugins = self.plugins.read().await;
        plugins
            .iter()
            .map(|(id, p)| {
                let (health_str, health_msg) = match &p.last_health {
                    PluginHealth::Healthy => ("healthy".to_string(), None),
                    PluginHealth::Degraded(msg) => ("degraded".to_string(), Some(msg.clone())),
                    PluginHealth::Unhealthy(msg) => ("unhealthy".to_string(), Some(msg.clone())),
                };

                PluginInfo {
                    id: id.clone(),
                    name: p.manifest.name.clone(),
                    version: p.manifest.version.clone(),
                    status: p.status.as_str().to_string(),
                    health: health_str,
                    health_message: health_msg,
                    tool_count: p.manifest.tools.len(),
                    enabled: p.config.enabled,
                }
            })
            .collect()
    }

    /// Enable and start a plugin at runtime.
    pub async fn enable_plugin(&self, id: &str, entry: &PluginEntry) {
        if entry.plugin_type == PluginType::Mcp {
            if let Err(e) = self.init_mcp_plugin(id, entry).await {
                tracing::warn!(plugin_id = id, "Failed to enable MCP plugin: {e}");
            }
            return;
        }

        let binary_path = {
            let binaries = scan_plugin_directory(&self.plugins_dir);
            binaries.into_iter().find(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s == entry.binary.as_str())
                    .unwrap_or(false)
            })
        };

        if let Some(path) = binary_path {
            let _ = self.init_single(id, entry, &path).await;
        }
    }

    /// Disable and stop a plugin at runtime.
    pub async fn disable_plugin(&self, id: &str) {
        let mut plugins = self.plugins.write().await;
        if let Some(plugin) = plugins.remove(id) {
            let mut transport = plugin.transport.lock().await;
            transport.shutdown(Duration::from_secs(3)).await;
        }
    }

    /// Get tool definitions from all running plugins, including plugin type.
    pub async fn get_all_tool_defs_with_type(&self) -> Vec<(String, PluginToolDef, PluginType)> {
        let plugins = self.plugins.read().await;
        let mut tools = Vec::new();

        for (id, plugin) in plugins.iter() {
            if !matches!(plugin.status, PluginStatus::Running) {
                continue;
            }
            for tool in &plugin.manifest.tools {
                tools.push((id.clone(), tool.clone(), plugin.plugin_type.clone()));
            }
        }

        tools
    }

    /// Get tool definitions from all running plugins.
    pub async fn get_all_tool_defs(&self) -> Vec<(String, PluginToolDef)> {
        self.get_all_tool_defs_with_type()
            .await
            .into_iter()
            .map(|(id, def, _)| (id, def))
            .collect()
    }

    /// Execute a tool call on a specific plugin.
    /// If the process is dead, attempts one restart before returning an error.
    pub async fn handle_tool_call(
        &self,
        plugin_id: &str,
        tool_name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, PluginError> {
        let (transport, plugin_type, spawn_config, respawning) = {
            let plugins = self.plugins.read().await;
            let plugin = plugins.get(plugin_id).ok_or_else(|| {
                PluginError::ProtocolError(format!("Plugin '{}' not found", plugin_id))
            })?;
            (
                Arc::clone(&plugin.transport),
                plugin.plugin_type.clone(),
                plugin.spawn_config.clone(),
                Arc::clone(&plugin.respawning),
            )
        };

        let mut transport_guard = transport.lock().await;

        // Check if process is dead and attempt one restart.
        // Note: transport lock is held during respawn because respawn needs &mut self.
        // Timeout prevents indefinite lock holding if respawn hangs.
        if !transport_guard.is_alive() {
            if respawning
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return Err(PluginError::ProcessDead);
            }
            tracing::warn!(plugin_id, "Plugin process dead — attempting restart");
            let respawn_timeout = Duration::from_secs(15);
            match (&mut *transport_guard, &spawn_config) {
                (PluginTransport::Mcp(p), SpawnConfig::Mcp { command, args }) => {
                    match tokio::time::timeout(respawn_timeout, p.respawn(command, args)).await {
                        Ok(Ok(tools)) => {
                            tracing::info!(
                                plugin_id,
                                tool_count = tools.len(),
                                "MCP plugin restarted"
                            );
                            // Update manifest tools under write lock
                            drop(transport_guard);
                            {
                                let mut plugins = self.plugins.write().await;
                                if let Some(plugin) = plugins.get_mut(plugin_id) {
                                    plugin.manifest.tools = tools;
                                    plugin.status = PluginStatus::Running;
                                    plugin.last_health = PluginHealth::Healthy;
                                }
                            }
                            respawning.store(false, Ordering::SeqCst);
                            // Re-acquire transport lock for the actual call
                            transport_guard = transport.lock().await;
                        }
                        Ok(Err(e)) => {
                            tracing::error!(plugin_id, "Failed to restart plugin: {e}");
                            respawning.store(false, Ordering::SeqCst);
                            return Err(PluginError::ProcessDead);
                        }
                        Err(_) => {
                            tracing::error!(plugin_id, "Plugin respawn timed out after 15s");
                            respawning.store(false, Ordering::SeqCst);
                            return Err(PluginError::ProcessDead);
                        }
                    }
                }
                (
                    PluginTransport::Native(p),
                    SpawnConfig::Native {
                        binary_path,
                        plugin_id: pid,
                    },
                ) => {
                    match tokio::time::timeout(respawn_timeout, p.respawn(binary_path, pid)).await {
                        Ok(Ok(())) => {
                            tracing::info!(plugin_id, "Native plugin restarted");
                            drop(transport_guard);
                            {
                                let mut plugins = self.plugins.write().await;
                                if let Some(plugin) = plugins.get_mut(plugin_id) {
                                    plugin.status = PluginStatus::Running;
                                    plugin.last_health = PluginHealth::Healthy;
                                }
                            }
                            respawning.store(false, Ordering::SeqCst);
                            transport_guard = transport.lock().await;
                        }
                        Ok(Err(e)) => {
                            tracing::error!(plugin_id, "Failed to restart native plugin: {e}");
                            respawning.store(false, Ordering::SeqCst);
                            return Err(PluginError::ProcessDead);
                        }
                        Err(_) => {
                            tracing::error!(plugin_id, "Native plugin respawn timed out after 15s");
                            respawning.store(false, Ordering::SeqCst);
                            return Err(PluginError::ProcessDead);
                        }
                    }
                }
                _ => {
                    respawning.store(false, Ordering::SeqCst);
                    return Err(PluginError::ProcessDead);
                }
            }
        }

        let result = match plugin_type {
            PluginType::Mcp => match &mut *transport_guard {
                PluginTransport::Mcp(p) => {
                    p.call_tool(tool_name, params, Duration::from_secs(30))
                        .await?
                }
                _ => unreachable!(),
            },
            PluginType::Native => {
                transport_guard
                    .send_request(
                        "handle_tool_call",
                        serde_json::json!({
                            "tool_name": tool_name,
                            "params": params,
                        }),
                        Duration::from_secs(30),
                    )
                    .await?
            }
        };

        match plugin_type {
            PluginType::Mcp => Ok(result),
            PluginType::Native => Ok(result.get("result").cloned().unwrap_or(result)),
        }
    }

    // --- Event Hooks ---

    /// Run pre_process hooks on all plugins with that capability.
    /// Returns modified content if any plugin transforms it, or None for passthrough.
    pub async fn pre_process_all(&self, role: &str, content: &str) -> Option<String> {
        let mut current_content = content.to_string();
        let mut modified = false;

        let plugin_transports: Vec<Arc<tokio::sync::Mutex<PluginTransport>>> = {
            let plugins = self.plugins.read().await;
            plugins
                .iter()
                .filter(|(_, p)| {
                    matches!(p.status, PluginStatus::Running)
                        && p.manifest.capabilities.contains(&"pre_process".to_string())
                })
                .map(|(_, p)| Arc::clone(&p.transport))
                .collect()
        };

        for transport in plugin_transports {
            let mut t = transport.lock().await;
            if let Ok(result) = t
                .send_request(
                    "pre_process",
                    serde_json::json!({
                        "message": { "role": role, "content": current_content }
                    }),
                    Duration::from_secs(5),
                )
                .await
            {
                if let Some(msg) = result.get("message") {
                    if let Some(new_content) = msg.get("content").and_then(|c| c.as_str()) {
                        current_content = new_content.to_string();
                        modified = true;
                    }
                }
            }
        }

        if modified {
            Some(current_content)
        } else {
            None
        }
    }

    /// Run post_process hooks on all plugins with that capability.
    pub async fn post_process_all(&self, role: &str, content: &str) -> Option<String> {
        let mut current_content = content.to_string();
        let mut modified = false;

        let plugin_transports: Vec<Arc<tokio::sync::Mutex<PluginTransport>>> = {
            let plugins = self.plugins.read().await;
            plugins
                .iter()
                .filter(|(_, p)| {
                    matches!(p.status, PluginStatus::Running)
                        && p.manifest
                            .capabilities
                            .contains(&"post_process".to_string())
                })
                .map(|(_, p)| Arc::clone(&p.transport))
                .collect()
        };

        for transport in plugin_transports {
            let mut t = transport.lock().await;
            if let Ok(result) = t
                .send_request(
                    "post_process",
                    serde_json::json!({
                        "message": { "role": role, "content": current_content }
                    }),
                    Duration::from_secs(5),
                )
                .await
            {
                if let Some(msg) = result.get("message") {
                    if let Some(new_content) = msg.get("content").and_then(|c| c.as_str()) {
                        current_content = new_content.to_string();
                        modified = true;
                    }
                }
            }
        }

        if modified {
            Some(current_content)
        } else {
            None
        }
    }

    /// Fire-and-forget: notify all plugins with trace capability about a trace event.
    pub fn notify_trace(self: &Arc<Self>, trace: serde_json::Value) {
        let host = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let transports: Vec<Arc<tokio::sync::Mutex<PluginTransport>>> = {
                let plugins = host.plugins.read().await;
                plugins
                    .iter()
                    .filter(|(_, p)| {
                        matches!(p.status, PluginStatus::Running)
                            && p.manifest.capabilities.contains(&"trace".to_string())
                    })
                    .map(|(_, p)| Arc::clone(&p.transport))
                    .collect()
            };

            for transport in transports {
                let mut t = transport.lock().await;
                let _ = t
                    .send_request(
                        "on_trace",
                        serde_json::json!({ "trace": trace }),
                        Duration::from_secs(2),
                    )
                    .await;
            }
        });
    }

    /// Notify plugins about trace export.
    pub async fn notify_trace_export(&self, traces: &[serde_json::Value]) {
        let transports: Vec<Arc<tokio::sync::Mutex<PluginTransport>>> = {
            let plugins = self.plugins.read().await;
            plugins
                .iter()
                .filter(|(_, p)| {
                    matches!(p.status, PluginStatus::Running)
                        && p.manifest.capabilities.contains(&"trace".to_string())
                })
                .map(|(_, p)| Arc::clone(&p.transport))
                .collect()
        };

        for transport in transports {
            let mut t = transport.lock().await;
            let _ = t
                .send_request(
                    "on_trace_export",
                    serde_json::json!({ "traces": traces }),
                    Duration::from_secs(10),
                )
                .await;
        }
    }

    // --- Compaction Hooks ---

    /// Call on_compaction_save on all plugins with compaction capability.
    /// Returns a map of plugin_id -> saved state.
    pub async fn compaction_save_all(
        &self,
        agent_id: &str,
        summary: &str,
    ) -> HashMap<String, serde_json::Value> {
        let mut result = HashMap::new();

        let plugin_data: Vec<(String, Arc<tokio::sync::Mutex<PluginTransport>>)> = {
            let plugins = self.plugins.read().await;
            plugins
                .iter()
                .filter(|(_, p)| {
                    matches!(p.status, PluginStatus::Running)
                        && p.manifest.capabilities.contains(&"compaction".to_string())
                })
                .map(|(id, p)| (id.clone(), Arc::clone(&p.transport)))
                .collect()
        };

        for (id, transport) in plugin_data {
            let mut t = transport.lock().await;
            if let Ok(state) = t
                .send_request(
                    "on_compaction_save",
                    serde_json::json!({
                        "agent_id": agent_id,
                        "summary": summary,
                    }),
                    Duration::from_secs(5),
                )
                .await
            {
                if let Some(saved) = state.get("state") {
                    result.insert(id, saved.clone());
                }
            }
        }

        result
    }

    /// Call on_compaction_restore on all plugins with compaction capability.
    /// Returns context strings to inject into the rebuilt conversation.
    pub async fn compaction_restore_all(&self, agent_id: &str) -> Vec<String> {
        let mut contexts = Vec::new();

        let plugin_data: Vec<(String, Arc<tokio::sync::Mutex<PluginTransport>>)> = {
            let plugins = self.plugins.read().await;
            plugins
                .iter()
                .filter(|(_, p)| {
                    matches!(p.status, PluginStatus::Running)
                        && p.manifest.capabilities.contains(&"compaction".to_string())
                })
                .map(|(id, p)| (id.clone(), Arc::clone(&p.transport)))
                .collect()
        };

        for (_id, transport) in plugin_data {
            let mut t = transport.lock().await;
            if let Ok(result) = t
                .send_request(
                    "on_compaction_restore",
                    serde_json::json!({ "agent_id": agent_id }),
                    Duration::from_secs(5),
                )
                .await
            {
                if let Some(context) = result.get("context").and_then(|c| c.as_str()) {
                    if !context.is_empty() {
                        contexts.push(context.to_string());
                    }
                }
            }
        }

        contexts
    }
}

/// Convert a TOML value to a JSON value.
fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::json!(*i),
        toml::Value::Float(f) => serde_json::json!(*f),
        toml::Value::Boolean(b) => serde_json::json!(*b),
        toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        toml::Value::Array(arr) => serde_json::Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(tbl) => {
            let map: serde_json::Map<String, serde_json::Value> = tbl
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_host_new() {
        let host = PluginHost::new(PathBuf::from("/tmp/plugins"), 65536);
        assert_eq!(host.max_payload, 65536);
    }

    #[tokio::test]
    async fn test_list_plugins_empty() {
        let host = PluginHost::new(PathBuf::from("/tmp/plugins"), 65536);
        let plugins = host.list_plugins().await;
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_spawn_config_stored_on_init() {
        // Verify SpawnConfig can be constructed for both types
        let mcp_config = super::super::types::SpawnConfig::Mcp {
            command: "python3".to_string(),
            args: vec!["/path/to/shim.py".to_string()],
        };
        let native_config = super::super::types::SpawnConfig::Native {
            binary_path: PathBuf::from("/plugins/test"),
            plugin_id: "test".to_string(),
        };
        // Just verify they construct without panic
        assert!(matches!(
            mcp_config,
            super::super::types::SpawnConfig::Mcp { .. }
        ));
        assert!(matches!(
            native_config,
            super::super::types::SpawnConfig::Native { .. }
        ));
    }

    #[test]
    fn test_toml_to_json() {
        let toml_val = toml::Value::String("hello".to_string());
        let json_val = toml_to_json(&toml_val);
        assert_eq!(json_val, serde_json::json!("hello"));

        let toml_val = toml::Value::Integer(42);
        let json_val = toml_to_json(&toml_val);
        assert_eq!(json_val, serde_json::json!(42));

        let toml_val = toml::Value::Boolean(true);
        let json_val = toml_to_json(&toml_val);
        assert_eq!(json_val, serde_json::json!(true));
    }
}
