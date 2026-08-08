use std::collections::HashSet;

use super::registry::SkillRegistry;
use super::types::{
    DeliveryMode, ResolvedSkill, SkillContext, SkillPriority, SkillSource, TriggerCondition,
};

/// Evaluate whether a single trigger condition matches the current context.
fn trigger_matches(
    trigger: &TriggerCondition,
    recent_tool_names: &[String],
    recent_file_paths: &[String],
    last_user_message: &str,
) -> bool {
    if trigger.always {
        return true;
    }
    if trigger.manual {
        return false;
    }

    // All present conditions must match (AND logic).
    let mut any_condition = false;

    if !trigger.tools.is_empty() {
        any_condition = true;
        let tools_match = trigger
            .tools
            .iter()
            .any(|t| recent_tool_names.iter().any(|rt| rt == t));
        if !tools_match {
            return false;
        }
    }

    if let Some(ref pattern) = trigger.pattern {
        any_condition = true;
        let pattern_match = recent_file_paths
            .iter()
            .any(|fp| glob_match::glob_match(pattern, fp));
        if !pattern_match {
            return false;
        }
    }

    if let Some(ref keyword) = trigger.keyword {
        any_condition = true;
        let kw_lower = keyword.to_lowercase();
        let msg_lower = last_user_message.to_lowercase();
        if !msg_lower.contains(&kw_lower) {
            return false;
        }
    }

    // If no conditions were present at all, this trigger has nothing to match on.
    any_condition
}

fn priority_rank(p: &SkillPriority) -> u8 {
    match p {
        SkillPriority::Low => 0,
        SkillPriority::Normal => 1,
        SkillPriority::High => 2,
    }
}

fn source_rank(s: &SkillSource) -> u8 {
    match s {
        SkillSource::AutoTriggered => 0,
        SkillSource::AgentAssigned => 2,
    }
}

