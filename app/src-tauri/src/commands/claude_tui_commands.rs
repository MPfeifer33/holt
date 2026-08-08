//! Tauri command for launching a Claude Code TUI session bound to a specific
//! Holt agent. Generates per-agent `mcp.json` and `settings.json` files
//! into `~/.config/holt/agents/<id>/`, then spawns `claude` with explicit
//! `--mcp-config`, `--strict-mcp-config`, `--settings`, and
//! `--setting-sources user,project` flags.
//!
//! When a `project_path` argument is supplied (TUI project switcher), the
//! cwd and claude session are resolved against the agent's `[[projects]]`
//! list in `config.toml`:
//! - first launch for a project → `--session-id <fresh-uuid>`, UUID
//!   persisted back to the project entry after spawn succeeds
//! - subsequent launches → `--resume <stored-uuid>`
//!
//! `--session-id` is NOT idempotent (claude rejects an in-use UUID), so the
//! two-path split is mandatory — verified empirically with claude 4.x in
//! pre-impl probes.
//!
//! Concurrent calls for the same agent are serialized via a per-agent mutex
//! on `AppState::claude_tui_locks` so a double-click can't race PTY spawn or
//! `config.toml` writes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::config::agent_projects;
use crate::config::app_config::agent_dir;
use crate::runtime::agent_claude_config;
use crate::terminal::manager::TerminalManager;
use crate::terminal::session::TerminalInfo;
use crate::AppState;

