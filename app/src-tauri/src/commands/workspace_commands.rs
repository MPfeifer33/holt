use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use super::error::CommandError;
use crate::AppState;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Folder entry for lazy directory listing.
#[derive(serde::Serialize)]
pub struct FolderEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub agent_dots: Vec<crate::runtime::agent::AgentColor>,
}

#[derive(serde::Serialize)]
pub struct FilePreview {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    pub line_count: u32,
    pub content: String,
    pub base64_content: Option<String>,
    pub media_type: Option<String>,
    pub is_binary: bool,
    pub is_image: bool,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Get the current workspace root path.
#[tauri::command]
pub async fn get_workspace_root(state: State<'_, Arc<AppState>>) -> Result<String, CommandError> {
    let config = state.config.read().await;
    Ok(config.workspace.root.to_string_lossy().to_string())
}

/// Set the workspace root path. Validates the path exists.
/// Returns list of agent names whose working directories are outside the new root (warnings).
#[tauri::command]
pub async fn set_workspace_root(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<Vec<String>, CommandError> {
    let new_root = PathBuf::from(&path);
    if !new_root.is_dir() {
        return Err(CommandError::validation(format!(
            "Path does not exist or is not a directory: {}",
            path
        )));
    }

    let canonical_root = dunce::canonicalize(&new_root)
        .map_err(|e| CommandError::internal(format!("Failed to resolve path: {}", e)))?;

    // Check which agents are outside the new root
    let agents = state.agents.read().await;
    let mut out_of_scope: Vec<String> = Vec::new();
    for agent in agents.iter() {
        if agent.working_directory.exists() {
            if let Ok(canonical_wd) = dunce::canonicalize(&agent.working_directory) {
                if !canonical_wd.starts_with(&canonical_root) {
                    out_of_scope.push(agent.name.clone());
                }
            }
        }
    }
    drop(agents);

    let mut config = state.config.write().await;
    config.workspace.root = new_root;

    // Persist to disk
    config
        .save(None)
        .map_err(|e| CommandError::internal(e.to_string()))?;

    Ok(out_of_scope)
}

/// Open a native OS folder picker dialog. Returns selected path or null.
/// Uses rfd directly (on-demand) to avoid GTK dialog subsystem loading at startup.
#[tauri::command]
pub async fn pick_folder(default_path: Option<String>) -> Result<Option<String>, CommandError> {
    let mut dialog = rfd::AsyncFileDialog::new().set_title("Select Folder");
    if let Some(ref p) = default_path {
        dialog = dialog.set_directory(p);
    }
    let result = dialog.pick_folder().await;
    Ok(result.map(|handle| handle.path().to_string_lossy().to_string()))
}

/// List directory contents for the folder tree (lazy loading).
/// Note: intentionally does NOT validate against workspace root — the UI folder picker
/// needs to navigate the full filesystem to let users select working directories.
#[tauri::command]
pub async fn list_directory(
    state: State<'_, Arc<AppState>>,
    path: String,
    include_hidden: bool,
) -> Result<Vec<FolderEntry>, CommandError> {
    let dir_path = PathBuf::from(&path);
    if !dir_path.is_dir() {
        return Err(CommandError::validation(format!(
            "Not a directory: {}",
            path
        )));
    }

    // Gather agent working directories for dot annotations
    let agents = state.agents.read().await;
    let agent_dirs: Vec<(PathBuf, crate::runtime::agent::AgentColor)> = agents
        .iter()
        .map(|a| (a.working_directory.clone(), a.color.clone()))
        .collect();
    drop(agents);

    let mut entries = Vec::new();
    let read_dir =
        std::fs::read_dir(&dir_path).map_err(|e| CommandError::internal(e.to_string()))?;

    for entry in read_dir {
        let entry = entry.map_err(|e| CommandError::internal(e.to_string()))?;
        let file_name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files/folders unless requested
        if !include_hidden && file_name.starts_with('.') {
            continue;
        }

        let entry_path = entry.path();
        let is_dir = entry_path.is_dir();

        // Find agent dots: agents whose working_directory starts with this path
        let dots: Vec<crate::runtime::agent::AgentColor> = if is_dir {
            agent_dirs
                .iter()
                .filter(|(wd, _)| wd.starts_with(&entry_path) || *wd == entry_path)
                .map(|(_, color)| color.clone())
                .collect()
        } else {
            vec![]
        };

        entries.push(FolderEntry {
            name: file_name,
            path: entry_path.to_string_lossy().to_string(),
            is_dir,
            agent_dots: dots,
        });
    }

    // Sort: directories first, then alphabetical
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(entries)
}

#[tauri::command]
pub async fn read_file_preview(
    path: String,
    max_lines: Option<u32>,
) -> Result<FilePreview, CommandError> {
    let file_path = PathBuf::from(&path);
    if !file_path.is_file() {
        return Err(CommandError::not_found(format!("Not a file: {}", path)));
    }

    let metadata =
        std::fs::metadata(&file_path).map_err(|e| CommandError::internal(e.to_string()))?;
    let size_bytes = metadata.len();
    if size_bytes > 5_000_000 {
        return Err(CommandError::validation("File too large (>5MB)"));
    }

    let name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let ext = file_path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    // Image detection
    let image_extensions = ["png", "jpg", "jpeg", "gif", "webp", "svg"];
    let is_image = image_extensions.contains(&ext.as_str());

    if is_image {
        let data = std::fs::read(&file_path).map_err(|e| CommandError::internal(e.to_string()))?;
        let base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
        let media_type = match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "svg" => "image/svg+xml",
            _ => "application/octet-stream",
        };
        return Ok(FilePreview {
            path,
            name,
            size_bytes,
            line_count: 0,
            content: String::new(),
            base64_content: Some(base64),
            media_type: Some(media_type.to_string()),
            is_binary: false,
            is_image: true,
        });
    }

    // Binary detection: read first 8KB and check for null bytes
    let raw = std::fs::read(&file_path).map_err(|e| CommandError::internal(e.to_string()))?;
    let check_len = raw.len().min(8192);
    let is_binary = raw[..check_len].contains(&0);

    if is_binary {
        return Ok(FilePreview {
            path,
            name,
            size_bytes,
            line_count: 0,
            content: String::new(),
            base64_content: None,
            media_type: None,
            is_binary: true,
            is_image: false,
        });
    }

    // Text file: count lines, return first N
    let text = String::from_utf8_lossy(&raw);
    let lines: Vec<&str> = text.lines().collect();
    let line_count = lines.len() as u32;
    let limit = max_lines.unwrap_or(50) as usize;
    let preview = lines[..lines.len().min(limit)].join("\n");

    Ok(FilePreview {
        path,
        name,
        size_bytes,
        line_count,
        content: preview,
        base64_content: None,
        media_type: None,
        is_binary: false,
        is_image: false,
    })
}
