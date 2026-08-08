//! Generates per-agent Claude Code config files (`mcp.json`, `settings.json`,
//! `persona.generated.md`) into `~/.config/holt/agents/<id>/` at TUI spawn
//! time.
//!
//! See docs/superpowers/specs/2026-05-16-dynamic-mcp-design.md
//! and docs/specs/UNIVERSAL_PERSONA_INJECTION_SPEC.md.

use crate::runtime::persona::PersonaFile;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Paths to the files written for an agent.
#[derive(Debug, Clone)]
pub struct GeneratedConfig {
    pub mcp_config_path: PathBuf,
    pub settings_path: PathBuf,
    pub persona_path: PathBuf,
}

/// Write `mcp.json`, `settings.json`, and `persona.generated.md` into the
/// agent's config directory. Creates the directory if it doesn't exist.
/// Idempotent — overwrites all files on every call so config edits propagate
/// on the next TUI spawn.
/// `dynamic_profile` is the session-start memory pack, fetched by the caller (which has the
/// async context and the engine) via `persona::fetch_dynamic_profile`. `None` yields a
/// static-only persona — the behaviour this lane had before 2026-07-26.
pub fn write(
    agent_id: &str,
    agent_config_dir: &Path,
    holt_mcp_url: Option<&str>,
    dynamic_profile: Option<PersonaFile>,
) -> std::io::Result<GeneratedConfig> {
    std::fs::create_dir_all(agent_config_dir)?;

    let mcp_config_path = agent_config_dir.join("mcp.json");
    let settings_path = agent_config_dir.join("settings.json");

    let mcp_json = build_mcp_json(agent_id, holt_mcp_url);
    let settings_json = build_settings_json();

    std::fs::write(&mcp_config_path, serde_json::to_string_pretty(&mcp_json)?)?;
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings_json)?,
    )?;

    // Generate concatenated persona file from the spawning agent's persona dir
    // + manifest. Public Holt has no built-in resident/canonical agent.
    let persona_path = generate_persona_file(agent_config_dir, dynamic_profile.as_ref())?;

    Ok(GeneratedConfig {
        mcp_config_path,
        settings_path,
        persona_path,
    })
}

/// Schema for `persona.manifest.json`.
#[derive(Debug, serde::Deserialize)]
struct PersonaManifest {
    #[allow(dead_code)]
    version: u32,
    files: Vec<String>,
    #[serde(default)]
    skip_when_native_memory: Vec<String>,
    /// Files excluded from the snapshot because they change faster than the
    /// process restarts — the snapshot is read once per claude process, so
    /// anything frozen here stays stale across every /clear until the lane
    /// relaunches. These arrive via the SessionStart hook instead; the
    /// snapshot carries a pointer section naming them.
    #[serde(default)]
    session_fresh: Vec<String>,
    /// Whether to append the dynamic memory profile to the snapshot. Defaults
    /// to true (pre-2026-08-07 behaviour). Set false when session-start
    /// injection carries memory instead — the profile's freshness stamps are
    /// computed at spawn and lie for the rest of the process lifetime.
    #[serde(default = "default_dynamic_profile")]
    dynamic_profile: bool,
}

fn default_dynamic_profile() -> bool {
    true
}

/// Read the persona manifest and concatenate listed files into a single
/// generated markdown file matching the SDK's `format_as_system_prompt()`
/// format.
///
/// NOTE: Reads from agents/{agent_id}/persona/ without locking.
/// A concurrent update_persona (SDK lane) can produce a torn read on the file
/// being written. Accepted: window is tiny, next spawn self-heals. If this
/// becomes a real problem, add a file lock or read-via-tempfile pattern.
fn generate_persona_file(
    source_dir: &Path,
    dynamic: Option<&PersonaFile>,
) -> std::io::Result<PathBuf> {
    let persona_dir = source_dir.join("persona");
    let manifest_path = source_dir.join("persona.manifest.json");
    let output_path = source_dir.join("persona.generated.md");

    // Read manifest — hard error if missing or malformed (spec: "do not fall
    // back to a hardcoded list — that's how drift starts").
    let manifest_raw = std::fs::read_to_string(&manifest_path).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!(
                "Persona manifest not found at {}: {e}. The SDK must write this file.",
                manifest_path.display()
            ),
        )
    })?;
    let manifest: PersonaManifest = serde_json::from_str(&manifest_raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Persona manifest at {} is malformed: {e}",
                manifest_path.display()
            ),
        )
    })?;

    let (prompt, files_loaded) = build_persona_prompt(&persona_dir, &manifest, dynamic)?;

    std::fs::write(&output_path, &prompt)?;

    tracing::info!(
        files_loaded,
        total_listed = manifest.files.len(),
        output = %output_path.display(),
        "Persona file generated"
    );

    Ok(output_path)
}

