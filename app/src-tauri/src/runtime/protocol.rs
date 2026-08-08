use super::connection::{AgentProtocol, ConnectionConfig};

/// Return the protocol Holt should use for a persisted connection config.
pub fn preferred_protocol(connection: &ConnectionConfig) -> AgentProtocol {
    match connection {
        ConnectionConfig::McpStdio { .. } => AgentProtocol::Mcp,
        ConnectionConfig::OAuth { provider, .. } => {
            if crate::runtime::codex::is_codex_provider(provider) {
                AgentProtocol::Codex
            } else {
                AgentProtocol::AnthropicMessages
            }
        }
        ConnectionConfig::AgentSdk => AgentProtocol::AgentSdk,
        _ => AgentProtocol::default(),
    }
}

/// Normalize a user-configured base URL by stripping trailing slashes and a
/// trailing `/v1` segment so callers can safely append `/v1/models` or `/mcp`
/// without producing a double-`/v1` path.
///
/// Users commonly configure either `https://api.openai.com/v1` (with `/v1`)
/// or `https://api.openai.com` (without) — both should resolve to the same
/// probe URLs. Without this normalization, the former produced
/// `https://api.openai.com/v1/v1/models` on the Chat Completions probe and
/// `https://api.openai.com/v1/mcp` on the MCP probe, both wrong.
fn strip_v1_suffix(base_url: &str) -> &str {
    base_url.trim_end_matches('/').trim_end_matches("/v1")
}

/// Errors during protocol negotiation.
#[derive(Debug, thiserror::Error)]
pub enum NegotiationError {
    #[error("MCP handshake failed: {0}")]
    McpFailed(String),
    #[error("ChatCompletions health check failed: {0}")]
    ChatCompletionsFailed(String),
    #[error("No protocol could be negotiated for endpoint")]
    NoProtocol,
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
}

/// Negotiate the best protocol for a connection (PRD Section 5.2).
///
/// For McpStdio: attempt MCP handshake over stdio. No fallback.
/// For Api/Local: attempt MCP first, fall back to ChatCompletions.
pub async fn negotiate_protocol(
    client: &reqwest::Client,
    connection: &ConnectionConfig,
) -> Result<AgentProtocol, NegotiationError> {
    match connection {
        ConnectionConfig::McpStdio { .. } => {
            // MCP over stdio — attempt handshake, no fallback
            // TODO: Implement real MCP stdio handshake via rmcp when integrated
            // For now, return Mcp protocol (will be connected when engine runs)
            Ok(AgentProtocol::Mcp)
        }

        ConnectionConfig::Api { endpoint, .. } => {
            let base_url = endpoint.clone();
            negotiate_http_protocol(client, &base_url).await
        }

        ConnectionConfig::Local { host, port, .. } => {
            let base_url = format!("http://{}:{}", host, port);
            negotiate_http_protocol(client, &base_url).await
        }

        ConnectionConfig::OAuth { .. } | ConnectionConfig::AgentSdk => {
            Ok(preferred_protocol(connection))
        }
    }
}

async fn negotiate_http_protocol(
    client: &reqwest::Client,
    base_url: &str,
) -> Result<AgentProtocol, NegotiationError> {
    // Step 1: Try MCP streamable HTTP
    if let Ok(()) = try_mcp_http(client, base_url).await {
        return Ok(AgentProtocol::Mcp);
    }

    // Step 2: Try ChatCompletions health check
    match try_chat_completions(client, base_url).await {
        Ok(()) => Ok(AgentProtocol::ChatCompletions),
        Err(e) => Err(NegotiationError::ChatCompletionsFailed(format!(
            "Both MCP and ChatCompletions failed for {}: {}",
            base_url, e
        ))),
    }
}

/// Attempt streamable HTTP MCP handshake at the endpoint.
async fn try_mcp_http(client: &reqwest::Client, base_url: &str) -> Result<(), NegotiationError> {
    // TODO: Implement real MCP HTTP handshake via rmcp StreamableHttpClientTransport
    // For now, probe for MCP endpoint and return error to trigger fallback
    let mcp_url = format!("{}/mcp", strip_v1_suffix(base_url));
    let resp = client
        .post(&mcp_url)
        .timeout(std::time::Duration::from_secs(5))
        .header("Content-Type", "application/json")
        .body(r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"holt","version":"0.1.0"}}}"#)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => Ok(()),
        Ok(r) => Err(NegotiationError::McpFailed(format!("HTTP {}", r.status()))),
        Err(e) => Err(NegotiationError::McpFailed(e.to_string())),
    }
}

