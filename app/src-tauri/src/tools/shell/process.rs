use serde_json::json;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use super::shell_config::ShellConfig;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

/// A background process managed by the ShellManager.
pub struct BackgroundProcess {
    pub id: String,
    pub label: String,
    pub agent_id: String,
    pub output_buffer: Arc<Mutex<VecDeque<String>>>,
    pub stdin: Option<tokio::process::ChildStdin>,
    pub running: Arc<Mutex<bool>>,
    pub exit_code: Arc<Mutex<Option<i32>>>,
    pub started_at: std::time::Instant,
    _child: Arc<Mutex<Child>>,
}

/// How a background command is launched: the program to exec and its
/// argument list. Sandboxed agents run inside the same bwrap namespace the
/// PTY shell uses; unsandboxed agents go straight to the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: String,
    pub args: Vec<String>,
}

/// Build the launch plan for `command` in `cwd` under the agent's sandbox
/// (`None` = no config, unrestricted). Fails closed: restricted agents
/// never start processes, and a sandboxed process cannot start outside
/// its workspace.
pub fn launch_plan(
    command: &str,
    cwd: &std::path::Path,
    sandbox: Option<(&crate::config::agent_config::ExecutionSandboxBlock, &std::path::Path)>,
) -> Result<LaunchPlan, ToolError> {
    let config = ShellConfig::for_current_os();
    match sandbox {
        Some((cfg, _)) if cfg.is_restricted() => Err(ToolError {
            code: ToolErrorCode::PermissionDenied,
            message: "Restricted agents cannot start processes".to_string(),
            retryable: false,
        }),
        Some((cfg, workspace_root)) if cfg.is_sandboxed() => {
            if !cwd.starts_with(workspace_root) {
                return Err(ToolError {
                    code: ToolErrorCode::PathOutOfBounds,
                    message: format!(
                        "Sandboxed agents can only start processes inside the workspace ({}); {} is outside it",
                        workspace_root.display(),
                        cwd.display()
                    ),
                    retryable: false,
                });
            }
            let mut args = super::pty_session::PtySession::build_bwrap_args(cwd, workspace_root, cfg)?;
            args.push("-c".to_string());
            args.push(command.to_string());
            Ok(LaunchPlan { program: "bwrap".to_string(), args })
        }
        _ => {
            let mut args = config.shell_args.clone();
            args.push(command.to_string());
            Ok(LaunchPlan { program: config.shell_binary.to_string_lossy().into_owned(), args })
        }
    }
}

const MAX_BUFFER_LINES: usize = 1000;
/// Maximum lifetime for a background process before auto-kill (1 hour).
const MAX_BACKGROUND_PROCESS_LIFETIME_SECS: u64 = 3600;

impl BackgroundProcess {
    // One caller; the sandbox is the eighth argument on purpose.
    #[allow(clippy::too_many_arguments)]
    pub async fn spawn(
        id: String,
        label: String,
        agent_id: String,
        command: &str,
        working_dir: Option<&str>,
        default_cwd: &std::path::Path,
        env: Option<std::collections::HashMap<String, String>>,
        sandbox: Option<(crate::config::agent_config::ExecutionSandboxBlock, std::path::PathBuf)>,
    ) -> Result<Self, ToolError> {
        let cwd = working_dir
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| default_cwd.to_path_buf());

        let plan = launch_plan(command, &cwd, sandbox.as_ref().map(|(c, w)| (c, w.as_path())))?;
        let mut cmd = Command::new(&plan.program);
        cmd.args(&plan.args)
            .current_dir(&cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true);

        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        if let Some(env_vars) = env {
            for (k, v) in &env_vars {
                cmd.env(k, v);
            }
        }

        let mut child = cmd.spawn().map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to spawn process: {}", e),
            retryable: true,
        })?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();
        let buffer: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        let running = Arc::new(Mutex::new(true));
        let exit_code: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
        let child = Arc::new(Mutex::new(child));

        // Spawn task to read stdout
        if let Some(stdout) = stdout {
            let buf = buffer.clone();
            tauri::async_runtime::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut b = buf.lock().await;
                    if b.len() >= MAX_BUFFER_LINES {
                        b.pop_front();
                    }
                    b.push_back(line);
                }
            });
        }

        // Spawn task to read stderr
        if let Some(stderr) = stderr {
            let buf = buffer.clone();
            tauri::async_runtime::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut b = buf.lock().await;
                    if b.len() >= MAX_BUFFER_LINES {
                        b.pop_front();
                    }
                    b.push_back(format!("[stderr] {}", line));
                }
            });
        }

        // Spawn task to monitor process exit (with 1-hour max lifetime)
        {
            let child_ref = child.clone();
            let running_ref = running.clone();
            let exit_ref = exit_code.clone();
            tauri::async_runtime::spawn(async move {
                let mut child_guard = child_ref.lock().await;
                tokio::select! {
                    status = child_guard.wait() => {
                        *running_ref.lock().await = false;
                        if let Ok(s) = status {
                            *exit_ref.lock().await = s.code();
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(MAX_BACKGROUND_PROCESS_LIFETIME_SECS)) => {
                        tracing::warn!("Background process exceeded max lifetime ({}s), killing", MAX_BACKGROUND_PROCESS_LIFETIME_SECS);
                        let _ = child_guard.kill().await;
                        *running_ref.lock().await = false;
                        *exit_ref.lock().await = Some(-9);
                    }
                }
            });
        }

        Ok(Self {
            id,
            label,
            agent_id,
            output_buffer: buffer,
            stdin,
            running,
            exit_code,
            started_at: std::time::Instant::now(),
            _child: child,
        })
    }
}