/// Build the persona prompt: static manifest files, then the dynamic memory profile.
///
/// Pure over its inputs so the tests exercise THIS function rather than a copy. The test
/// module used to carry its own `generate_persona_from` reimplementation of this logic —
/// which meant the persona tests would pass even if the real generator broke.
fn build_persona_prompt(
    persona_dir: &Path,
    manifest: &PersonaManifest,
    dynamic: Option<&PersonaFile>,
) -> std::io::Result<(String, usize)> {
    let mut prompt = String::from("# Persona Context\n\n");
    let mut files_loaded = 0;

    for filename in &manifest.files {
        // Skip MEMORY.md (and anything else in skip_when_native_memory) when native memory
        // is active. It always is for us, but the manifest drives it, not an assumption.
        if manifest.skip_when_native_memory.contains(filename) {
            continue;
        }
        // Session-fresh files never enter the snapshot — they'd freeze for the
        // process lifetime. The pointer section below names them instead.
        if manifest.session_fresh.contains(filename) {
            continue;
        }
        let file_path = persona_dir.join(filename);
        match std::fs::read_to_string(&file_path) {
            Ok(content) => {
                prompt.push_str(&format!("## {filename}\n\n"));
                prompt.push_str(&content);
                prompt.push_str("\n\n---\n\n");
                files_loaded += 1;
            }
            Err(_) => {
                // Missing files are skipped silently, matching load_persona_files().
                tracing::debug!(
                    file = %filename,
                    "Persona file listed in manifest but not found on disk, skipping"
                );
            }
        }
    }

    if files_loaded == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "No persona files loaded from {}. All {} manifest entries were missing.",
                persona_dir.display(),
                manifest.files.len()
            ),
        ));
    }

    // Pointer for the session-fresh layer: the snapshot must say what it
    // deliberately lacks, or a session whose SessionStart hook failed has no
    // hint the excluded state exists at all.
    if !manifest.session_fresh.is_empty() {
        prompt.push_str("## Session-fresh state (deliberately not in this snapshot)\n\n");
        prompt.push_str(&format!(
            "This snapshot is read once per process and would go stale across /clear. \
             These persona files are excluded and injected FRESH at every session start \
             by the SessionStart hook instead: {}. Trust the session-start injection and \
             the live canonical files in the persona directory — never a frozen copy.\n\n---\n\n",
            manifest.session_fresh.join(", ")
        ));
    }

    // Session-start memory, appended after the static files exactly as the Holt-driven
    // lanes do it. Until 2026-07-26 this lane got no memory at all: `fetch_dynamic_profile`
    // was wired into payload.rs, agent_sdk.rs and codex.rs but never here, so pinned
    // memories — including the cold-start protocol — never reached the TUI.
    // Manifest-gated since 2026-08-07: when session-start injection carries memory,
    // a spawn-time profile with frozen freshness stamps is stale data, not a fallback.
    if manifest.dynamic_profile {
        if let Some(profile) = dynamic {
            prompt.push_str(&format!("## {}\n\n", profile.name));
            prompt.push_str(&profile.content);
            prompt.push_str("\n\n---\n\n");
        }
    }

    Ok((prompt, files_loaded))
}

fn build_mcp_json(agent_id: &str, holt_mcp_url: Option<&str>) -> serde_json::Value {
    let holt_url = match holt_mcp_url {
        Some(base) => format!("{base}?agent_id={agent_id}"),
        None => format!("http://localhost:0/mcp?agent_id={agent_id}"),
    };
    json!({
        "mcpServers": {
            "holt": {
                "type": "http",
                "url": holt_url
            },
            // Mobbin — real-app UI reference library (design de-risk kit). Remote HTTP MCP,
            // OAuth on first use. Stopgap hardcode; Holt's agent-surface makes this config-driven.
            "mobbin": {
                "type": "http",
                "url": "https://api.mobbin.com/mcp"
            }
        }
    })
}

