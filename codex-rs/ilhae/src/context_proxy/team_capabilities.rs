//! Resolve a role's capabilities and materialize private engine configuration.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use ilhae_common::agent_profiles::AgentProfileStore;
use serde_json::Value;
use serde_json::json;

use super::TeamRoleTarget;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentCapabilities {
    pub skills: Option<Vec<String>>,
    pub mcp_servers: Option<Vec<String>>,
    pub disabled_skills: Vec<String>,
    pub disabled_mcps: Vec<String>,
}

pub fn resolve_agent_capabilities(
    data_dir: &Path,
    agent: &TeamRoleTarget,
) -> Result<AgentCapabilities, String> {
    if let Some(profile) = AgentProfileStore::new(data_dir).resolve(&agent.role)? {
        return Ok(AgentCapabilities {
            skills: Some(profile.skills),
            mcp_servers: Some(profile.mcp_servers),
            ..Default::default()
        });
    }
    let settings = read_json(&data_dir.join("brain/settings/app_settings.json"))?;
    let disabled = settings
        .pointer("/agent/team_agent_disabled_capabilities")
        .and_then(Value::as_object)
        .and_then(|roles| {
            roles
                .iter()
                .find(|(role, _)| role.eq_ignore_ascii_case(&agent.role))
                .map(|(_, value)| value)
        });
    let read_names = |key: &str| {
        disabled
            .and_then(|value| value.get(key))
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(AgentCapabilities {
        skills: (!agent.skills.is_empty()).then(|| agent.skills.clone()),
        mcp_servers: (!agent.mcp_servers.is_empty()).then(|| agent.mcp_servers.clone()),
        disabled_skills: read_names("skills"),
        disabled_mcps: read_names("mcps"),
    })
}

fn read_json(path: &Path) -> Result<Value, String> {
    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content)
            .map_err(|error| format!("Invalid capability settings {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(format!("Cannot read {}: {error}", path.display())),
    }
}

fn reset_owned_directory(path: &Path) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        } else {
            fs::remove_dir_all(path).map_err(|error| error.to_string())?;
        }
    }
    fs::create_dir_all(path).map_err(|error| error.to_string())
}

fn write_owned_file(path: &Path, content: &str) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    fs::write(path, content).map_err(|error| format!("Cannot write {}: {error}", path.display()))
}

fn skill_catalog(roots: &[PathBuf]) -> BTreeMap<String, PathBuf> {
    let mut catalog = BTreeMap::new();
    for root in roots {
        for entry in walkdir::WalkDir::new(root)
            .follow_links(true)
            .sort_by_file_name()
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_name() == "SKILL.md"
                && let Some(dir) = entry.path().parent()
                && let Ok(name) = ilhae_common::agent_profiles::skill_name(entry.path())
            {
                catalog.entry(name).or_insert_with(|| dir.to_path_buf());
            }
        }
    }
    catalog
}

fn copy_skill(source: &Path, target: &Path) -> Result<(), String> {
    for entry in walkdir::WalkDir::new(source).follow_links(true) {
        let entry = entry.map_err(|error| error.to_string())?;
        let destination = target.join(
            entry
                .path()
                .strip_prefix(source)
                .map_err(|error| error.to_string())?,
        );
        if entry.file_type().is_dir() {
            fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
        } else if entry.file_type().is_file() {
            fs::copy(entry.path(), &destination).map_err(|error| error.to_string())?;
        }
    }
    let definition =
        ilhae_common::agent_profiles::normalized_skill_document(&source.join("SKILL.md"))?;
    fs::write(target.join("SKILL.md"), definition).map_err(|error| error.to_string())?;
    Ok(())
}

