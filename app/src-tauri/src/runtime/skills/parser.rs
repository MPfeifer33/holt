use std::path::Path;

use super::types::{SkillDefinition, SkillFrontmatter};

const MAX_SUPPORTING_FILES: usize = 20;

/// Strip YAML frontmatter from markdown content.
/// If the content starts with `---`, removes everything up to and including the closing `---`.
fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content.trim();
    }
    let after_open = &trimmed[3..];
    match after_open.find("\n---") {
        Some(pos) => after_open[pos + 4..].trim(),
        None => content.trim(),
    }
}

/// Parse a skill markdown file with YAML frontmatter.
///
/// Expected format:
/// ```text
/// ---
/// name: my-skill
/// description: Does something useful
/// ---
/// Body content here...
/// ```
pub fn parse_skill_file(path: &Path) -> Result<SkillDefinition, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read skill file {}: {}", path.display(), e))?;

    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(format!(
            "Skill file {} has no YAML frontmatter (missing opening ---)",
            path.display()
        ));
    }

    // Skip the opening "---" and find the closing "---"
    let after_open = &trimmed[3..];
    let close_pos = after_open.find("\n---").ok_or_else(|| {
        format!(
            "Skill file {} has no closing --- for frontmatter",
            path.display()
        )
    })?;

    let yaml_str = &after_open[..close_pos];
    let body_start = close_pos + 4; // skip "\n---"
    let body = after_open[body_start..].trim().to_string();

    let frontmatter: SkillFrontmatter = serde_yaml::from_str(yaml_str)
        .map_err(|e| format!("Failed to parse frontmatter in {}: {}", path.display(), e))?;

    let file_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(SkillDefinition {
        file_name,
        frontmatter,
        body,
        file_path: path.to_path_buf(),
        is_directory: false,
        files: vec![],
    })
}

/// Parse a directory-based multi-file skill.
///
/// Expects `dir_path` to contain a `SKILL.md` entry point.
/// Collects all other `.md` files (flat only, no subdirectories) and concatenates them
/// into the body after the SKILL.md content.
pub fn parse_skill_directory(dir_path: &Path) -> Result<SkillDefinition, String> {
    let skill_md_path = dir_path.join("SKILL.md");
    if !skill_md_path.exists() {
        return Err(format!(
            "Directory {} has no SKILL.md entry point",
            dir_path.display()
        ));
    }

    // Parse SKILL.md as the primary skill definition
    let mut skill = parse_skill_file(&skill_md_path)?;

    // Override file_name to be the directory name (not "SKILL")
    skill.file_name = dir_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    skill.file_path = dir_path.to_path_buf();
    skill.is_directory = true;

    // Collect supporting .md files (flat only — skip subdirectories)
    let mut supporting: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if fname == "SKILL.md" {
                continue;
            }
            supporting.push(fname.to_string());
        }
    }

    // Enforce file cap
    supporting.sort();
    if supporting.len() > MAX_SUPPORTING_FILES {
        tracing::warn!(
            skill = %skill.file_name,
            count = supporting.len(),
            "Skill has more than {} supporting files, loading first {} alphabetically",
            MAX_SUPPORTING_FILES,
            MAX_SUPPORTING_FILES
        );
        supporting.truncate(MAX_SUPPORTING_FILES);
    }

    // Determine concatenation order based on `includes` frontmatter
    let ordered = match &skill.frontmatter.includes {
        Some(includes) => {
            let mut ordered: Vec<String> = Vec::new();
            for inc in includes {
                if supporting.contains(inc) {
                    ordered.push(inc.clone());
                }
            }
            for f in &supporting {
                if !ordered.contains(f) {
                    ordered.push(f.clone());
                }
            }
            ordered
        }
        None => supporting.clone(),
    };

    // Concatenate supporting files into body.
    // Only files that load successfully are included in skill.files,
    // keeping the files list in sync with body content.
    let mut body = skill.body.clone();
    let mut loaded_files: Vec<String> = Vec::new();
    for fname in &ordered {
        let fpath = dir_path.join(fname);
        match std::fs::read_to_string(&fpath) {
            Ok(content) => {
                let stripped = strip_frontmatter(&content);
                body.push_str(&format!("\n\n## {}\n\n{}", fname, stripped));
                loaded_files.push(fname.clone());
            }
            Err(e) => {
                tracing::warn!(
                    skill = %skill.file_name,
                    file = %fname,
                    "Failed to read supporting file: {}",
                    e
                );
            }
        }
    }

    skill.body = body;
    skill.files = loaded_files;

    Ok(skill)
}

