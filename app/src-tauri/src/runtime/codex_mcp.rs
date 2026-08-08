use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::net::TcpListener;
use tokio::sync::RwLock;

use crate::config::agent_config::AgentConfigFile;
use crate::providers::types::{StreamEvent, StreamEventType};
use crate::tools::registry::ToolRegistry;
use crate::tools::types::ToolContext;
use crate::AppState;

#[derive(Clone)]
struct CodexMcpState {
    app_state: Arc<AppState>,
    app_handle: AppHandle,
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

pub fn spawn_server(app_state: Arc<AppState>, app_handle: AppHandle) {
    let shutdown = app_state.shutdown_token.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = run_server(app_state, app_handle, shutdown).await {
            tracing::warn!("Codex MCP server failed to start: {err}");
        }
    });
}

async fn run_server(
    app_state: Arc<AppState>,
    app_handle: AppHandle,
    shutdown: tokio_util::sync::CancellationToken,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| {
        tracing::error!("Codex MCP server failed to bind: {e}");
        e
    })?;
    let addr = listener.local_addr()?;
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    {
        let mut stored = app_state.codex_mcp_url.write().await;
        *stored = Some(url.clone());
    }

    tracing::info!(url, "Codex MCP server listening");

    let router = Router::new()
        .route("/mcp", post(handle_mcp))
        .with_state(CodexMcpState {
            app_state,
            app_handle,
        });

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown.cancelled().await;
        })
        .await?;

    Ok(())
}

async fn handle_mcp(
    State(state): State<CodexMcpState>,
    Query(query): Query<HashMap<String, String>>,
    Json(request): Json<JsonRpcRequest>,
) -> Response {
    let is_notification = request.id.is_none();
    let agent_id = query.get("agent_id").cloned();
    let response = dispatch_request(&state, agent_id.as_deref(), request).await;

    if is_notification && response.error.is_none() {
        StatusCode::ACCEPTED.into_response()
    } else {
        (StatusCode::OK, Json(response)).into_response()
    }
}

async fn dispatch_request(
    state: &CodexMcpState,
    agent_id: Option<&str>,
    request: JsonRpcRequest,
) -> JsonRpcResponse {
    let id = request.id.clone();

    if request
        .jsonrpc
        .as_deref()
        .is_some_and(|version| version != "2.0")
    {
        return error_response(id, -32600, "JSON-RPC version must be 2.0");
    }

    match request.method.as_str() {
        "initialize" => success_response(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {
                        "listChanged": false
                    }
                },
                "serverInfo": {
                    "name": "holt",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "notifications/initialized" => success_response(id, Value::Null),
        "tools/list" => match build_registry(agent_id, &state.app_state).await {
            Ok(registry) => {
                let profile = match agent_id {
                    Some(aid) => resolve_profile_for_agent(aid).await,
                    None => crate::config::profile::Profile::lookup("sandboxed")
                        .expect("sandboxed profile must exist"),
                };
                let tools = registry
                    .definitions()
                    .into_iter()
                    .filter_map(openai_tool_to_mcp_tool)
                    .filter(|tool| {
                        let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        filter_tool_for_profile(name, &profile)
                    })
                    .collect::<Vec<_>>();
                tracing::info!(
                    agent_id = agent_id.unwrap_or("unknown"),
                    profile = profile.name,
                    tool_count = tools.len(),
                    "Codex MCP tools/list"
                );
                success_response(id, json!({ "tools": tools }))
            }
            Err(message) => error_response(id, -32000, message),
        },
        "tools/call" => match call_tool(state, agent_id, request.params).await {
            Ok(result) => success_response(id, result),
            Err(message) => error_response(id, -32000, message),
        },
        other => error_response(id, -32601, format!("Unknown MCP method: {other}")),
    }
}

async fn build_registry(
    agent_id: Option<&str>,
    app_state: &Arc<AppState>,
) -> Result<ToolRegistry, String> {
    let agent_id = agent_id.ok_or_else(|| "Missing agent_id query parameter".to_string())?;
    let tools_config = load_tools_config(agent_id);
    Ok(
        ToolRegistry::build_for_codex_agent_with_tools(&tools_config, Some(&app_state.plugin_host))
            .await,
    )
}

/// Decide whether a tool is exposed under a given profile.
/// Always-on tools pass through any profile. Unclassified tools also pass
/// through (the profile system can only gate what it classifies).
fn filter_tool_for_profile(tool_name: &str, profile: &crate::config::profile::Profile) -> bool {
    use crate::config::profile::{always_on_tool_names, tool_family_for};
    if always_on_tool_names().contains(&tool_name) {
        return true;
    }
    match tool_family_for(tool_name) {
        Some(family) => profile.allows_family(family),
        None => true,
    }
}

/// Resolve the permission profile for a given agent by reading
/// ~/.config/holt/agents/<id>/config.toml. Falls back to the
/// sandboxed default if the file is missing or has no profile field.
///
/// Uses a tolerant slice parser that only inspects the `profile` field —
/// this lets partial/legacy config.toml files still drive profile selection
/// without needing every required AgentConfigFile field to be present.
async fn resolve_profile_for_agent(agent_id: &str) -> crate::config::profile::Profile {
    use crate::config::app_config::agent_dir;
    use crate::config::profile::Profile;

    #[derive(serde::Deserialize)]
    struct ProfileSlice {
        #[serde(default)]
        profile: Option<String>,
    }

    let fallback = || Profile::lookup("sandboxed").expect("sandboxed profile must exist");

    let config_path = agent_dir(agent_id).join("config.toml");
    let Ok(text) = std::fs::read_to_string(&config_path) else {
        tracing::debug!(
            ?config_path,
            agent_id,
            "no per-agent config.toml; using sandboxed profile"
        );
        return fallback();
    };

    let slice: ProfileSlice = match toml::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                ?config_path,
                agent_id,
                error = %e,
                "failed to parse per-agent config slice; using sandboxed"
            );
            return fallback();
        }
    };

    let Some(name) = slice.profile else {
        tracing::debug!(
            agent_id,
            "no profile field in agent config; using sandboxed"
        );
        return fallback();
    };

    Profile::lookup(&name).unwrap_or_else(|| {
        tracing::warn!(
            agent_id,
            profile = name,
            "unknown profile name in agent config; using sandboxed"
        );
        fallback()
    })
}

