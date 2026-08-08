use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Maximum IPC payload size in bytes. Raised from the PRD Section 8.3 default
/// of 64KB, which rejected healthy plugin tool results (PDFs, long web reads)
/// at the transport boundary. 100MB is a sanity backstop, not a working limit.
pub const MAX_IPC_PAYLOAD_BYTES: usize = 100 * 1024 * 1024;

// --- Plugin Manifest (returned by plugin on `initialize`) ---

/// What a plugin reports about itself after initialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Which hooks this plugin supports: "pre_process", "post_process", "tools",
    /// "trace", "compaction"
    pub capabilities: Vec<String>,
    /// Tool definitions the plugin provides to agents.
    #[serde(default)]
    pub tools: Vec<PluginToolDef>,
}

/// A tool definition provided by a plugin (mirrors OpenAI function format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDef {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
}

// --- Plugin Health ---

/// Health status of a running plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", content = "message")]
pub enum PluginHealth {
    Healthy,
    Degraded(String),
    Unhealthy(String),
}

impl Default for PluginHealth {
    fn default() -> Self {
        Self::Unhealthy("Not initialized".to_string())
    }
}

// --- Plugin Status ---

/// Lifecycle status of a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginStatus {
    Starting,
    Running,
    Stopped,
    Error(String),
}

impl PluginStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Error(_) => "error",
        }
    }
}

// --- Plugin Info (for frontend) ---

/// Serializable summary sent to the frontend for the Plugins section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub status: String,
    pub health: String,
    pub health_message: Option<String>,
    pub tool_count: usize,
    pub enabled: bool,
}

// --- Compaction State ---

/// State dump passed to plugins during compaction save.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionStateDump {
    pub agent_id: String,
    pub summary: String,
    pub metadata: HashMap<String, serde_json::Value>,
}

// --- Spawn Config ---

/// Stores original spawn parameters so a dead plugin process can be restarted.
#[derive(Debug, Clone)]
pub enum SpawnConfig {
    Native {
        binary_path: std::path::PathBuf,
        plugin_id: String,
    },
    Mcp {
        command: String,
        args: Vec<String>,
    },
}

// --- JSON-RPC 2.0 Types ---

/// A JSON-RPC 2.0 request message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        }
    }
}

/// A JSON-RPC 2.0 response message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// --- Plugin Errors ---

/// Errors that can occur during plugin operations.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("Failed to spawn plugin process: {0}")]
    SpawnFailed(String),

    #[error("Plugin operation timed out after {0}ms")]
    Timeout(u64),

    #[error("Protocol error: {0}")]
    ProtocolError(String),

    #[error("IPC payload too large: {size} bytes (max {max})")]
    PayloadTooLarge { size: usize, max: usize },

    #[error("Plugin process is dead")]
    ProcessDead,

    #[error("JSON-RPC error ({code}): {message}")]
    RpcError { code: i32, message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_request_serialization() {
        let req = JsonRpcRequest::new(1, "initialize", serde_json::json!({"config": {}}));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"initialize\""));
        assert!(json.contains("\"id\":1"));

        let parsed: JsonRpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.method, "initialize");
        assert_eq!(parsed.id, 1);
    }

    #[test]
    fn test_json_rpc_response_success() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: 1,
            result: Some(serde_json::json!({"id": "test-plugin", "name": "Test"})),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: JsonRpcResponse = serde_json::from_str(&json).unwrap();
        assert!(parsed.result.is_some());
        assert!(parsed.error.is_none());
    }

    #[test]
    fn test_json_rpc_response_error() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: 1,
            result: None,
            error: Some(JsonRpcError {
                code: -32600,
                message: "Invalid request".to_string(),
                data: None,
            }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: JsonRpcResponse = serde_json::from_str(&json).unwrap();
        assert!(parsed.result.is_none());
        assert!(parsed.error.is_some());
        assert_eq!(parsed.error.unwrap().code, -32600);
    }

    #[test]
    fn test_plugin_manifest_roundtrip() {
        let manifest = PluginManifest {
            id: "test-plugin".to_string(),
            name: "Test Plugin".to_string(),
            version: "1.0.0".to_string(),
            capabilities: vec!["tools".to_string(), "pre_process".to_string()],
            tools: vec![PluginToolDef {
                name: "my_tool".to_string(),
                description: "A test tool".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "input": { "type": "string" }
                    }
                }),
            }],
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: PluginManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "test-plugin");
        assert_eq!(parsed.tools.len(), 1);
        assert_eq!(parsed.capabilities.len(), 2);
    }

    #[test]
    fn test_spawn_config_mcp() {
        let config = SpawnConfig::Mcp {
            command: "python3".to_string(),
            args: vec!["/path/to/shim.py".to_string()],
        };
        match config {
            SpawnConfig::Mcp { command, args } => {
                assert_eq!(command, "python3");
                assert_eq!(args.len(), 1);
            }
            _ => panic!("Expected MCP config"),
        }
    }

    #[test]
    fn test_spawn_config_native() {
        let config = SpawnConfig::Native {
            binary_path: std::path::PathBuf::from("/plugins/my-plugin"),
            plugin_id: "my-plugin".to_string(),
        };
        match config {
            SpawnConfig::Native {
                binary_path,
                plugin_id,
            } => {
                assert_eq!(plugin_id, "my-plugin");
                assert!(binary_path.to_str().unwrap().contains("my-plugin"));
            }
            _ => panic!("Expected Native config"),
        }
    }

    #[test]
    fn test_plugin_health_serialization() {
        let healthy = PluginHealth::Healthy;
        let json = serde_json::to_string(&healthy).unwrap();
        assert!(json.contains("Healthy"));

        let degraded = PluginHealth::Degraded("slow response".to_string());
        let json = serde_json::to_string(&degraded).unwrap();
        assert!(json.contains("Degraded"));
        assert!(json.contains("slow response"));
    }
}
