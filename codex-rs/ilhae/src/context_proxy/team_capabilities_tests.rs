use super::*;
use ilhae_common::agent_profiles::AgentCapabilityProfile;
use pretty_assertions::assert_eq;

fn agent(role: &str) -> TeamRoleTarget {
    TeamRoleTarget {
        role: role.to_owned(),
        endpoint: String::new(),
        system_prompt: String::new(),
        engine: "gemini".to_owned(),
        model: String::new(),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        is_main: false,
    }
}

fn profile(id: &str, skills: &[&str], mcps: &[&str]) -> AgentCapabilityProfile {
    AgentCapabilityProfile {
        id: id.to_owned(),
        name: id.to_owned(),
        description: String::new(),
        skills: skills.iter().map(|name| (*name).to_owned()).collect(),
        mcp_servers: mcps.iter().map(|name| (*name).to_owned()).collect(),
    }
}

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let home = temp.path().join("source-home");
    fs::create_dir_all(home.join(".gemini")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    for name in ["research", "build"] {
        let path = data.join("brain/skills/custom").join(name);
        fs::create_dir_all(path.join("scripts")).unwrap();
        fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test\n---\n{name}"),
        )
        .unwrap();
        fs::write(path.join("scripts/tool.py"), "print('tool')").unwrap();
    }
    fs::write(
        home.join(".gemini/settings.json"),
        r#"{"mcpServers":{"docs":{"command":"docs"},"git":{"command":"git"}}}"#,
    )
    .unwrap();
    fs::write(home.join(".codex/config.toml"), "model = 'configured'\n[mcp_servers.unselected]\ncommand = 'unselected'\n[profiles.custom.mcp_servers.leak]\ncommand = 'leak'\n").unwrap();
    (temp, data, home)
}