async fn call_tool(
    state: &CodexMcpState,
    agent_id: Option<&str>,
    params: Value,
) -> Result<Value, String> {
    let agent_id = agent_id.ok_or_else(|| "Missing agent_id query parameter".to_string())?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "tools/call params.name must be a string".to_string())?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // Enforce the agent's profile at the call boundary as well as the listing.
    let agent_profile = resolve_profile_for_agent(agent_id).await;
    if !filter_tool_for_profile(name, &agent_profile) {
        return Err(format!(
            "Tool '{name}' is not available under profile '{}'",
            agent_profile.name
        ));
    }

    let registry = build_registry(Some(agent_id), &state.app_state).await?;
    let tool_ctx = build_tool_context(agent_id, &state.app_state, &state.app_handle).await?;

    emit_stream_event(
        &state.app_handle,
        agent_id,
        StreamEventType::ToolCall,
        json!({
            "tool": name,
            "arguments": arguments,
            "tool_call_id": format!("codex-mcp-{}-{}", name, Utc::now().timestamp_millis()),
            "authority_class": registry.get_authority_class(name),
        }),
    );

    match registry.execute(name, arguments, &tool_ctx).await {
        Ok(result) => {
            emit_stream_event(
                &state.app_handle,
                agent_id,
                StreamEventType::ToolResult,
                json!({
                    "tool": name,
                    "result": result.content,
                    "status": "done",
                }),
            );
            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": stringify_tool_content(&result.content)
                }],
                "isError": false
            }))
        }
        Err(err) => {
            let message = err.to_string();
            emit_stream_event(
                &state.app_handle,
                agent_id,
                StreamEventType::ToolResult,
                json!({
                    "tool": name,
                    "result": { "error": message },
                    "status": "failed",
                }),
            );
            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": message
                }],
                "isError": true
            }))
        }
    }
}