/// Start a Claude Code TUI session for the given agent.
///
/// `project_path`, when provided, must match an entry in the agent's
/// `[[projects]]` list (compared after `canonicalize`). When omitted, falls
/// back to the agent's `[workspace].working_directory` with no
/// `--resume`/`--session-id` flag (caller is not using the switcher).
#[tauri::command]
pub async fn claude_tui_start(
    app_state: State<'_, Arc<AppState>>,
    terminal_state: State<'_, Arc<TerminalManager>>,
    app_handle: AppHandle,
    agent_id: String,
    project_path: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<TerminalInfo, String> {
    // Serialize concurrent spawns for the same agent. Mutex is lazily created
    // on first use; cheap to keep around since there's a handful of agents.
    let lock = app_state
        .claude_tui_locks
        .entry(agent_id.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;

    // Generate per-agent claude config files (mcp.json + settings.json).
    let dir = agent_dir(&agent_id);
    let mcp_url = app_state.codex_mcp_url.read().await;

    // Session-start memory for this lane. Fetched here because this is where the async
    // context and the engine live; the config writer stays sync. Uses the same
    // `fetch_dynamic_profile` as payload.rs/agent_sdk.rs/codex.rs rather than a fourth copy.
    let dynamic_profile = match app_state.get_memory_engine() {
        Some(engine) => {
            crate::runtime::persona::fetch_dynamic_profile(
                &engine,
                &agent_id,
                crate::runtime::persona::TUI_DYNAMIC_PROFILE_TOKEN_BUDGET,
            )
            .await
        }
        None => None,
    };
    if dynamic_profile.is_some() {
        tracing::info!(agent_id, "Dynamic memory profile injected into TUI persona");
    }

    let generated =
        agent_claude_config::write(&agent_id, &dir, mcp_url.as_deref(), dynamic_profile)
            .map_err(|e| format!("Failed to write per-agent claude config: {e}"))?;

    // Resolve cwd + session flag.
    let SpawnResolution {
        cwd,
        session_flag,
        persist_after_spawn,
    } = resolve_spawn(&agent_id, project_path.as_deref(), &app_state).await?;

    // Build the launch command. portable_pty's CommandBuilder treats the
    // "shell" argument as the binary path, so a compound `claude --flag ...`
    // string needs to dispatch through /bin/sh -c. terminal/session.rs
    // detects spaces in the shell argument and routes accordingly.
    let mut shell_command = format!(
        "claude --mcp-config {mcp} --strict-mcp-config --settings {settings} --setting-sources user,project --append-system-prompt-file {persona}",
        mcp = shell_quote(&generated.mcp_config_path.to_string_lossy()),
        settings = shell_quote(&generated.settings_path.to_string_lossy()),
        persona = shell_quote(&generated.persona_path.to_string_lossy()),
    );
    if let Some(flag) = session_flag {
        shell_command.push(' ');
        shell_command.push_str(&flag);
    }

    let info = terminal_state
        .inner()
        .create(
            Some(shell_command),
            Some(cwd.to_string_lossy().into_owned()),
            cols,
            rows,
            app_handle,
        )
        .await?;

    // Persist the freshly-generated UUID back to config.toml. We only get
    // here if PTY creation succeeded — claude is now starting up.
    if let Some((project_name, uuid)) = persist_after_spawn {
        if let Err(e) = agent_projects::set_session_id(&agent_id, &project_name, uuid) {
            tracing::warn!(
                "claude_tui_start: spawned PTY but failed to persist session_id for project '{project_name}': {e}"
            );
        }
    }

    Ok(info)
}

struct SpawnResolution {
    cwd: PathBuf,
    /// Either `--resume <uuid>` (existing session) or `--session-id <uuid>`
    /// (first launch). `None` when no project_path was supplied (legacy
    /// path) — no resume semantics, claude starts fresh and forgets after.
    session_flag: Option<String>,
    /// When `Some((project_name, uuid))`, write `uuid` to the project's
    /// `session_id` field after spawn succeeds.
    persist_after_spawn: Option<(String, Uuid)>,
}

async fn resolve_spawn(
    agent_id: &str,
    project_path: Option<&str>,
    app_state: &Arc<AppState>,
) -> Result<SpawnResolution, String> {
    match project_path {
        None => {
            // Backwards-compat path: use the agent's working_directory and
            // give claude no resume hint.
            let working_dir = {
                let agents = app_state.agents.read().await;
                agents
                    .iter()
                    .find(|a| a.id == agent_id)
                    .map(|a| a.working_directory.clone())
                    .ok_or_else(|| format!("Unknown agent: {agent_id}"))?
            };
            Ok(SpawnResolution {
                cwd: working_dir,
                session_flag: None,
                persist_after_spawn: None,
            })
        }
        Some(path_str) => {
            let path = PathBuf::from(path_str);
            let entry = agent_projects::find_by_path(agent_id, &path)?.ok_or_else(|| {
                format!(
                    "No project registered at path '{}' for agent '{agent_id}'",
                    path.display()
                )
            })?;
            match entry.session_id {
                Some(uuid) => {
                    // The stored uuid is the session Holt *assigned*. If the
                    // user has since run /clear or /resume, Claude Code rotated to
                    // a new session without telling us, and the stored file still
                    // exists -- so an existence check cannot catch it. Prefer the
                    // newest transcript for this cwd and re-persist, otherwise a
                    // restart silently resumes a pre-/clear conversation.
                    if let Some(newest) = newest_session_uuid(&entry.path).filter(|n| *n != uuid) {
                        tracing::info!(
                            agent = %agent_id,
                            project = %entry.name,
                            stored_uuid = %uuid,
                            newest_uuid = %newest,
                            "Stored session is not the newest for this directory -- resuming the \
                             newest (session was rotated by /clear or /resume)"
                        );
                        return Ok(SpawnResolution {
                            cwd: entry.path.clone(),
                            session_flag: Some(format!(
                                "--resume {}",
                                shell_quote(&newest.to_string())
                            )),
                            persist_after_spawn: Some((entry.name, newest)),
                        });
                    }

                    // Auto-recovery: verify the session file actually exists
                    // before attempting --resume. If the file is gone (binary
                    // rebuild, manual cleanup, etc.), wipe the stale UUID and
                    // start fresh instead of letting claude reject and exit.
                    if session_file_exists(&entry.path, &uuid) {
                        Ok(SpawnResolution {
                            cwd: entry.path,
                            session_flag: Some(format!(
                                "--resume {}",
                                shell_quote(&uuid.to_string())
                            )),
                            persist_after_spawn: None,
                        })
                    } else {
                        tracing::warn!(
                            agent = %agent_id,
                            project = %entry.name,
                            stale_uuid = %uuid,
                            "Session file not found — wiping stale session_id and starting fresh"
                        );
                        // Wipe the stale UUID from config.toml
                        if let Err(e) = agent_projects::clear_session_id(agent_id, &entry.name) {
                            tracing::warn!(
                                "Failed to clear stale session_id from config.toml: {e}"
                            );
                        }
                        // Fall through to fresh session
                        let uuid = Uuid::new_v4();
                        Ok(SpawnResolution {
                            cwd: entry.path,
                            session_flag: Some(format!(
                                "--session-id {}",
                                shell_quote(&uuid.to_string())
                            )),
                            persist_after_spawn: Some((entry.name, uuid)),
                        })
                    }
                }
                None => {
                    let uuid = Uuid::new_v4();
                    Ok(SpawnResolution {
                        cwd: entry.path,
                        session_flag: Some(format!(
                            "--session-id {}",
                            shell_quote(&uuid.to_string())
                        )),
                        persist_after_spawn: Some((entry.name, uuid)),
                    })
                }
            }
        }
    }
}

/// Directory holding Claude Code's session transcripts for a working directory.
///
/// Claude stores sessions at `~/.claude/projects/<slugified-path>/<uuid>.jsonl`,
/// where the slug replaces `/` with `-` (the leading separator becomes a leading
/// `-`, so `/home/u/projects` → `-home-u-projects`).
fn claude_project_dir(project_path: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let slug = project_path.to_string_lossy().replace('/', "-");
    Some(home.join(".claude").join("projects").join(slug))
}

/// Check whether the Claude Code session file exists on disk.
fn session_file_exists(project_path: &Path, uuid: &Uuid) -> bool {
    claude_project_dir(project_path)
        .map(|dir| dir.join(format!("{uuid}.jsonl")).exists())
        .unwrap_or(false)
}

/// The most recently modified session transcript in a Claude project directory.
///
/// Split from [`newest_session_uuid`] so the selection logic is testable without
/// depending on `$HOME`.
fn newest_session_uuid_in(dir: &Path) -> Option<Uuid> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "jsonl" {
                return None;
            }
            let uuid = Uuid::parse_str(path.file_stem()?.to_str()?).ok()?;
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, uuid))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, uuid)| uuid)
}

