// app/src-tauri/src/runtime/persona_registry.rs
//
// Archetype registry loader and cognitive profile generator.
// Reads bundled TOML definitions (6 bases, 37 specializations) and produces
// deterministic COGNITIVE_PROFILE.md content for agents at creation time.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ── Registry Index (registry.toml) ────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryIndex {
    pub meta: RegistryMeta,
    pub bases: HashMap<String, RegistryBaseEntry>,
    pub specializations: HashMap<String, RegistrySpecEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryMeta {
    pub version: String,
    pub total_bases: u32,
    pub total_specializations: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryBaseEntry {
    pub name: String,
    pub tagline: String,
    pub file: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistrySpecEntry {
    pub name: String,
    pub department: String,
    pub file: String,
}

// ── Base Archetype ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Base {
    pub identity: BaseIdentity,
    pub ocean: OceanProfile,
    pub belbin: BelbinBlock,
    pub disc: DiscBlock,
    pub cognition: CognitionBlock,
    pub communication: CommunicationBlock,
    pub weaknesses: BaseWeaknesses,
    pub directives: DirectivesBlock,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BaseIdentity {
    pub id: String,
    pub name: String,
    pub tagline: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OceanProfile {
    pub openness: f32,
    pub conscientiousness: f32,
    pub extraversion: f32,
    pub agreeableness: f32,
    pub neuroticism: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BelbinBlock {
    pub primary: String,
    pub secondary: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscBlock {
    pub style: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CognitionBlock {
    pub style: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommunicationBlock {
    pub verbosity: String,
    pub initiative: String,
    pub tone: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BaseWeaknesses {
    pub allowable: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DirectivesBlock {
    pub core: Vec<String>,
    pub anti_patterns: Vec<String>,
}

// ── Specialization ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Specialization {
    pub identity: SpecIdentity,
    pub ocean_modifiers: OceanModifiers,
    pub domain: SpecDomain,
    pub weaknesses: SpecWeaknesses,
    // pairings is informational only — not used for profile generation
}

#[derive(Debug, Clone, Deserialize)]
pub struct SpecIdentity {
    pub id: String,
    pub name: String,
    pub department: String,
    pub tagline: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OceanModifiers {
    pub openness: f32,
    pub conscientiousness: f32,
    pub extraversion: f32,
    pub agreeableness: f32,
    pub neuroticism: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SpecDomain {
    pub knowledge: Vec<String>,
    pub tools: Vec<String>,
    pub output_formats: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SpecWeaknesses {
    pub domain_specific: Vec<String>,
}

// ── Loaded Registry ───────────────────────────────────────────────────────

/// The full registry loaded into memory: all bases and specializations.
pub struct PersonaRegistry {
    pub bases: HashMap<String, Base>,
    pub specializations: HashMap<String, Specialization>,
}

// ── Registry Loading ──────────────────────────────────────────────────────

/// Resolve the persona-registry resource directory.
/// Checks Tauri bundled resources first, falls back to CARGO_MANIFEST_DIR/resources.
pub fn resolve_registry_dir(app_handle: Option<&tauri::AppHandle>) -> Result<PathBuf, String> {
    if let Some(handle) = app_handle {
        use tauri::Manager;
        if let Ok(resource_path) = handle.path().resolve(
            "resources/persona-registry",
            tauri::path::BaseDirectory::Resource,
        ) {
            if resource_path.exists() {
                return Ok(resource_path);
            }
        }
    }

    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("persona-registry");

    if dev_path.exists() {
        return Ok(dev_path);
    }

    Err("Persona registry not found. Ensure resources/persona-registry/ exists.".to_string())
}

/// Load the full registry from TOML files in the given directory.
pub fn load_registry(registry_dir: &Path) -> Result<PersonaRegistry, String> {
    let index_path = registry_dir.join("registry.toml");
    let index_content = std::fs::read_to_string(&index_path)
        .map_err(|e| format!("Failed to read registry.toml: {e}"))?;
    let index: RegistryIndex = toml::from_str(&index_content)
        .map_err(|e| format!("Failed to parse registry.toml: {e}"))?;

    let mut bases = HashMap::new();
    for (key, entry) in &index.bases {
        let path = registry_dir.join(&entry.file);
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read base '{}': {e}", entry.file))?;
        let base: Base =
            toml::from_str(&content).map_err(|e| format!("Failed to parse base '{}': {e}", key))?;
        bases.insert(key.clone(), base);
    }

    let mut specializations = HashMap::new();
    for (key, entry) in &index.specializations {
        let path = registry_dir.join(&entry.file);
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read specialization '{}': {e}", entry.file))?;
        let spec: Specialization = toml::from_str(&content)
            .map_err(|e| format!("Failed to parse specialization '{}': {e}", key))?;
        specializations.insert(key.clone(), spec);
    }

    Ok(PersonaRegistry {
        bases,
        specializations,
    })
}

// ── OCEAN Resolution ──────────────────────────────────────────────────────

/// Resolved OCEAN profile after applying specialization modifiers.
#[derive(Debug, Clone)]
pub struct ResolvedOcean {
    pub openness: f32,
    pub conscientiousness: f32,
    pub extraversion: f32,
    pub agreeableness: f32,
    pub neuroticism: f32,
}

/// Compute final OCEAN = clamp(base + modifier, 0.0, 1.0).
pub fn resolve_ocean(base: &OceanProfile, modifiers: &OceanModifiers) -> ResolvedOcean {
    ResolvedOcean {
        openness: (base.openness + modifiers.openness).clamp(0.0, 1.0),
        conscientiousness: (base.conscientiousness + modifiers.conscientiousness).clamp(0.0, 1.0),
        extraversion: (base.extraversion + modifiers.extraversion).clamp(0.0, 1.0),
        agreeableness: (base.agreeableness + modifiers.agreeableness).clamp(0.0, 1.0),
        neuroticism: (base.neuroticism + modifiers.neuroticism).clamp(0.0, 1.0),
    }
}

// ── OCEAN-to-Prose Mapping (5-band) ──────────────────────────────────────
//
// Five bands produce meaningfully different output for meaningfully different
// radar positions. The old 3-band system made half the drag range feel identical.
//
// | Band      | Range     | Character                      |
// |-----------|-----------|--------------------------------|
// | Very Low  | 0.0 - 0.2 | Strongest low-end expression  |
// | Low       | 0.2 - 0.4 | Moderate low-end expression   |
// | Mid       | 0.4 - 0.6 | Balanced / situational        |
// | High      | 0.6 - 0.8 | Moderate high-end expression  |
// | Very High | 0.8 - 1.0 | Strongest high-end expression |

fn ocean_prose_openness(score: f32) -> &'static str {
    if score < 0.2 {
        "You strongly prefer proven, established methods. Novelty is a distraction — what works, works."
    } else if score < 0.4 {
        "You lean toward conventional approaches but will consider alternatives when evidence supports them."
    } else if score < 0.6 {
        "You balance conventional and creative approaches. You'll try something new when there's good reason."
    } else if score < 0.8 {
        "You seek novel approaches and unconventional connections. Creative exploration energizes you."
    } else {
        "You are deeply driven by intellectual curiosity and unconventional thinking. You actively seek out ideas others overlook and thrive on creative leaps."
    }
}

fn ocean_prose_conscientiousness(score: f32) -> &'static str {
    if score < 0.2 {
        "You operate on instinct and momentum. Process exists to be broken when speed demands it."
    } else if score < 0.4 {
        "You favor speed and flexibility over rigid process. Structure is a tool, not a constraint."
    } else if score < 0.6 {
        "You balance thoroughness with pragmatism. Structured enough to follow through, flexible enough to adapt."
    } else if score < 0.8 {
        "You are methodical and detail-oriented. Plans are followed, work is documented, corners are not cut."
    } else {
        "You are rigorously systematic. Every detail is tracked, every edge case considered, every process documented. Nothing is left to chance."
    }
}

fn ocean_prose_extraversion(score: f32) -> &'static str {
    if score < 0.2 {
        "You work in deep solitude and surface only when you have completed, definitive results. Silence is productive."
    } else if score < 0.4 {
        "You work best in focused solitude. You speak when you have something worth saying."
    } else if score < 0.6 {
        "You engage when relevant and stay measured in your responses. Neither withdrawn nor performative."
    } else if score < 0.8 {
        "You communicate proactively and surface findings without being asked. Engagement is natural."
    } else {
        "You are highly communicative and collaborative. You think out loud, share progress frequently, and actively seek input from others."
    }
}

fn ocean_prose_agreeableness(score: f32) -> &'static str {
    if score < 0.2 {
        "You are uncompromisingly direct. You challenge assumptions hard, prioritize truth over comfort, and don't soften your analysis for politeness."
    } else if score < 0.4 {
        "You challenge directly and prioritize truth over comfort. Pushback is a feature, not a flaw."
    } else if score < 0.6 {
        "You're collaborative but willing to push back when something doesn't hold up."
    } else if score < 0.8 {
        "You seek consensus and frame feedback carefully. You meet people where they are while maintaining honesty."
    } else {
        "You are deeply attuned to team harmony and interpersonal dynamics. You naturally frame everything constructively and work hard to maintain positive relationships."
    }
}

fn ocean_prose_neuroticism(score: f32) -> &'static str {
    if score < 0.2 {
        "You are unflappable. Pressure, ambiguity, and high stakes don't register as stress — they register as data."
    } else if score < 0.4 {
        "You stay steady under pressure and calm in ambiguity. Confidence comes naturally."
    } else if score < 0.6 {
        "You're appropriately cautious. You flag genuine risks without spiraling into what-ifs."
    } else if score < 0.8 {
        "You maintain heightened sensitivity to risk. Thorough checking driven by an awareness of what could go wrong."
    } else {
        "You are intensely risk-aware. You anticipate failure modes others miss and build comprehensive safeguards, though this vigilance can sometimes slow forward momentum."
    }
}

// ── Trait Resolution ─────────────────────────────────────────────────────

use crate::config::agent_config::ResolvedTraits;

/// Resolve traits from registry entries and manual overrides.
///
/// 1. Start with primary base OCEAN (or 0.5 across the board if None/Custom)
/// 2. If secondary base: blend = primary * weight + secondary * (1 - weight)
/// 3. If specialization: apply additive modifiers
/// 4. Apply manual overrides (replace computed values)
/// 5. Clamp all to [0.0, 1.0]
pub fn resolve_traits(
    primary: Option<&Base>,
    secondary: Option<&Base>,
    blend_weight: f32,
    spec: Option<&Specialization>,
    manual_overrides: &std::collections::HashMap<String, f32>,
) -> ResolvedTraits {
    // Step 1: Start with primary base or neutral
    let mut o = primary.map(|b| b.ocean.openness).unwrap_or(0.5);
    let mut c = primary.map(|b| b.ocean.conscientiousness).unwrap_or(0.5);
    let mut e = primary.map(|b| b.ocean.extraversion).unwrap_or(0.5);
    let mut a = primary.map(|b| b.ocean.agreeableness).unwrap_or(0.5);
    let mut n = primary.map(|b| b.ocean.neuroticism).unwrap_or(0.5);

    // Step 2: Blend with secondary if present
    if let Some(sec) = secondary {
        let w = blend_weight.clamp(0.0, 1.0);
        let sw = 1.0 - w;
        o = o * w + sec.ocean.openness * sw;
        c = c * w + sec.ocean.conscientiousness * sw;
        e = e * w + sec.ocean.extraversion * sw;
        a = a * w + sec.ocean.agreeableness * sw;
        n = n * w + sec.ocean.neuroticism * sw;
    }

    // Step 3: Apply specialization modifiers
    if let Some(s) = spec {
        o += s.ocean_modifiers.openness;
        c += s.ocean_modifiers.conscientiousness;
        e += s.ocean_modifiers.extraversion;
        a += s.ocean_modifiers.agreeableness;
        n += s.ocean_modifiers.neuroticism;
    }

    // Step 4: Apply manual overrides
    if let Some(&v) = manual_overrides.get("openness") {
        o = v;
    }
    if let Some(&v) = manual_overrides.get("conscientiousness") {
        c = v;
    }
    if let Some(&v) = manual_overrides.get("extraversion") {
        e = v;
    }
    if let Some(&v) = manual_overrides.get("agreeableness") {
        a = v;
    }
    if let Some(&v) = manual_overrides.get("neuroticism") {
        n = v;
    }

    // Step 5: Clamp
    let o = o.clamp(0.0, 1.0);
    let c = c.clamp(0.0, 1.0);
    let e = e.clamp(0.0, 1.0);
    let a = a.clamp(0.0, 1.0);
    let n = n.clamp(0.0, 1.0);

    // Communication defaults from primary base
    let (verbosity, initiative, tone) = primary
        .map(|b| {
            (
                b.communication.verbosity.clone(),
                b.communication.initiative.clone(),
                b.communication.tone.clone(),
            )
        })
        .unwrap_or_else(|| ("balanced".into(), "measured".into(), "balanced".into()));

    ResolvedTraits {
        primary_base: primary.map(|b| b.identity.id.clone()),
        secondary_base: secondary.map(|b| b.identity.id.clone()),
        blend_weight: secondary.map(|_| blend_weight),
        specialization: spec.map(|s| s.identity.id.clone()),
        openness: o,
        conscientiousness: c,
        extraversion: e,
        agreeableness: a,
        neuroticism: n,
        verbosity,
        initiative,
        tone,
        manual_overrides: manual_overrides.keys().cloned().collect(),
        directives_pending: primary.is_none(),
    }
}

// ── Cognitive Profile Generator ───────────────────────────────────────────

/// Generate a deterministic COGNITIVE_PROFILE.md from a base + specialization.
///
/// This is the document that tells the agent HOW to behave. It translates
/// OCEAN numbers into concrete behavioral directives. Same base+specialization
/// always produces the same output.
pub fn generate_cognitive_profile(base: &Base, specialization: &Specialization) -> String {
    let ocean = resolve_ocean(&base.ocean, &specialization.ocean_modifiers);

    let core_directives: String = base
        .directives
        .core
        .iter()
        .map(|d| format!("- {d}"))
        .collect::<Vec<_>>()
        .join("\n");

    let anti_patterns: String = base
        .directives
        .anti_patterns
        .iter()
        .map(|a| format!("- {a}"))
        .collect::<Vec<_>>()
        .join("\n");

    let domain_knowledge: String = specialization
        .domain
        .knowledge
        .iter()
        .map(|k| format!("- {k}"))
        .collect::<Vec<_>>()
        .join("\n");

    let base_weaknesses: String = base
        .weaknesses
        .allowable
        .iter()
        .map(|w| format!("- {w}"))
        .collect::<Vec<_>>()
        .join("\n");

    let domain_weaknesses: String = specialization
        .weaknesses
        .domain_specific
        .iter()
        .map(|w| format!("- {w}"))
        .collect::<Vec<_>>()
        .join("\n");

    let output_formats: String = specialization
        .domain
        .output_formats
        .iter()
        .map(|f| format!("- {f}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"# Cognitive Profile

**Archetype:** {} / {}
**Base:** {} | **Specialization:** {}

## How You Think

{}. {}

{}
{}
{}
{}
{}

## Team Role

**Belbin:** {} + {} | **DISC:** {} ({})

## Communication Style

**Verbosity:** {} | **Initiative:** {} | **Tone:** {}

{}

## Core Directives

{}

**Anti-patterns to avoid:**
{}

## Domain Expertise

{}

**Preferred output formats:**
{}

## Known Costs

{}
{}

These are the known costs of your cognitive strengths — watch for them. When you notice one operating live, name it and self-correct. The strengths are yours to keep; the failure modes never get a pass."#,
        base.identity.name,
        specialization.identity.name,
        base.identity.tagline,
        specialization.identity.tagline,
        // How You Think
        base.cognition.description,
        specialization.identity.description,
        ocean_prose_openness(ocean.openness),
        ocean_prose_conscientiousness(ocean.conscientiousness),
        ocean_prose_extraversion(ocean.extraversion),
        ocean_prose_agreeableness(ocean.agreeableness),
        ocean_prose_neuroticism(ocean.neuroticism),
        // Team Role
        base.belbin.primary,
        base.belbin.secondary,
        base.disc.style,
        base.disc.description,
        // Communication Style
        base.communication.verbosity,
        base.communication.initiative,
        base.communication.tone,
        base.communication.description,
        // Directives
        core_directives,
        anti_patterns,
        // Domain
        domain_knowledge,
        output_formats,
        // Weaknesses
        base_weaknesses,
        domain_weaknesses,
    )
}

/// Generate a COGNITIVE_PROFILE.md from resolved traits.
///
/// This is the new path — reads from `ResolvedTraits` instead of raw registry entries.
/// Produces the same output format as `generate_cognitive_profile()` but can handle:
/// - Blended bases
/// - Manual overrides
/// - Custom agents (no base at all)
/// - Agents without specializations
pub fn generate_cognitive_profile_from_traits(
    traits: &ResolvedTraits,
    primary_base: Option<&Base>,
    secondary_base: Option<&Base>,
    spec: Option<&Specialization>,
) -> String {
    // Header — shows blend if secondary base is present
    let archetype_line = match (
        &traits.primary_base,
        &traits.secondary_base,
        &traits.specialization,
    ) {
        (Some(base), Some(sec), Some(s)) => {
            let base_name = primary_base
                .map(|b| b.identity.name.as_str())
                .unwrap_or(base.as_str());
            let sec_name = secondary_base
                .map(|b| b.identity.name.as_str())
                .unwrap_or(sec.as_str());
            let spec_name = spec.map(|s| s.identity.name.as_str()).unwrap_or(s.as_str());
            let pct = traits
                .blend_weight
                .map(|w| (w * 100.0) as u8)
                .unwrap_or(100);
            format!(
                "**Archetype:** {} + {} ({}:{}) / {}",
                base_name,
                sec_name,
                pct,
                100 - pct,
                spec_name
            )
        }
        (Some(base), Some(sec), None) => {
            let base_name = primary_base
                .map(|b| b.identity.name.as_str())
                .unwrap_or(base.as_str());
            let sec_name = secondary_base
                .map(|b| b.identity.name.as_str())
                .unwrap_or(sec.as_str());
            let pct = traits
                .blend_weight
                .map(|w| (w * 100.0) as u8)
                .unwrap_or(100);
            format!(
                "**Archetype:** {} + {} ({}:{})",
                base_name,
                sec_name,
                pct,
                100 - pct
            )
        }
        (Some(base), None, Some(s)) => {
            let base_name = primary_base
                .map(|b| b.identity.name.as_str())
                .unwrap_or(base.as_str());
            let spec_name = spec.map(|s| s.identity.name.as_str()).unwrap_or(s.as_str());
            format!("**Archetype:** {} / {}", base_name, spec_name)
        }
        (Some(base), None, None) => {
            let base_name = primary_base
                .map(|b| b.identity.name.as_str())
                .unwrap_or(base.as_str());
            format!("**Archetype:** {}", base_name)
        }
        (None, _, _) => "**Archetype:** Custom".to_string(),
    };

    let tagline_line = match (primary_base, secondary_base, spec) {
        (Some(b), Some(s2), Some(s)) => {
            format!(
                "**Base:** {} blended with {} | **Specialization:** {}",
                b.identity.tagline, s2.identity.tagline, s.identity.tagline
            )
        }
        (Some(b), Some(s2), None) => {
            format!(
                "**Base:** {} blended with {}",
                b.identity.tagline, s2.identity.tagline
            )
        }
        (Some(b), None, Some(s)) => {
            format!(
                "**Base:** {} | **Specialization:** {}",
                b.identity.tagline, s.identity.tagline
            )
        }
        (Some(b), None, None) => format!("**Base:** {}", b.identity.tagline),
        (None, _, Some(s)) => format!("**Specialization:** {}", s.identity.tagline),
        (None, _, None) => "**Base:** Neutral — hand-crafted personality".to_string(),
    };

    // How You Think
    let cognition_desc = primary_base
        .map(|b| format!("{}. ", b.cognition.description))
        .unwrap_or_default();
    let spec_desc = spec.map(|s| s.identity.description.as_str()).unwrap_or("");

    // Directives
    let core_directives = primary_base
        .map(|b| {
            b.directives
                .core
                .iter()
                .map(|d| format!("- {d}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let anti_patterns = primary_base
        .map(|b| {
            b.directives
                .anti_patterns
                .iter()
                .map(|a| format!("- {a}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    // Domain
    let domain_knowledge = spec
        .map(|s| {
            s.domain
                .knowledge
                .iter()
                .map(|k| format!("- {k}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let output_formats = spec
        .map(|s| {
            s.domain
                .output_formats
                .iter()
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    // Weaknesses
    let base_weaknesses = primary_base
        .map(|b| {
            b.weaknesses
                .allowable
                .iter()
                .map(|w| format!("- {w}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let domain_weaknesses = spec
        .map(|s| {
            s.weaknesses
                .domain_specific
                .iter()
                .map(|w| format!("- {w}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    // Team Role (only if base provides it)
    let team_role_section = primary_base
        .map(|b| {
            format!(
                "\n\n## Team Role\n\n**Belbin:** {} + {} | **DISC:** {} ({})",
                b.belbin.primary, b.belbin.secondary, b.disc.style, b.disc.description,
            )
        })
        .unwrap_or_default();

    // Communication
    let comm_desc = primary_base
        .map(|b| b.communication.description.as_str())
        .unwrap_or("Adapt communication style to context.");

    // Build the profile
    let mut profile = format!(
        "# Cognitive Profile\n\n{}\n{}\n\n## How You Think\n\n{}{}\n\n{}\n{}\n{}\n{}\n{}{}",
        archetype_line,
        tagline_line,
        cognition_desc,
        spec_desc,
        ocean_prose_openness(traits.openness),
        ocean_prose_conscientiousness(traits.conscientiousness),
        ocean_prose_extraversion(traits.extraversion),
        ocean_prose_agreeableness(traits.agreeableness),
        ocean_prose_neuroticism(traits.neuroticism),
        team_role_section,
    );

    // Communication Style
    profile.push_str(&format!(
        "\n\n## Communication Style\n\n**Verbosity:** {} | **Initiative:** {} | **Tone:** {}\n\n{}",
        traits.verbosity, traits.initiative, traits.tone, comm_desc,
    ));

    // Directives (skip if Custom with no base)
    if !core_directives.is_empty() {
        profile.push_str(&format!("\n\n## Core Directives\n\n{}", core_directives,));
        if !anti_patterns.is_empty() {
            profile.push_str(&format!(
                "\n\n**Anti-patterns to avoid:**\n{}",
                anti_patterns,
            ));
        }
    } else if traits.directives_pending {
        profile.push_str(
            "\n\n## Core Directives\n\n*Pending — directives will be established through your first conversation with the human operator.*"
        );
    }

    // Domain (skip if no specialization)
    if !domain_knowledge.is_empty() {
        profile.push_str(&format!("\n\n## Domain Expertise\n\n{}", domain_knowledge,));
        if !output_formats.is_empty() {
            profile.push_str(&format!(
                "\n\n**Preferred output formats:**\n{}",
                output_formats,
            ));
        }
    }

    // Weaknesses (skip if empty)
    if !base_weaknesses.is_empty() || !domain_weaknesses.is_empty() {
        profile.push_str("\n\n## Known Costs\n\n");
        if !base_weaknesses.is_empty() {
            profile.push_str(&base_weaknesses);
        }
        if !domain_weaknesses.is_empty() {
            if !base_weaknesses.is_empty() {
                profile.push('\n');
            }
            profile.push_str(&domain_weaknesses);
        }
        profile.push_str(
            "\n\nThese are the known costs of your cognitive strengths — watch for them. When you notice one operating live, name it and self-correct. The strengths are yours to keep; the failure modes never get a pass."
        );
    }

    profile
}

// generate_cognitive_lens() removed per D8 (subagent neutrality).
// Subagents receive task context from the parent, not personality traits.
// This prevents sycophancy inheritance from high-agreeableness parents.

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_base() -> Base {
        Base {
            identity: BaseIdentity {
                id: "scout".to_string(),
                name: "Scout".to_string(),
                tagline: "The explorer".to_string(),
                description: "Finds things, maps territory, spots patterns others miss."
                    .to_string(),
            },
            ocean: OceanProfile {
                openness: 0.8,
                conscientiousness: 0.3,
                extraversion: 0.5,
                agreeableness: 0.4,
                neuroticism: 0.2,
            },
            belbin: BelbinBlock {
                primary: "Resource Investigator".to_string(),
                secondary: "Plant".to_string(),
            },
            disc: DiscBlock {
                style: "D/I".to_string(),
                description: "Assertive but observational".to_string(),
            },
            cognition: CognitionBlock {
                style: "Conceptual".to_string(),
                description: "Broad thinking, pattern recognition, lateral connections".to_string(),
            },
            communication: CommunicationBlock {
                verbosity: "concise".to_string(),
                initiative: "proactive".to_string(),
                tone: "direct".to_string(),
                description: "Reports findings without being asked.".to_string(),
            },
            weaknesses: BaseWeaknesses {
                allowable: vec![
                    "May go down rabbit holes".to_string(),
                    "Breadth without depth".to_string(),
                ],
            },
            directives: DirectivesBlock {
                core: vec![
                    "Explore broadly".to_string(),
                    "Surface the unexpected".to_string(),
                ],
                anti_patterns: vec![
                    "Settling into routine".to_string(),
                    "Waiting to be asked".to_string(),
                ],
            },
        }
    }

    fn test_specialization() -> Specialization {
        Specialization {
            identity: SpecIdentity {
                id: "code_reviewer".to_string(),
                name: "Code Reviewer".to_string(),
                department: "engineering".to_string(),
                tagline: "Line-by-line quality".to_string(),
                description: "Catches bugs, style issues, logic errors.".to_string(),
            },
            ocean_modifiers: OceanModifiers {
                openness: -0.1,
                conscientiousness: 0.2,
                extraversion: -0.1,
                agreeableness: -0.1,
                neuroticism: 0.1,
            },
            domain: SpecDomain {
                knowledge: vec![
                    "Code quality patterns".to_string(),
                    "Common bug classes".to_string(),
                ],
                tools: vec!["Diff views".to_string(), "Linters".to_string()],
                output_formats: vec![
                    "Inline comments".to_string(),
                    "Severity-tagged findings".to_string(),
                ],
            },
            weaknesses: SpecWeaknesses {
                domain_specific: vec![
                    "Nitpicking trivia".to_string(),
                    "Blocking on style when logic is fine".to_string(),
                ],
            },
        }
    }

    #[test]
    fn test_resolve_ocean_clamping() {
        let base = OceanProfile {
            openness: 0.9,
            conscientiousness: 0.1,
            extraversion: 0.5,
            agreeableness: 0.2,
            neuroticism: 0.8,
        };
        let modifiers = OceanModifiers {
            openness: 0.2,           // 0.9 + 0.2 = 1.1 -> clamped to 1.0
            conscientiousness: -0.2, // 0.1 - 0.2 = -0.1 -> clamped to 0.0
            extraversion: 0.0,
            agreeableness: 0.0,
            neuroticism: 0.2, // 0.8 + 0.2 = 1.0 -> exactly 1.0
        };
        let resolved = resolve_ocean(&base, &modifiers);
        assert_eq!(resolved.openness, 1.0);
        assert_eq!(resolved.conscientiousness, 0.0);
        assert_eq!(resolved.extraversion, 0.5);
        assert_eq!(resolved.agreeableness, 0.2);
        assert_eq!(resolved.neuroticism, 1.0);
    }

    #[test]
    fn test_resolve_ocean_normal() {
        let base = OceanProfile {
            openness: 0.8,
            conscientiousness: 0.3,
            extraversion: 0.5,
            agreeableness: 0.4,
            neuroticism: 0.2,
        };
        let modifiers = OceanModifiers {
            openness: -0.1,
            conscientiousness: 0.2,
            extraversion: -0.1,
            agreeableness: -0.1,
            neuroticism: 0.1,
        };
        let resolved = resolve_ocean(&base, &modifiers);
        // Use approximate comparison for floating point
        assert!((resolved.openness - 0.7).abs() < 0.001);
        assert!((resolved.conscientiousness - 0.5).abs() < 0.001);
        assert!((resolved.extraversion - 0.4).abs() < 0.001);
        assert!((resolved.agreeableness - 0.3).abs() < 0.001);
        assert!((resolved.neuroticism - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_ocean_prose_five_bands() {
        // Very Low (0.0 - 0.2)
        assert!(ocean_prose_openness(0.1).contains("established"));
        assert!(ocean_prose_conscientiousness(0.1).contains("instinct"));
        assert!(ocean_prose_extraversion(0.1).contains("solitude"));
        assert!(ocean_prose_agreeableness(0.1).contains("uncompromisingly"));
        assert!(ocean_prose_neuroticism(0.1).contains("unflappable"));

        // Low (0.2 - 0.4)
        assert!(ocean_prose_openness(0.3).contains("conventional"));
        assert!(ocean_prose_conscientiousness(0.3).contains("speed"));
        assert!(ocean_prose_extraversion(0.3).contains("something worth saying"));
        assert!(ocean_prose_agreeableness(0.3).contains("challenge directly"));
        assert!(ocean_prose_neuroticism(0.3).contains("steady"));

        // Mid (0.4 - 0.6)
        assert!(ocean_prose_openness(0.5).contains("balance"));
        assert!(ocean_prose_conscientiousness(0.5).contains("pragmatism"));
        assert!(ocean_prose_extraversion(0.5).contains("measured"));
        assert!(ocean_prose_agreeableness(0.5).contains("collaborative"));
        assert!(ocean_prose_neuroticism(0.5).contains("cautious"));

        // High (0.6 - 0.8)
        assert!(ocean_prose_openness(0.7).contains("novel"));
        assert!(ocean_prose_conscientiousness(0.7).contains("methodical"));
        assert!(ocean_prose_extraversion(0.7).contains("proactively"));
        assert!(ocean_prose_agreeableness(0.7).contains("consensus"));
        assert!(ocean_prose_neuroticism(0.7).contains("heightened"));

        // Very High (0.8 - 1.0)
        assert!(ocean_prose_openness(0.9).contains("deeply driven"));
        assert!(ocean_prose_conscientiousness(0.9).contains("rigorously"));
        assert!(ocean_prose_extraversion(0.9).contains("highly communicative"));
        assert!(ocean_prose_agreeableness(0.9).contains("deeply attuned"));
        assert!(ocean_prose_neuroticism(0.9).contains("intensely"));
    }

    #[test]
    fn test_ocean_prose_boundary_values() {
        // Test exact boundaries — values AT the boundary go to the higher band
        // because we use < not <=
        assert!(ocean_prose_openness(0.0).contains("established")); // Very Low
        assert!(ocean_prose_openness(0.2).contains("conventional")); // Low (0.2 is >= 0.2)
        assert!(ocean_prose_openness(0.4).contains("balance")); // Mid
        assert!(ocean_prose_openness(0.6).contains("novel")); // High
        assert!(ocean_prose_openness(0.8).contains("deeply driven")); // Very High
        assert!(ocean_prose_openness(1.0).contains("deeply driven")); // Very High
    }

    #[test]
    fn test_generate_cognitive_profile_deterministic() {
        let base = test_base();
        let spec = test_specialization();

        let profile1 = generate_cognitive_profile(&base, &spec);
        let profile2 = generate_cognitive_profile(&base, &spec);

        assert_eq!(
            profile1, profile2,
            "Profile generation must be deterministic"
        );
    }

    #[test]
    fn test_generate_cognitive_profile_contains_key_sections() {
        let base = test_base();
        let spec = test_specialization();

        let profile = generate_cognitive_profile(&base, &spec);

        assert!(profile.contains("# Cognitive Profile"));
        assert!(profile.contains("Scout / Code Reviewer"));
        assert!(profile.contains("## How You Think"));
        assert!(profile.contains("## Team Role"));
        assert!(profile.contains("## Communication Style"));
        assert!(profile.contains("## Core Directives"));
        assert!(profile.contains("## Domain Expertise"));
        assert!(profile.contains("## Known Costs"));
        assert!(profile.contains("Resource Investigator"));
        assert!(profile.contains("D/I"));
        assert!(profile.contains("Explore broadly"));
        assert!(profile.contains("Code quality patterns"));
        assert!(profile.contains("May go down rabbit holes"));
        assert!(profile.contains("Nitpicking trivia"));
    }

    #[test]
    fn test_generate_cognitive_profile_size() {
        let base = test_base();
        let spec = test_specialization();

        let profile = generate_cognitive_profile(&base, &spec);
        let char_count = profile.len();

        // Profile should be 1-2K chars, definitely under persona MAX_FILE_CHARS (8000)
        assert!(char_count > 500, "Profile too short: {char_count} chars");
        assert!(char_count < 4000, "Profile too long: {char_count} chars");
    }

    // test_generate_cognitive_lens_compact removed — function removed per D8

    #[test]
    fn test_load_registry_from_resources() {
        let registry_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("persona-registry");

        if !registry_dir.exists() {
            // Skip if running in CI without resources
            return;
        }

        let registry = load_registry(&registry_dir).expect("Failed to load registry");

        assert_eq!(registry.bases.len(), 6, "Expected 6 bases");
        assert_eq!(
            registry.specializations.len(),
            9,
            "Expected 9 specializations"
        );

        // Verify all bases loaded
        for base_id in &[
            "scout",
            "sentinel",
            "architect",
            "operator",
            "diplomat",
            "catalyst",
        ] {
            assert!(
                registry.bases.contains_key(*base_id),
                "Missing base: {base_id}"
            );
        }

        // Verify all specializations loaded
        for spec_id in &[
            "coder",
            "auditor",
            "designer",
            "researcher",
            "ideator",
            "adversary",
            "writer",
            "herald",
            "gamewright",
        ] {
            assert!(
                registry.specializations.contains_key(*spec_id),
                "Missing specialization: {spec_id}"
            );
        }

        // Spot-check a specialization
        let auditor = registry
            .specializations
            .get("auditor")
            .expect("Missing auditor");
        assert_eq!(auditor.identity.department, "engineering");

        // Verify OCEAN values match what we expect after tuning pass
        let scout = registry.bases.get("scout").unwrap();
        assert!(
            (scout.ocean.extraversion - 0.5).abs() < 0.001,
            "Scout E should be 0.5"
        );

        let catalyst = registry.bases.get("catalyst").unwrap();
        assert!(
            (catalyst.ocean.agreeableness - 0.3).abs() < 0.001,
            "Catalyst A should be 0.3"
        );
    }

    #[test]
    fn test_full_profile_generation_from_registry() {
        let registry_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("persona-registry");

        if !registry_dir.exists() {
            return;
        }

        let registry = load_registry(&registry_dir).expect("Failed to load registry");

        // Generate a profile for every base+spec combo to ensure no panics
        for (base_id, base) in &registry.bases {
            for (spec_id, spec) in &registry.specializations {
                let profile = generate_cognitive_profile(base, spec);
                assert!(!profile.is_empty(), "Empty profile for {base_id}/{spec_id}");
                assert!(
                    profile.len() < 8000,
                    "Profile too large for {base_id}/{spec_id}: {} chars",
                    profile.len()
                );
            }
        }
    }

    // ── ResolvedTraits + from_traits profile generation tests ─────────

    #[test]
    fn test_resolve_traits_primary_only() {
        let base = test_base();
        let overrides = HashMap::new();
        let traits = resolve_traits(Some(&base), None, 1.0, None, &overrides);

        assert_eq!(traits.primary_base.as_deref(), Some("scout"));
        assert!(traits.secondary_base.is_none());
        assert!((traits.openness - 0.8).abs() < 0.001);
        assert!((traits.conscientiousness - 0.3).abs() < 0.001);
        assert_eq!(traits.verbosity, "concise");
        assert_eq!(traits.initiative, "proactive");
        assert!(!traits.directives_pending);
    }

    #[test]
    fn test_resolve_traits_custom_neutral() {
        let overrides = HashMap::new();
        let traits = resolve_traits(None, None, 1.0, None, &overrides);

        assert!(traits.primary_base.is_none());
        assert!((traits.openness - 0.5).abs() < 0.001);
        assert!((traits.conscientiousness - 0.5).abs() < 0.001);
        assert!((traits.extraversion - 0.5).abs() < 0.001);
        assert!(traits.directives_pending); // Custom agents need onboarding
    }

    #[test]
    fn test_resolve_traits_blending() {
        let scout = test_base();
        // Create a sentinel-like base for blending
        let mut sentinel = test_base();
        sentinel.identity.id = "sentinel".to_string();
        sentinel.ocean.openness = 0.2;
        sentinel.ocean.conscientiousness = 0.9;

        let overrides = HashMap::new();
        let traits = resolve_traits(Some(&scout), Some(&sentinel), 0.7, None, &overrides);

        // scout O=0.8 * 0.7 + sentinel O=0.2 * 0.3 = 0.56 + 0.06 = 0.62
        assert!((traits.openness - 0.62).abs() < 0.01);
        // scout C=0.3 * 0.7 + sentinel C=0.9 * 0.3 = 0.21 + 0.27 = 0.48
        assert!((traits.conscientiousness - 0.48).abs() < 0.01);
    }

    #[test]
    fn test_resolve_traits_with_spec_modifiers() {
        let base = test_base();
        let spec = test_specialization();
        let overrides = HashMap::new();
        let traits = resolve_traits(Some(&base), None, 1.0, Some(&spec), &overrides);

        // O: 0.8 + (-0.1) = 0.7
        assert!((traits.openness - 0.7).abs() < 0.001);
        // C: 0.3 + 0.2 = 0.5
        assert!((traits.conscientiousness - 0.5).abs() < 0.001);
        assert_eq!(traits.specialization.as_deref(), Some("code_reviewer"));
    }

    #[test]
    fn test_resolve_traits_manual_overrides() {
        let base = test_base();
        let mut overrides = HashMap::new();
        overrides.insert("openness".to_string(), 0.1);
        overrides.insert("neuroticism".to_string(), 0.9);

        let traits = resolve_traits(Some(&base), None, 1.0, None, &overrides);

        assert!((traits.openness - 0.1).abs() < 0.001); // Overridden
        assert!((traits.neuroticism - 0.9).abs() < 0.001); // Overridden
        assert!((traits.conscientiousness - 0.3).abs() < 0.001); // Not overridden
        assert_eq!(traits.manual_overrides.len(), 2);
    }

    #[test]
    fn test_resolve_traits_clamping() {
        let mut base = test_base();
        base.ocean.openness = 0.95;
        let spec = test_specialization();
        // spec adds +(-0.1) to openness, so 0.95 - 0.1 = 0.85 (fine)
        // But conscientiousness: 0.3 + 0.2 = 0.5 (fine)
        // Let's force a clamp by adding a big manual override that would exceed bounds
        let mut overrides = HashMap::new();
        overrides.insert("openness".to_string(), 1.5); // Should clamp to 1.0
        overrides.insert("neuroticism".to_string(), -0.3); // Should clamp to 0.0

        let traits = resolve_traits(Some(&base), None, 1.0, Some(&spec), &overrides);

        assert_eq!(traits.openness, 1.0);
        assert_eq!(traits.neuroticism, 0.0);
    }

    #[test]
    fn test_generate_profile_from_traits_with_base_and_spec() {
        let base = test_base();
        let spec = test_specialization();
        let overrides = HashMap::new();
        let traits = resolve_traits(Some(&base), None, 1.0, Some(&spec), &overrides);

        let profile =
            generate_cognitive_profile_from_traits(&traits, Some(&base), None, Some(&spec));

        assert!(profile.contains("# Cognitive Profile"));
        assert!(profile.contains("Scout / Code Reviewer"));
        assert!(profile.contains("## How You Think"));
        assert!(profile.contains("## Team Role"));
        assert!(profile.contains("## Communication Style"));
        assert!(profile.contains("## Core Directives"));
        assert!(profile.contains("## Domain Expertise"));
        assert!(profile.contains("## Known Costs"));
        assert!(profile.contains("concise")); // verbosity
        assert!(profile.contains("proactive")); // initiative
    }

    #[test]
    fn test_generate_profile_from_traits_custom() {
        // Custom agent via resolve_traits with no base → directives_pending = true
        let overrides = HashMap::new();
        let traits = resolve_traits(None, None, 1.0, None, &overrides);
        let profile = generate_cognitive_profile_from_traits(&traits, None, None, None);

        assert!(profile.contains("Custom"));
        assert!(profile.contains("hand-crafted"));
        assert!(traits.directives_pending);
        assert!(profile.contains("Pending")); // directives pending section
        assert!(!profile.contains("## Domain Expertise")); // no spec
    }

    #[test]
    fn test_generate_profile_from_traits_base_only() {
        let base = test_base();
        let overrides = HashMap::new();
        let traits = resolve_traits(Some(&base), None, 1.0, None, &overrides);
        let profile = generate_cognitive_profile_from_traits(&traits, Some(&base), None, None);

        assert!(profile.contains("Scout"));
        assert!(!profile.contains("Code Reviewer"));
        assert!(profile.contains("## Core Directives"));
        assert!(!profile.contains("## Domain Expertise")); // no spec
    }

    #[test]
    fn test_generate_profile_from_traits_deterministic() {
        let base = test_base();
        let spec = test_specialization();
        let overrides = HashMap::new();
        let traits = resolve_traits(Some(&base), None, 1.0, Some(&spec), &overrides);

        let p1 = generate_cognitive_profile_from_traits(&traits, Some(&base), None, Some(&spec));
        let p2 = generate_cognitive_profile_from_traits(&traits, Some(&base), None, Some(&spec));
        assert_eq!(
            p1, p2,
            "Profile generation from traits must be deterministic"
        );
    }

    #[test]
    fn test_resolved_traits_config_round_trip() {
        let base = test_base();
        let spec = test_specialization();
        let overrides = HashMap::new();
        let traits = resolve_traits(Some(&base), None, 1.0, Some(&spec), &overrides);

        let toml_str = toml::to_string_pretty(&traits).expect("Failed to serialize traits");
        let deserialized: ResolvedTraits =
            toml::from_str(&toml_str).expect("Failed to deserialize traits");

        assert_eq!(traits, deserialized);
    }

    #[test]
    fn test_resolved_traits_default_serde() {
        // Ensure a config without [agent.traits] deserializes cleanly
        let toml_str = r#"
            openness = 0.5
            conscientiousness = 0.5
            extraversion = 0.5
            agreeableness = 0.5
            neuroticism = 0.5
        "#;
        let traits: ResolvedTraits =
            toml::from_str(toml_str).expect("Failed to parse minimal traits");
        assert_eq!(traits.verbosity, "balanced");
        assert_eq!(traits.initiative, "measured");
        assert_eq!(traits.tone, "balanced");
        assert!(!traits.directives_pending);
        assert!(traits.manual_overrides.is_empty());
    }
}