/// Rebuild managed capability files on every launch so removed selections cannot linger.
/// The source home and shared catalog are read-only; coordination MCP is infrastructure.
pub fn prepare_agent_capabilities(
    data_dir: &Path,
    source_home: &Path,
    workspace: &Path,
    agent: &TeamRoleTarget,
    team_mcp_bin: &str,
) -> Result<AgentCapabilities, String> {
    let mut selected = resolve_agent_capabilities(data_dir, agent)?;
    let catalog = skill_catalog(&[
        data_dir.join("brain/skills"),
        source_home.join(".agents/skills"),
        source_home.join(".gemini/skills"),
        source_home.join(".codex/skills"),
    ]);
    if selected.skills.is_none() && !selected.disabled_skills.is_empty() {
        selected.skills = Some(catalog.keys().cloned().collect());
    }
    if let Some(names) = selected.skills.as_mut() {
        names.retain(|name| !selected.disabled_skills.contains(name));
    }
    let agent_skills = workspace.join(".agents/skills");
    reset_owned_directory(&workspace.join(".agents"))?;
    fs::create_dir_all(&agent_skills).map_err(|error| error.to_string())?;
    reset_owned_directory(&workspace.join(".gemini/skills"))?;
    reset_owned_directory(&workspace.join("skills"))?;
    let names = if let Some(names) = &selected.skills {
        names.clone()
    } else if agent.is_main {
        Vec::new()
    } else {
        skill_catalog(&[source_home.join(".agents/skills")])
            .keys()
            .cloned()
            .collect()
    };
    let mut effective_names = Vec::new();
    for requested in names {
        // Frontmatter names are canonical; folder names remain accepted for legacy role lists.
        let resolved = if let Some(source) = catalog.get(&requested) {
            Some((&requested, source))
        } else {
            let mut aliases = catalog.iter().filter(|(_, source)| {
                source.file_name().and_then(|name| name.to_str()) == Some(requested.as_str())
            });
            let resolved = aliases.next();
            if aliases.next().is_some() {
                return Err(format!("Ambiguous legacy skill folder: {requested}"));
            }
            resolved
        };
        let (name, source) =
            resolved.ok_or_else(|| format!("Unknown selected skill: {requested}"))?;
        if !selected.disabled_skills.contains(name) && !effective_names.contains(name) {
            copy_skill(source, &agent_skills.join(name))?;
            effective_names.push(name.clone());
        }
    }
    if selected.skills.is_some() {
        selected.skills = Some(effective_names);
    }

    let mut gemini = read_json(&source_home.join(".gemini/settings.json"))?;
    if !gemini.is_object() {
        return Err("Gemini settings must be an object".to_owned());
    }
    let legacy = read_json(&source_home.join(".gemini/brain/settings/app_settings.json"))?;
    let codex_path = source_home.join(".codex/config.toml");
    let mut codex: toml::Value = match fs::read_to_string(&codex_path) {
        Ok(content) => {
            toml::from_str(&content).map_err(|error| format!("Invalid Codex settings: {error}"))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(Default::default())
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut servers = BTreeMap::new();
    for source in [&legacy, &gemini] {
        if let Some(entries) = source.get("mcpServers").and_then(Value::as_object) {
            servers.extend(
                entries
                    .iter()
                    .map(|(name, config)| (name.clone(), config.clone())),
            );
        }
    }
    if let Some(entries) = codex.get("mcp_servers").and_then(toml::Value::as_table) {
        for (name, config) in entries {
            let mut config = serde_json::to_value(config).map_err(|error| error.to_string())?;
            if let Some(url) = config.get("url").cloned() {
                config["httpUrl"] = url;
            }
            servers.entry(name.clone()).or_insert(config);
        }
    }
    let mut enabled = serde_json::Map::new();
    if agent.role.eq_ignore_ascii_case("user_agent") {
        selected.skills = Some(Vec::new());
    } else {
        for name in selected.mcp_servers.as_deref().unwrap_or_default() {
            if selected.disabled_mcps.contains(name) || name == "ilhae-tools" {
                continue;
            }
            let config = servers
                .get(name)
                .ok_or_else(|| format!("Unknown selected MCP server: {name}"))?;
            enabled.insert(name.clone(), config.clone());
        }
        enabled.insert(
            "ilhae-tools".to_owned(),
            json!({
                "command": team_mcp_bin, "args": [], "trust": true,
                "env": { "ILHAE_DIR": data_dir.to_string_lossy() }
            }),
        );
    }
    selected.mcp_servers = Some(enabled.keys().cloned().collect());
    gemini["mcpServers"] = json!(enabled);
    gemini["skills"] = json!({ "disabled": selected.disabled_skills });
    write_owned_file(
        &workspace.join(".gemini/settings.json"),
        &serde_json::to_string_pretty(&gemini).map_err(|error| error.to_string())?,
    )?;
    let enablement: serde_json::Map<String, Value> = enabled
        .keys()
        .map(|name| (name.clone(), json!({"enabled":true})))
        .collect();
    write_owned_file(
        &workspace.join(".gemini/mcp-server-enablement.json"),
        &serde_json::to_string_pretty(&enablement).map_err(|error| error.to_string())?,
    )?;
    let table = codex
        .as_table_mut()
        .ok_or("Codex settings must be a table")?;
    table.remove("mcp_servers");
    // Profile-level MCP tables must not bypass the role's explicit selection.
    if let Some(profiles) = table
        .get_mut("profiles")
        .and_then(toml::Value::as_table_mut)
    {
        for profile in profiles
            .iter_mut()
            .filter_map(|(_, value)| value.as_table_mut())
        {
            profile.remove("mcp_servers");
        }
    }
    let mut codex_servers = toml::map::Map::new();
    for (name, config) in enabled {
        let mut config = config;
        if let Some(config) = config.as_object_mut() {
            if let Some(url) = config.remove("httpUrl").or_else(|| config.remove("url")) {
                config.insert("url".to_owned(), url);
            }
            for field in [
                "trust",
                "description",
                "timeout",
                "includeTools",
                "excludeTools",
            ] {
                config.remove(field);
            }
            config.insert("enabled".to_owned(), json!(true));
        }
        codex_servers.insert(
            name,
            serde_json::from_value::<toml::Value>(config).map_err(|error| error.to_string())?,
        );
    }
    table.insert("mcp_servers".to_owned(), toml::Value::Table(codex_servers));
    write_owned_file(
        &workspace.join("config.toml"),
        &toml::to_string_pretty(&codex).map_err(|error| error.to_string())?,
    )?;
    Ok(selected)
}

#[cfg(test)]
#[path = "team_capabilities_tests.rs"]
mod tests;