fn build_settings_json() -> serde_json::Value {
    json!({
        "permissions": {
            "allow": [
                "mcp__holt__*",
                "mcp__mobbin__*",
                "Bash(git:*)",
                "Bash(cargo check:*)",
                "Bash(cargo test:*)",
                "Bash(cargo build:*)",
                "Bash(npm run:*)",
                "Bash(ls:*)",
                "Bash(find:*)",
                "Bash(grep:*)",
                "Bash(journalctl:*)",
                "Bash(systemctl --user:*)"
            ]
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed_persona(agent_dir: &Path) {
        let persona_dir = agent_dir.join("persona");
        std::fs::create_dir_all(&persona_dir).unwrap();
        std::fs::write(persona_dir.join("SOUL.md"), "Be helpful.").unwrap();
        std::fs::write(
            agent_dir.join("persona.manifest.json"),
            serde_json::json!({"version": 1, "files": ["SOUL.md"]}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn write_creates_dir_and_both_files() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("agent-terminal");
        seed_persona(&agent_dir);

        let result = write(
            "agent-terminal",
            &agent_dir,
            Some("http://127.0.0.1:54321/mcp"),
            None,
        )
        .unwrap();

        assert!(agent_dir.is_dir());
        assert!(result.mcp_config_path.is_file());
        assert!(result.settings_path.is_file());
        assert!(result.persona_path.is_file());
    }

    #[test]
    fn mcp_json_embeds_agent_id_in_holt_url() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent-terminal");
        seed_persona(&agent_dir);
        let result = write(
            "agent-terminal",
            &agent_dir,
            Some("http://127.0.0.1:54321/mcp"),
            None,
        )
        .unwrap();

        let text = std::fs::read_to_string(&result.mcp_config_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        let url = parsed["mcpServers"]["holt"]["url"].as_str().unwrap();
        assert_eq!(url, "http://127.0.0.1:54321/mcp?agent_id=agent-terminal");
    }

    #[test]
    fn mcp_and_settings_include_mobbin_not_dead_bridge() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent-terminal");
        seed_persona(&agent_dir);
        let result = write(
            "agent-terminal",
            &agent_dir,
            Some("http://127.0.0.1:54321/mcp"),
            None,
        )
        .unwrap();

        // mcp.json: mobbin present, retired memory_bridge gone
        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&result.mcp_config_path).unwrap())
                .unwrap();
        assert_eq!(
            mcp["mcpServers"]["mobbin"]["url"].as_str().unwrap(),
            "https://api.mobbin.com/mcp"
        );
        assert!(
            mcp["mcpServers"]["memory_bridge"].is_null(),
            "retired memory_bridge should be gone"
        );

        // settings.json: mobbin tools pre-allowed, dead bridge allow gone
        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&result.settings_path).unwrap()).unwrap();
        let allow: Vec<&str> = settings["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            allow.contains(&"mcp__holt__*"),
            "holt tools must be pre-allowed"
        );
        assert!(
            allow.contains(&"mcp__mobbin__*"),
            "mobbin tools must be pre-allowed"
        );
        assert!(
            !allow.contains(&"mcp__memory_bridge__*"),
            "dead bridge allow should be gone"
        );
    }

    #[test]
    fn write_is_idempotent_overwriting_existing_files() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent-terminal");
        seed_persona(&agent_dir);
        write(
            "agent-terminal",
            &agent_dir,
            Some("http://127.0.0.1:54321/mcp"),
            None,
        )
        .unwrap();
        // Second call must succeed and overwrite.
        let result = write(
            "agent-terminal",
            &agent_dir,
            Some("http://127.0.0.1:54321/mcp"),
            None,
        )
        .unwrap();
        assert!(result.mcp_config_path.is_file());
    }

    // -- Persona generation --
    //
    // These call the REAL `build_persona_prompt`. This shim used to be a full
    // reimplementation of it, which meant these tests would pass even if the production
    // generator broke. Only the file I/O around it is test-local now.
    fn generate_persona_from(
        persona_dir: &Path,
        manifest_path: &Path,
        output_path: &Path,
    ) -> std::io::Result<()> {
        generate_persona_from_with(persona_dir, manifest_path, output_path, None)
    }

    fn generate_persona_from_with(
        persona_dir: &Path,
        manifest_path: &Path,
        output_path: &Path,
        dynamic: Option<&PersonaFile>,
    ) -> std::io::Result<()> {
        let manifest_raw = std::fs::read_to_string(manifest_path)?;
        let manifest: PersonaManifest = serde_json::from_str(&manifest_raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let (prompt, _) = build_persona_prompt(persona_dir, &manifest, dynamic)?;
        std::fs::write(output_path, &prompt)
    }

    /// The TUI lane got no memory at all until 2026-07-26: `fetch_dynamic_profile` was
    /// wired into payload.rs, agent_sdk.rs and codex.rs but never here, so pinned memories
    /// — including the cold-start protocol — never reached it. This pins the fix.
    #[test]
    fn dynamic_profile_is_appended_after_the_static_files() {
        let tmp = TempDir::new().unwrap();
        let persona_dir = tmp.path().join("persona");
        std::fs::create_dir(&persona_dir).unwrap();
        std::fs::write(persona_dir.join("SOUL.md"), "Be helpful.").unwrap();

        let manifest_path = tmp.path().join("persona.manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::json!({"version": 1, "files": ["SOUL.md"]}).to_string(),
        )
        .unwrap();

        let output_path = tmp.path().join("persona.generated.md");
        let profile = PersonaFile {
            name: "MEMORY_CONTEXT.md".to_string(),
            content: "COLD START PROTOCOL — read ACTIVE_WORK.md first.".to_string(),
        };
        generate_persona_from_with(&persona_dir, &manifest_path, &output_path, Some(&profile))
            .unwrap();

        let out = std::fs::read_to_string(&output_path).unwrap();
        assert!(
            out.contains("## MEMORY_CONTEXT.md"),
            "memory section missing"
        );
        assert!(
            out.contains("COLD START PROTOCOL"),
            "memory content missing"
        );
        assert!(
            out.find("## SOUL.md").unwrap() < out.find("## MEMORY_CONTEXT.md").unwrap(),
            "memory must come after the static files, matching the other lanes"
        );
    }

    /// No engine, or a failed fetch, must degrade to exactly the old behaviour rather than
    /// emitting an empty memory heading.
    #[test]
    fn absent_dynamic_profile_leaves_a_static_only_persona() {
        let tmp = TempDir::new().unwrap();
        let persona_dir = tmp.path().join("persona");
        std::fs::create_dir(&persona_dir).unwrap();
        std::fs::write(persona_dir.join("SOUL.md"), "Be helpful.").unwrap();

        let manifest_path = tmp.path().join("persona.manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::json!({"version": 1, "files": ["SOUL.md"]}).to_string(),
        )
        .unwrap();

        let output_path = tmp.path().join("persona.generated.md");
        generate_persona_from_with(&persona_dir, &manifest_path, &output_path, None).unwrap();

        let out = std::fs::read_to_string(&output_path).unwrap();
        assert!(out.contains("## SOUL.md"));
        assert_eq!(
            out.matches("## ").count(),
            1,
            "only the static file may appear"
        );
    }

    #[test]
    fn persona_gen_produces_sdk_format() {
        let tmp = TempDir::new().unwrap();
        let persona_dir = tmp.path().join("persona");
        std::fs::create_dir(&persona_dir).unwrap();

        std::fs::write(persona_dir.join("SOUL.md"), "Be helpful.").unwrap();
        std::fs::write(persona_dir.join("IDENTITY.md"), "Name: Example").unwrap();

        let manifest = serde_json::json!({
            "version": 1,
            "files": ["SOUL.md", "IDENTITY.md"],
            "skip_when_native_memory": ["MEMORY.md"]
        });
        let manifest_path = tmp.path().join("persona.manifest.json");
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();

        let output_path = tmp.path().join("persona.generated.md");
        generate_persona_from(&persona_dir, &manifest_path, &output_path).unwrap();

        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.starts_with("# Persona Context\n\n"));
        assert!(content.contains("## SOUL.md\n\nBe helpful.\n\n---\n\n"));
        assert!(content.contains("## IDENTITY.md\n\nName: Example\n\n---\n\n"));
        // SOUL before IDENTITY (manifest order preserved)
        let soul_pos = content.find("## SOUL.md").unwrap();
        let identity_pos = content.find("## IDENTITY.md").unwrap();
        assert!(soul_pos < identity_pos);
    }

    #[test]
    fn persona_gen_skips_missing_files() {
        let tmp = TempDir::new().unwrap();
        let persona_dir = tmp.path().join("persona");
        std::fs::create_dir(&persona_dir).unwrap();

        // Only SOUL.md exists; COGNITIVE_PROFILE.md is listed but missing
        std::fs::write(persona_dir.join("SOUL.md"), "Be helpful.").unwrap();

        let manifest = serde_json::json!({
            "version": 1,
            "files": ["SOUL.md", "COGNITIVE_PROFILE.md"],
            "skip_when_native_memory": []
        });
        let manifest_path = tmp.path().join("persona.manifest.json");
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();

        let output_path = tmp.path().join("persona.generated.md");
        generate_persona_from(&persona_dir, &manifest_path, &output_path).unwrap();

        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("## SOUL.md"));
        assert!(!content.contains("COGNITIVE_PROFILE.md"));
    }

    #[test]
    fn persona_gen_skips_native_memory_files() {
        let tmp = TempDir::new().unwrap();
        let persona_dir = tmp.path().join("persona");
        std::fs::create_dir(&persona_dir).unwrap();

        std::fs::write(persona_dir.join("SOUL.md"), "Be helpful.").unwrap();
        std::fs::write(persona_dir.join("MEMORY.md"), "Old memories.").unwrap();

        let manifest = serde_json::json!({
            "version": 1,
            "files": ["SOUL.md", "MEMORY.md"],
            "skip_when_native_memory": ["MEMORY.md"]
        });
        let manifest_path = tmp.path().join("persona.manifest.json");
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();

        let output_path = tmp.path().join("persona.generated.md");
        generate_persona_from(&persona_dir, &manifest_path, &output_path).unwrap();

        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("## SOUL.md"));
        assert!(!content.contains("MEMORY.md"));
    }

    #[test]
    fn persona_gen_errors_when_all_files_missing() {
        let tmp = TempDir::new().unwrap();
        let persona_dir = tmp.path().join("persona");
        std::fs::create_dir(&persona_dir).unwrap();

        // No files exist on disk
        let manifest = serde_json::json!({
            "version": 1,
            "files": ["NONEXISTENT.md"],
            "skip_when_native_memory": []
        });
        let manifest_path = tmp.path().join("persona.manifest.json");
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();

        let output_path = tmp.path().join("persona.generated.md");
        let result = generate_persona_from(&persona_dir, &manifest_path, &output_path);
        assert!(result.is_err());
    }

    #[test]
    fn persona_gen_errors_on_missing_manifest() {
        let tmp = TempDir::new().unwrap();
        let persona_dir = tmp.path().join("persona");
        std::fs::create_dir(&persona_dir).unwrap();

        let manifest_path = tmp.path().join("persona.manifest.json");
        // manifest_path does NOT exist
        let output_path = tmp.path().join("persona.generated.md");
        let result = generate_persona_from(&persona_dir, &manifest_path, &output_path);
        assert!(result.is_err());
    }

    #[test]
    fn persona_gen_errors_on_malformed_manifest() {
        let tmp = TempDir::new().unwrap();
        let persona_dir = tmp.path().join("persona");
        std::fs::create_dir(&persona_dir).unwrap();

        let manifest_path = tmp.path().join("persona.manifest.json");
        std::fs::write(&manifest_path, "not valid json {{{").unwrap();

        let output_path = tmp.path().join("persona.generated.md");
        let result = generate_persona_from(&persona_dir, &manifest_path, &output_path);
        assert!(result.is_err());
    }

    // -- Session-fresh persona (2026-08-07) --
    //
    // The snapshot is read ONCE per claude process (`--append-system-prompt-file`),
    // so fast-moving files frozen into it go stale across every /clear until the
    // lane relaunches. Files the manifest marks `session_fresh` are excluded from
    // the snapshot and arrive via the SessionStart hook instead; the snapshot
    // carries a pointer saying so.

    #[test]
    fn session_fresh_files_are_excluded_with_a_pointer() {
        let tmp = TempDir::new().unwrap();
        let persona_dir = tmp.path().join("persona");
        std::fs::create_dir(&persona_dir).unwrap();
        std::fs::write(persona_dir.join("SOUL.md"), "Be helpful.").unwrap();
        std::fs::write(
            persona_dir.join("ACTIVE_WORK.md"),
            "STALE STATE FROM LAUNCH DAY",
        )
        .unwrap();

        let manifest_path = tmp.path().join("persona.manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::json!({
                "version": 1,
                "files": ["SOUL.md", "ACTIVE_WORK.md"],
                "session_fresh": ["ACTIVE_WORK.md"]
            })
            .to_string(),
        )
        .unwrap();

        let output_path = tmp.path().join("persona.generated.md");
        generate_persona_from(&persona_dir, &manifest_path, &output_path).unwrap();

        let out = std::fs::read_to_string(&output_path).unwrap();
        assert!(
            out.contains("## SOUL.md"),
            "slow-layer file must still be present"
        );
        assert!(
            !out.contains("STALE STATE FROM LAUNCH DAY"),
            "session-fresh file content must not be frozen into the snapshot"
        );
        assert!(
            !out.contains("## ACTIVE_WORK.md"),
            "session-fresh file must not get a persona section"
        );
        assert!(
            out.contains("ACTIVE_WORK.md"),
            "pointer must name the excluded file"
        );
        assert!(
            out.contains("session start"),
            "pointer must say the live copy arrives at session start"
        );
    }

    #[test]
    fn dynamic_profile_false_keeps_memory_out_of_the_snapshot() {
        let tmp = TempDir::new().unwrap();
        let persona_dir = tmp.path().join("persona");
        std::fs::create_dir(&persona_dir).unwrap();
        std::fs::write(persona_dir.join("SOUL.md"), "Be helpful.").unwrap();

        let manifest_path = tmp.path().join("persona.manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::json!({
                "version": 1,
                "files": ["SOUL.md"],
                "dynamic_profile": false
            })
            .to_string(),
        )
        .unwrap();

        let output_path = tmp.path().join("persona.generated.md");
        let profile = PersonaFile {
            name: "ACTIVE_MEMORY_PROFILE".to_string(),
            content: "pinned memories with frozen staleness stamps".to_string(),
        };
        generate_persona_from_with(&persona_dir, &manifest_path, &output_path, Some(&profile))
            .unwrap();

        let out = std::fs::read_to_string(&output_path).unwrap();
        assert!(out.contains("## SOUL.md"));
        assert!(
            !out.contains("ACTIVE_MEMORY_PROFILE"),
            "profile must not be appended when the manifest disables it"
        );
        assert!(!out.contains("pinned memories with frozen staleness stamps"));
    }

    #[test]
    fn manifest_without_new_fields_keeps_old_behaviour_and_emits_no_pointer() {
        let tmp = TempDir::new().unwrap();
        let persona_dir = tmp.path().join("persona");
        std::fs::create_dir(&persona_dir).unwrap();
        std::fs::write(persona_dir.join("SOUL.md"), "Be helpful.").unwrap();

        let manifest_path = tmp.path().join("persona.manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::json!({"version": 1, "files": ["SOUL.md"]}).to_string(),
        )
        .unwrap();

        let output_path = tmp.path().join("persona.generated.md");
        let profile = PersonaFile {
            name: "MEMORY_CONTEXT.md".to_string(),
            content: "still appended by default".to_string(),
        };
        generate_persona_from_with(&persona_dir, &manifest_path, &output_path, Some(&profile))
            .unwrap();

        let out = std::fs::read_to_string(&output_path).unwrap();
        assert!(
            out.contains("## MEMORY_CONTEXT.md"),
            "dynamic profile must default to appended"
        );
        assert!(
            !out.contains("Session-fresh"),
            "no pointer section when nothing is excluded"
        );
    }
}