/// The session Claude Code would consider current for this working directory.
///
/// `/clear` and `/resume` start a *new* session inside the TUI without telling
/// Holt. The uuid recorded at spawn time therefore goes stale while remaining
/// a perfectly valid file on disk, so the existing "does the file exist?" check
/// cannot detect it — and `--resume <stale-uuid>` silently drops the user into an
/// older conversation. Preferring the newest transcript tracks those rotations.
///
/// Limitation: transcripts are keyed by working directory, not by agent. Two lanes
/// sharing a cwd share this directory, so the newest transcript may belong to the
/// other lane. There is no fix for that from outside the TUI — Holt cannot see
/// which session the child process actually adopted.
fn newest_session_uuid(project_path: &Path) -> Option<Uuid> {
    newest_session_uuid_in(&claude_project_dir(project_path)?)
}

/// Minimal POSIX shell quoting. Wraps in single quotes; escapes embedded
/// single quotes with the standard '\'' dance. Paths under
/// ~/.config/holt/ shouldn't contain quote characters, but be defensive.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    /// Write a transcript file with an explicit age, so ordering is unambiguous
    /// without depending on filesystem timestamp resolution.
    fn write_session(dir: &Path, uuid: Uuid, age: Duration) {
        let path = dir.join(format!("{uuid}.jsonl"));
        fs::write(&path, b"{}\n").unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(std::time::SystemTime::now() - age)
            .unwrap();
    }

    #[test]
    fn newest_session_picks_most_recently_modified() {
        let dir = tempfile::tempdir().unwrap();
        let old = Uuid::new_v4();
        let recent = Uuid::new_v4();
        write_session(dir.path(), old, Duration::from_secs(86_400));
        write_session(dir.path(), recent, Duration::from_secs(10));

        assert_eq!(newest_session_uuid_in(dir.path()), Some(recent));
    }

    #[test]
    fn newest_session_ignores_non_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        let real = Uuid::new_v4();
        write_session(dir.path(), real, Duration::from_secs(60));
        // Newer, but not a session transcript.
        fs::write(dir.path().join("notes.md"), b"hi").unwrap();
        fs::write(dir.path().join("not-a-uuid.jsonl"), b"{}").unwrap();

        assert_eq!(newest_session_uuid_in(dir.path()), Some(real));
    }

    #[test]
    fn newest_session_is_none_for_empty_or_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(newest_session_uuid_in(dir.path()), None);
        assert_eq!(newest_session_uuid_in(&dir.path().join("nope")), None);
    }

    /// The regression: a stored uuid that still exists on disk but is no longer
    /// the newest must not win. This is the /clear case -- an existence check
    /// alone cannot distinguish it, which is why restarts resumed stale lanes.
    #[test]
    fn stale_but_existing_session_loses_to_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let stored = Uuid::new_v4();
        let after_clear = Uuid::new_v4();
        write_session(dir.path(), stored, Duration::from_secs(3 * 86_400));
        write_session(dir.path(), after_clear, Duration::from_secs(30));

        let newest = newest_session_uuid_in(dir.path()).unwrap();
        assert_ne!(newest, stored, "stored session must not be selected");
        assert_eq!(newest, after_clear);
        // And the guard used at the call site must fire.
        assert!(Some(newest).filter(|n| *n != stored).is_some());
    }

    #[test]
    fn project_dir_slug_matches_claude_layout() {
        let dir = claude_project_dir(Path::new("/home/u/projects")).unwrap();
        assert!(
            dir.ends_with("-home-u-projects"),
            "unexpected slug: {}",
            dir.display()
        );
    }
}