#[cfg(test)]
mod tests {
    use super::super::types::SkillPriority;
    use super::*;
    use std::io::Write as _;

    #[test]
    fn test_parse_skill_file() {
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("code-review.md");
        let mut f = std::fs::File::create(&skill_path).unwrap();
        write!(
            f,
            r#"---
name: code-review
description: Review code for bugs and style issues
triggers:
  - tools: ["read_file", "list_files"]
    keyword: review
  - pattern: "review.*code"
priority: high
max_tokens: 2000
---
You are a code reviewer. Examine the code carefully and provide feedback."#
        )
        .unwrap();

        let skill = parse_skill_file(&skill_path).unwrap();
        assert_eq!(skill.file_name, "code-review");
        assert_eq!(skill.frontmatter.name, "code-review");
        assert_eq!(
            skill.frontmatter.description,
            "Review code for bugs and style issues"
        );
        assert_eq!(skill.frontmatter.triggers.len(), 2);
        assert_eq!(
            skill.frontmatter.triggers[0].tools,
            vec!["read_file", "list_files"]
        );
        assert_eq!(
            skill.frontmatter.triggers[0].keyword,
            Some("review".to_string())
        );
        assert_eq!(
            skill.frontmatter.triggers[1].pattern,
            Some("review.*code".to_string())
        );
        assert_eq!(skill.frontmatter.priority, SkillPriority::High);
        assert_eq!(skill.frontmatter.max_tokens, 2000);
        assert!(skill.body.contains("You are a code reviewer"));
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("bad-skill.md");
        std::fs::write(&skill_path, "Just some plain markdown content.").unwrap();

        let result = parse_skill_file(&skill_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no YAML frontmatter"));
    }

    #[test]
    fn test_parse_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("minimal.md");
        std::fs::write(
            &skill_path,
            r#"---
name: minimal-skill
description: A minimal skill
---
Just a body."#,
        )
        .unwrap();

        let skill = parse_skill_file(&skill_path).unwrap();
        assert_eq!(skill.file_name, "minimal");
        assert_eq!(skill.frontmatter.name, "minimal-skill");
        assert!(skill.frontmatter.triggers.is_empty());
        assert_eq!(skill.frontmatter.priority, SkillPriority::Normal);
        assert_eq!(skill.frontmatter.max_tokens, 1000);
        assert_eq!(skill.body, "Just a body.");
    }

    #[test]
    fn test_parse_frontmatter_with_includes() {
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("ordered.md");
        std::fs::write(
            &skill_path,
            r#"---
name: ordered-skill
description: Has includes
includes:
  - b.md
  - a.md
---
Body."#,
        )
        .unwrap();

        let skill = parse_skill_file(&skill_path).unwrap();
        assert_eq!(
            skill.frontmatter.includes,
            Some(vec!["b.md".to_string(), "a.md".to_string()])
        );
    }

    #[test]
    fn test_parse_frontmatter_without_includes() {
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("no-includes.md");
        std::fs::write(
            &skill_path,
            "---\nname: basic\ndescription: No includes\n---\nBody.",
        )
        .unwrap();

        let skill = parse_skill_file(&skill_path).unwrap();
        assert_eq!(skill.frontmatter.includes, None);
    }