#[test]
fn selected_capabilities_are_isolated_and_rebuilt_without_changing_sources() {
    let (temp, data, home) = fixture();
    let original_gemini = fs::read(home.join(".gemini/settings.json")).unwrap();
    let original_codex = fs::read(home.join(".codex/config.toml")).unwrap();
    let store = AgentProfileStore::new(&data);
    store
        .save(profile("researcher", &["research"], &["docs"]))
        .unwrap();
    store
        .save(profile("creator", &["build"], &["git"]))
        .unwrap();
    store.assign("Researcher", "researcher").unwrap();
    store.assign("Creator", "creator").unwrap();
    let research_ws = temp.path().join("researcher");
    let creator_ws = temp.path().join("creator");
    prepare_agent_capabilities(
        &data,
        &home,
        &research_ws,
        &agent("researcher"),
        "team-tools",
    )
    .unwrap();
    prepare_agent_capabilities(&data, &home, &creator_ws, &agent("creator"), "team-tools").unwrap();
    let settings = read_json(&research_ws.join(".gemini/settings.json")).unwrap();
    assert_eq!(
        settings["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["docs", "ilhae-tools"]
    );
    assert!(
        research_ws
            .join(".agents/skills/research/scripts/tool.py")
            .is_file()
    );
    assert!(!research_ws.join(".agents/skills/build").exists());
    assert!(creator_ws.join(".agents/skills/build/SKILL.md").is_file());
    assert!(!creator_ws.join(".agents/skills/research").exists());
    let codex: toml::Value =
        toml::from_str(&fs::read_to_string(research_ws.join("config.toml")).unwrap()).unwrap();
    assert_eq!(
        codex["mcp_servers"]
            .as_table()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["docs", "ilhae-tools"]
    );
    assert_eq!(codex["model"].as_str(), Some("configured"));
    assert!(codex["profiles"]["custom"].get("mcp_servers").is_none());
    store.save(profile("researcher", &[], &[])).unwrap();
    prepare_agent_capabilities(
        &data,
        &home,
        &research_ws,
        &agent("researcher"),
        "team-tools",
    )
    .unwrap();
    assert!(!research_ws.join(".agents/skills/research").exists());
    assert_eq!(
        read_json(&research_ws.join(".gemini/settings.json")).unwrap()["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["ilhae-tools"]
    );
    assert_eq!(
        fs::read(home.join(".gemini/settings.json")).unwrap(),
        original_gemini
    );
    assert_eq!(
        fs::read(home.join(".codex/config.toml")).unwrap(),
        original_codex
    );
}

#[test]
fn missing_selections_and_invalid_stores_fail_closed() {
    let (temp, data, home) = fixture();
    let store = AgentProfileStore::new(&data);
    store.save(profile("bad", &["missing"], &[])).unwrap();
    store.assign("Creator", "bad").unwrap();
    assert!(
        prepare_agent_capabilities(
            &data,
            &home,
            &temp.path().join("creator"),
            &agent("creator"),
            "team-tools"
        )
        .unwrap_err()
        .contains("Unknown selected skill")
    );
    store.save(profile("bad", &[], &["missing"])).unwrap();
    assert!(
        prepare_agent_capabilities(
            &data,
            &home,
            &temp.path().join("creator"),
            &agent("creator"),
            "team-tools"
        )
        .unwrap_err()
        .contains("Unknown selected MCP")
    );
    fs::write(data.join("brain/settings/agent_profiles.json"), "invalid").unwrap();
    assert!(resolve_agent_capabilities(&data, &agent("creator")).is_err());
}

#[test]
fn assigned_profiles_override_legacy_and_unassigned_roles_keep_legacy_filters() {
    let (_temp, data, _home) = fixture();
    let store = AgentProfileStore::new(&data);
    store.save(profile("empty", &[], &[])).unwrap();
    store.assign("Creator", "empty").unwrap();
    fs::write(data.join("brain/settings/app_settings.json"), r#"{"agent":{"team_agent_disabled_capabilities":{"RESEARCHER":{"skills":["build"],"mcps":["git"]},"creator":{"skills":["research"]}}}}"#).unwrap();
    assert_eq!(
        resolve_agent_capabilities(&data, &agent("CREATOR")).unwrap(),
        AgentCapabilities {
            skills: Some(Vec::new()),
            mcp_servers: Some(Vec::new()),
            ..Default::default()
        }
    );
    let mut target = agent("Researcher");
    target.skills = vec!["research".to_owned()];
    target.mcp_servers = vec!["docs".to_owned()];
    assert_eq!(
        resolve_agent_capabilities(&data, &target).unwrap(),
        AgentCapabilities {
            skills: Some(vec!["research".to_owned()]),
            mcp_servers: Some(vec!["docs".to_owned()]),
            disabled_skills: vec!["build".to_owned()],
            disabled_mcps: vec!["git".to_owned()]
        }
    );
}

#[cfg(unix)]
#[test]
fn replacing_old_config_symlinks_does_not_mutate_shared_config() {
    let (temp, data, home) = fixture();
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    std::os::unix::fs::symlink(
        home.join(".codex/config.toml"),
        workspace.join("config.toml"),
    )
    .unwrap();
    let original = fs::read(home.join(".codex/config.toml")).unwrap();
    prepare_agent_capabilities(&data, &home, &workspace, &agent("creator"), "team-tools").unwrap();
    assert!(
        !fs::symlink_metadata(workspace.join("config.toml"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(home.join(".codex/config.toml")).unwrap(), original);
}

#[test]
fn frontmatter_names_are_canonical_even_when_folders_differ() {
    let (temp, data, home) = fixture();
    let skill = data.join("brain/skills/custom/research/SKILL.md");
    fs::write(
        &skill,
        "---\nname: actual-research\ndescription: test\n---\ncontent",
    )
    .unwrap();
    let store = AgentProfileStore::new(&data);
    store
        .save(profile("canonical", &["actual-research"], &[]))
        .unwrap();
    store.assign("Researcher", "canonical").unwrap();
    let workspace = temp.path().join("researcher");
    let selected =
        prepare_agent_capabilities(&data, &home, &workspace, &agent("researcher"), "team-tools")
            .unwrap();
    assert_eq!(selected.skills, Some(vec!["actual-research".to_owned()]));
    assert!(
        workspace
            .join(".agents/skills/actual-research/SKILL.md")
            .exists()
    );
    assert!(!workspace.join(".agents/skills/research").exists());
    // Existing folder-based role definitions still resolve to the engine-visible name.
    store.unassign("Researcher").unwrap();
    let mut legacy = agent("researcher");
    legacy.skills = vec!["research".to_owned()];
    let selected =
        prepare_agent_capabilities(&data, &home, &workspace, &legacy, "team-tools").unwrap();
    assert_eq!(selected.skills, Some(vec!["actual-research".to_owned()]));
}

#[test]
fn numeric_mcp_timeouts_remain_loadable_toml() {
    let (temp, data, home) = fixture();
    fs::write(home.join(".gemini/settings.json"), r#"{"mcpServers":{"docs":{"command":"docs","startup_timeout_sec":30,"tool_timeout_sec":60.5}}}"#).unwrap();
    let store = AgentProfileStore::new(&data);
    store.save(profile("researcher", &[], &["docs"])).unwrap();
    store.assign("Researcher", "researcher").unwrap();
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(workspace.join(".gemini")).unwrap();
    prepare_agent_capabilities(&data, &home, &workspace, &agent("Researcher"), "team-mcp").unwrap();
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(workspace.join("config.toml")).unwrap()).unwrap();
    assert_eq!(
        config["mcp_servers"]["docs"]["startup_timeout_sec"].as_integer(),
        Some(30)
    );
    assert_eq!(
        config["mcp_servers"]["docs"]["tool_timeout_sec"].as_float(),
        Some(60.5)
    );
}
