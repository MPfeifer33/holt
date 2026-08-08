//! Per-agent switchable project list. Backs the TUI project switcher.
//!
//! Storage: a top-level `[[projects]]` array in each agent's `config.toml`,
//! alongside `profile` and `coordinator_mode`. Each entry pairs a display
//! `name` with a `path` and (once a TUI session has spawned for it) a
//! claude `session_id` UUID that `claude_tui_start` uses with `--resume`.
//!
//! Writes go through `toml_edit::DocumentMut` so user-authored comments and
//! formatting in `config.toml` survive add/remove/set_session_id.

use crate::config::app_config::agent_dir;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEntry {
    pub name: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
}

fn config_path(agent_id: &str) -> PathBuf {
    agent_dir(agent_id).join("config.toml")
}

// ---------------------------------------------------------------------------
// Public API (agent_id → file path)
// ---------------------------------------------------------------------------

pub fn list(agent_id: &str) -> Result<Vec<ProjectEntry>, String> {
    list_at(&config_path(agent_id))
}

pub fn add(agent_id: &str, name: &str, path: &Path) -> Result<(), String> {
    add_at(&config_path(agent_id), name, path)
}

pub fn remove(agent_id: &str, name: &str) -> Result<(), String> {
    remove_at(&config_path(agent_id), name)
}

pub fn set_session_id(agent_id: &str, name: &str, session_id: Uuid) -> Result<(), String> {
    set_session_id_at(&config_path(agent_id), name, session_id)
}

pub fn clear_session_id(agent_id: &str, name: &str) -> Result<(), String> {
    clear_session_id_at(&config_path(agent_id), name)
}

pub fn find_by_path(agent_id: &str, path: &Path) -> Result<Option<ProjectEntry>, String> {
    find_by_path_at(&config_path(agent_id), path)
}

// ---------------------------------------------------------------------------
// Path-pure helpers (testable in isolation; no global config dir dependency)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ProjectsShim {
    #[serde(default)]
    projects: Vec<ProjectEntry>,
}

fn list_at(config_file: &Path) -> Result<Vec<ProjectEntry>, String> {
    if !config_file.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(config_file)
        .map_err(|e| format!("read {}: {e}", config_file.display()))?;
    let parsed: ProjectsShim =
        toml::from_str(&content).map_err(|e| format!("parse {}: {e}", config_file.display()))?;
    Ok(parsed.projects)
}

fn add_at(config_file: &Path, name: &str, project_path: &Path) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("project name must not be empty".into());
    }
    if !project_path.exists() || !project_path.is_dir() {
        return Err(format!(
            "path does not exist or is not a directory: {}",
            project_path.display()
        ));
    }
    let canonical = std::fs::canonicalize(project_path)
        .map_err(|e| format!("canonicalize {}: {e}", project_path.display()))?;

    let existing = list_at(config_file)?;
    if existing.iter().any(|p| p.name == trimmed) {
        return Err(format!("a project named '{trimmed}' already exists"));
    }
    if existing.iter().any(|p| p.path == canonical) {
        return Err(format!(
            "a project with path '{}' already exists",
            canonical.display()
        ));
    }

    let mut doc = read_doc(config_file)?;
    let projects = ensure_projects_array(&mut doc);
    let mut entry = Table::new();
    entry["name"] = value(trimmed);
    entry["path"] = value(canonical.to_string_lossy().to_string());
    projects.push(entry);
    write_doc(config_file, &doc)
}

fn remove_at(config_file: &Path, name: &str) -> Result<(), String> {
    if !config_file.exists() {
        return Ok(());
    }
    let mut doc = read_doc(config_file)?;
    let projects = ensure_projects_array(&mut doc);
    let mut idx_to_remove: Option<usize> = None;
    for (i, table) in projects.iter().enumerate() {
        if table.get("name").and_then(|v| v.as_str()) == Some(name) {
            idx_to_remove = Some(i);
            break;
        }
    }
    if let Some(i) = idx_to_remove {
        projects.remove(i);
        write_doc(config_file, &doc)?;
    }
    Ok(())
}

