use serde::Serialize;
use std::path::PathBuf;

use super::error::CommandError;

/// Summary of a persona template available for agent creation.
#[derive(Debug, Clone, Serialize)]
pub struct PersonaTemplateSummary {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// Return the persona templates directory path.
pub fn templates_dir() -> PathBuf {
    crate::config::app_config::base_config_dir().join("persona-templates")
}

/// Title-case a string (first letter uppercase, rest lowercase).
fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + &chars.as_str().to_lowercase(),
    }
}

/// List all available persona templates from the templates directory.
#[tauri::command]
pub async fn list_persona_templates() -> Result<Vec<PersonaTemplateSummary>, CommandError> {
    let dir = templates_dir();
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut templates = Vec::new();

    let entries = std::fs::read_dir(&dir)
        .map_err(|e| CommandError::internal(format!("Failed to read templates dir: {e}")))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let folder_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        let description = {
            let desc_path = path.join("DESCRIPTION.md");
            if desc_path.exists() {
                std::fs::read_to_string(&desc_path)
                    .unwrap_or_default()
                    .trim()
                    .chars()
                    .take(500)
                    .collect::<String>()
            } else {
                String::new()
            }
        };

        templates.push(PersonaTemplateSummary {
            id: folder_name.clone(),
            name: title_case(&folder_name),
            description,
        });
    }

    templates.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(templates)
}

// ── Archetype Registry ───────────────────────────────────────────────────

/// OCEAN scores for frontend visualization (radar chart).
#[derive(Debug, Clone, Serialize)]
pub struct OceanScores {
    pub openness: f32,
    pub conscientiousness: f32,
    pub extraversion: f32,
    pub agreeableness: f32,
    pub neuroticism: f32,
}

/// Summary of a cognitive base archetype for the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct ArchetypeBaseSummary {
    pub id: String,
    pub name: String,
    pub tagline: String,
    pub description: String,
    pub ocean: OceanScores,
    pub belbin_primary: String,
    pub disc_style: String,
    pub cognition_style: String,
}

/// Summary of an archetype specialization for the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct ArchetypeSpecSummary {
    pub id: String,
    pub name: String,
    pub department: String,
    pub tagline: String,
    pub description: String,
    pub ocean_modifiers: OceanScores,
}

/// Combined archetype registry listing for the creation modal.
#[derive(Debug, Clone, Serialize)]
pub struct ArchetypeRegistryListing {
    pub bases: Vec<ArchetypeBaseSummary>,
    pub specializations: Vec<ArchetypeSpecSummary>,
}

/// List all available archetype bases and specializations from the bundled registry.
#[tauri::command]
pub async fn list_archetypes(
    app_handle: tauri::AppHandle,
) -> Result<ArchetypeRegistryListing, CommandError> {
    let registry_dir = crate::runtime::persona_registry::resolve_registry_dir(Some(&app_handle))
        .map_err(|e| CommandError::internal(format!("Registry not found: {e}")))?;

    let registry = crate::runtime::persona_registry::load_registry(&registry_dir)
        .map_err(|e| CommandError::internal(format!("Failed to load registry: {e}")))?;

    let mut bases: Vec<ArchetypeBaseSummary> = registry
        .bases
        .iter()
        .map(|(key, base)| ArchetypeBaseSummary {
            id: key.clone(),
            name: base.identity.name.clone(),
            tagline: base.identity.tagline.clone(),
            description: base.identity.description.clone(),
            ocean: OceanScores {
                openness: base.ocean.openness,
                conscientiousness: base.ocean.conscientiousness,
                extraversion: base.ocean.extraversion,
                agreeableness: base.ocean.agreeableness,
                neuroticism: base.ocean.neuroticism,
            },
            belbin_primary: base.belbin.primary.clone(),
            disc_style: base.disc.style.clone(),
            cognition_style: base.cognition.style.clone(),
        })
        .collect();
    bases.sort_by(|a, b| a.name.cmp(&b.name));

    let mut specializations: Vec<ArchetypeSpecSummary> = registry
        .specializations
        .iter()
        .map(|(key, spec)| ArchetypeSpecSummary {
            id: key.clone(),
            name: spec.identity.name.clone(),
            department: spec.identity.department.clone(),
            tagline: spec.identity.tagline.clone(),
            description: spec.identity.description.clone(),
            ocean_modifiers: OceanScores {
                openness: spec.ocean_modifiers.openness,
                conscientiousness: spec.ocean_modifiers.conscientiousness,
                extraversion: spec.ocean_modifiers.extraversion,
                agreeableness: spec.ocean_modifiers.agreeableness,
                neuroticism: spec.ocean_modifiers.neuroticism,
            },
        })
        .collect();
    specializations.sort_by(|a, b| a.department.cmp(&b.department).then(a.name.cmp(&b.name)));

    Ok(ArchetypeRegistryListing {
        bases,
        specializations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_title_case() {
        assert_eq!(title_case("scout"), "Scout");
        assert_eq!(title_case("ORACLE"), "Oracle");
        assert_eq!(title_case(""), "");
        assert_eq!(title_case("dispatch"), "Dispatch");
    }

    #[test]
    fn test_list_templates_no_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        // Only files, no subdirectories
        std::fs::write(dir.path().join("USER.md"), "shared user").unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_list_templates_with_and_without_description() {
        let dir = tempfile::tempdir().unwrap();

        // Template with description
        let scout_dir = dir.path().join("scout");
        std::fs::create_dir(&scout_dir).unwrap();
        std::fs::write(scout_dir.join("SOUL.md"), "# Soul").unwrap();
        std::fs::write(
            scout_dir.join("DESCRIPTION.md"),
            "Fast exploration and codebase mapping.",
        )
        .unwrap();

        // Template without description
        let oracle_dir = dir.path().join("oracle");
        std::fs::create_dir(&oracle_dir).unwrap();
        std::fs::write(oracle_dir.join("SOUL.md"), "# Soul").unwrap();

        // Build templates manually (simulating the command logic)
        let mut templates = Vec::new();
        for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let folder_name = path.file_name().unwrap().to_str().unwrap().to_string();
            let description = {
                let desc_path = path.join("DESCRIPTION.md");
                if desc_path.exists() {
                    std::fs::read_to_string(&desc_path)
                        .unwrap_or_default()
                        .trim()
                        .chars()
                        .take(500)
                        .collect()
                } else {
                    String::new()
                }
            };
            templates.push(PersonaTemplateSummary {
                id: folder_name.clone(),
                name: title_case(&folder_name),
                description,
            });
        }
        templates.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(templates.len(), 2);
        assert_eq!(templates[0].name, "Oracle");
        assert_eq!(templates[0].description, "");
        assert_eq!(templates[1].name, "Scout");
        assert_eq!(
            templates[1].description,
            "Fast exploration and codebase mapping."
        );
    }

    #[test]
    fn test_description_truncation() {
        let long_desc = "a".repeat(1000);
        let truncated: String = long_desc.trim().chars().take(500).collect();
        assert_eq!(truncated.len(), 500);
    }
}
