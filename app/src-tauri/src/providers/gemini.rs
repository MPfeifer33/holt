//! Google Gemini provider.
//!
//! Handles streaming SSE responses from the Gemini `streamGenerateContent` API,
//! including message format conversion (OpenAI → Gemini), tool/function call
//! translation, and JSON Schema field stripping for Gemini compatibility.

use async_trait::async_trait;
use futures::StreamExt;

use crate::providers::types::*;
use crate::runtime::agent::ToolCallRecord;

// =============================================================================
// SSE deserialization structs
// =============================================================================

/// Gemini SSE chunk (from streamGenerateContent?alt=sse).
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiChunk {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    finish_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct GeminiContent {
    parts: Option<Vec<GeminiPart>>,
    #[allow(dead_code)]
    role: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    text: Option<String>,
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, serde::Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
}

// =============================================================================
// Message/tool conversion helpers
// =============================================================================

/// Convert OpenAI-format messages to Gemini contents + system instruction.
/// Returns (system_instruction, contents).
fn convert_to_gemini_messages(
    messages: &[serde_json::Value],
) -> (Option<serde_json::Value>, Vec<serde_json::Value>) {
    let mut system_instruction = None;
    let mut contents: Vec<serde_json::Value> = Vec::new();

    for msg in messages {
        let role = msg["role"].as_str().unwrap_or("user");

        match role {
            "system" => {
                // Use Gemini's systemInstruction field
                let text = msg["content"].as_str().unwrap_or("");
                system_instruction = Some(serde_json::json!({
                    "parts": [{"text": text}]
                }));
            }
            "assistant" => {
                // Check if this is an assistant message with tool calls
                if let Some(tool_calls) = msg.get("tool_calls") {
                    if let Some(tcs) = tool_calls.as_array() {
                        let mut parts: Vec<serde_json::Value> = Vec::new();
                        // Include text content if any
                        let text = msg["content"].as_str().unwrap_or("");
                        if !text.is_empty() {
                            parts.push(serde_json::json!({"text": text}));
                        }
                        // Convert tool calls to functionCall parts
                        for tc in tcs {
                            let name = tc["function"]["name"].as_str().unwrap_or("");
                            let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                            let args: serde_json::Value =
                                serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                            parts.push(serde_json::json!({
                                "functionCall": {
                                    "name": name,
                                    "args": args,
                                }
                            }));
                        }
                        contents.push(serde_json::json!({
                            "role": "model",
                            "parts": parts,
                        }));
                    }
                } else {
                    let text = msg["content"].as_str().unwrap_or("");
                    contents.push(serde_json::json!({
                        "role": "model",
                        "parts": [{"text": text}]
                    }));
                }
            }
            "tool" => {
                // Tool results become functionResponse parts under "user" role
                // Gemini expects tool results grouped as user messages
                let tool_call_id = msg["tool_call_id"].as_str().unwrap_or("");
                let content_str = msg["content"].as_str().unwrap_or("{}");
                let response: serde_json::Value = serde_json::from_str(content_str)
                    .unwrap_or(serde_json::json!({"result": content_str}));

                // Try to find the tool name from the tool_call_id by looking back
                // at previous assistant messages. Fall back to the id itself.
                let tool_name = find_tool_name_for_id(messages, tool_call_id)
                    .unwrap_or_else(|| tool_call_id.to_string());

                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": tool_name,
                            "response": response,
                        }
                    }]
                }));
            }
            _ => {
                // "user" and anything else
                let content_value = msg.get("content");
                let mut parts: Vec<serde_json::Value> = Vec::new();

                if let Some(blocks) = content_value.and_then(|c| c.as_array()) {
                    // Multimodal content blocks — convert to Gemini parts
                    for block in blocks {
                        match block.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                    parts.push(serde_json::json!({"text": text}));
                                }
                            }
                            Some("image") => {
                                if let (Some(data), Some(mime)) = (
                                    block.get("data").and_then(|d| d.as_str()),
                                    block.get("media_type").and_then(|m| m.as_str()),
                                ) {
                                    parts.push(serde_json::json!({
                                        "inlineData": {
                                            "mimeType": mime,
                                            "data": data,
                                        }
                                    }));
                                }
                            }
                            _ => {}
                        }
                    }
                } else {
                    // Plain text (existing behavior)
                    let text = content_value.and_then(|c| c.as_str()).unwrap_or("");
                    parts.push(serde_json::json!({"text": text}));
                }

                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": parts,
                }));
            }
        }
    }

    (system_instruction, contents)
}