/// Attempt ChatCompletions health check (GET /v1/models).
///
/// Uses `strip_v1_suffix` to normalize the base URL so an endpoint like
/// `https://api.openai.com/v1` produces `https://api.openai.com/v1/models`
/// (not `/v1/v1/models`). Falls back to `{normalized}/models` for servers
/// that use the native path (e.g., llama-server).
async fn try_chat_completions(
    client: &reqwest::Client,
    base_url: &str,
) -> Result<(), NegotiationError> {
    let normalized = strip_v1_suffix(base_url);
    let url = format!("{}/v1/models", normalized);
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => Ok(()),
        // Some servers (like llama-server) use /models instead of /v1/models
        Ok(_) => {
            let url2 = format!("{}/models", normalized);
            let resp2 = client
                .get(&url2)
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await;
            match resp2 {
                Ok(r) if r.status().is_success() => Ok(()),
                Ok(r) => Err(NegotiationError::ChatCompletionsFailed(format!(
                    "HTTP {}",
                    r.status()
                ))),
                Err(e) => Err(NegotiationError::ChatCompletionsFailed(e.to_string())),
            }
        }
        Err(e) => Err(NegotiationError::ChatCompletionsFailed(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_preferred_protocol_keeps_anthropic_oauth_on_messages() {
        let connection = ConnectionConfig::OAuth {
            provider: "anthropic".to_string(),
            credentials_path: std::path::PathBuf::from("/tmp/credentials.json"),
            model: "claude-opus-4-6".to_string(),
        };

        assert_eq!(
            preferred_protocol(&connection),
            AgentProtocol::AnthropicMessages
        );
    }

    #[test]
    fn test_preferred_protocol_routes_openai_oauth_to_codex() {
        let connection = ConnectionConfig::OAuth {
            provider: crate::runtime::codex::OPENAI_OAUTH_PROVIDER.to_string(),
            credentials_path: std::path::PathBuf::from("/tmp/codex-auth.json"),
            model: crate::runtime::codex::default_model(),
        };

        assert_eq!(preferred_protocol(&connection), AgentProtocol::Codex);
    }

    #[test]
    fn test_preferred_protocol_preserves_existing_sdk_and_mcp_rules() {
        assert_eq!(
            preferred_protocol(&ConnectionConfig::AgentSdk),
            AgentProtocol::AgentSdk
        );
        assert_eq!(
            preferred_protocol(&ConnectionConfig::McpStdio {
                command: "echo".to_string(),
                args: vec![],
                env: HashMap::new(),
            }),
            AgentProtocol::Mcp
        );
    }

    #[test]
    fn test_strip_v1_suffix_with_trailing_v1() {
        // The motivating case: api.openai.com/v1 must NOT produce a double /v1.
        assert_eq!(
            strip_v1_suffix("https://api.openai.com/v1"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn test_strip_v1_suffix_with_trailing_slash_after_v1() {
        assert_eq!(
            strip_v1_suffix("https://api.openai.com/v1/"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn test_strip_v1_suffix_without_v1() {
        // Endpoint without /v1 is unchanged.
        assert_eq!(
            strip_v1_suffix("https://api.openai.com"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn test_strip_v1_suffix_localhost() {
        assert_eq!(
            strip_v1_suffix("http://localhost:8080"),
            "http://localhost:8080"
        );
        assert_eq!(
            strip_v1_suffix("http://localhost:8080/"),
            "http://localhost:8080"
        );
        assert_eq!(
            strip_v1_suffix("http://localhost:11434/v1"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn test_strip_v1_suffix_does_not_strip_v1beta() {
        // Gemini uses /v1beta — must NOT match /v1.
        assert_eq!(
            strip_v1_suffix("https://generativelanguage.googleapis.com/v1beta"),
            "https://generativelanguage.googleapis.com/v1beta"
        );
    }

    #[test]
    fn test_strip_v1_suffix_handles_double_v1() {
        // Pathological input — if someone already has /v1/v1, strip both.
        // trim_end_matches repeats the pattern match.
        assert_eq!(
            strip_v1_suffix("https://api.openai.com/v1/v1"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn test_chat_completions_url_construction() {
        // Verify that the url a caller would build using strip_v1_suffix produces
        // the correct path for the canonical OpenAI endpoint shape.
        fn build_chat_completions_url(base: &str) -> String {
            format!("{}/v1/models", strip_v1_suffix(base))
        }

        assert_eq!(
            build_chat_completions_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            build_chat_completions_url("https://api.openai.com"),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            build_chat_completions_url("https://api.x.ai/v1"),
            "https://api.x.ai/v1/models"
        );
        assert_eq!(
            build_chat_completions_url("http://localhost:8080"),
            "http://localhost:8080/v1/models"
        );
        assert_eq!(
            build_chat_completions_url("http://localhost:11434/v1"),
            "http://localhost:11434/v1/models"
        );
    }

    #[test]
    fn test_mcp_url_construction() {
        // MCP endpoints live at /mcp, not under /v1 — verify the same helper
        // keeps the MCP probe URL correct regardless of whether the user
        // included /v1 in their endpoint.
        fn build_mcp_url(base: &str) -> String {
            format!("{}/mcp", strip_v1_suffix(base))
        }

        assert_eq!(
            build_mcp_url("https://api.openai.com/v1"),
            "https://api.openai.com/mcp"
        );
        assert_eq!(
            build_mcp_url("http://localhost:8080"),
            "http://localhost:8080/mcp"
        );
    }
}
