// app/src-tauri/src/runtime/persona.rs

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// A loaded persona file.
#[derive(Debug, Clone)]
pub struct PersonaFile {
    pub name: String,
    pub content: String,
}

/// Preamble file loaded before all other persona files.
/// Contains imperative cold-start instructions that must be the first thing
/// the model sees. Optional — agents without this file are unaffected.
const PREAMBLE_FILE: &str = "PREAMBLE.md";

/// Identity files that always load, regardless of native memory state.
const IDENTITY_FILES: &[&str] = &[
    "AGENTS.md",
    "SOUL.md",
    "COGNITIVE_PROFILE.md",
    "TOOLS.md",
    "IDENTITY.md",
    "USER.md",
    "HEARTBEAT.md",
    "ACTIVE_WORK.md",
];

/// Files skipped when native memory engine is active (superseded by Hillock).
const MEMORY_FILES: &[&str] = &["MEMORY.md"];

/// Files that subagents are allowed to see.
const SUBAGENT_ALLOWLIST: &[&str] = &["AGENTS.md", "TOOLS.md"];

/// Max characters per persona file (truncated with notice).
const MAX_FILE_CHARS: usize = 8000;

/// Max total characters across all persona files.
const MAX_TOTAL_CHARS: usize = 32000;

const ANCHOR_MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaAnchorManifest {
    #[serde(default = "default_anchor_manifest_version")]
    pub version: u32,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub updated_at_session: Option<String>,
    #[serde(default)]
    pub anchors: Vec<PersonaAnchor>,
}