/// Look back through messages to find the tool name for a given tool_call_id.
fn find_tool_name_for_id(messages: &[serde_json::Value], tool_call_id: &str) -> Option<String> {
    for msg in messages.iter().rev() {
        if let Some(tool_calls) = msg.get("tool_calls") {
            if let Some(tcs) = tool_calls.as_array() {
                for tc in tcs {
                    if tc["id"].as_str() == Some(tool_call_id) {
                        return tc["function"]["name"].as_str().map(|s| s.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Convert OpenAI-format tool definitions to Gemini functionDeclarations.
fn convert_tools_to_gemini(tools: &[serde_json::Value]) -> serde_json::Value {
    let declarations: Vec<serde_json::Value> = tools
        .iter()
        .filter_map(|tool| {
            let func = tool.get("function")?;
            let mut decl = serde_json::json!({
                "name": func["name"],
                "description": func["description"],
            });
            if let Some(params) = func.get("parameters") {
                let mut cleaned = params.clone();
                strip_unsupported_schema_fields(&mut cleaned);
                decl["parameters"] = cleaned;
            }
            Some(decl)
        })
        .collect();

    serde_json::json!([{
        "functionDeclarations": declarations,
    }])
}

/// Recursively remove JSON Schema fields that Gemini doesn't support.
/// Gemini accepts: type, properties, required, items, enum, description, format, nullable
/// Gemini rejects: additionalProperties, $schema, $ref, definitions, allOf, anyOf, oneOf, not, etc.
fn strip_unsupported_schema_fields(value: &mut serde_json::Value) {
    if let Some(obj) = value.as_object_mut() {
        // Remove unsupported top-level fields
        const UNSUPPORTED: &[&str] = &[
            "additionalProperties",
            "$schema",
            "$ref",
            "definitions",
            "allOf",
            "anyOf",
            "oneOf",
            "not",
            "patternProperties",
            "minProperties",
            "maxProperties",
            "minItems",
            "maxItems",
            "uniqueItems",
            "minimum",
            "maximum",
            "exclusiveMinimum",
            "exclusiveMaximum",
            "multipleOf",
            "minLength",
            "maxLength",
            "pattern",
            "default",
            "examples",
            "title",
        ];
        for field in UNSUPPORTED {
            obj.remove(*field);
        }

        // Recurse into nested schemas
        if let Some(props) = obj.get_mut("properties") {
            if let Some(props_obj) = props.as_object_mut() {
                for (_, prop_value) in props_obj.iter_mut() {
                    strip_unsupported_schema_fields(prop_value);
                }
            }
        }
        if let Some(items) = obj.get_mut("items") {
            strip_unsupported_schema_fields(items);
        }
    }
}

// =============================================================================
// GeminiProvider
// =============================================================================

#[derive(Debug)]
pub struct GeminiProvider {
    pub(crate) endpoint: String,
    pub(crate) api_key: Option<String>,
    pub(crate) model: String,
}

impl GeminiProvider {
    pub fn new(endpoint: &str, api_key: Option<&str>, model: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            api_key: api_key.map(|s| s.to_string()),
            model: model.to_string(),
        }
    }
}

#[async_trait]
impl CompletionProvider for GeminiProvider {
    fn name(&self) -> &'static str {
        "Google Gemini"
    }

    async fn send_completion(
        &self,
        client: &reqwest::Client,
        messages: &[serde_json::Value],
        tools: Option<&[serde_json::Value]>,
        max_tokens: u32,
        on_token: &(dyn Fn(StreamEvent) + Send + Sync),
        agent_id: &str,
    ) -> Result<AssembledResponse, StreamError> {
        // Build URL: {base}/models/{model}:streamGenerateContent?alt=sse&key={key}
        let base = self.endpoint.trim_end_matches('/');
        let mut url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            base, self.model
        );
        if let Some(key) = &self.api_key {
            url.push_str(&format!("&key={}", key));
        }

        // Convert messages from OpenAI format to Gemini format
        let (system_instruction, contents) = convert_to_gemini_messages(messages);

        let mut body = serde_json::json!({
            "contents": contents,
            "generationConfig": {
                "maxOutputTokens": max_tokens,
            },
        });

        if let Some(si) = system_instruction {
            body["systemInstruction"] = si;
        }

        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = convert_tools_to_gemini(tools);
            }
        }

        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(StreamError::Network)?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            return Err(StreamError::RateLimited { retry_after });
        }
        if status.is_server_error() {
            return Err(StreamError::ServerError(status.as_u16()));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(StreamError::HttpError {
                status: status.as_u16(),
                body,
            });
        }

        // Parse SSE stream (Gemini with ?alt=sse sends standard `data: {...}` lines)
        let mut content = String::new();
        let mut tool_calls: Vec<ToolCallRecord> = Vec::new();
        let mut usage = None;
        let mut finish_reason = None;
        let mut tool_call_counter: u32 = 0;

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(StreamError::Network)?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim_end_matches('\r').to_string();
                buffer = buffer[line_end + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }

                    match serde_json::from_str::<GeminiChunk>(data) {
                        Err(_) => {}
                        Ok(chunk) => {
                            // Usage metadata
                            if let Some(um) = chunk.usage_metadata {
                                usage = Some(UsageInfo {
                                    prompt_tokens: um.prompt_token_count,
                                    completion_tokens: um.candidates_token_count,
                                    total_tokens: um.total_token_count,
                                });
                            }

                            if let Some(candidates) = chunk.candidates {
                                for candidate in candidates {
                                    if let Some(fr) = candidate.finish_reason {
                                        finish_reason = Some(fr);
                                    }
                                    if let Some(gemini_content) = candidate.content {
                                        if let Some(parts) = gemini_content.parts {
                                            for part in parts {
                                                // Text content
                                                if let Some(text) = part.text {
                                                    content.push_str(&text);
                                                    on_token(StreamEvent {
                                                        agent_id: agent_id.to_string(),
                                                        event_type: StreamEventType::Token,
                                                        data: serde_json::json!({"text": text}),
                                                    });
                                                }
                                                // Function calls
                                                if let Some(fc) = part.function_call {
                                                    tool_call_counter += 1;
                                                    let call_id = format!(
                                                        "gemini_call_{}",
                                                        tool_call_counter
                                                    );
                                                    tool_calls.push(ToolCallRecord {
                                                        id: call_id,
                                                        name: fc.name,
                                                        arguments: fc
                                                            .args
                                                            .unwrap_or(serde_json::json!({})),
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Process remaining buffer
        let remaining = buffer.trim();
        if let Some(data) = remaining.strip_prefix("data: ") {
            if data != "[DONE]" {
                if let Ok(chunk) = serde_json::from_str::<GeminiChunk>(data) {
                    if let Some(um) = chunk.usage_metadata {
                        usage = Some(UsageInfo {
                            prompt_tokens: um.prompt_token_count,
                            completion_tokens: um.candidates_token_count,
                            total_tokens: um.total_token_count,
                        });
                    }
                    if let Some(candidates) = chunk.candidates {
                        for candidate in candidates {
                            if let Some(fr) = candidate.finish_reason {
                                finish_reason = Some(fr);
                            }
                            if let Some(gemini_content) = candidate.content {
                                if let Some(parts) = gemini_content.parts {
                                    for part in parts {
                                        if let Some(text) = part.text {
                                            content.push_str(&text);
                                            on_token(StreamEvent {
                                                agent_id: agent_id.to_string(),
                                                event_type: StreamEventType::Token,
                                                data: serde_json::json!({"text": text}),
                                            });
                                        }
                                        if let Some(fc) = part.function_call {
                                            tool_call_counter += 1;
                                            let call_id =
                                                format!("gemini_call_{}", tool_call_counter);
                                            tool_calls.push(ToolCallRecord {
                                                id: call_id,
                                                name: fc.name,
                                                arguments: fc.args.unwrap_or(serde_json::json!({})),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Emit done event
        on_token(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Done,
            data: serde_json::json!({}),
        });

        Ok(AssembledResponse {
            content,
            tool_calls,
            usage,
            finish_reason,
            thinking_content: None,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- SSE parsing ---

    #[test]
    fn test_parse_gemini_text_chunk() {
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"Hello from Gemini"}],"role":"model"},"finishReason":"STOP"}]}"#;
        let chunk: GeminiChunk = serde_json::from_str(data).unwrap();
        let candidates = chunk.candidates.unwrap();
        let parts = candidates[0]
            .content
            .as_ref()
            .unwrap()
            .parts
            .as_ref()
            .unwrap();
        assert_eq!(parts[0].text.as_ref().unwrap(), "Hello from Gemini");
        assert_eq!(candidates[0].finish_reason.as_ref().unwrap(), "STOP");
    }

    #[test]
    fn test_parse_gemini_function_call_chunk() {
        let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read_file","args":{"path":"/test.txt"}}}],"role":"model"},"finishReason":"STOP"}]}"#;
        let chunk: GeminiChunk = serde_json::from_str(data).unwrap();
        let candidates = chunk.candidates.unwrap();
        let parts = candidates[0]
            .content
            .as_ref()
            .unwrap()
            .parts
            .as_ref()
            .unwrap();
        let fc = parts[0].function_call.as_ref().unwrap();
        assert_eq!(fc.name, "read_file");
        assert_eq!(fc.args.as_ref().unwrap()["path"], "/test.txt");
    }

    #[test]
    fn test_parse_gemini_usage_metadata() {
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"Hi"}],"role":"model"}}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":15}}"#;
        let chunk: GeminiChunk = serde_json::from_str(data).unwrap();
        let um = chunk.usage_metadata.unwrap();
        assert_eq!(um.prompt_token_count, Some(10));
        assert_eq!(um.candidates_token_count, Some(5));
        assert_eq!(um.total_token_count, Some(15));
    }

    // --- Message conversion ---

    #[test]
    fn test_convert_system_message() {
        let messages = vec![
            serde_json::json!({"role": "system", "content": "You are helpful."}),
            serde_json::json!({"role": "user", "content": "Hello"}),
        ];
        let (si, contents) = convert_to_gemini_messages(&messages);
        assert!(si.is_some());
        assert_eq!(si.unwrap()["parts"][0]["text"], "You are helpful.");
        assert_eq!(contents.len(), 1); // Only user message in contents
        assert_eq!(contents[0]["role"], "user");
    }

    #[test]
    fn test_convert_assistant_to_model() {
        let messages = vec![
            serde_json::json!({"role": "user", "content": "Hi"}),
            serde_json::json!({"role": "assistant", "content": "Hello!"}),
        ];
        let (_, contents) = convert_to_gemini_messages(&messages);
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["text"], "Hello!");
    }

    #[test]
    fn test_convert_tool_result_to_function_response() {
        let messages = vec![
            serde_json::json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"/test.txt\"}"}}]
            }),
            serde_json::json!({
                "role": "tool",
                "content": "{\"result\":\"file contents\"}",
                "tool_call_id": "call_1"
            }),
        ];
        let (_, contents) = convert_to_gemini_messages(&messages);
        assert_eq!(contents.len(), 2);
        // Tool result should be a user message with functionResponse
        assert_eq!(contents[1]["role"], "user");
        let fr = &contents[1]["parts"][0]["functionResponse"];
        assert_eq!(fr["name"], "read_file");
    }

    #[test]
    fn test_convert_tools_to_gemini_format() {
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    },
                    "required": ["path"]
                }
            }
        })];
        let gemini_tools = convert_tools_to_gemini(&tools);
        let decls = &gemini_tools[0]["functionDeclarations"];
        assert_eq!(decls[0]["name"], "read_file");
        assert_eq!(decls[0]["description"], "Read a file");
        assert!(decls[0]["parameters"].is_object());
    }

    #[test]
    fn test_strip_unsupported_schema_fields() {
        // Mimics the start_process tool's env parameter with additionalProperties
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "start_process",
                "description": "Start a process",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "Command to run"},
                        "env": {
                            "type": "object",
                            "description": "Environment variables",
                            "additionalProperties": {"type": "string"}
                        }
                    },
                    "required": ["command"]
                }
            }
        })];
        let gemini_tools = convert_tools_to_gemini(&tools);
        let params = &gemini_tools[0]["functionDeclarations"][0]["parameters"];
        // additionalProperties should be stripped
        assert!(params["properties"]["env"]
            .get("additionalProperties")
            .is_none());
        // But type and description should remain
        assert_eq!(params["properties"]["env"]["type"], "object");
        assert_eq!(
            params["properties"]["env"]["description"],
            "Environment variables"
        );
    }

    #[test]
    fn test_gemini_provider_construction() {
        let provider = GeminiProvider::new(
            "https://generativelanguage.googleapis.com/v1beta",
            Some("AIza-test"),
            "gemini-2.5-pro",
        );
        assert_eq!(provider.name(), "Google Gemini");
        assert_eq!(provider.model, "gemini-2.5-pro");
    }

    #[test]
    fn test_gemini_message_conversion_with_image() {
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "What is this?"},
                {"type": "image", "data": "abc", "media_type": "image/png"}
            ]
        })];
        let (_, result) = convert_to_gemini_messages(&messages);
        let parts = result[0]["parts"].as_array().unwrap();
        assert!(parts.iter().any(|p| p.get("text").is_some()));
        assert!(parts.iter().any(|p| p.get("inlineData").is_some()));
        let img = parts
            .iter()
            .find(|p| p.get("inlineData").is_some())
            .unwrap();
        assert_eq!(img["inlineData"]["mimeType"], "image/png");
        assert_eq!(img["inlineData"]["data"], "abc");
    }

    #[test]
    fn test_gemini_message_plain_text_unchanged() {
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": "Hello"
        })];
        let (_, result) = convert_to_gemini_messages(&messages);
        let parts = result[0]["parts"].as_array().unwrap();
        assert_eq!(parts[0]["text"], "Hello");
    }
}
