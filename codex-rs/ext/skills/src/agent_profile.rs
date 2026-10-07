//! Applies the Team profile after all skill roots/providers have been discovered.

use std::collections::HashSet;
use std::sync::Arc;

use crate::SkillLoadOutcome;
use crate::catalog::SkillCatalog;

fn parse_selection(raw: Option<&str>) -> Result<Option<HashSet<String>>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let names: Vec<String> = serde_json::from_str(raw)
        .map_err(|_| "ILHAE_AGENT_SKILLS must be a JSON array of skill names".to_string())?;
    if names
        .iter()
        .any(|name| name.is_empty() || name.trim() != name)
    {
        return Err("ILHAE_AGENT_SKILLS contains an empty or padded skill name".into());
    }
    Ok(Some(
        names.into_iter().map(|name| name.to_lowercase()).collect(),
    ))
}

fn selection_from_environment() -> Result<Option<HashSet<String>>, String> {
    match std::env::var("ILHAE_AGENT_SKILLS") {
        Ok(raw) => parse_selection(Some(&raw)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("ILHAE_AGENT_SKILLS must be UTF-8 JSON".into())
        }
    }
}

pub(crate) fn apply_to_catalog(mut catalog: SkillCatalog) -> SkillCatalog {
    match selection_from_environment() {
        Ok(None) => {}
        Ok(Some(names)) => catalog
            .entries
            .retain(|entry| names.contains(&entry.name.to_lowercase())),
        Err(message) => {
            catalog.entries.clear();
            catalog.warnings.push(message);
        }
    }
    catalog
}

pub(crate) fn apply_to_outcome(outcome: SkillLoadOutcome) -> SkillLoadOutcome {
    restrict_outcome(outcome, selection_from_environment())
}

fn restrict_outcome(
    mut outcome: SkillLoadOutcome,
    selection: Result<Option<HashSet<String>>, String>,
) -> SkillLoadOutcome {
    match selection {
        Ok(None) => return outcome,
        Ok(Some(names)) => outcome
            .skills
            .retain(|skill| names.contains(&skill.name.to_lowercase())),
        Err(message) => {
            tracing::error!("{message}");
            if let Some(path) = outcome
                .skills
                .first()
                .map(|skill| skill.path_to_skills_md.clone())
            {
                outcome
                    .errors
                    .push(codex_skills::SkillError { path, message });
            }
            outcome.skills.clear();
        }
    }
    let paths: HashSet<_> = outcome
        .skills
        .iter()
        .map(|skill| skill.path_to_skills_md.clone())
        .collect();
    Arc::make_mut(&mut outcome.skill_root_by_path).retain(|path, _| paths.contains(path));
    Arc::make_mut(&mut outcome.skill_discovery_path_by_path).retain(|path, _| paths.contains(path));
    outcome
        .agent_plugin_skill_paths
        .retain(|path| paths.contains(path));
    Arc::make_mut(&mut outcome.implicit_skills_by_scripts_dir)
        .retain(|_, skill| paths.contains(&skill.path_to_skills_md));
    Arc::make_mut(&mut outcome.implicit_skills_by_doc_path)
        .retain(|_, skill| paths.contains(&skill.path_to_skills_md));
    outcome
}

#[cfg(test)]
#[path = "agent_profile_tests.rs"]
mod tests;