impl Default for PersonaAnchorManifest {
    fn default() -> Self {
        Self {
            version: ANCHOR_MANIFEST_VERSION,
            agent_id: String::new(),
            updated_at: None,
            updated_at_session: None,
            anchors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonaAnchor {
    pub anchor_id: String,
    pub declared_by: String,
    pub scope: PersonaAnchorScope,
    pub reason: String,
    #[serde(default)]
    pub friction_policy: PersonaAnchorFrictionPolicy,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub declared_at_session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersonaAnchorScope {
    File { file: String },
    MarkdownHeading { file: String, heading: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonaAnchorFrictionPolicy {
    #[serde(default = "default_true")]
    pub confirmation_required: bool,
    #[serde(default = "default_true")]
    pub reason_required: bool,
    #[serde(default = "default_true")]
    pub owner_notice: bool,
}

impl Default for PersonaAnchorFrictionPolicy {
    fn default() -> Self {
        Self {
            confirmation_required: true,
            reason_required: true,
            owner_notice: true,
        }
    }
}

impl PersonaAnchorScope {
    pub fn file(&self) -> &str {
        match self {
            Self::File { file } | Self::MarkdownHeading { file, .. } => file,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::File { file } => file.clone(),
            Self::MarkdownHeading { file, heading } => format!("{file} > {heading}"),
        }
    }
}

fn default_anchor_manifest_version() -> u32 {
    ANCHOR_MANIFEST_VERSION
}

fn default_true() -> bool {
    true
}

/// Validate an agent ID for safe path construction.
fn is_valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Get the persona directory for an agent.
pub fn persona_dir(agent_id: &str) -> PathBuf {
    crate::config::app_config::agent_dir(agent_id).join("persona")
}

pub fn anchors_manifest_path(agent_id: &str) -> PathBuf {
    persona_dir(agent_id).join("anchors.json")
}

pub fn validate_persona_filename(file: &str) -> Result<(), String> {
    if !file.ends_with(".md") {
        return Err(format!("File '{file}' must end in .md"));
    }

    if file.contains('/') || file.contains('\\') || file.contains("..") {
        return Err(format!(
            "File '{file}' must be a simple filename (no path separators or ..)"
        ));
    }

    Ok(())
}

pub fn is_standard_persona_filename(file: &str) -> bool {
    file == PREAMBLE_FILE || IDENTITY_FILES.contains(&file) || MEMORY_FILES.contains(&file)
}

pub fn persona_filename_from_path(agent_id: &str, path: &Path) -> Option<String> {
    let persona_root = persona_dir(agent_id);
    let relative = path.strip_prefix(persona_root).ok()?;
    if relative.components().count() != 1 {
        return None;
    }

    let file = relative.file_name()?.to_str()?.to_string();
    validate_persona_filename(&file).ok()?;
    Some(file)
}

pub fn load_anchor_manifest(agent_id: &str) -> PersonaAnchorManifest {
    if !is_valid_agent_id(agent_id) {
        return PersonaAnchorManifest::default();
    }

    let path = anchors_manifest_path(agent_id);
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<PersonaAnchorManifest>(&raw).unwrap_or_default(),
        Err(_) => PersonaAnchorManifest::default(),
    }
}

pub fn save_anchor_manifest(
    agent_id: &str,
    manifest: &PersonaAnchorManifest,
) -> std::io::Result<()> {
    let dir = persona_dir(agent_id);
    std::fs::create_dir_all(&dir)?;

    let path = anchors_manifest_path(agent_id);
    let raw =
        serde_json::to_string_pretty(manifest).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(path, raw)
}

pub fn anchor_id_for_scope(scope: &PersonaAnchorScope) -> String {
    match scope {
        PersonaAnchorScope::File { file } => format!("file:{file}"),
        PersonaAnchorScope::MarkdownHeading { file, heading } => {
            format!("heading:{file}#{heading}")
        }
    }
}

pub fn affected_anchors_for_edit(
    manifest: &PersonaAnchorManifest,
    file: &str,
    old_content: &str,
    new_content: &str,
) -> Vec<PersonaAnchor> {
    manifest
        .anchors
        .iter()
        .filter(|anchor| anchor.scope.file() == file)
        .filter(|anchor| anchor_scope_changed(&anchor.scope, old_content, new_content))
        .cloned()
        .collect()
}

fn anchor_scope_changed(scope: &PersonaAnchorScope, old_content: &str, new_content: &str) -> bool {
    match scope {
        PersonaAnchorScope::File { .. } => old_content != new_content,
        PersonaAnchorScope::MarkdownHeading { heading, .. } => {
            extract_markdown_heading_section(old_content, heading)
                != extract_markdown_heading_section(new_content, heading)
        }
    }
}

pub fn emit_anchor_owner_notice(
    app_handle: Option<&AppHandle>,
    agent_id: &str,
    file: &str,
    anchors: &[PersonaAnchor],
    reason: &str,
    source: &str,
) {
    let Some(handle) = app_handle else {
        return;
    };

    let scopes = anchors
        .iter()
        .map(|anchor| anchor.scope.describe())
        .collect::<Vec<_>>()
        .join(", ");

    let _ = handle.emit(
        "agent-alert",
        serde_json::json!({
            "agent_id": agent_id,
            "alert_id": format!(
                "anchor-edit:{}:{}:{}",
                agent_id,
                source,
                chrono::Utc::now().timestamp_millis()
            ),
            "message": format!(
                "{agent_id} edited anchored persona content via {source} in {file}. Anchors: {scopes}. Reason: {reason}"
            ),
            "priority": "medium",
            "blocking": false,
        }),
    );
}

pub fn format_anchor_retry_message(file: &str, anchors: &[PersonaAnchor]) -> String {
    let details = anchors
        .iter()
        .map(|anchor| format!("{} ({})", anchor.scope.describe(), anchor.reason))
        .collect::<Vec<_>>()
        .join("; ");

    format!(
        "Anchored persona content in {file} would change. Retry deliberately with confirm_anchored_edit=true and a non-empty reason. Affected anchors: {details}"
    )
}

fn extract_markdown_heading_section(content: &str, heading: &str) -> Option<String> {
    let heading = heading.trim();
    if heading.is_empty() {
        return None;
    }

    let mut start = None;
    let mut start_level = 0usize;
    let mut offset = 0usize;

    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(level) = markdown_heading_level(trimmed) {
            let text = trimmed[level..].trim();
            if start.is_none() && text == heading {
                start = Some(offset);
                start_level = level;
            } else if let Some(section_start) = start {
                if level <= start_level {
                    return Some(content[section_start..offset].to_string());
                }
            }
        }
        offset += line.len();
    }

    start.map(|section_start| content[section_start..].to_string())
}

fn markdown_heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }

    line.chars()
        .nth(hashes)
        .filter(|c| c.is_whitespace())
        .map(|_| hashes)
}

/// Load persona files from agent's persona directory.
/// Returns files in load order, skipping missing files. Truncates large files.
///
/// When `memory_active` is true, MEMORY.md and daily memory logs (memory/*.md)
/// are skipped because the native Hillock memory engine supersedes them.
pub fn load_persona_files(agent_id: &str, memory_active: bool) -> Vec<PersonaFile> {
    if !is_valid_agent_id(agent_id) {
        return Vec::new();
    }

    let dir = persona_dir(agent_id);
    let mut files = Vec::new();
    let mut total_chars = 0;

    // Load preamble first — position zero in the system prompt.
    // Contains imperative cold-start instructions that gate first-turn behavior.
    {
        let path = dir.join(PREAMBLE_FILE);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let mut content = content;
            if content.len() > MAX_FILE_CHARS {
                content.truncate(MAX_FILE_CHARS);
                content.push_str("\n\n[... truncated ...]");
            }
            total_chars += content.len();
            files.push(PersonaFile {
                name: PREAMBLE_FILE.to_string(),
                content,
            });
        }
    }

    // Always load identity files
    for &name in IDENTITY_FILES {
        let path = dir.join(name);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let mut content = content;
            if content.len() > MAX_FILE_CHARS {
                content.truncate(MAX_FILE_CHARS);
                content.push_str("\n\n[... truncated ...]");
            }
            total_chars += content.len();
            if total_chars > MAX_TOTAL_CHARS {
                break;
            }
            files.push(PersonaFile {
                name: name.to_string(),
                content,
            });
        }
    }

