//! Superpowers Skills Provisioner
//!
//! Provisions the 5 core Superpowers skills as SKILL.md files in `brain/skills/`.
//! - ACP mode: `capabilities.rs` walkdir scans `brain/skills/` directly
//! - A2A mode: `add_brain_directories` adds brain dir to agents → `skillManager.ts` extraDirs
//!
//! Based on <https://github.com/obra/superpowers> (MIT License).

use std::fs;
use tracing::{info, warn};

/// (skill_dir_name, SKILL.md content)
const SUPERPOWERS_SKILLS: &[(&str, &str)] = &[
    (
        "brainstorming",
        include_str!("superpowers/brainstorming.md"),
    ),
    (
        "writing-plans",
        include_str!("superpowers/writing-plans.md"),
    ),
    (
        "executing-plans",
        include_str!("superpowers/executing-plans.md"),
    ),
    (
        "verification-before-completion",
        include_str!("superpowers/verification-before-completion.md"),
    ),
    (
        "subagent-driven-development",
        include_str!("superpowers/subagent-driven-development.md"),
    ),
];

/// Provision Superpowers skill files into `brain/skills/` (the active vault).
/// Skips files that already exist (preserves user customizations).
pub fn provision_superpowers_skills() {
    let skills_dir = crate::config::get_active_vault_dir().join("skills");

    for (name, content) in SUPERPOWERS_SKILLS {
        let skill_dir = skills_dir.join(name);
        let skill_file = skill_dir.join("SKILL.md");

        // Don't overwrite user edits
        if skill_file.exists() {
            continue;
        }

        if let Err(e) = fs::create_dir_all(&skill_dir) {
            warn!("[Superpowers] Failed to create dir {:?}: {}", skill_dir, e);
            continue;
        }

        match fs::write(&skill_file, content) {
            Ok(()) => info!("[Superpowers] Provisioned {:?}", skill_file),
            Err(e) => warn!("[Superpowers] Failed to write {:?}: {}", skill_file, e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_contents_are_valid() {
        for (name, content) in SUPERPOWERS_SKILLS {
            assert!(!content.is_empty(), "Skill {} has empty content", name);
            assert!(
                content.contains("---"),
                "Skill {} missing frontmatter delimiter",
                name
            );
            assert!(
                content.contains("description:"),
                "Skill {} missing description in frontmatter",
                name
            );
        }
    }
}
