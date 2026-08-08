use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectionConfig {
    Api {
        endpoint: String,
        /// Reference to the API key stored in OS keychain (resolved at connection time)
        api_key_ref: String,
        model: String,
    },
    Local {
        host: String,
        port: u16,
        model: Option<String>,
    },
    McpStdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    OAuth {
        provider: String,
        credentials_path: std::path::PathBuf,
        model: String,
    },
    #[serde(alias = "ClaudeCode")]
    AgentSdk,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum AgentProtocol {
    Mcp,
    #[default]
    ChatCompletions,
    AnthropicMessages,
    Codex,
    #[serde(alias = "ClaudeCodeSdk")]
    AgentSdk,
}

/// A local model service discovered via auto-detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedModel {
    pub service_name: String,
    pub endpoint: String,
    pub port: u16,
    pub models: Vec<String>,
    /// Detected context window size (from server probe)
    pub context_size: Option<u32>,
}

/// Auto-detection of local model services (PRD Section 5.3).
pub struct AutoDetector {
    client: reqwest::Client,
}

impl Default for AutoDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoDetector {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap_or_default();
        Self { client }
    }

    /// Scan all known local endpoints and return discovered services.
    pub async fn scan_local_models(&self) -> Vec<DetectedModel> {
        let mut detected = Vec::new();

        // Run all probes concurrently
        let (ollama, lm_studio, llama_server) = tokio::join!(
            self.probe_ollama(),
            self.probe_lm_studio(),
            self.probe_llama_server(),
        );

        if let Some(d) = ollama {
            detected.push(d);
        }
        if let Some(d) = lm_studio {
            detected.push(d);
        }
        if let Some(d) = llama_server {
            detected.push(d);
        }

        detected
    }

    async fn probe_ollama(&self) -> Option<DetectedModel> {
        let resp = self
            .client
            .get("http://localhost:11434/api/tags")
            .send()
            .await
            .ok()?;
        let json: serde_json::Value = resp.json().await.ok()?;
        let models: Vec<String> = json["models"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m["name"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // Probe context size for the first model
        let context_size = if let Some(first_model) = models.first() {
            self.probe_context_ollama("http://localhost:11434", first_model)
                .await
        } else {
            None
        };

        Some(DetectedModel {
            service_name: "Ollama".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            port: 11434,
            models,
            context_size,
        })
    }

    async fn probe_lm_studio(&self) -> Option<DetectedModel> {
        let resp = self
            .client
            .get("http://localhost:1234/v1/models")
            .send()
            .await
            .ok()?;
        let json: serde_json::Value = resp.json().await.ok()?;
        let models = json["data"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m["id"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Some(DetectedModel {
            service_name: "LM Studio".to_string(),
            endpoint: "http://localhost:1234".to_string(),
            port: 1234,
            models,
            context_size: None, // LM Studio doesn't reliably expose context size
        })
    }

    async fn probe_llama_server(&self) -> Option<DetectedModel> {
        let endpoint = "http://localhost:8080";
        let resp = self
            .client
            .get(format!("{}/health", endpoint))
            .send()
            .await
            .ok()?;
        if resp.status().is_success() {
            let context_size = self.probe_context_llama_server(endpoint).await;
            // Query /v1/models to get the loaded model name
            let models = if let Ok(resp) = self
                .client
                .get(format!("{}/v1/models", endpoint))
                .send()
                .await
            {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    json["data"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| m["id"].as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                }
            } else {
                vec![]
            };
            Some(DetectedModel {
                service_name: "llama-server".to_string(),
                endpoint: endpoint.to_string(),
                port: 8080,
                models,
                context_size,
            })
        } else {
            None
        }
    }

    /// Probe llama-server /props endpoint for actual context window size.
    async fn probe_context_llama_server(&self, endpoint: &str) -> Option<u32> {
        let resp = self
            .client
            .get(format!("{}/props", endpoint))
            .send()
            .await
            .ok()?;
        let json: serde_json::Value = resp.json().await.ok()?;
        let n_ctx = json
            .get("default_generation_settings")
            .and_then(|s| s.get("n_ctx"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)?;
        tracing::info!(
            "Detected context window: {} for llama-server at {}",
            n_ctx,
            endpoint
        );
        Some(n_ctx)
    }

    /// Probe Ollama /api/show endpoint for actual context window size.
    /// Context length is at parameters.num_ctx (or context_length in newer versions).
    async fn probe_context_ollama(&self, endpoint: &str, model: &str) -> Option<u32> {
        let resp = self
            .client
            .post(format!("{}/api/show", endpoint))
            .json(&serde_json::json!({"name": model}))
            .send()
            .await
            .ok()?;
        let json: serde_json::Value = resp.json().await.ok()?;
        // Try parameters as string (Ollama returns newline-separated key-value pairs)
        let n_ctx = json
            .get("parameters")
            .and_then(|p| p.as_str())
            .and_then(|params_str| {
                params_str
                    .lines()
                    .find(|line| line.trim().starts_with("num_ctx"))
                    .and_then(|line| line.split_whitespace().last())
                    .and_then(|v| v.parse::<u32>().ok())
            })
            // Fallback: try context_length field (newer Ollama versions)
            .or_else(|| {
                json.get("context_length")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
            });
        if let Some(ctx) = n_ctx {
            tracing::info!(
                "Detected context window: {} for Ollama model {} at {}",
                ctx,
                model,
                endpoint
            );
        }
        n_ctx
    }
}

/// Detect if an endpoint is a Google Gemini API.
fn is_gemini_endpoint(endpoint: &str) -> bool {
    endpoint.contains("googleapis.com") || endpoint.contains("generativelanguage")
}

/// Extract context window size from an OpenAI-compatible /v1/models response.
/// Tries field names: context_window (Groq), context_length (xAI, OpenRouter), max_model_len (vLLM).
fn extract_context_from_models_response(json: &serde_json::Value, model_id: &str) -> Option<u32> {
    let data = json.get("data")?.as_array()?;
    let model_entry = data
        .iter()
        .find(|m| m.get("id").and_then(|id| id.as_str()) == Some(model_id))?;

    model_entry
        .get("context_window")
        .or_else(|| model_entry.get("context_length"))
        .or_else(|| model_entry.get("max_model_len"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
}

/// Extract context window size from a Gemini model metadata response.
fn extract_context_from_gemini_response(json: &serde_json::Value) -> Option<u32> {
    json.get("inputTokenLimit")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
}

/// Probe a cloud API endpoint for model context window size.
///
/// For OpenAI-compatible APIs: GET /v1/models, find model, extract context size.
/// For Gemini: GET /v1beta/models/{model}, extract inputTokenLimit.
/// Returns None on any failure (timeout, parse error, model not found).
pub async fn probe_cloud_context_size(
    connection: &ConnectionConfig,
    api_key: &str,
    client: &reqwest::Client,
) -> Option<u32> {
    let (endpoint, model) = match connection {
        ConnectionConfig::Api {
            endpoint, model, ..
        } => (endpoint.as_str(), model.as_str()),
        _ => return None,
    };

    let probe_client = client;

    if is_gemini_endpoint(endpoint) {
        probe_gemini_context(&probe_client, endpoint, model, api_key).await
    } else {
        probe_openai_compatible_context(&probe_client, endpoint, model, api_key).await
    }
}

async fn probe_openai_compatible_context(
    client: &reqwest::Client,
    endpoint: &str,
    model: &str,
    api_key: &str,
) -> Option<u32> {
    // Strip trailing /v1 if present — endpoints are often stored as "https://api.x.ai/v1"
    let base = endpoint.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{}/v1/models", base);
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
        .map_err(|e| {
            tracing::debug!("Cloud API probe failed for {}: {}", endpoint, e);
            e
        })
        .ok()?;

    if !resp.status().is_success() {
        tracing::debug!(
            "Cloud API probe failed for {}: HTTP {}",
            endpoint,
            resp.status()
        );
        return None;
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| {
            tracing::debug!("Cloud API probe parse failed for {}: {}", endpoint, e);
            e
        })
        .ok()?;

    let size = extract_context_from_models_response(&json, model);
    if let Some(s) = size {
        tracing::info!(
            "Cloud API probe: {} context window = {} (from {})",
            model,
            s,
            endpoint
        );
    } else {
        tracing::debug!(
            "Cloud API probe: model {} not found or no context field in {}",
            model,
            endpoint
        );
    }
    size
}

async fn probe_gemini_context(
    client: &reqwest::Client,
    endpoint: &str,
    model: &str,
    api_key: &str,
) -> Option<u32> {
    // Gemini model names may or may not have "models/" prefix
    let model_path = if model.starts_with("models/") {
        model.to_string()
    } else {
        format!("models/{}", model)
    };
    let base = endpoint.trim_end_matches('/').trim_end_matches("/v1beta");
    let url = format!("{}/v1beta/{}?key={}", base, model_path, api_key);

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| {
            // Redact API key from error messages (reqwest may include the full URL)
            let sanitized = format!("{}", e).replace(api_key, "[REDACTED]");
            tracing::debug!("Gemini API probe failed for {}: {}", endpoint, sanitized);
            e
        })
        .ok()?;

    if !resp.status().is_success() {
        tracing::debug!(
            "Gemini API probe failed for {}: HTTP {}",
            endpoint,
            resp.status()
        );
        return None;
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| {
            let sanitized = format!("{}", e).replace(api_key, "[REDACTED]");
            tracing::debug!(
                "Gemini API probe parse failed for {}: {}",
                endpoint,
                sanitized
            );
            e
        })
        .ok()?;

    let size = extract_context_from_gemini_response(&json);
    if let Some(s) = size {
        tracing::info!(
            "Cloud API probe: {} context window = {} (from Gemini {})",
            model,
            s,
            endpoint
        );
    } else {
        tracing::debug!(
            "Gemini API probe: no inputTokenLimit for {} at {}",
            model,
            endpoint
        );
    }
    size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_context_from_openai_compatible_context_window() {
        // Groq-style: "context_window" field
        let json: serde_json::Value = serde_json::json!({
            "data": [
                {"id": "llama-3.1-70b", "context_window": 131072},
                {"id": "mixtral-8x7b", "context_window": 32768}
            ]
        });
        assert_eq!(
            extract_context_from_models_response(&json, "llama-3.1-70b"),
            Some(131072)
        );
        assert_eq!(
            extract_context_from_models_response(&json, "mixtral-8x7b"),
            Some(32768)
        );
    }

    #[test]
    fn test_extract_context_from_openai_compatible_context_length() {
        // xAI-style: "context_length" field
        let json: serde_json::Value = serde_json::json!({
            "data": [
                {"id": "grok-4", "context_length": 2000000}
            ]
        });
        assert_eq!(
            extract_context_from_models_response(&json, "grok-4"),
            Some(2_000_000)
        );
    }

    #[test]
    fn test_extract_context_model_not_found() {
        let json: serde_json::Value = serde_json::json!({
            "data": [
                {"id": "grok-4", "context_window": 131072}
            ]
        });
        assert_eq!(
            extract_context_from_models_response(&json, "nonexistent-model"),
            None
        );
    }

    #[test]
    fn test_extract_context_no_context_fields() {
        // OpenAI-style: no context size fields
        let json: serde_json::Value = serde_json::json!({
            "data": [
                {"id": "gpt-4o", "object": "model", "owned_by": "openai"}
            ]
        });
        assert_eq!(extract_context_from_models_response(&json, "gpt-4o"), None);
    }

    #[test]
    fn test_extract_context_from_gemini_response() {
        let json: serde_json::Value = serde_json::json!({
            "name": "models/gemini-2.5-pro",
            "inputTokenLimit": 1048576,
            "outputTokenLimit": 65536
        });
        assert_eq!(extract_context_from_gemini_response(&json), Some(1_048_576));
    }

    #[test]
    fn test_extract_context_from_gemini_missing_field() {
        let json: serde_json::Value = serde_json::json!({
            "name": "models/gemini-2.5-pro"
        });
        assert_eq!(extract_context_from_gemini_response(&json), None);
    }

    #[test]
    fn test_extract_context_from_vllm_max_model_len() {
        // vLLM-style: "max_model_len" field
        let json: serde_json::Value = serde_json::json!({
            "data": [
                {"id": "meta-llama/Llama-3-70b", "max_model_len": 131072}
            ]
        });
        assert_eq!(
            extract_context_from_models_response(&json, "meta-llama/Llama-3-70b"),
            Some(131072)
        );
    }

    #[test]
    fn test_claude_code_connection_serde() {
        let conn = ConnectionConfig::AgentSdk;
        let json = serde_json::to_string(&conn).unwrap();
        let decoded: ConnectionConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, ConnectionConfig::AgentSdk));
    }

    #[test]
    fn test_claude_code_sdk_protocol_serde() {
        let proto = AgentProtocol::AgentSdk;
        let json = serde_json::to_string(&proto).unwrap();
        let decoded: AgentProtocol = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, AgentProtocol::AgentSdk);
    }

    #[test]
    fn test_codex_protocol_serde() {
        let proto = AgentProtocol::Codex;
        let json = serde_json::to_string(&proto).unwrap();
        let decoded: AgentProtocol = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, AgentProtocol::Codex);
    }

    #[test]
    fn test_is_gemini_endpoint() {
        assert!(is_gemini_endpoint(
            "https://generativelanguage.googleapis.com"
        ));
        assert!(is_gemini_endpoint(
            "https://generativelanguage.googleapis.com/v1beta"
        ));
        assert!(!is_gemini_endpoint("https://api.openai.com/v1"));
        assert!(!is_gemini_endpoint("https://api.x.ai/v1"));
    }
}
