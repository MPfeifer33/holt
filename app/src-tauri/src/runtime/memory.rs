use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::app_config::agent_dir;

const MAX_COMPACTION_SUMMARIES: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionMemory {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub summary: String,
    pub model_based: bool,
    pub messages_compacted: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub key_facts: Vec<String>,
    pub files_modified: Vec<String>,
    pub working_directory: String,
}

fn memory_dir(agent_id: &str) -> PathBuf {
    agent_dir(agent_id).join("memory")
}

/// Persist a compaction summary to the agent's memory directory.
pub fn save_compaction_memory(
    agent_id: &str,
    memory: &CompactionMemory,
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = memory_dir(agent_id);
    std::fs::create_dir_all(&dir)?;

    let path = dir.join(format!("{}.json", memory.id));
    let json = serde_json::to_string_pretty(memory)?;
    std::fs::write(path, json)?;

    // Auto-prune: keep only the most recent N summaries
    prune_compaction_memories(agent_id, MAX_COMPACTION_SUMMARIES)?;

    Ok(())
}

/// Load the most recent compaction summary for an agent.
pub fn load_latest_memory(agent_id: &str) -> Option<CompactionMemory> {
    let dir = memory_dir(agent_id);
    if !dir.exists() {
        return None;
    }

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("compaction-"))
        })
        .collect();

    entries.sort_by_key(|e| e.file_name());
    entries.last().and_then(|e| {
        let content = std::fs::read_to_string(e.path()).ok()?;
        serde_json::from_str(&content).ok()
    })
}

/// Build a context injection string from the most recent memory.
pub fn inject_memory_context(agent_id: &str) -> Option<String> {
    let memory = load_latest_memory(agent_id)?;

    let facts = if memory.key_facts.is_empty() {
        String::new()
    } else {
        format!(
            "\nKey facts:\n{}",
            memory
                .key_facts
                .iter()
                .map(|f| format!("- {}", f))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let files = if memory.files_modified.is_empty() {
        String::new()
    } else {
        format!("\nFiles worked on: {}", memory.files_modified.join(", "))
    };

    Some(format!(
        "[Previous session context — auto-recovered from memory]\nSummary: {}{}{}\nWorking directory: {}",
        memory.summary, facts, files, memory.working_directory
    ))
}

/// Extract file paths from tool call content (simple heuristic).
pub fn extract_files_from_tools(tool_results: &[String]) -> Vec<String> {
    let mut files = Vec::new();
    for result in tool_results {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(result) {
            if let Some(path) = val.get("path").and_then(|p| p.as_str()) {
                files.push(path.to_string());
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn prune_compaction_memories(
    agent_id: &str,
    keep: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = memory_dir(agent_id);
    if !dir.exists() {
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("compaction-"))
        })
        .collect();

    entries.sort_by_key(|e| e.file_name());

    if entries.len() > keep {
        for entry in entries.iter().take(entries.len() - keep) {
            let _ = std::fs::remove_file(entry.path());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compaction_memory_serialization() {
        let memory = CompactionMemory {
            id: "compaction-1234".to_string(),
            timestamp: Utc::now(),
            summary: "User asked about config loading".to_string(),
            model_based: true,
            messages_compacted: 15,
            tokens_before: 50000,
            tokens_after: 10000,
            key_facts: vec!["Config uses TOML format".to_string()],
            files_modified: vec!["src/config.rs".to_string()],
            working_directory: "/home/user/project".to_string(),
        };

        let json = serde_json::to_string(&memory).unwrap();
        let deserialized: CompactionMemory = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "compaction-1234");
        assert_eq!(deserialized.messages_compacted, 15);
        assert!(deserialized.model_based);
    }

    #[test]
    fn test_inject_memory_context_format() {
        let memory = CompactionMemory {
            id: "compaction-1".to_string(),
            timestamp: Utc::now(),
            summary: "Discussed file structure".to_string(),
            model_based: false,
            messages_compacted: 10,
            tokens_before: 30000,
            tokens_after: 5000,
            key_facts: vec!["Uses Rust backend".to_string()],
            files_modified: vec!["lib.rs".to_string(), "main.rs".to_string()],
            working_directory: "/tmp/test".to_string(),
        };

        // Test the formatting logic directly
        let facts = format!(
            "\nKey facts:\n{}",
            memory
                .key_facts
                .iter()
                .map(|f| format!("- {}", f))
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(facts.contains("Uses Rust backend"));

        let files = format!("\nFiles worked on: {}", memory.files_modified.join(", "));
        assert!(files.contains("lib.rs, main.rs"));
    }

    #[test]
    fn test_extract_files_from_tools() {
        let results = vec![
            r#"{"path": "/home/user/src/main.rs", "content": "..."}"#.to_string(),
            r#"{"status": "ok"}"#.to_string(),
            r#"{"path": "/home/user/src/lib.rs"}"#.to_string(),
            r#"{"path": "/home/user/src/main.rs"}"#.to_string(), // duplicate
        ];

        let files = extract_files_from_tools(&results);
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"/home/user/src/lib.rs".to_string()));
        assert!(files.contains(&"/home/user/src/main.rs".to_string()));
    }
}
