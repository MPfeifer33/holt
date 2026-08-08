pub mod parser;
pub mod registry;
pub mod resolver;
pub mod types;

// Re-export all public items at the module root for API compatibility.
// Callers using `crate::runtime::skills::SkillRegistry` etc. continue to work.
pub use parser::{parse_skill_directory, parse_skill_file};
pub use registry::SkillRegistry;
pub use resolver::resolve_skills;
pub use types::{
    DeliveryMode, ResolvedSkill, SkillContext, SkillDefinition, SkillFrontmatter, SkillMode,
    SkillPriority, SkillSource, TriggerCondition, AUTO_MODE_THRESHOLD,
};
