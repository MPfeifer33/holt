use serde::{Deserialize, Serialize};

/// MCP server name used by the Agent SDK bridge. Referenced in bridge JS
/// and agent config allowed_tools patterns. If this changes, update
/// agent-sdk-bridge.js to match and rebuild the sidecar bundle.
pub const MCP_SERVER_NAME: &str = "holt";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSdkBlock {
    /// Whether the Agent SDK bridge is enabled (master switch)
    pub enabled: bool,
    /// Path to node binary
    pub node_path: String,
    /// Model for SDK agents (explicit, not auto-detected)
    pub model: String,
}

impl Default for AgentSdkBlock {
    fn default() -> Self {
        Self {
            enabled: true,
            node_path: "node".to_string(),
            model: "claude-opus-4-6".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_sdk_block_roundtrip() {
        let block = AgentSdkBlock {
            enabled: true,
            node_path: "node".to_string(),
            model: "claude-opus-4-6".to_string(),
        };
        let toml_str = toml::to_string(&block).expect("serialize");
        let decoded: AgentSdkBlock = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(decoded.enabled, true);
        assert_eq!(decoded.model, "claude-opus-4-6");
    }

    #[test]
    fn test_agent_sdk_block_defaults() {
        let toml_str = "";
        let block: AgentSdkBlock = toml::from_str(toml_str).expect("deserialize empty");
        assert_eq!(block.enabled, true);
        assert_eq!(block.node_path, "node");
        assert_eq!(block.model, "claude-opus-4-6");
    }

    #[test]
    fn test_agent_sdk_block_deserializes_with_extra_fields() {
        // Old config.toml files may have extra fields — they should
        // deserialize without error (serde ignores unknown fields with #[serde(default)])
        let toml_str = r#"
enabled = true
node_path = "node"
model = "claude-opus-4-6"
legacy_mode = "ignored"
"#;
        let block: AgentSdkBlock = toml::from_str(toml_str).expect("deserialize old format");
        assert_eq!(block.enabled, true);
        assert_eq!(block.model, "claude-opus-4-6");
    }
}
