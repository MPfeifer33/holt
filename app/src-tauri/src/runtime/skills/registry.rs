use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use super::parser::{parse_skill_directory, parse_skill_file};
use super::types::SkillDefinition;

/// Registry that loads and caches skill definitions from a directory of markdown files.
pub struct SkillRegistry {
    skills: RwLock<HashMap<String, SkillDefinition>>,
    skills_dir: PathBuf,
}

impl SkillRegistry {
    pub fn new(skills_dir: PathBuf) -> Self {
        let registry = Self {
            skills: RwLock::new(HashMap::new()),
            skills_dir,
        };
        registry.reload();
        registry
    }

    /// Re-scan the skills directory and replace the cached skill map.
    pub fn reload(&self) {
        let mut map = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&self.skills_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
                    // Single-file skill
                    match parse_skill_file(&path) {
                        Ok(skill) => {
                            tracing::info!(skill = %skill.file_name, "Loaded skill: {}", skill.frontmatter.name);
                            map.insert(skill.file_name.clone(), skill);
                        }
                        Err(e) => tracing::warn!("Failed to parse skill file: {}", e),
                    }
                } else if path.is_dir() {
                    // Potential multi-file skill directory
                    if path.join("SKILL.md").exists() {
                        match parse_skill_directory(&path) {
                            Ok(skill) => {
                                tracing::info!(
                                    skill = %skill.file_name,
                                    files = skill.files.len(),
                                    "Loaded multi-file skill: {}",
                                    skill.frontmatter.name
                                );
                                map.insert(skill.file_name.clone(), skill);
                            }
                            Err(e) => tracing::warn!("Failed to parse skill directory: {}", e),
                        }
                    } else {
                        tracing::warn!("Skipping directory without SKILL.md: {}", path.display());
                    }
                }
            }
        }
        *self.skills.write().unwrap() = map;
    }

    /// Look up a single skill by file-stem name.
    pub fn get(&self, name: &str) -> Option<SkillDefinition> {
        self.skills.read().unwrap().get(name).cloned()
    }

    /// Return all loaded skills.
    pub fn list(&self) -> Vec<SkillDefinition> {
        self.skills.read().unwrap().values().cloned().collect()
    }

    /// Path to the directory this registry scans.
    pub fn skills_dir(&self) -> &Path {
        &self.skills_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_loads_skills() {
        let dir = tempfile::tempdir().unwrap();

        // Write two valid skill files
        std::fs::write(
            dir.path().join("alpha.md"),
            "---\nname: alpha\ndescription: Alpha skill\n---\nAlpha body.",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("beta.md"),
            "---\nname: beta\ndescription: Beta skill\n---\nBeta body.",
        )
        .unwrap();

        // Write a non-md file that should be ignored
        std::fs::write(dir.path().join("readme.txt"), "ignore me").unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let skills = registry.list();
        assert_eq!(skills.len(), 2);

        let alpha = registry.get("alpha").expect("alpha skill should exist");
        assert_eq!(alpha.frontmatter.name, "alpha");
        assert_eq!(alpha.body, "Alpha body.");

        let beta = registry.get("beta").expect("beta skill should exist");
        assert_eq!(beta.frontmatter.description, "Beta skill");

        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_reload() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("first.md"),
            "---\nname: first\ndescription: First\n---\nBody.",
        )
        .unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        assert_eq!(registry.list().len(), 1);

        // Add another skill file and reload
        std::fs::write(
            dir.path().join("second.md"),
            "---\nname: second\ndescription: Second\n---\nBody 2.",
        )
        .unwrap();

        registry.reload();
        assert_eq!(registry.list().len(), 2);
        assert!(registry.get("second").is_some());
    }

    #[test]
    fn test_registry_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SkillRegistry::new(dir.path().to_path_buf());
        assert!(registry.list().is_empty());
    }

    #[test]
    fn test_registry_nonexistent_dir() {
        let registry = SkillRegistry::new(PathBuf::from("/tmp/nonexistent-skills-dir-xyz"));
        assert!(registry.list().is_empty());
    }

    #[test]
    fn test_registry_skips_bad_files() {
        let dir = tempfile::tempdir().unwrap();

        // Valid skill
        std::fs::write(
            dir.path().join("good.md"),
            "---\nname: good\ndescription: Good\n---\nBody.",
        )
        .unwrap();

        // Invalid skill (no frontmatter)
        std::fs::write(dir.path().join("bad.md"), "No frontmatter here.").unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        assert_eq!(registry.list().len(), 1);
        assert!(registry.get("good").is_some());
        assert!(registry.get("bad").is_none());
    }

    #[test]
    fn test_registry_loads_mixed_formats() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("flat.md"),
            "---\nname: flat\ndescription: Flat\n---\nFlat body.",
        )
        .unwrap();

        let skill_dir = dir.path().join("multi");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: multi\ndescription: Multi\n---\nMulti body.",
        )
        .unwrap();
        std::fs::write(skill_dir.join("support.md"), "Support content.").unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        assert_eq!(registry.list().len(), 2);

        let flat = registry.get("flat").unwrap();
        assert!(!flat.is_directory);
        assert!(flat.files.is_empty());

        let multi = registry.get("multi").unwrap();
        assert!(multi.is_directory);
        assert_eq!(multi.files, vec!["support.md".to_string()]);
        assert!(multi.body.contains("Multi body."));
        assert!(multi.body.contains("Support content."));
    }

    #[test]
    fn test_registry_skips_directory_without_skill_md() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("good.md"),
            "---\nname: good\ndescription: Good\n---\nBody.",
        )
        .unwrap();

        let bad_dir = dir.path().join("bad-dir");
        std::fs::create_dir(&bad_dir).unwrap();
        std::fs::write(bad_dir.join("random.md"), "No SKILL.md here.").unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        assert_eq!(registry.list().len(), 1);
        assert!(registry.get("good").is_some());
        assert!(registry.get("bad-dir").is_none());
    }

    #[test]
    fn test_registry_reload_after_adding_directory_skill() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("flat.md"),
            "---\nname: flat\ndescription: Flat\n---\nBody.",
        )
        .unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        assert_eq!(registry.list().len(), 1);

        let skill_dir = dir.path().join("multi");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: multi\ndescription: Multi\n---\nMulti body.",
        )
        .unwrap();
        std::fs::write(skill_dir.join("extra.md"), "Extra.").unwrap();

        registry.reload();
        assert_eq!(registry.list().len(), 2);

        let multi = registry.get("multi").unwrap();
        assert!(multi.is_directory);
        assert_eq!(multi.files, vec!["extra.md".to_string()]);
    }

    #[test]
    fn test_auto_conversion_single_to_directory() {
        let dir = tempfile::tempdir().unwrap();

        let original_content =
            "---\nname: convert-me\ndescription: Will become directory\n---\nOriginal body.";
        std::fs::write(dir.path().join("convert-me.md"), original_content).unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let skill = registry.get("convert-me").unwrap();
        assert!(!skill.is_directory);
        assert_eq!(skill.body, "Original body.");

        // Simulate auto-conversion steps (what write_skill_file will do)
        let skill_dir = dir.path().join("convert-me");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), original_content).unwrap();
        std::fs::write(skill_dir.join("extra.md"), "Extra content.").unwrap();
        std::fs::remove_file(dir.path().join("convert-me.md")).unwrap();

        registry.reload();

        let converted = registry.get("convert-me").unwrap();
        assert!(converted.is_directory);
        assert_eq!(converted.files, vec!["extra.md".to_string()]);
        assert!(converted.body.contains("Original body."));
        assert!(converted.body.contains("Extra content."));
        assert!(converted.body.contains("## extra.md"));
        assert_eq!(converted.frontmatter.name, "convert-me");
        assert_eq!(converted.frontmatter.description, "Will become directory");
    }

    #[test]
    fn test_directory_skill_file_count_for_delete_safety() {
        let dir = tempfile::tempdir().unwrap();

        let skill_dir = dir.path().join("big-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: big-skill\ndescription: Has files\n---\nMain.",
        )
        .unwrap();
        std::fs::write(skill_dir.join("a.md"), "A.").unwrap();
        std::fs::write(skill_dir.join("b.md"), "B.").unwrap();

        let file_count: usize = std::fs::read_dir(&skill_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .count();
        assert_eq!(file_count, 3); // SKILL.md + a.md + b.md

        std::fs::write(
            dir.path().join("simple.md"),
            "---\nname: simple\ndescription: Simple\n---\nBody.",
        )
        .unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let simple = registry.get("simple").unwrap();
        assert!(!simple.is_directory);

        let big = registry.get("big-skill").unwrap();
        assert!(big.is_directory);
        assert_eq!(big.files.len(), 2);
    }

    #[test]
    fn test_create_skill_directory_template_loads() {
        let dir = tempfile::tempdir().unwrap();

        let skill_dir = dir.path().join("new-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        let template = "---\nname: new-skill\ndescription: New skill\ntriggers: []\npriority: normal\nmax_tokens: 1000\n---\n";
        std::fs::write(skill_dir.join("SKILL.md"), template).unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let skill = registry.get("new-skill").unwrap();
        assert!(skill.is_directory);
        assert!(skill.files.is_empty());
        assert_eq!(skill.frontmatter.name, "new-skill");
        assert_eq!(skill.frontmatter.description, "New skill");
        assert_eq!(skill.frontmatter.max_tokens, 1000);
    }

    #[test]
    fn test_delete_supporting_file_and_reload() {
        let dir = tempfile::tempdir().unwrap();

        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: Test\n---\nMain.",
        )
        .unwrap();
        std::fs::write(skill_dir.join("a.md"), "A content.").unwrap();
        std::fs::write(skill_dir.join("b.md"), "B content.").unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let skill = registry.get("my-skill").unwrap();
        assert_eq!(skill.files.len(), 2);
        assert!(skill.body.contains("A content."));
        assert!(skill.body.contains("B content."));

        std::fs::remove_file(skill_dir.join("a.md")).unwrap();
        registry.reload();

        let updated = registry.get("my-skill").unwrap();
        assert_eq!(updated.files.len(), 1);
        assert_eq!(updated.files[0], "b.md");
        assert!(!updated.body.contains("A content."));
        assert!(updated.body.contains("B content."));
    }

    #[test]
    fn test_read_supporting_file_from_directory_skill() {
        let dir = tempfile::tempdir().unwrap();

        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: Test\n---\nMain.",
        )
        .unwrap();
        std::fs::write(skill_dir.join("helper.md"), "Helper file content.").unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let skill = registry.get("my-skill").unwrap();
        assert!(skill.is_directory);

        let file_path = skill.file_path.join("helper.md");
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Helper file content.");

        let bad_path = skill.file_path.join("nonexistent.md");
        assert!(!bad_path.exists());
    }
}