// --- start_process ---

pub struct StartProcessTool;

#[async_trait::async_trait]
impl Tool for StartProcessTool {
    fn name(&self) -> &'static str {
        "start_process"
    }

    fn description(&self) -> &'static str {
        "Start a long-running background process. Returns immediately with a process ID."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Command to run" },
                "label": { "type": "string", "description": "Human-readable label" },
                "working_dir": { "type": "string", "description": "Override working directory" },
                "env": { "type": "object", "description": "Additional environment variables", "additionalProperties": { "type": "string" } }
            },
            "required": ["command"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let command = arguments
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: command".to_string(),
                retryable: false,
            })?;

        let label = arguments
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or(command);

        let working_dir = arguments.get("working_dir").and_then(|v| v.as_str());

        let env: Option<std::collections::HashMap<String, String>> = arguments
            .get("env")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        // Check limits
        {
            let manager = context.shell_manager.read().await;
            let agent_count = manager
                .processes
                .values()
                .filter(|p| p.agent_id == context.agent_id)
                .count();
            if agent_count >= 5 {
                return Err(ToolError {
                    code: ToolErrorCode::ProcessLimitReached,
                    message: "Maximum 5 background processes per agent".to_string(),
                    retryable: false,
                });
            }
            if manager.processes.len() >= 20 {
                return Err(ToolError {
                    code: ToolErrorCode::ProcessLimitReached,
                    message: "Maximum 20 total background processes".to_string(),
                    retryable: false,
                });
            }
        }

        let process_id = format!(
            "proc-{}",
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("000")
        );

        // The agent's sandbox: what the PTY shell was spawned with, or the
        // config file if no shell session exists yet. A config that won't
        // parse is an error, not a free pass.
        let sandbox = {
            let manager = context.shell_manager.read().await;
            manager.sandbox_configs.get(&context.agent_id).cloned()
        };
        let sandbox = match sandbox {
            Some(s) => Some(s),
            None => crate::config::agent_config::execution_sandbox_for(&context.agent_id)
                .map_err(|e| ToolError {
                    code: ToolErrorCode::PermissionDenied,
                    message: format!("Cannot determine the agent's sandbox level: {e}"),
                    retryable: false,
                })?
                .map(|cfg| (cfg, context.workspace_root.clone())),
        };

        let process = BackgroundProcess::spawn(
            process_id.clone(),
            label.to_string(),
            context.agent_id.clone(),
            command,
            working_dir,
            &context.working_directory,
            env,
            sandbox,
        )
        .await?;

        let result_label = process.label.clone();

        {
            let mut manager = context.shell_manager.write().await;
            manager.processes.insert(process_id.clone(), process);
        }

        Ok(ToolResult {
            content: json!({
                "process_id": process_id,
                "status": "running",
                "label": result_label,
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

// --- read_process ---

pub struct ReadProcessTool;

#[async_trait::async_trait]
impl Tool for ReadProcessTool {
    fn name(&self) -> &'static str {
        "read_process"
    }

    fn description(&self) -> &'static str {
        "Read recent output from a running background process."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "process_id": { "type": "string", "description": "Process ID to read from" },
                "lines": { "type": "integer", "description": "Number of recent lines (default: 50)" }
            },
            "required": ["process_id"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let process_id = arguments
            .get("process_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: process_id".to_string(),
                retryable: false,
            })?;

        let lines = arguments
            .get("lines")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;

        let manager = context.shell_manager.read().await;
        let process = manager.processes.get(process_id).ok_or_else(|| ToolError {
            code: ToolErrorCode::ProcessNotFound,
            message: format!("Process '{}' not found", process_id),
            retryable: false,
        })?;

        let buffer = process.output_buffer.lock().await;
        let output: Vec<&str> = buffer
            .iter()
            .rev()
            .take(lines)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|s| s.as_str())
            .collect();

        let running = *process.running.lock().await;
        let exit_code = *process.exit_code.lock().await;

        Ok(ToolResult {
            content: json!({
                "output": output.join("\n"),
                "running": running,
                "exit_code": exit_code,
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

// --- write_process ---

pub struct WriteProcessTool;

#[async_trait::async_trait]
impl Tool for WriteProcessTool {
    fn name(&self) -> &'static str {
        "write_process"
    }

    fn description(&self) -> &'static str {
        "Send input to a running background process stdin."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "process_id": { "type": "string", "description": "Process ID to write to" },
                "input": { "type": "string", "description": "Text to send (include \\n for enter)" }
            },
            "required": ["process_id", "input"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let process_id = arguments
            .get("process_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: process_id".to_string(),
                retryable: false,
            })?;

        let input = arguments
            .get("input")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: input".to_string(),
                retryable: false,
            })?;

        let mut manager = context.shell_manager.write().await;
        let process = manager
            .processes
            .get_mut(process_id)
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::ProcessNotFound,
                message: format!("Process '{}' not found", process_id),
                retryable: false,
            })?;

        if let Some(ref mut stdin) = process.stdin {
            stdin
                .write_all(input.as_bytes())
                .await
                .map_err(|e| ToolError {
                    code: ToolErrorCode::InternalError,
                    message: format!("Failed to write to process: {}", e),
                    retryable: true,
                })?;
            stdin.flush().await.ok();
        } else {
            return Err(ToolError {
                code: ToolErrorCode::InternalError,
                message: "Process stdin is not available".to_string(),
                retryable: false,
            });
        }

        Ok(ToolResult {
            content: json!({ "sent": true }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

// --- kill_process ---

pub struct KillProcessTool;

#[async_trait::async_trait]
impl Tool for KillProcessTool {
    fn name(&self) -> &'static str {
        "kill_process"
    }

    fn description(&self) -> &'static str {
        "Terminate a running background process."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "process_id": { "type": "string", "description": "Process ID to kill" }
            },
            "required": ["process_id"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let process_id = arguments
            .get("process_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: process_id".to_string(),
                retryable: false,
            })?;

        let mut manager = context.shell_manager.write().await;
        let process = manager
            .processes
            .remove(process_id)
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::ProcessNotFound,
                message: format!("Process '{}' not found", process_id),
                retryable: false,
            })?;

        // Drop the process — kill_on_drop will terminate it
        let exit_code = *process.exit_code.lock().await;
        drop(process);

        Ok(ToolResult {
            content: json!({
                "status": "terminated",
                "exit_code": exit_code,
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

// --- list_processes ---

pub struct ListProcessesTool;

#[async_trait::async_trait]
impl Tool for ListProcessesTool {
    fn name(&self) -> &'static str {
        "list_processes"
    }

    fn description(&self) -> &'static str {
        "List all running background processes for this agent."
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
        let manager = context.shell_manager.read().await;
        let mut processes = Vec::new();

        for process in manager.processes.values() {
            if process.agent_id == context.agent_id {
                let running = *process.running.lock().await;
                processes.push(json!({
                    "id": process.id,
                    "label": process.label,
                    "running": running,
                    "uptime_seconds": process.started_at.elapsed().as_secs(),
                }));
            }
        }

        Ok(ToolResult {
            content: json!({ "processes": processes }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_buffer_lines_reasonable() {
        assert!(MAX_BUFFER_LINES > 0);
        assert!(MAX_BUFFER_LINES <= 10_000);
    }

    #[test]
    fn test_max_lifetime_is_one_hour() {
        assert_eq!(MAX_BACKGROUND_PROCESS_LIFETIME_SECS, 3600);
    }

    #[test]
    fn test_max_lifetime_not_zero() {
        // Zero timeout would instantly kill every process
        assert!(MAX_BACKGROUND_PROCESS_LIFETIME_SECS > 0);
    }
    #[test]
    fn sandboxed_agents_start_processes_inside_bwrap() {
        let mut cfg = crate::config::agent_config::ExecutionSandboxBlock::legacy_default();
        cfg.level = "sandboxed".into();
        let ws = std::path::Path::new("/tmp/ws");
        let plan = launch_plan("echo hi", ws, Some((&cfg, ws))).unwrap();
        assert_eq!(plan.program, "bwrap");
        assert!(plan.args.iter().any(|a| a == "--unshare-pid"), "{:?}", plan.args);
        let n = plan.args.len();
        assert_eq!(&plan.args[n - 2..], ["-c", "echo hi"], "{:?}", plan.args);
    }

    #[test]
    fn unsandboxed_agents_start_processes_through_the_shell_directly() {
        let plan = launch_plan("echo hi", std::path::Path::new("/tmp"), None).unwrap();
        let shell = ShellConfig::for_current_os();
        assert_eq!(plan.program, shell.shell_binary.to_string_lossy());
        assert_eq!(plan.args.last().map(String::as_str), Some("echo hi"));
    }

    #[test]
    fn a_sandboxed_process_cannot_start_outside_the_workspace() {
        let mut cfg = crate::config::agent_config::ExecutionSandboxBlock::legacy_default();
        cfg.level = "sandboxed".into();
        let err = launch_plan("ls", std::path::Path::new("/etc"), Some((&cfg, std::path::Path::new("/tmp/ws")))).unwrap_err();
        assert!(err.message.contains("workspace"), "{}", err.message);
    }
}