/// Resolve which skills should be active given the registry and context.
///
/// Deduplicates across agent-assigned and auto-triggered sources,
/// then enforces the token budget by dropping lowest-value skills first.
pub fn resolve_skills(registry: &SkillRegistry, ctx: &SkillContext) -> Vec<ResolvedSkill> {
    let mut resolved: Vec<ResolvedSkill> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 1. Agent-assigned skills (highest source priority)
    for name in &ctx.agent_skill_names {
        if let Some(def) = registry.get(name) {
            seen.insert(name.clone());
            let delivery = def.effective_mode();
            resolved.push(ResolvedSkill {
                definition: def,
                source: SkillSource::AgentAssigned,
                delivery,
            });
        }
    }

    // 2. Auto-triggered skills (skip duplicates, evaluate triggers)
    for skill in registry.list() {
        if seen.contains(&skill.file_name) {
            continue;
        }
        let triggered = skill.frontmatter.triggers.iter().any(|t| {
            trigger_matches(
                t,
                &ctx.recent_tool_names,
                &ctx.recent_file_paths,
                &ctx.last_user_message,
            )
        });
        if triggered {
            seen.insert(skill.file_name.clone());
            let delivery = skill.effective_mode();
            resolved.push(ResolvedSkill {
                definition: skill,
                source: SkillSource::AutoTriggered,
                delivery,
            });
        }
    }

    // 4. Token budget enforcement (only inject-mode skills count against budget)
    let inject_total: usize = resolved
        .iter()
        .filter(|r| r.delivery == DeliveryMode::Inject)
        .map(|r| r.definition.frontmatter.max_tokens)
        .sum();
    if inject_total > ctx.token_budget {
        // Sort inject skills by (priority_rank ASC, source_rank ASC) — lowest-value first.
        // Only demote inject skills; reference skills are unaffected.
        let mut inject_indices: Vec<usize> = resolved
            .iter()
            .enumerate()
            .filter(|(_, r)| r.delivery == DeliveryMode::Inject)
            .map(|(i, _)| i)
            .collect();

        inject_indices.sort_by(|&a, &b| {
            let pa = priority_rank(&resolved[a].definition.frontmatter.priority);
            let pb = priority_rank(&resolved[b].definition.frontmatter.priority);
            let sa = source_rank(&resolved[a].source);
            let sb = source_rank(&resolved[b].source);
            pa.cmp(&pb).then(sa.cmp(&sb))
        });

        // Drop lowest-value inject skills until we fit budget
        let mut running = inject_total;
        let mut drop_set: Vec<usize> = Vec::new();
        for &idx in &inject_indices {
            if running <= ctx.token_budget {
                break;
            }
            running -= resolved[idx].definition.frontmatter.max_tokens;
            drop_set.push(idx);
        }
        // Remove in reverse order to preserve indices
        drop_set.sort_unstable_by(|a, b| b.cmp(a));
        for idx in drop_set {
            resolved.remove(idx);
        }
    }

    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Helper: create a skill file and return its file-stem name.
    fn write_skill(dir: &Path, stem: &str, yaml_extra: &str) {
        let content =
            format!("---\nname: {stem}\ndescription: {stem} skill\n{yaml_extra}---\n{stem} body.",);
        std::fs::write(dir.join(format!("{stem}.md")), content).unwrap();
    }

    fn default_ctx() -> SkillContext {
        SkillContext {
            agent_skill_names: vec![],
            recent_tool_names: vec![],
            recent_file_paths: vec![],
            last_user_message: String::new(),
            token_budget: 100_000,
        }
    }

    #[test]
    fn test_resolve_agent_assigned() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "alpha", "");
        write_skill(dir.path(), "beta", "");

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let mut ctx = default_ctx();
        ctx.agent_skill_names = vec!["alpha".into(), "beta".into()];

        let resolved = resolve_skills(&registry, &ctx);
        assert_eq!(resolved.len(), 2);
        assert!(resolved
            .iter()
            .all(|r| r.source == SkillSource::AgentAssigned));
        let names: Vec<&str> = resolved
            .iter()
            .map(|r| r.definition.file_name.as_str())
            .collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    #[test]
    fn test_resolve_auto_trigger_keyword() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "review", "triggers:\n  - keyword: review\n");
        write_skill(dir.path(), "deploy", "triggers:\n  - keyword: deploy\n");

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let mut ctx = default_ctx();
        ctx.last_user_message = "Please review this code".into();

        let resolved = resolve_skills(&registry, &ctx);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].definition.file_name, "review");
        assert_eq!(resolved[0].source, SkillSource::AutoTriggered);
    }

    #[test]
    fn test_resolve_auto_trigger_tools() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "file-ops",
            "triggers:\n  - tools: [\"read_file\", \"write_file\"]\n",
        );

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let mut ctx = default_ctx();
        ctx.recent_tool_names = vec!["read_file".into(), "bash".into()];

        let resolved = resolve_skills(&registry, &ctx);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].definition.file_name, "file-ops");
        assert_eq!(resolved[0].source, SkillSource::AutoTriggered);
    }

    #[test]
    fn test_resolve_token_budget_drops_lowest() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "high-pri",
            "mode: inject\npriority: high\nmax_tokens: 500\n",
        );
        write_skill(
            dir.path(),
            "low-pri",
            "mode: inject\npriority: low\nmax_tokens: 500\n",
        );
        write_skill(
            dir.path(),
            "normal-pri",
            "mode: inject\npriority: normal\nmax_tokens: 500\n",
        );

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let mut ctx = default_ctx();
        ctx.agent_skill_names = vec!["high-pri".into(), "low-pri".into(), "normal-pri".into()];
        ctx.token_budget = 1000; // only room for 2 out of 3

        let resolved = resolve_skills(&registry, &ctx);
        // Only inject skills count against budget, and low-pri gets dropped
        let inject_skills: Vec<&ResolvedSkill> = resolved
            .iter()
            .filter(|r| r.delivery == DeliveryMode::Inject)
            .collect();
        assert_eq!(inject_skills.len(), 2);
        let names: Vec<&str> = inject_skills
            .iter()
            .map(|r| r.definition.file_name.as_str())
            .collect();
        // Low priority should be dropped
        assert!(!names.contains(&"low-pri"));
        assert!(names.contains(&"high-pri"));
        assert!(names.contains(&"normal-pri"));
    }

    #[test]
    fn test_trigger_always() {
        let trigger = TriggerCondition {
            tools: vec![],
            pattern: None,
            keyword: None,
            always: true,
            manual: false,
        };
        assert!(trigger_matches(&trigger, &[], &[], ""));
        assert!(trigger_matches(&trigger, &[], &[], "anything"));
    }

    #[test]
    fn test_trigger_manual_never_auto() {
        let trigger = TriggerCondition {
            tools: vec!["read_file".into()],
            pattern: None,
            keyword: Some("review".into()),
            always: false,
            manual: true,
        };
        // Even with matching tools and keyword, manual should never fire.
        assert!(!trigger_matches(
            &trigger,
            &["read_file".into()],
            &[],
            "review this code",
        ));
    }

    #[test]
    fn test_resolve_skills_works_with_directory_skills() {
        let dir = tempfile::tempdir().unwrap();

        let skill_dir = dir.path().join("always-dir");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: always-dir\ndescription: Always fires\ntriggers:\n  - always: true\n---\nMain.",
        )
        .unwrap();
        std::fs::write(skill_dir.join("ref.md"), "Reference.").unwrap();

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let ctx = default_ctx();

        let resolved = resolve_skills(&registry, &ctx);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].definition.file_name, "always-dir");
        assert!(resolved[0].definition.is_directory);
        assert!(resolved[0].definition.body.contains("Main."));
        assert!(resolved[0].definition.body.contains("Reference."));
    }

    #[test]
    fn test_auto_mode_below_threshold_is_inject() {
        let dir = tempfile::tempdir().unwrap();
        // max_tokens=400 (at threshold) → inject
        write_skill(dir.path(), "short", "max_tokens: 400\n");

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let mut ctx = default_ctx();
        ctx.agent_skill_names = vec!["short".into()];

        let resolved = resolve_skills(&registry, &ctx);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].delivery, DeliveryMode::Inject);
    }

    #[test]
    fn test_auto_mode_above_threshold_is_reference() {
        let dir = tempfile::tempdir().unwrap();
        // max_tokens=401 (above threshold) → reference
        write_skill(dir.path(), "long", "max_tokens: 401\n");

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let mut ctx = default_ctx();
        ctx.agent_skill_names = vec!["long".into()];

        let resolved = resolve_skills(&registry, &ctx);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].delivery, DeliveryMode::Reference);
    }

    #[test]
    fn test_explicit_inject_mode_overrides_auto() {
        let dir = tempfile::tempdir().unwrap();
        // Large token budget but explicitly inject mode
        write_skill(
            dir.path(),
            "forced-inject",
            "mode: inject\nmax_tokens: 5000\n",
        );

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let mut ctx = default_ctx();
        ctx.agent_skill_names = vec!["forced-inject".into()];

        let resolved = resolve_skills(&registry, &ctx);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].delivery, DeliveryMode::Inject);
    }

    #[test]
    fn test_explicit_reference_mode_overrides_auto() {
        let dir = tempfile::tempdir().unwrap();
        // Small token budget but explicitly reference mode
        write_skill(
            dir.path(),
            "forced-ref",
            "mode: reference\nmax_tokens: 100\n",
        );

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let mut ctx = default_ctx();
        ctx.agent_skill_names = vec!["forced-ref".into()];

        let resolved = resolve_skills(&registry, &ctx);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].delivery, DeliveryMode::Reference);
    }

    #[test]
    fn test_reference_skills_dont_count_against_budget() {
        let dir = tempfile::tempdir().unwrap();
        // Two reference skills that would bust the budget if counted
        write_skill(
            dir.path(),
            "ref1",
            "mode: reference\nmax_tokens: 5000\ntriggers:\n  - always: true\n",
        );
        write_skill(
            dir.path(),
            "ref2",
            "mode: reference\nmax_tokens: 5000\ntriggers:\n  - always: true\n",
        );
        // One inject skill within budget
        write_skill(
            dir.path(),
            "inj1",
            "mode: inject\nmax_tokens: 500\ntriggers:\n  - always: true\n",
        );

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let mut ctx = default_ctx();
        ctx.token_budget = 1000; // Only 1000 tokens for inject

        let resolved = resolve_skills(&registry, &ctx);
        // All three should be present: 2 reference + 1 inject
        assert_eq!(resolved.len(), 3);
        let inject_count = resolved
            .iter()
            .filter(|r| r.delivery == DeliveryMode::Inject)
            .count();
        let ref_count = resolved
            .iter()
            .filter(|r| r.delivery == DeliveryMode::Reference)
            .count();
        assert_eq!(inject_count, 1);
        assert_eq!(ref_count, 2);
    }

    #[test]
    fn test_budget_only_drops_inject_skills() {
        let dir = tempfile::tempdir().unwrap();
        // Three inject skills, budget only fits 2
        write_skill(
            dir.path(),
            "high",
            "mode: inject\npriority: high\nmax_tokens: 500\n",
        );
        write_skill(
            dir.path(),
            "low",
            "mode: inject\npriority: low\nmax_tokens: 500\n",
        );
        write_skill(
            dir.path(),
            "normal",
            "mode: inject\npriority: normal\nmax_tokens: 500\n",
        );
        // One reference skill that should never be dropped
        write_skill(
            dir.path(),
            "ref-big",
            "mode: reference\nmax_tokens: 99999\n",
        );

        let registry = SkillRegistry::new(dir.path().to_path_buf());
        let mut ctx = default_ctx();
        ctx.agent_skill_names = vec![
            "high".into(),
            "low".into(),
            "normal".into(),
            "ref-big".into(),
        ];
        ctx.token_budget = 1000; // Room for 2 inject skills

        let resolved = resolve_skills(&registry, &ctx);
        let names: Vec<&str> = resolved
            .iter()
            .map(|r| r.definition.file_name.as_str())
            .collect();
        // Low priority inject should be dropped
        assert!(!names.contains(&"low"));
        // High and normal inject should remain
        assert!(names.contains(&"high"));
        assert!(names.contains(&"normal"));
        // Reference skill always remains
        assert!(names.contains(&"ref-big"));
    }
}