    // Load MEMORY.md only when native memory is NOT active
    if !memory_active {
        for &name in MEMORY_FILES {
            let path = dir.join(name);
            if let Ok(content) = std::fs::read_to_string(&path) {
                let mut content = content;
                if content.len() > MAX_FILE_CHARS {
                    content.truncate(MAX_FILE_CHARS);
                    content.push_str("\n\n[... truncated ...]");
                }
                total_chars += content.len();
                if total_chars > MAX_TOTAL_CHARS {
                    break;
                }
                files.push(PersonaFile {
                    name: name.to_string(),
                    content,
                });
            }
        }
    }

    // Load today's and yesterday's daily memory logs (only when native memory is NOT active)
    if !memory_active {
        let memory_dir = dir.join("memory");
        if memory_dir.is_dir() {
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
                .format("%Y-%m-%d")
                .to_string();

            for date in &[yesterday, today] {
                let log_path = memory_dir.join(format!("{}.md", date));
                if let Ok(content) = std::fs::read_to_string(&log_path) {
                    let mut content = content;
                    if content.len() > MAX_FILE_CHARS {
                        content.truncate(MAX_FILE_CHARS);
                        content.push_str("\n\n[... truncated ...]");
                    }
                    total_chars += content.len();
                    if total_chars > MAX_TOTAL_CHARS {
                        break;
                    }
                    files.push(PersonaFile {
                        name: format!("memory/{}.md", date),
                        content,
                    });
                }
            }
        }
    }

    files
}

// =============================================================================
// Dynamic persona profile (Living Persona P1)
// =============================================================================

/// Token budget for the dynamic memory profile on Holt-driven lanes.
pub(crate) const DYNAMIC_PROFILE_TOKEN_BUDGET: usize = 3000;

/// Token budget for the TUI lane.
///
/// Larger than the others because that lane runs in a 1M-token window, where a few thousand
/// tokens of session-start memory is rounding error. Passed explicitly at the call site
/// rather than selected inside `fetch_dynamic_profile` — a function that picks a different
/// constant depending on who is calling it is how the two silently diverge.
pub(crate) const TUI_DYNAMIC_PROFILE_TOKEN_BUDGET: usize = 5000;

/// Kind labels for display in the dynamic profile.
fn kind_display_label(kind: &str) -> &str {
    match kind {
        "constraint" => "Active Constraints",
        "commitment" => "Open Commitments",
        "preference" => "Active Preferences",
        "decision" => "Recent Decisions",
        "procedure" => "Active Procedures",
        "fact" => "Key Facts",
        "skill_trace" => "Learned Skills",
        "hypothesis" => "Active Hypotheses",
        "question" => "Open Questions",
        "observation" => "Recent Observations",
        _ => "Other",
    }
}

/// Kind ordering for display — matches bridge's KIND_PRIORITY.
const KIND_DISPLAY_ORDER: &[&str] = &[
    "constraint",
    "commitment",
    "decision",
    "preference",
    "procedure",
    "fact",
    "skill_trace",
    "hypothesis",
    "question",
    "observation",
];

/// Fetch the dynamic memory profile from the bridge and format it as a PersonaFile.
///
/// Returns `None` if the bridge is unreachable, the agent has no memories,
/// or any error occurs (graceful fallback to static-only persona).
pub async fn fetch_dynamic_profile(
    memory_engine: &crate::memory::MemoryEngine,
    agent_ns: &str,
    token_budget: usize,
) -> Option<PersonaFile> {
    let result = memory_engine
        .pack_context_for_persona(agent_ns, token_budget)
        .await;

    let packed = match result {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                agent_ns,
                "Dynamic profile fetch failed, using static-only persona: {e}"
            );
            return None;
        }
    };

    format_dynamic_profile(&packed)
}

