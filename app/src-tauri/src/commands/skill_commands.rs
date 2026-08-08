use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use super::error::CommandError;
use crate::AppState;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct SkillSummary {
    pub file_name: String,
    pub name: String,
    pub description: String,
    pub priority: String,
    pub mode: String,
    pub effective_mode: String,
    pub trigger_count: usize,
    pub max_tokens: usize,
    pub is_directory: bool,
    pub file_count: usize,
}

#[derive(Serialize)]
pub struct SkillDetail {
    pub file_name: String,
    pub name: String,
    pub description: String,
    pub priority: String,
    pub mode: String,
    pub effective_mode: String,
    pub triggers: serde_json::Value,
    pub max_tokens: usize,
    pub body: String,
    pub raw_content: String,
    pub is_directory: bool,
    pub files: Vec<String>,
}

#[derive(Serialize)]
pub struct DeleteSkillResult {
    pub deleted: bool,
    pub file_count: usize,
    pub needs_confirmation: bool,
}

#[derive(Serialize)]
pub struct ActiveSkillInfo {
    pub name: String,
    pub source: String,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_skills(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<SkillSummary>, CommandError> {
    let skills = state.skill_registry.list();
    let summaries = skills
        .into_iter()
        .map(|s| {
            let effective = s.effective_mode();
            SkillSummary {
                file_name: s.file_name.clone(),
                name: s.frontmatter.name.clone(),
                description: s.frontmatter.description.clone(),
                priority: format!("{:?}", s.frontmatter.priority).to_lowercase(),
                mode: format!("{:?}", s.frontmatter.mode).to_lowercase(),
                effective_mode: format!("{:?}", effective).to_lowercase(),
                trigger_count: s.frontmatter.triggers.len(),
                max_tokens: s.frontmatter.max_tokens,
                is_directory: s.is_directory,
                file_count: s.files.len(),
            }
        })
        .collect();
    Ok(summaries)
}

#[tauri::command]
pub async fn get_skill(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<SkillDetail, CommandError> {
    let skill = state
        .skill_registry
        .get(&name)
        .ok_or_else(|| CommandError::not_found(format!("Skill '{}' not found", name)))?;

    // Read the raw file content for editing.
    // For multi-file skills, read SKILL.md only (not concatenated body).
    // Single read — parse body from the raw content in memory to avoid double I/O.
    let read_path = if skill.is_directory {
        skill.file_path.join("SKILL.md")
    } else {
        skill.file_path.clone()
    };

    let raw_content = std::fs::read_to_string(&read_path)
        .map_err(|e| CommandError::internal(format!("Failed to read skill file: {}", e)))?;

    let body = if skill.is_directory {
        // Extract body from raw content in memory (skip frontmatter)
        let trimmed = raw_content.trim_start();
        if trimmed.starts_with("---") {
            let after_open = &trimmed[3..];
            match after_open.find("\n---") {
                Some(pos) => after_open[pos + 4..].trim().to_string(),
                None => skill.body.clone(),
            }
        } else {
            skill.body.clone()
        }
    } else {
        skill.body.clone()
    };

    let triggers = serde_json::to_value(&skill.frontmatter.triggers)
        .map_err(|e| CommandError::internal(format!("Failed to serialize triggers: {}", e)))?;

    let effective = skill.effective_mode();
    Ok(SkillDetail {
        file_name: skill.file_name,
        name: skill.frontmatter.name.clone(),
        description: skill.frontmatter.description.clone(),
        priority: format!("{:?}", skill.frontmatter.priority).to_lowercase(),
        mode: format!("{:?}", skill.frontmatter.mode).to_lowercase(),
        effective_mode: format!("{:?}", effective).to_lowercase(),
        triggers,
        max_tokens: skill.frontmatter.max_tokens,
        body,
        raw_content,
        is_directory: skill.is_directory,
        files: skill.files,
    })
}

#[tauri::command]
pub async fn create_skill(
    state: State<'_, Arc<AppState>>,
    file_name: String,
    content: String,
) -> Result<(), CommandError> {
    let skills_dir = state.skill_registry.skills_dir();
    let path = skills_dir.join(format!("{}.md", file_name));

    if path.exists() {
        return Err(CommandError::conflict(format!(
            "Skill file '{}' already exists",
            file_name
        )));
    }

    std::fs::write(&path, &content)
        .map_err(|e| CommandError::internal(format!("Failed to write skill file: {}", e)))?;

    state.skill_registry.reload();
    Ok(())
}

#[tauri::command]
pub async fn update_skill(
    state: State<'_, Arc<AppState>>,
    file_name: String,
    content: String,
) -> Result<(), CommandError> {
    let skills_dir = state.skill_registry.skills_dir();

    // Check for directory-based skill first
    let dir_path = skills_dir.join(&file_name);
    let file_path = skills_dir.join(format!("{}.md", file_name));

    let target_path = if dir_path.is_dir() && dir_path.join("SKILL.md").exists() {
        dir_path.join("SKILL.md")
    } else if file_path.exists() {
        file_path
    } else {
        return Err(CommandError::not_found(format!(
            "Skill '{}' does not exist",
            file_name
        )));
    };

    std::fs::write(&target_path, &content)
        .map_err(|e| CommandError::internal(format!("Failed to write skill file: {}", e)))?;

    state.skill_registry.reload();
    Ok(())
}

#[tauri::command]
pub async fn delete_skill(
    state: State<'_, Arc<AppState>>,
    file_name: String,
    confirmed: Option<bool>,
) -> Result<DeleteSkillResult, CommandError> {
    let skills_dir = state.skill_registry.skills_dir();

    let dir_path = skills_dir.join(&file_name);
    let file_path = skills_dir.join(format!("{}.md", file_name));

    if dir_path.is_dir() && dir_path.join("SKILL.md").exists() {
        // Count files in directory
        let file_count = std::fs::read_dir(&dir_path)
            .map_err(|e| CommandError::internal(format!("Failed to read skill directory: {}", e)))?
            .flatten()
            .filter(|e| e.path().is_file())
            .count();

        if file_count > 1 && confirmed != Some(true) {
            // Needs confirmation — return file count without deleting
            return Ok(DeleteSkillResult {
                deleted: false,
                file_count,
                needs_confirmation: true,
            });
        }

        // Note: uses remove_dir_all which permanently deletes. The confirmation
        // gate above mitigates accidental loss. Trash support (e.g., via the
        // `trash` crate) could be added later if needed.
        std::fs::remove_dir_all(&dir_path).map_err(|e| {
            CommandError::internal(format!("Failed to delete skill directory: {}", e))
        })?;

        state.skill_registry.reload();
        return Ok(DeleteSkillResult {
            deleted: true,
            file_count,
            needs_confirmation: false,
        });
    }

    if file_path.exists() {
        std::fs::remove_file(&file_path)
            .map_err(|e| CommandError::internal(format!("Failed to delete skill file: {}", e)))?;

        state.skill_registry.reload();
        return Ok(DeleteSkillResult {
            deleted: true,
            file_count: 1,
            needs_confirmation: false,
        });
    }

    Err(CommandError::not_found(format!(
        "Skill '{}' does not exist",
        file_name
    )))
}

#[tauri::command]
pub async fn assign_skill_to_agent(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    skill_name: String,
) -> Result<(), CommandError> {
    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent '{}' not found", agent_id)))?;

    if !agent.skills.contains(&skill_name) {
        agent.skills.push(skill_name);
        agent.invalidate_cached_params();
        agent.is_dirty = true;
    }

    Ok(())
}

#[tauri::command]
pub async fn unassign_skill_from_agent(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    skill_name: String,
) -> Result<(), CommandError> {
    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent '{}' not found", agent_id)))?;

    agent.skills.retain(|s| s != &skill_name);
    agent.invalidate_cached_params();
    agent.is_dirty = true;

    Ok(())
}

#[tauri::command]
pub async fn get_active_skills(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<Vec<ActiveSkillInfo>, CommandError> {
    let agents = state.agents.read().await;
    let agent = agents
        .iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent '{}' not found", agent_id)))?;

    // For now, return agent-assigned skills as active.
    // Full resolver integration (war room + auto-triggered) happens in engine.rs.
    let active: Vec<ActiveSkillInfo> = agent
        .skills
        .iter()
        .filter_map(|name| {
            state.skill_registry.get(name).map(|_| ActiveSkillInfo {
                name: name.clone(),
                source: "agent".to_string(),
            })
        })
        .collect();

    Ok(active)
}

// ---------------------------------------------------------------------------
// Multi-file skill commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_skill_file(
    state: State<'_, Arc<AppState>>,
    skill_name: String,
    file_name: String,
) -> Result<String, CommandError> {
    let skill = state
        .skill_registry
        .get(&skill_name)
        .ok_or_else(|| CommandError::not_found(format!("Skill '{}' not found", skill_name)))?;

    if !skill.is_directory {
        return Err(CommandError::validation(format!(
            "Skill '{}' is a single-file skill — use get_skill instead",
            skill_name
        )));
    }

    let file_path = skill.file_path.join(&file_name);
    if !file_path.exists() {
        return Err(CommandError::not_found(format!(
            "File '{}' not found in skill '{}'",
            file_name, skill_name
        )));
    }

    std::fs::read_to_string(&file_path)
        .map_err(|e| CommandError::internal(format!("Failed to read file: {}", e)))
}

#[tauri::command]
pub async fn write_skill_file(
    state: State<'_, Arc<AppState>>,
    skill_name: String,
    file_name: String,
    content: String,
) -> Result<(), CommandError> {
    let skills_dir = state.skill_registry.skills_dir();

    // Prevent writing SKILL.md via this command — use update_skill instead
    if file_name == "SKILL.md" {
        return Err(CommandError::validation(
            "Use update_skill to modify SKILL.md".to_string(),
        ));
    }

    let dir_path = skills_dir.join(&skill_name);
    let file_path = skills_dir.join(format!("{}.md", &skill_name));

    if dir_path.is_dir() && dir_path.join("SKILL.md").exists() {
        // Already a directory — write the file directly
        std::fs::write(dir_path.join(&file_name), &content)
            .map_err(|e| CommandError::internal(format!("Failed to write file: {}", e)))?;
    } else if file_path.exists() {
        // Auto-convert: single-file → directory.
        // Use a temp directory with .converting suffix to avoid a window where
        // both the old file and new directory exist simultaneously.
        let tmp_dir = dir_path.with_extension("converting");

        let existing_content = std::fs::read_to_string(&file_path)
            .map_err(|e| CommandError::internal(format!("Failed to read existing skill: {}", e)))?;

        // Build the new directory structure in a temp location
        if let Err(e) = (|| -> Result<(), std::io::Error> {
            std::fs::create_dir_all(&tmp_dir)?;
            std::fs::write(tmp_dir.join("SKILL.md"), &existing_content)?;
            std::fs::write(tmp_dir.join(&file_name), &content)?;
            Ok(())
        })() {
            // Cleanup temp dir on failure, preserve original
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(CommandError::internal(format!(
                "Failed to build skill directory: {}",
                e
            )));
        }

        // Atomic swap: remove original, rename temp to final
        if let Err(e) = std::fs::remove_file(&file_path) {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(CommandError::internal(format!(
                "Failed to remove original file: {}",
                e
            )));
        }

        if let Err(e) = std::fs::rename(&tmp_dir, &dir_path) {
            // Original is already gone — try to recover by moving temp to final anyway
            tracing::error!("Rename failed after original removal: {e}");
            // Last resort: try copy
            let _ = std::fs::rename(&tmp_dir, &dir_path);
            return Err(CommandError::internal(format!(
                "Failed to finalize skill directory: {}",
                e
            )));
        }
    } else {
        return Err(CommandError::not_found(format!(
            "Skill '{}' does not exist",
            skill_name
        )));
    }

    state.skill_registry.reload();
    Ok(())
}

#[tauri::command]
pub async fn delete_skill_file(
    state: State<'_, Arc<AppState>>,
    skill_name: String,
    file_name: String,
) -> Result<(), CommandError> {
    if file_name == "SKILL.md" {
        return Err(CommandError::validation(
            "Cannot delete SKILL.md — use delete_skill to remove the entire skill".to_string(),
        ));
    }

    let skill = state
        .skill_registry
        .get(&skill_name)
        .ok_or_else(|| CommandError::not_found(format!("Skill '{}' not found", skill_name)))?;

    if !skill.is_directory {
        return Err(CommandError::validation(format!(
            "Skill '{}' is a single-file skill — nothing to delete",
            skill_name
        )));
    }

    let file_path = skill.file_path.join(&file_name);
    if !file_path.exists() {
        return Err(CommandError::not_found(format!(
            "File '{}' not found in skill '{}'",
            file_name, skill_name
        )));
    }

    std::fs::remove_file(&file_path)
        .map_err(|e| CommandError::internal(format!("Failed to delete file: {}", e)))?;

    state.skill_registry.reload();
    Ok(())
}

#[tauri::command]
pub async fn create_skill_directory(
    state: State<'_, Arc<AppState>>,
    dir_name: String,
) -> Result<(), CommandError> {
    let skills_dir = state.skill_registry.skills_dir();
    let dir_path = skills_dir.join(&dir_name);
    let file_path = skills_dir.join(format!("{}.md", &dir_name));

    if dir_path.exists() || file_path.exists() {
        return Err(CommandError::conflict(format!(
            "Skill '{}' already exists",
            dir_name
        )));
    }

    std::fs::create_dir_all(&dir_path)
        .map_err(|e| CommandError::internal(format!("Failed to create directory: {}", e)))?;

    let template = format!(
        "---\nname: {}\ndescription: New skill\ntriggers: []\npriority: normal\nmax_tokens: 1000\n---\n",
        dir_name
    );
    std::fs::write(dir_path.join("SKILL.md"), template)
        .map_err(|e| CommandError::internal(format!("Failed to write SKILL.md: {}", e)))?;

    state.skill_registry.reload();
    Ok(())
}

// ---------------------------------------------------------------------------
// Import commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn fetch_skill_preview(
    state: State<'_, Arc<AppState>>,
    url: String,
) -> Result<super::import::ImportPreview, CommandError> {
    let parsed = super::import::parse_skill_url(&url).map_err(|e| CommandError::validation(e))?;

    let mut preview = super::import::fetch_from_github(&parsed, &url)
        .await
        .map_err(|e| CommandError::internal(e))?;

    // Check if skill already exists in registry
    preview.already_exists = state.skill_registry.get(&preview.skill_name).is_some();

    Ok(preview)
}

#[tauri::command]
pub async fn install_imported_skill(
    state: State<'_, Arc<AppState>>,
    skill_name: String,
    files: Vec<super::import::ImportFile>,
    source_url: String,
    commit_sha: String,
    overwrite: bool,
) -> Result<(), CommandError> {
    let skills_dir = state.skill_registry.skills_dir();

    super::import::install_skill_files(
        skills_dir,
        &skill_name,
        &files,
        overwrite,
        &source_url,
        &commit_sha,
    )
    .map_err(|e| CommandError::internal(e))?;

    state.skill_registry.reload();
    Ok(())
}