    #[test]
    fn test_single_file_skill_has_directory_false() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("flat.md"),
            "---\nname: flat\ndescription: Flat skill\n---\nBody.",
        )
        .unwrap();

        let skill = parse_skill_file(dir.path().join("flat.md").as_path()).unwrap();
        assert!(!skill.is_directory);
        assert!(skill.files.is_empty());
    }

    #[test]
    fn test_strip_frontmatter_removes_yaml() {
        let content = "---\nname: foo\ndescription: bar\n---\nActual content here.";
        assert_eq!(strip_frontmatter(content), "Actual content here.");
    }

    #[test]
    fn test_strip_frontmatter_no_frontmatter() {
        let content = "Just plain markdown.";
        assert_eq!(strip_frontmatter(content), "Just plain markdown.");
    }

    #[test]
    fn test_strip_frontmatter_empty() {
        assert_eq!(strip_frontmatter(""), "");
    }

    #[test]
    fn test_parse_skill_directory_basic() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir(&skill_dir).unwrap();

        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: A multi-file skill\n---\nMain body.",
        )
        .unwrap();
        std::fs::write(skill_dir.join("helpers.md"), "Helper content.").unwrap();
        std::fs::write(skill_dir.join("examples.md"), "Example content.").unwrap();

        let skill = parse_skill_directory(&skill_dir).unwrap();
        assert_eq!(skill.file_name, "my-skill");
        assert_eq!(skill.frontmatter.name, "my-skill");
        assert!(skill.is_directory);
        assert_eq!(skill.files.len(), 2);
        assert!(skill.files.contains(&"examples.md".to_string()));
        assert!(skill.files.contains(&"helpers.md".to_string()));
        assert!(skill.body.starts_with("Main body."));
        assert!(skill.body.contains("## examples.md"));
        assert!(skill.body.contains("Example content."));
        assert!(skill.body.contains("## helpers.md"));
        assert!(skill.body.contains("Helper content."));
        let ex_pos = skill.body.find("## examples.md").unwrap();
        let he_pos = skill.body.find("## helpers.md").unwrap();
        assert!(ex_pos < he_pos);
    }

    #[test]
    fn test_parse_skill_directory_with_includes_ordering() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("ordered");
        std::fs::create_dir(&skill_dir).unwrap();

        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: ordered\ndescription: Has includes\nincludes:\n  - helpers.md\n  - examples.md\n---\nMain.",
        )
        .unwrap();
        std::fs::write(skill_dir.join("helpers.md"), "Helpers.").unwrap();
        std::fs::write(skill_dir.join("examples.md"), "Examples.").unwrap();
        std::fs::write(skill_dir.join("extras.md"), "Extras.").unwrap();

        let skill = parse_skill_directory(&skill_dir).unwrap();
        assert_eq!(skill.files.len(), 3);
        let h_pos = skill.body.find("## helpers.md").unwrap();
        let e_pos = skill.body.find("## examples.md").unwrap();
        let x_pos = skill.body.find("## extras.md").unwrap();
        assert!(h_pos < e_pos);
        assert!(e_pos < x_pos);
    }

    #[test]
    fn test_parse_skill_directory_missing_skill_md() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("bad-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("helpers.md"), "No SKILL.md here.").unwrap();

        let result = parse_skill_directory(&skill_dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("SKILL.md"));
    }

    #[test]
    fn test_parse_skill_directory_strips_supporting_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("stripped");
        std::fs::create_dir(&skill_dir).unwrap();

        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: stripped\ndescription: Test stripping\n---\nMain.",
        )
        .unwrap();
        std::fs::write(
            skill_dir.join("support.md"),
            "---\ntitle: Internal\n---\nClean content.",
        )
        .unwrap();

        let skill = parse_skill_directory(&skill_dir).unwrap();
        assert!(skill.body.contains("Clean content."));
        assert!(!skill.body.contains("title: Internal"));
    }

    #[test]
    fn test_parse_skill_directory_ignores_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("flat-only");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::create_dir(skill_dir.join("nested")).unwrap();

        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: flat-only\ndescription: No nesting\n---\nMain.",
        )
        .unwrap();
        std::fs::write(skill_dir.join("support.md"), "Support.").unwrap();
        std::fs::write(
            skill_dir.join("nested").join("hidden.md"),
            "Should not appear.",
        )
        .unwrap();

        let skill = parse_skill_directory(&skill_dir).unwrap();
        assert_eq!(skill.files.len(), 1);
        assert_eq!(skill.files[0], "support.md");
        assert!(!skill.body.contains("Should not appear"));
    }

    #[test]
    fn test_parse_skill_directory_file_cap_20() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("big-skill");
        std::fs::create_dir(&skill_dir).unwrap();

        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: big-skill\ndescription: Too many files\n---\nMain.",
        )
        .unwrap();
        for i in 0..25 {
            std::fs::write(
                skill_dir.join(format!("file-{:02}.md", i)),
                format!("Content {}", i),
            )
            .unwrap();
        }

        let skill = parse_skill_directory(&skill_dir).unwrap();
        assert_eq!(skill.files.len(), 20);
        assert!(skill.files.contains(&"file-00.md".to_string()));
        assert!(skill.files.contains(&"file-19.md".to_string()));
        assert!(!skill.files.contains(&"file-20.md".to_string()));
    }

    #[test]
    fn test_parse_skill_directory_only_md_files() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("mixed-files");
        std::fs::create_dir(&skill_dir).unwrap();

        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: mixed-files\ndescription: Has non-md files\n---\nMain.",
        )
        .unwrap();
        std::fs::write(skill_dir.join("valid.md"), "Valid.").unwrap();
        std::fs::write(skill_dir.join("ignore.txt"), "Should be ignored.").unwrap();
        std::fs::write(skill_dir.join("ignore.json"), "{}").unwrap();

        let skill = parse_skill_directory(&skill_dir).unwrap();
        assert_eq!(skill.files.len(), 1);
        assert_eq!(skill.files[0], "valid.md");
        assert!(!skill.body.contains("Should be ignored"));
    }
}