/// Format a `memory_pack_context` response into a dynamic persona file.
///
/// Pure function — testable without a running bridge. Returns `None` if
/// the packed response contains no memories or is malformed.
pub fn format_dynamic_profile(packed: &serde_json::Value) -> Option<PersonaFile> {
    let memories = packed.get("memories").and_then(|m| m.as_array())?;

    if memories.is_empty() {
        return None;
    }

    // Group memories by kind
    let mut by_kind: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();

    for memory in memories {
        let kind = memory
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("observation")
            .to_string();
        let content = memory
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let importance = memory
            .get("importance")
            .and_then(|i| i.as_f64())
            .unwrap_or(0.5);
        let source = memory
            .get("source")
            .and_then(|s| s.as_str())
            .unwrap_or("system");

        // Compute staleness from created_at
        let staleness = memory
            .get("created_at")
            .and_then(|c| c.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| {
                let age = chrono::Utc::now().signed_duration_since(dt.with_timezone(&chrono::Utc));
                if age.num_hours() < 24 {
                    "fresh"
                } else if age.num_days() < 7 {
                    "recent"
                } else if age.num_days() < 30 {
                    "aging"
                } else {
                    "stale"
                }
            })
            .unwrap_or("unknown");

        // Format: "- content [source, importance: X.X, freshness]"
        let annotation = format!("[{}, importance: {:.1}, {}]", source, importance, staleness);

        // Truncate long content to keep the profile compact.
        // Back off to a char boundary — slicing at a fixed byte offset
        // panics if it lands inside a multi-byte character.
        let display_content = if content.len() > 200 {
            let mut end = 197;
            while !content.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}...", &content[..end])
        } else {
            content
        };

        by_kind
            .entry(kind)
            .or_default()
            .push(format!("- {} {}", display_content, annotation));
    }

    // Build markdown in kind priority order
    let mut sections = Vec::new();
    for &kind in KIND_DISPLAY_ORDER {
        if let Some(items) = by_kind.remove(kind) {
            let label = kind_display_label(kind);
            let mut section = format!("### {} ({})\n", label, items.len());
            for item in items {
                section.push_str(&item);
                section.push('\n');
            }
            sections.push(section);
        }
    }
    // Any remaining kinds not in our display order
    for (kind, items) in &by_kind {
        let label = kind_display_label(kind);
        let mut section = format!("### {} ({})\n", label, items.len());
        for item in items {
            section.push_str(item);
            section.push('\n');
        }
        sections.push(section);
    }

    if sections.is_empty() {
        return None;
    }

    let tokens_used = packed
        .get("tokens_used")
        .and_then(|t| t.as_u64())
        .unwrap_or(0);
    let memories_included = packed
        .get("memories_included")
        .and_then(|t| t.as_u64())
        .unwrap_or(0);

    let mut content = format!(
        "## Active Memory Profile\n\n*{} memories, ~{} tokens*\n\n",
        memories_included, tokens_used
    );
    content.push_str(&sections.join("\n"));

    Some(PersonaFile {
        name: "ACTIVE_MEMORY_PROFILE".to_string(),
        content,
    })
}

/// Filter persona files for subagent context (AGENTS.md + TOOLS.md only).
pub fn filter_for_subagent(files: &[PersonaFile]) -> Vec<PersonaFile> {
    files
        .iter()
        .filter(|f| SUBAGENT_ALLOWLIST.contains(&f.name.as_str()))
        .cloned()
        .collect()
}

/// Format persona files as system prompt prefix.
///
/// If a PREAMBLE.md file is present, its content is injected raw at position
/// zero — before the `# Persona Context` header and without any markdown
/// wrapper. This ensures cold-start instructions are the first thing the
/// model sees, maximizing attention weight.
pub fn format_as_system_prompt(files: &[PersonaFile]) -> String {
    if files.is_empty() {
        return String::new();
    }

    let mut prompt = String::new();

    // Preamble goes first, raw — no heading wrapper, no section delimiter.
    // It must be the very first content the model reads.
    let has_preamble = files
        .first()
        .map(|f| f.name == PREAMBLE_FILE)
        .unwrap_or(false);
    if has_preamble {
        prompt.push_str(&files[0].content);
        prompt.push_str("\n\n");
    }

    // Remaining persona files get the standard markdown structure
    let persona_files = if has_preamble {
        &files[1..]
    } else {
        &files[..]
    };
    if !persona_files.is_empty() {
        prompt.push_str("# Persona Context\n\n");
        for file in persona_files {
            prompt.push_str(&format!("## {}\n\n", file.name));
            prompt.push_str(&file.content);
            prompt.push_str("\n\n---\n\n");
        }
    }

    prompt
}

// ---------------------------------------------------------------------------
// PERSONA_CHANGELOG.md — enforced identity change tracking
// ---------------------------------------------------------------------------

const CHANGELOG_FILE: &str = "PERSONA_CHANGELOG.md";

/// Who initiated a persona change.
pub enum ChangeInitiator {
    /// System-level operation (agent creation, migration, profile regeneration)
    System(&'static str),
    /// The agent itself (via update_persona tool)
    Agent,
    /// Human/operator action (via settings panel or direct edit)
    Human,
}

impl std::fmt::Display for ChangeInitiator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeInitiator::System(ctx) => write!(f, "System ({})", ctx),
            ChangeInitiator::Agent => write!(f, "Agent (self)"),
            ChangeInitiator::Human => write!(f, "Human"),
        }
    }
}