fn set_session_id_at(config_file: &Path, name: &str, session_id: Uuid) -> Result<(), String> {
    let mut doc = read_doc(config_file)?;
    let idx = {
        let projects = ensure_projects_array(&mut doc);
        projects
            .iter()
            .position(|t| t.get("name").and_then(|v| v.as_str()) == Some(name))
            .ok_or_else(|| format!("no project named '{name}'"))?
    };
    let projects = ensure_projects_array(&mut doc);
    projects.get_mut(idx).unwrap()["session_id"] = value(session_id.to_string());
    write_doc(config_file, &doc)
}

fn clear_session_id_at(config_file: &Path, name: &str) -> Result<(), String> {
    let mut doc = read_doc(config_file)?;
    let idx = {
        let projects = ensure_projects_array(&mut doc);
        projects
            .iter()
            .position(|t| t.get("name").and_then(|v| v.as_str()) == Some(name))
            .ok_or_else(|| format!("no project named '{name}'"))?
    };
    let projects = ensure_projects_array(&mut doc);
    let table = projects.get_mut(idx).unwrap();
    table.remove("session_id");
    write_doc(config_file, &doc)
}

fn find_by_path_at(config_file: &Path, path: &Path) -> Result<Option<ProjectEntry>, String> {
    // canonicalize fails if the path no longer exists — fall back to the
    // original so the caller can still locate (and remove) a stale entry.
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let entries = list_at(config_file)?;
    Ok(entries
        .into_iter()
        .find(|e| e.path == canonical || e.path == path))
}

// ---------------------------------------------------------------------------
// TOML document helpers
// ---------------------------------------------------------------------------

fn read_doc(config_file: &Path) -> Result<DocumentMut, String> {
    let content = if config_file.exists() {
        std::fs::read_to_string(config_file)
            .map_err(|e| format!("read {}: {e}", config_file.display()))?
    } else {
        String::new()
    };
    content
        .parse::<DocumentMut>()
        .map_err(|e| format!("parse {}: {e}", config_file.display()))
}

fn write_doc(config_file: &Path, doc: &DocumentMut) -> Result<(), String> {
    if let Some(parent) = config_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    std::fs::write(config_file, doc.to_string())
        .map_err(|e| format!("write {}: {e}", config_file.display()))
}

