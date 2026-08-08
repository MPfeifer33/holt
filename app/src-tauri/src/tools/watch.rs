use async_trait::async_trait;
use serde_json::json;

use crate::runtime::triggers::{WatchConfig, WatchEventType};
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

const MAX_CUSTOM_WATCHES: usize = 10;

fn all_events() -> Vec<WatchEventType> {
    vec![
        WatchEventType::Create,
        WatchEventType::Modify,
        WatchEventType::Delete,
    ]
}

// ============================================
// watch_path
// ============================================

pub struct WatchPathTool;

#[async_trait]
impl Tool for WatchPathTool {
    fn name(&self) -> &str {
        "watch_path"
    }

    fn description(&self) -> &str {
        "Watch a file or directory for changes. You'll be notified when files are created, modified, or deleted. Max 10 watches."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"path": "/home/user/project/src", "glob": "*.rs"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to file or directory to watch"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Watch subdirectories (default true)"
                },
                "glob": {
                    "type": "string",
                    "description": "Optional file pattern filter (e.g., '*.md', '*.rs')"
                },
                "events": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["create", "modify", "delete"] },
                    "description": "Event types to watch (default: all three)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "path is required".into(),
                retryable: false,
            })?;

        let path = std::path::PathBuf::from(path_str);
        if !path.is_absolute() {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "path must be absolute".into(),
                retryable: false,
            });
        }
        if !path.exists() {
            return Err(ToolError {
                code: ToolErrorCode::FileNotFound,
                message: format!("path does not exist: {}", path_str),
                retryable: false,
            });
        }

        let recursive = arguments
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let glob_filter = arguments
            .get("glob")
            .and_then(|v| v.as_str())
            .map(String::from);

        let events = if let Some(arr) = arguments.get("events").and_then(|v| v.as_array()) {
            arr.iter()
                .filter_map(|v| match v.as_str()? {
                    "create" => Some(WatchEventType::Create),
                    "modify" => Some(WatchEventType::Modify),
                    "delete" => Some(WatchEventType::Delete),
                    _ => None,
                })
                .collect()
        } else {
            all_events()
        };

        let app_state = context.app_state.as_ref().ok_or_else(|| ToolError {
            code: ToolErrorCode::PermissionDenied,
            message: "AppState not available".into(),
            retryable: false,
        })?;

        // Check limit
        let current_count = app_state
            .trigger_registry
            .custom_watch_count(&context.agent_id)
            .await;
        if current_count >= MAX_CUSTOM_WATCHES {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!(
                    "Maximum {} custom watches reached. Use unwatch_path to remove one first.",
                    MAX_CUSTOM_WATCHES
                ),
                retryable: false,
            });
        }

        let config = WatchConfig {
            path: path.clone(),
            recursive,
            glob_filter,
            events,
        };

        let job = app_state
            .trigger_registry
            .create_file_watch(
                format!(
                    "watch-{}",
                    path.file_name()
                        .unwrap_or_default()
                        .to_str()
                        .unwrap_or("unknown")
                ),
                context.agent_id.clone(),
                config,
                false,
            )
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::PermissionDenied,
                message: e,
                retryable: false,
            })?;

        // Register with live watcher immediately
        app_state
            .file_watch_manager
            .register_watch(&path, recursive)
            .await;

        Ok(ToolResult {
            content: json!({
                "success": true,
                "watch_id": job.id,
                "path": path_str,
                "recursive": recursive,
                "message": format!("Now watching {} for changes", path_str),
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

// ============================================
// unwatch_path
// ============================================

pub struct UnwatchPathTool;

#[async_trait]
impl Tool for UnwatchPathTool {
    fn name(&self) -> &str {
        "unwatch_path"
    }

    fn description(&self) -> &str {
        "Stop watching a file or directory."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"path": "/home/user/project/src"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to stop watching"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "path is required".into(),
                retryable: false,
            })?;

        let path = std::path::PathBuf::from(path_str);
        let app_state = context.app_state.as_ref().ok_or_else(|| ToolError {
            code: ToolErrorCode::PermissionDenied,
            message: "AppState not available".into(),
            retryable: false,
        })?;

        let watches = app_state
            .trigger_registry
            .file_watch_jobs_for_agent(&context.agent_id)
            .await;
        let matching = watches.iter().find(|j| {
            j.watch_config
                .as_ref()
                .map(|c| c.path == path)
                .unwrap_or(false)
        });

        match matching {
            Some(job) if job.internal => {
                let _ = app_state.trigger_registry.toggle(&job.id).await;
                // Unregister from live watcher
                app_state.file_watch_manager.unregister_watch(&path).await;
                Ok(ToolResult {
                    content: json!({
                        "success": true,
                        "path": path_str,
                        "message": format!("Default watch on {} disabled (can be re-enabled in Triggers panel)", path_str),
                    }),
                    truncated: false,
                    trace_id: None,
                    image_content: None,
                })
            }
            Some(job) => {
                app_state
                    .trigger_registry
                    .delete(&job.id)
                    .await
                    .map_err(|e| ToolError {
                        code: ToolErrorCode::PermissionDenied,
                        message: e,
                        retryable: false,
                    })?;
                // Unregister from live watcher
                app_state.file_watch_manager.unregister_watch(&path).await;
                Ok(ToolResult {
                    content: json!({
                        "success": true,
                        "path": path_str,
                        "message": format!("Stopped watching {}", path_str),
                    }),
                    truncated: false,
                    trace_id: None,
                    image_content: None,
                })
            }
            None => Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("No active watch found for {}", path_str),
                retryable: false,
            }),
        }
    }
}

// ============================================
// list_watches
// ============================================

pub struct ListWatchesTool;

#[async_trait]
impl Tool for ListWatchesTool {
    fn name(&self) -> &str {
        "list_watches"
    }

    fn description(&self) -> &str {
        "List your active file/directory watches."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(
        &self,
        _arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let app_state = context.app_state.as_ref().ok_or_else(|| ToolError {
            code: ToolErrorCode::PermissionDenied,
            message: "AppState not available".into(),
            retryable: false,
        })?;

        let watches = app_state
            .trigger_registry
            .file_watch_jobs_for_agent(&context.agent_id)
            .await;

        let entries: Vec<serde_json::Value> = watches.iter().map(|j| {
            let config = j.watch_config.as_ref();
            json!({
                "id": j.id,
                "name": j.name,
                "path": config.map(|c| c.path.display().to_string()).unwrap_or_default(),
                "recursive": config.map(|c| c.recursive).unwrap_or(false),
                "glob": config.and_then(|c| c.glob_filter.as_ref()),
                "events": config.map(|c| c.events.iter().map(|e| format!("{:?}", e).to_lowercase()).collect::<Vec<_>>()).unwrap_or_default(),
                "enabled": j.enabled,
                "internal": j.internal,
                "fire_count": j.fire_count,
                "last_fired": j.last_fired.map(|t| t.to_rfc3339()),
            })
        }).collect();

        Ok(ToolResult {
            content: json!({
                "watches": entries,
                "count": entries.len(),
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
