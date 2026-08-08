use crate::config::app_config::base_config_dir;

/// Move a file, falling back to copy+delete if rename fails (cross-device).
fn safe_rename(
    from: &std::path::Path,
    to: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            // Cross-device: copy then delete source
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Migrate from flat agent files to per-agent directories.
/// Runs once on first launch after update. Idempotent.
pub fn migrate_to_per_agent_dirs() -> Result<(), Box<dyn std::error::Error>> {
    let agents_dir = base_config_dir().join("agents");
    let sessions_dir = base_config_dir().join("sessions");
    let marker = base_config_dir().join(".migrated-v2");

    if marker.exists() || !agents_dir.exists() {
        return Ok(());
    }

    // Check for old-format .toml files at top level
    let toml_files: Vec<_> = std::fs::read_dir(&agents_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
        .collect();

    if toml_files.is_empty() {
        // No old-format files — write marker and return
        std::fs::write(&marker, "migrated")?;
        return Ok(());
    }

    for entry in toml_files {
        let path = entry.path();
        let agent_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();

        if agent_id.is_empty() {
            continue;
        }

        let agent_dir = agents_dir.join(&agent_id);
        std::fs::create_dir_all(&agent_dir)?;
        std::fs::create_dir_all(agent_dir.join("memory"))?;
        std::fs::create_dir_all(agent_dir.join("checkpoints"))?;
        std::fs::create_dir_all(agent_dir.join("exports"))?;

        // Move config: agents/{id}.toml → agents/{id}/config.toml
        let new_config = agent_dir.join("config.toml");
        if !new_config.exists() {
            safe_rename(&path, &new_config)?;
        }

        // Move session: sessions/{id}.session.json → agents/{id}/session.json
        let old_session = sessions_dir.join(format!("{}.session.json", agent_id));
        let new_session = agent_dir.join("session.json");
        if old_session.exists() && !new_session.exists() {
            safe_rename(&old_session, &new_session)?;
        }

        // Move checkpoints: sessions/checkpoints/{id}/*.json → agents/{id}/checkpoints/
        let old_cp_dir = sessions_dir.join("checkpoints").join(&agent_id);
        let new_cp_dir = agent_dir.join("checkpoints");
        if old_cp_dir.exists() {
            for cp_entry in std::fs::read_dir(&old_cp_dir)? {
                let cp = cp_entry?;
                let dest = new_cp_dir.join(cp.file_name());
                if !dest.exists() {
                    safe_rename(&cp.path(), &dest)?;
                }
            }
            let _ = std::fs::remove_dir(&old_cp_dir); // clean empty dir
        }
    }

    std::fs::write(&marker, "migrated")?;
    tracing::info!("Migrated to per-agent directory structure");
    Ok(())
}
