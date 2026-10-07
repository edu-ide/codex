use super::*;
use codex_protocol::protocol::SkillScope;
use codex_skills::SkillMetadata;
use codex_utils_absolute_path::AbsolutePathBuf;

fn discovered_skills() -> SkillLoadOutcome {
    let root = std::env::current_dir().unwrap();
    SkillLoadOutcome {
        skills: [
            ("writer", SkillScope::User),
            ("reviewer", SkillScope::Repo),
            ("builtin", SkillScope::System),
        ]
        .into_iter()
        .map(|(name, scope)| SkillMetadata {
            name: name.into(),
            description: name.into(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            path_to_skills_md: AbsolutePathBuf::try_from(root.join(name).join("SKILL.md")).unwrap(),
            scope,
            plugin_id: None,
            remote_plugin_id: None,
        })
        .collect(),
        ..Default::default()
    }
}

#[test]
fn agent_profile_selects_across_repo_user_and_builtin_roots() {
    let outcome = restrict_outcome(discovered_skills(), parse_selection(Some("[\"writer\"]")));
    assert_eq!(
        outcome
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        ["writer"]
    );
    assert!(
        outcome
            .with_disabled_paths(HashSet::new())
            .allowed_skills_for_implicit_invocation()
            .iter()
            .all(|s| s.name == "writer")
    );
}

#[test]
fn missing_profile_preserves_existing_skills_but_empty_profile_removes_them() {
    assert_eq!(
        restrict_outcome(discovered_skills(), parse_selection(None))
            .skills
            .len(),
        3
    );
    assert!(
        restrict_outcome(discovered_skills(), parse_selection(Some("[]")))
            .skills
            .is_empty()
    );
}

#[test]
fn invalid_profile_never_falls_back_to_unrestricted_skills() {
    for raw in ["{}", "[3]", "[\" \"]", "invalid"] {
        let outcome = restrict_outcome(discovered_skills(), parse_selection(Some(raw)));
        assert!(outcome.skills.is_empty());
        assert_eq!(outcome.errors.len(), 1);
    }
}