async fn build_tool_context(
    agent_id: &str,
    app_state: &Arc<AppState>,
    app_handle: &AppHandle,
) -> Result<ToolContext, String> {
    let (working_directory, session_id) = {
        let agents = app_state.agents.read().await;
        let agent = agents
            .iter()
            .find(|agent| agent.id == agent_id)
            .ok_or_else(|| format!("Agent not found: {agent_id}"))?;
        (
            agent.working_directory.clone(),
            agent
                .agent_session_id
                .clone()
                .unwrap_or_else(|| format!("codex-mcp-{}", agent.id)),
        )
    };

    Ok(ToolContext {
        agent_id: agent_id.to_string(),
        working_directory: working_directory.clone(),
        workspace_root: working_directory,
        trace_engine: Arc::clone(&app_state.trace_engine),
        app_handle: Some(app_handle.clone()),
        config: Arc::new(RwLock::new(app_state.config.read().await.clone())),
        shell_manager: Arc::clone(&app_state.shell_manager),
        session_id,
        vfl_registry: Arc::clone(&app_state.vfl_registry),
        subagent_manager: Arc::clone(&app_state.subagent_manager),
        agents: Arc::clone(&app_state.agents),
        a2a_registry: Arc::clone(&app_state.a2a_registry),
        a2a_send_guard: None,
        app_state: Some(Arc::clone(app_state)),
        http_client: app_state.http_client.clone(),
    })
}

fn load_tools_config(agent_id: &str) -> crate::config::agent_config::AgentToolsBlock {
    let config_path = crate::config::app_config::agent_dir(agent_id).join("config.toml");
    if config_path.exists() {
        AgentConfigFile::load(&config_path)
            .map(|cfg| cfg.tools)
            .unwrap_or_default()
    } else {
        crate::config::agent_config::AgentToolsBlock::default()
    }
}

fn openai_tool_to_mcp_tool(tool: Value) -> Option<Value> {
    let function = tool.get("function")?;
    Some(json!({
        "name": function.get("name")?.clone(),
        "description": function.get("description").cloned().unwrap_or_else(|| json!("")),
        "inputSchema": function
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
    }))
}

fn stringify_tool_content(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn success_response(id: Option<Value>, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

fn error_response(id: Option<Value>, code: i64, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
        }),
    }
}

fn emit_stream_event(
    app_handle: &AppHandle,
    agent_id: &str,
    event_type: StreamEventType,
    data: Value,
) {
    let _ = app_handle.emit(
        "agent-stream",
        StreamEvent {
            agent_id: agent_id.to_string(),
            event_type,
            data,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_openai_function_definition_to_mcp_tool() {
        let tool = json!({
            "type": "function",
            "function": {
                "name": "list_available_tools",
                "description": "List tools",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        });

        let converted = openai_tool_to_mcp_tool(tool).expect("converted tool");

        assert_eq!(converted["name"], "list_available_tools");
        assert_eq!(converted["description"], "List tools");
        assert_eq!(converted["inputSchema"]["type"], "object");
    }

    #[test]
    fn stringifies_structured_tool_content_as_json() {
        let content = json!({ "status": "ok" });
        let text = stringify_tool_content(&content);

        assert!(text.contains("\"status\": \"ok\""));
    }

    #[test]
    fn initialized_notification_has_no_json_rpc_id() {
        let request = JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: None,
            method: "notifications/initialized".to_string(),
            params: Value::Null,
        };

        assert!(request.id.is_none());
    }
}

#[cfg(test)]
mod profile_filter_tests {
    use super::*;
    use crate::config::profile::Profile;

    #[test]
    fn restricted_profile_strips_filesystem_write_tools() {
        let profile = Profile::lookup("restricted").unwrap();
        assert!(!filter_tool_for_profile("write_file", &profile));
        assert!(!filter_tool_for_profile("delete_file", &profile));
        assert!(filter_tool_for_profile("read_file", &profile));
    }

    #[test]
    fn unrestricted_profile_includes_all_classified_tools() {
        let profile = Profile::lookup("unrestricted").unwrap();
        assert!(filter_tool_for_profile("write_file", &profile));
        assert!(filter_tool_for_profile("bash", &profile));
        assert!(filter_tool_for_profile("update_persona", &profile));
    }

    #[test]
    fn always_on_tools_pass_through_any_profile() {
        let profile = Profile::lookup("restricted").unwrap();
        assert!(filter_tool_for_profile("list_agents", &profile));
        assert!(filter_tool_for_profile("alert_user", &profile));
        assert!(filter_tool_for_profile("get_agent_info", &profile));
    }

    #[test]
    fn unclassified_tools_pass_through() {
        // Tools not in tool_family_for() are not yet covered by the profile system.
        // They pass through unfiltered — the profile system can only gate what it classifies.
        let profile = Profile::lookup("restricted").unwrap();
        assert!(filter_tool_for_profile(
            "some_unknown_future_tool",
            &profile
        ));
    }
}