fn ensure_projects_array(doc: &mut DocumentMut) -> &mut ArrayOfTables {
    if !doc.contains_key("projects") {
        doc.insert("projects", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    doc["projects"]
        .as_array_of_tables_mut()
        .expect("projects must be an array-of-tables")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, PathBuf, PathBuf) {
        // (config_dir, config_file_path, project_dir_path)
        let cfg_dir = TempDir::new().unwrap();
        let cfg_file = cfg_dir.path().join("config.toml");
        let project_dir = cfg_dir.path().join("a-project");
        std::fs::create_dir(&project_dir).unwrap();
        (cfg_dir, cfg_file, project_dir)
    }

    #[test]
    fn list_returns_empty_when_file_missing() {
        let (_d, cfg, _p) = fixture();
        assert_eq!(list_at(&cfg).unwrap(), Vec::new());
    }

    #[test]
    fn list_returns_empty_when_no_projects_section() {
        let (_d, cfg, _p) = fixture();
        std::fs::write(&cfg, "profile = \"unrestricted\"\n").unwrap();
        assert_eq!(list_at(&cfg).unwrap(), Vec::new());
    }

    #[test]
    fn add_then_list() {
        let (_d, cfg, p) = fixture();
        add_at(&cfg, "First", &p).unwrap();
        let entries = list_at(&cfg).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "First");
        assert_eq!(entries[0].path, std::fs::canonicalize(&p).unwrap());
        assert_eq!(entries[0].session_id, None);
    }

    #[test]
    fn add_rejects_empty_name() {
        let (_d, cfg, p) = fixture();
        assert!(add_at(&cfg, "   ", &p).is_err());
    }

    #[test]
    fn add_rejects_nonexistent_path() {
        let (_d, cfg, _p) = fixture();
        let missing = PathBuf::from("/tmp/does-not-exist-xyz123");
        assert!(add_at(&cfg, "Bad", &missing).is_err());
    }

    #[test]
    fn add_rejects_duplicate_name() {
        let (_d, cfg, p) = fixture();
        let p2 = p.parent().unwrap().join("another");
        std::fs::create_dir(&p2).unwrap();
        add_at(&cfg, "Same", &p).unwrap();
        assert!(add_at(&cfg, "Same", &p2).is_err());
    }

    #[test]
    fn add_rejects_duplicate_path() {
        let (_d, cfg, p) = fixture();
        add_at(&cfg, "A", &p).unwrap();
        assert!(add_at(&cfg, "B", &p).is_err());
    }

    #[test]
    fn remove_removes_entry() {
        let (_d, cfg, p) = fixture();
        add_at(&cfg, "ToRemove", &p).unwrap();
        remove_at(&cfg, "ToRemove").unwrap();
        assert!(list_at(&cfg).unwrap().is_empty());
    }

    #[test]
    fn remove_missing_is_noop() {
        let (_d, cfg, _p) = fixture();
        std::fs::write(&cfg, "profile = \"x\"\n").unwrap();
        remove_at(&cfg, "ghost").unwrap();
    }

    #[test]
    fn set_session_id_persists() {
        let (_d, cfg, p) = fixture();
        add_at(&cfg, "WithSession", &p).unwrap();
        let id = Uuid::new_v4();
        set_session_id_at(&cfg, "WithSession", id).unwrap();
        let entries = list_at(&cfg).unwrap();
        assert_eq!(entries[0].session_id, Some(id));
    }

    #[test]
    fn set_session_id_errors_on_missing() {
        let (_d, cfg, _p) = fixture();
        std::fs::write(&cfg, "profile = \"x\"\n").unwrap();
        assert!(set_session_id_at(&cfg, "ghost", Uuid::new_v4()).is_err());
    }

    #[test]
    fn find_by_path_matches_canonical() {
        let (_d, cfg, p) = fixture();
        add_at(&cfg, "FindMe", &p).unwrap();
        let found = find_by_path_at(&cfg, &p).unwrap();
        assert_eq!(found.unwrap().name, "FindMe");
    }

    #[test]
    fn find_by_path_returns_none_for_unknown() {
        let (_d, cfg, p) = fixture();
        add_at(&cfg, "Known", &p).unwrap();
        let other = p.parent().unwrap().join("other");
        std::fs::create_dir(&other).unwrap();
        assert!(find_by_path_at(&cfg, &other).unwrap().is_none());
    }

    #[test]
    fn format_preservation_round_trip() {
        // Existing config with user comments and unrelated fields.
        let (_d, cfg, p) = fixture();
        let initial = "# top-level comment\n\
                       profile = \"unrestricted\"\n\
                       coordinator_mode = false  # inline comment\n\
                       \n\
                       [workspace]\n\
                       working_directory = \"/home/user/x\"\n";
        std::fs::write(&cfg, initial).unwrap();

        add_at(&cfg, "P1", &p).unwrap();
        let after_add = std::fs::read_to_string(&cfg).unwrap();
        assert!(after_add.contains("# top-level comment"));
        assert!(after_add.contains("# inline comment"));
        assert!(after_add.contains("[workspace]"));
        assert!(after_add.contains("[[projects]]"));
        assert!(after_add.contains("name = \"P1\""));

        remove_at(&cfg, "P1").unwrap();
        let after_remove = std::fs::read_to_string(&cfg).unwrap();
        assert!(after_remove.contains("# top-level comment"));
        assert!(after_remove.contains("# inline comment"));
        assert!(!after_remove.contains("name = \"P1\""));
    }

    #[test]
    fn clear_session_id_removes_field() {
        let (_d, cfg, p) = fixture();
        add_at(&cfg, "Clearable", &p).unwrap();
        let id = Uuid::new_v4();
        set_session_id_at(&cfg, "Clearable", id).unwrap();
        assert!(list_at(&cfg).unwrap()[0].session_id.is_some());
        clear_session_id_at(&cfg, "Clearable").unwrap();
        let entries = list_at(&cfg).unwrap();
        assert_eq!(entries[0].session_id, None);
        assert_eq!(entries[0].name, "Clearable");
    }

    #[test]
    fn back_compat_existing_config_without_projects() {
        // Real terminal-lane-style config minus [[projects]]. Must parse and yield
        // an empty list without errors.
        let (_d, cfg, _p) = fixture();
        let real = "profile = \"unrestricted\"\n\
                    coordinator_mode = false\n\
                    \n\
                    [workspace]\n\
                    working_directory = \"/home/x/y\"\n";
        std::fs::write(&cfg, real).unwrap();
        assert_eq!(list_at(&cfg).unwrap(), Vec::new());
    }
}
