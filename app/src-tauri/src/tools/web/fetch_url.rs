use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct FetchUrlTool;

#[async_trait::async_trait]
impl Tool for FetchUrlTool {
    fn name(&self) -> &'static str {
        "fetch_url"
    }

    fn description(&self) -> &'static str {
        "Fetch the content of a URL. Extracts readable text from HTML by default."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"url": "https://example.com"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "URL to fetch (http:// or https://)" },
                "extract_text": { "type": "boolean", "description": "Strip HTML tags (default: true)" },
                "max_bytes": { "type": "integer", "description": "Max response size (default: 1MB)" },
                "headers": { "type": "object", "description": "Custom request headers", "additionalProperties": { "type": "string" } }
            },
            "required": ["url"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let url = arguments
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: url".to_string(),
                retryable: false,
            })?;

        // Validate scheme
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Only http:// and https:// URLs are supported".to_string(),
                retryable: false,
            });
        }

        let extract_text = arguments
            .get("extract_text")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let max_bytes = arguments
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(1_048_576); // 1MB

        let custom_headers: Option<HashMap<String, String>> = arguments
            .get("headers")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        // Sliding window rate limiter: max 60 requests per 60-second window
        static RATE_LIMITER: Mutex<Option<(Instant, u64)>> = Mutex::new(None);
        {
            let mut guard = RATE_LIMITER.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            match guard.as_mut() {
                Some((window_start, count)) => {
                    if window_start.elapsed() > Duration::from_secs(60) {
                        // Window expired — reset
                        *window_start = now;
                        *count = 1;
                    } else if *count >= 60 {
                        return Err(ToolError {
                            code: ToolErrorCode::RateLimited,
                            message: "Rate limit exceeded: max 60 requests per minute. Try again shortly.".to_string(),
                            retryable: true,
                        });
                    } else {
                        *count += 1;
                    }
                }
                None => {
                    *guard = Some((now, 1));
                }
            }
        }

        let mut request = context
            .http_client
            .get(url)
            .timeout(std::time::Duration::from_secs(30))
            .header(reqwest::header::USER_AGENT, "Holt/1.0 (Agent Harness)");

        if let Some(headers) = custom_headers {
            for (k, v) in &headers {
                request = request.header(k.as_str(), v.as_str());
            }
        }

        let response = request.send().await.map_err(|e| ToolError {
            code: ToolErrorCode::NetworkError,
            message: format!("Request failed: {}", e),
            retryable: true,
        })?;

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();
        let final_url = response.url().to_string();

        // Check for non-text content
        if content_type.starts_with("image/")
            || content_type.starts_with("audio/")
            || content_type.starts_with("video/")
            || content_type.contains("octet-stream")
        {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!(
                    "Non-text content type: {}. Cannot fetch binary content.",
                    content_type
                ),
                retryable: false,
            });
        }

        let bytes = response.bytes().await.map_err(|e| ToolError {
            code: ToolErrorCode::NetworkError,
            message: format!("Failed to read response: {}", e),
            retryable: true,
        })?;

        let truncated = bytes.len() as u64 > max_bytes;
        let body_bytes = if truncated {
            &bytes[..max_bytes as usize]
        } else {
            &bytes[..]
        };

        let raw_text = String::from_utf8_lossy(body_bytes);

        let content = if extract_text && content_type.contains("html") {
            extract_text_from_html(&raw_text)
        } else {
            raw_text.to_string()
        };

        Ok(ToolResult {
            content: json!({
                "url": final_url,
                "status": status,
                "content": content,
                "content_type": content_type,
                "truncated": truncated,
            }),
            truncated,
            trace_id: None,
            image_content: None,
        })
    }
}

/// Strip HTML tags and extract readable text using the scraper crate.
fn extract_text_from_html(html: &str) -> String {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);

    // Try to get main content, falling back to body
    let selectors = ["main", "article", "body"];

    for sel_str in &selectors {
        if let Ok(selector) = Selector::parse(sel_str) {
            if let Some(element) = document.select(&selector).next() {
                let text: String = element
                    .text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if !text.is_empty() {
                    return text;
                }
            }
        }
    }

    // Fallback: all text from document
    document
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
