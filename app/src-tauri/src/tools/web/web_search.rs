use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct WebSearchTool;

#[async_trait::async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Search the web and return titles, URLs, and snippets."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"query": "rust async trait", "num_results": 5}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "num_results": { "type": "integer", "description": "Number of results (default: 5, max: 20)" }
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: query".to_string(),
                retryable: false,
            })?;

        let num_results = arguments
            .get("num_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(20) as usize;

        // Get search config
        let (provider, endpoint) = {
            let config = context.config.read().await;
            (
                config.tools.web.search_provider.clone(),
                config.tools.web.search_endpoint.clone(),
            )
        };

        if endpoint.is_empty() {
            return Err(ToolError {
                code: ToolErrorCode::SearchProviderNotConfigured,
                message: format!(
                    "Search provider '{}' is not configured. Set search_endpoint in config.",
                    provider
                ),
                retryable: false,
            });
        }

        match provider.as_str() {
            "searxng" => search_searxng(&context.http_client, &endpoint, query, num_results).await,
            other => Err(ToolError {
                code: ToolErrorCode::SearchProviderNotConfigured,
                message: format!(
                    "Search provider '{}' is not supported. Supported: searxng",
                    other
                ),
                retryable: false,
            }),
        }
    }
}

async fn search_searxng(
    client: &reqwest::Client,
    endpoint: &str,
    query: &str,
    num_results: usize,
) -> Result<ToolResult, ToolError> {
    let url = format!(
        "{}/search?q={}&format=json&pageno=1",
        endpoint.trim_end_matches('/'),
        urlencoding::encode(query)
    );

    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(15))
        .header(reqwest::header::USER_AGENT, "Holt/1.0 (Agent Harness)")
        .send()
        .await
        .map_err(|e| ToolError {
            code: ToolErrorCode::NetworkError,
            message: format!("Search request failed: {}", e),
            retryable: true,
        })?;

    let body: serde_json::Value = response.json().await.map_err(|e| ToolError {
        code: ToolErrorCode::NetworkError,
        message: format!("Failed to parse search results: {}", e),
        retryable: true,
    })?;

    let results = body
        .get("results")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .take(num_results)
                .map(|r| {
                    json!({
                        "title": r.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                        "url": r.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                        "snippet": r.get("content").and_then(|v| v.as_str()).unwrap_or(""),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let total = body.get("number_of_results").and_then(|v| v.as_u64());

    Ok(ToolResult {
        content: json!({
            "query": query,
            "results": results,
            "total_results_estimate": total,
        }),
        truncated: false,
        trace_id: None,
        image_content: None,
    })
}

// URL encoding now uses the `urlencoding` crate (re-exported via Cargo.toml).
// The previous hand-rolled implementation was replaced for correctness with
// multi-byte Unicode and RFC 3986 edge cases.

#[cfg(test)]
mod tests {
    #[test]
    fn test_url_encoding_special_chars() {
        let encoded = urlencoding::encode("hello world & foo=bar");
        assert!(encoded.contains("hello"));
        assert!(!encoded.contains(' '));
        assert!(!encoded.contains('&'));
        assert!(!encoded.contains('='));
    }

    #[test]
    fn test_url_encoding_unicode() {
        let encoded = urlencoding::encode("café résumé");
        // Should percent-encode the non-ASCII chars
        assert!(encoded.contains('%'));
        assert!(!encoded.contains('é'));
    }

    #[test]
    fn test_url_encoding_plain_ascii() {
        let encoded = urlencoding::encode("simple");
        assert_eq!(&*encoded, "simple");
    }

    #[test]
    fn test_url_encoding_empty() {
        let encoded = urlencoding::encode("");
        assert_eq!(&*encoded, "");
    }
}