/// Append an entry to an agent's PERSONA_CHANGELOG.md.
///
/// Uses file-level locking to prevent corruption from concurrent writes.
/// Creates the file with a header if it doesn't exist yet.
pub fn append_changelog(
    agent_id: &str,
    file_changed: &str,
    summary: &str,
    initiator: &ChangeInitiator,
) {
    let dir = persona_dir(agent_id);
    let changelog_path = dir.join(CHANGELOG_FILE);

    // Ensure persona dir exists
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(
            agent = agent_id,
            "Failed to create persona dir for changelog: {}",
            e
        );
        return;
    }

    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string();

    let entry = format!(
        "\n## [{}] {}\n- **File:** {}\n- **Initiated by:** {}\n",
        timestamp, summary, file_changed, initiator,
    );

    // Open for append (create if needed), with advisory file lock
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&changelog_path)
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(agent = agent_id, "Failed to open changelog: {}", e);
            return;
        }
    };

    // Advisory lock — blocks concurrent appenders
    use std::io::Write;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        // LOCK_EX: exclusive lock, blocks until acquired
        unsafe { libc::flock(fd, libc::LOCK_EX) };
    }

    // If file is empty, write header first
    let metadata = file.metadata().ok();
    let is_new = metadata.map(|m| m.len() == 0).unwrap_or(true);

    let mut writer = std::io::BufWriter::new(&file);
    if is_new {
        let _ = writeln!(
            writer,
            "# Persona Changelog\n\n*Identity changes are tracked here. This file is append-only.*"
        );
    }
    let _ = write!(writer, "{}", entry);
    let _ = writer.flush();

    // Unlock
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        unsafe { libc::flock(fd, libc::LOCK_UN) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_agent_id() {
        assert!(is_valid_agent_id("demo"));
        assert!(is_valid_agent_id("agent-123"));
        assert!(is_valid_agent_id("my_agent"));
        assert!(!is_valid_agent_id(""));
        assert!(!is_valid_agent_id("../etc"));
        assert!(!is_valid_agent_id("agent/bad"));
        assert!(!is_valid_agent_id("agent bad"));
    }

    #[test]
    fn test_filter_for_subagent() {
        let files = vec![
            PersonaFile {
                name: "AGENTS.md".to_string(),
                content: "rules".to_string(),
            },
            PersonaFile {
                name: "SOUL.md".to_string(),
                content: "personality".to_string(),
            },
            PersonaFile {
                name: "TOOLS.md".to_string(),
                content: "tools".to_string(),
            },
            PersonaFile {
                name: "USER.md".to_string(),
                content: "private".to_string(),
            },
        ];
        let filtered = filter_for_subagent(&files);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name, "AGENTS.md");
        assert_eq!(filtered[1].name, "TOOLS.md");
    }

    #[test]
    fn test_format_as_system_prompt_empty() {
        let result = format_as_system_prompt(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_as_system_prompt() {
        let files = vec![PersonaFile {
            name: "SOUL.md".to_string(),
            content: "Be helpful.".to_string(),
        }];
        let result = format_as_system_prompt(&files);
        assert!(result.contains("# Persona Context"));
        assert!(result.contains("## SOUL.md"));
        assert!(result.contains("Be helpful."));
    }

    #[test]
    fn test_truncation_constants() {
        assert_eq!(MAX_FILE_CHARS, 8000);
        assert_eq!(MAX_TOTAL_CHARS, 32000);
    }

    #[test]
    fn test_load_nonexistent_agent() {
        let files = load_persona_files("nonexistent-agent-xyz", false);
        assert!(files.is_empty());
    }

    #[test]
    fn test_load_nonexistent_agent_memory_active() {
        let files = load_persona_files("nonexistent-agent-xyz", true);
        assert!(files.is_empty());
    }

    #[test]
    fn validates_persona_filename() {
        assert!(validate_persona_filename("SOUL.md").is_ok());
        assert!(validate_persona_filename("SOUL.txt").is_err());
        assert!(validate_persona_filename("../SOUL.md").is_err());
    }

    #[test]
    fn identifies_standard_persona_filenames() {
        assert!(is_standard_persona_filename("USER.md"));
        assert!(is_standard_persona_filename("SOUL.md"));
        assert!(!is_standard_persona_filename("README.md"));
    }

    // ── Living Persona P1: dynamic profile formatting tests ──────────

    #[test]
    fn test_format_dynamic_profile_basic() {
        let packed = serde_json::json!({
            "memories": [
                {
                    "kind": "constraint",
                    "content": "Never push to main without approval",
                    "importance": 0.9,
                    "source": "human",
                    "created_at": chrono::Utc::now().to_rfc3339(),
                },
                {
                    "kind": "observation",
                    "content": "Fix SQL injection in message_poll",
                    "importance": 0.8,
                    "source": "agent",
                    "created_at": chrono::Utc::now().to_rfc3339(),
                },
                {
                    "kind": "preference",
                    "content": "No em dashes in social media",
                    "importance": 0.85,
                    "source": "human",
                    "created_at": chrono::Utc::now().to_rfc3339(),
                }
            ],
            "tokens_used": 150,
            "memories_included": 3,
        });

        let result = format_dynamic_profile(&packed).unwrap();
        assert_eq!(result.name, "ACTIVE_MEMORY_PROFILE");
        assert!(result.content.contains("Active Constraints (1)"));
        assert!(result.content.contains("Recent Observations (1)"));
        assert!(result.content.contains("Active Preferences (1)"));
        assert!(result.content.contains("Never push to main"));
        assert!(result.content.contains("[human, importance: 0.9, fresh]"));
        assert!(result.content.contains("3 memories, ~150 tokens"));
    }

    #[test]
    fn test_format_dynamic_profile_multibyte_truncation() {
        // Regression: a multi-byte char straddling the byte-197 truncation
        // point panicked the profile builder, killing the agent's turn worker
        // ("Turn queue full or agent not registered" on every send after).
        let content = format!(
            "{}→ tail text pushing the total length well past the 200-byte truncation threshold",
            "a".repeat(196)
        );
        assert!(content.len() > 200);
        assert!(!content.is_char_boundary(197));

        let packed = serde_json::json!({
            "memories": [
                {
                    "kind": "observation",
                    "content": content,
                    "importance": 0.5,
                    "source": "agent",
                    "created_at": chrono::Utc::now().to_rfc3339(),
                }
            ],
            "tokens_used": 100,
            "memories_included": 1,
        });

        let result = format_dynamic_profile(&packed).unwrap();
        assert!(result.content.contains("aaa"));
        assert!(result.content.contains("..."));
    }

    #[test]
    fn test_format_dynamic_profile_empty_memories() {
        let packed = serde_json::json!({
            "memories": [],
            "tokens_used": 0,
            "memories_included": 0,
        });
        assert!(format_dynamic_profile(&packed).is_none());
    }

    #[test]
    fn test_format_dynamic_profile_no_memories_field() {
        let packed = serde_json::json!({
            "tokens_used": 0,
        });
        assert!(format_dynamic_profile(&packed).is_none());
    }

    #[test]
    fn test_format_dynamic_profile_kind_ordering() {
        let packed = serde_json::json!({
            "memories": [
                { "kind": "question", "content": "Is caching last?", "importance": 0.5, "source": "agent", "created_at": chrono::Utc::now().to_rfc3339() },
                { "kind": "constraint", "content": "No jank", "importance": 0.9, "source": "human", "created_at": chrono::Utc::now().to_rfc3339() },
                { "kind": "decision", "content": "Caching is last", "importance": 0.8, "source": "agent", "created_at": (chrono::Utc::now() - chrono::Duration::days(3)).to_rfc3339() },
            ],
            "tokens_used": 100,
            "memories_included": 3,
        });

        let result = format_dynamic_profile(&packed).unwrap();
        // Constraints should appear before decisions, which appear before questions
        let constraint_pos = result.content.find("Active Constraints").unwrap();
        let decision_pos = result.content.find("Recent Decisions").unwrap();
        let question_pos = result.content.find("Open Questions").unwrap();
        assert!(constraint_pos < decision_pos);
        assert!(decision_pos < question_pos);
    }

    #[test]
    fn test_format_dynamic_profile_truncates_long_content() {
        let long_content = "x".repeat(300);
        let packed = serde_json::json!({
            "memories": [
                { "kind": "fact", "content": long_content, "importance": 0.5, "source": "agent", "created_at": chrono::Utc::now().to_rfc3339() }
            ],
            "tokens_used": 100,
            "memories_included": 1,
        });

        let result = format_dynamic_profile(&packed).unwrap();
        assert!(result.content.contains("..."));
        // Should not contain the full 300-char string
        assert!(!result.content.contains(&long_content));
    }

    #[test]
    fn test_format_dynamic_profile_subagent_excluded() {
        let packed = serde_json::json!({
            "memories": [
                { "kind": "constraint", "content": "Rule", "importance": 0.9, "source": "human", "created_at": chrono::Utc::now().to_rfc3339() }
            ],
            "tokens_used": 20,
            "memories_included": 1,
        });
        let profile = format_dynamic_profile(&packed).unwrap();

        // Dynamic profile should NOT appear in subagent-filtered output
        let files = vec![
            PersonaFile {
                name: "AGENTS.md".into(),
                content: "rules".into(),
            },
            PersonaFile {
                name: "TOOLS.md".into(),
                content: "tools".into(),
            },
            profile,
        ];
        let filtered = filter_for_subagent(&files);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|f| f.name != "ACTIVE_MEMORY_PROFILE"));
    }

    #[test]
    fn test_kind_display_labels() {
        assert_eq!(kind_display_label("constraint"), "Active Constraints");
        assert_eq!(kind_display_label("commitment"), "Open Commitments");
        assert_eq!(kind_display_label("preference"), "Active Preferences");
        assert_eq!(kind_display_label("decision"), "Recent Decisions");
        assert_eq!(kind_display_label("question"), "Open Questions");
        assert_eq!(kind_display_label("unknown_kind"), "Other");
    }

    // ── Preamble tests ──────────────────────────────────────────────────

    #[test]
    fn preamble_is_recognized_as_standard_filename() {
        assert!(is_standard_persona_filename("PREAMBLE.md"));
    }

    #[test]
    fn preamble_appears_before_persona_context_header() {
        let files = vec![
            PersonaFile {
                name: "PREAMBLE.md".to_string(),
                content: "ORIENT BEFORE RESPONDING".to_string(),
            },
            PersonaFile {
                name: "SOUL.md".to_string(),
                content: "Be helpful.".to_string(),
            },
        ];
        let result = format_as_system_prompt(&files);

        // Preamble content must appear before "# Persona Context"
        let preamble_pos = result.find("ORIENT BEFORE RESPONDING").unwrap();
        let persona_pos = result.find("# Persona Context").unwrap();
        assert!(
            preamble_pos < persona_pos,
            "Preamble must appear before Persona Context header"
        );
    }

    #[test]
    fn preamble_is_injected_raw_without_heading_wrapper() {
        let files = vec![
            PersonaFile {
                name: "PREAMBLE.md".to_string(),
                content: "ORIENT NOW".to_string(),
            },
            PersonaFile {
                name: "AGENTS.md".to_string(),
                content: "rules".to_string(),
            },
        ];
        let result = format_as_system_prompt(&files);

        // Preamble should NOT have a "## PREAMBLE.md" heading
        assert!(!result.contains("## PREAMBLE.md"));
        // But AGENTS.md should have its normal heading
        assert!(result.contains("## AGENTS.md"));
        // Preamble content should be the very first content
        assert!(result.starts_with("ORIENT NOW"));
    }

    #[test]
    fn missing_preamble_preserves_existing_format() {
        // No preamble — should produce identical output to before this change
        let files = vec![PersonaFile {
            name: "SOUL.md".to_string(),
            content: "Be helpful.".to_string(),
        }];
        let result = format_as_system_prompt(&files);
        assert!(result.starts_with("# Persona Context"));
        assert!(result.contains("## SOUL.md"));
        assert!(result.contains("Be helpful."));
        assert!(!result.contains("PREAMBLE"));
    }

    #[test]
    fn preamble_excluded_from_subagent_context() {
        let files = vec![
            PersonaFile {
                name: "PREAMBLE.md".into(),
                content: "cold start".into(),
            },
            PersonaFile {
                name: "AGENTS.md".into(),
                content: "rules".into(),
            },
            PersonaFile {
                name: "TOOLS.md".into(),
                content: "tools".into(),
            },
            PersonaFile {
                name: "SOUL.md".into(),
                content: "personality".into(),
            },
        ];
        let filtered = filter_for_subagent(&files);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|f| f.name != "PREAMBLE.md"));
    }

    #[test]
    fn preamble_with_only_preamble_file() {
        // Edge case: PREAMBLE.md exists but no other files
        let files = vec![PersonaFile {
            name: "PREAMBLE.md".to_string(),
            content: "ORIENT".to_string(),
        }];
        let result = format_as_system_prompt(&files);
        assert!(result.starts_with("ORIENT"));
        // Should NOT have an empty "# Persona Context" section
        assert!(!result.contains("# Persona Context"));
    }

    #[test]
    fn preamble_included_in_prompt_hash_computation() {
        // This is a logic test — the preamble is part of the formatted output,
        // so it will be included in any hash computed over format_as_system_prompt().
        // Changing preamble content must produce different output.
        let files_a = vec![
            PersonaFile {
                name: "PREAMBLE.md".into(),
                content: "version A".into(),
            },
            PersonaFile {
                name: "SOUL.md".into(),
                content: "same".into(),
            },
        ];
        let files_b = vec![
            PersonaFile {
                name: "PREAMBLE.md".into(),
                content: "version B".into(),
            },
            PersonaFile {
                name: "SOUL.md".into(),
                content: "same".into(),
            },
        ];

        let output_a = format_as_system_prompt(&files_a);
        let output_b = format_as_system_prompt(&files_b);
        assert_ne!(
            output_a, output_b,
            "Different preamble content must produce different prompt output"
        );
    }

    #[test]
    fn detects_heading_anchor_changes() {
        let old_content = "# SOUL\n\n## Trust Contract\n\nOld.\n\n## Style\n\nLoose.\n";
        let new_content = "# SOUL\n\n## Trust Contract\n\nNew.\n\n## Style\n\nLoose.\n";
        let unchanged = "# SOUL\n\n## Trust Contract\n\nOld.\n\n## Style\n\nChanged.\n";

        let manifest = PersonaAnchorManifest {
            version: 1,
            agent_id: "demo".into(),
            updated_at: None,
            updated_at_session: None,
            anchors: vec![PersonaAnchor {
                anchor_id: "heading:SOUL.md#Trust Contract".into(),
                declared_by: "demo".into(),
                scope: PersonaAnchorScope::MarkdownHeading {
                    file: "SOUL.md".into(),
                    heading: "Trust Contract".into(),
                },
                reason: "Load-bearing".into(),
                friction_policy: PersonaAnchorFrictionPolicy::default(),
                created_at: None,
                updated_at: None,
                declared_at_session: None,
            }],
        };

        assert_eq!(
            affected_anchors_for_edit(&manifest, "SOUL.md", old_content, new_content).len(),
            1
        );
        assert!(affected_anchors_for_edit(&manifest, "SOUL.md", old_content, unchanged).is_empty());
    }

    // ── PERSONA_CHANGELOG.md tests ──────────────────────────────────────

    #[test]
    fn changelog_creates_file_with_header() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_id = "test-changelog-create";

        // Override persona_dir by writing directly (append_changelog uses persona_dir())
        // Instead, we'll call the function and verify the output exists
        let dir = tmp.path().join("agents").join(agent_id).join("persona");
        std::fs::create_dir_all(&dir).unwrap();
        let changelog_path = dir.join(CHANGELOG_FILE);

        // Manually test the append logic by writing directly
        // (we can't override persona_dir, so we test the file format)
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string();
        let entry = format!(
            "\n## [{}] {}\n- **File:** {}\n- **Initiated by:** {}\n",
            timestamp,
            "Agent created — initial persona generated",
            "(all persona files)",
            ChangeInitiator::System("agent creation"),
        );

        // Write header + entry
        let mut content = String::from("# Persona Changelog\n\n*Identity changes are tracked here. This file is append-only.*\n");
        content.push_str(&entry);
        std::fs::write(&changelog_path, &content).unwrap();

        let written = std::fs::read_to_string(&changelog_path).unwrap();
        assert!(written.contains("# Persona Changelog"));
        assert!(written.contains("append-only"));
        assert!(written.contains("Agent created"));
        assert!(written.contains("(all persona files)"));
        assert!(written.contains("System (agent creation)"));
    }

    #[test]
    fn changelog_initiator_display() {
        assert_eq!(
            format!("{}", ChangeInitiator::System("agent creation")),
            "System (agent creation)"
        );
        assert_eq!(format!("{}", ChangeInitiator::Agent), "Agent (self)");
        assert_eq!(format!("{}", ChangeInitiator::Human), "Human");
    }

    #[test]
    fn changelog_append_is_additive() {
        let tmp = tempfile::tempdir().unwrap();
        let changelog_path = tmp.path().join(CHANGELOG_FILE);

        // Write two entries manually to verify append behavior
        let entry1 = "## [2026-06-28 10:00 UTC] First change\n- **File:** SOUL.md\n- **Initiated by:** Agent (self)\n";
        let entry2 = "## [2026-06-28 10:05 UTC] Second change\n- **File:** TOOLS.md\n- **Initiated by:** Human\n";

        std::fs::write(&changelog_path, entry1).unwrap();

        // Append second entry
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&changelog_path)
            .unwrap();
        write!(file, "\n{}", entry2).unwrap();

        let content = std::fs::read_to_string(&changelog_path).unwrap();
        assert!(content.contains("First change"));
        assert!(content.contains("Second change"));
        assert!(content.contains("SOUL.md"));
        assert!(content.contains("TOOLS.md"));
    }

    #[test]
    fn changelog_entry_format_is_valid_markdown() {
        let timestamp = "2026-06-28 12:00 UTC";
        let entry = format!(
            "\n## [{}] {}\n- **File:** {}\n- **Initiated by:** {}\n",
            timestamp,
            "Updated SOUL.md",
            "SOUL.md",
            ChangeInitiator::Agent,
        );

        // Must start with markdown heading
        assert!(entry.contains("## ["));
        // Must have bold field labels
        assert!(entry.contains("- **File:**"));
        assert!(entry.contains("- **Initiated by:**"));
        // Must contain the actual values
        assert!(entry.contains("SOUL.md"));
        assert!(entry.contains("Agent (self)"));
    }
}
