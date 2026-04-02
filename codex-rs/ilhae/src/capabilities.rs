use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EngineCapabilityProfile {
    pub engine: String,
    pub supports_interrupt: bool,
    pub supports_steer: bool,
    pub supports_resume: bool,
    pub supports_fork: bool,
    pub supports_rich_reasoning_deltas: bool,
    pub supports_tool_approval: bool,
    pub supports_terminal_streaming: bool,
    pub supports_realtime_audio: bool,
    pub supports_a2a_facade: bool,
    pub supports_native_app_server: bool,
    pub supports_global_session_mapping: bool,
}

impl EngineCapabilityProfile {
    fn new(engine: &str) -> Self {
        Self {
            engine: engine.to_string(),
            supports_interrupt: false,
            supports_steer: false,
            supports_resume: false,
            supports_fork: false,
            supports_rich_reasoning_deltas: false,
            supports_tool_approval: false,
            supports_terminal_streaming: false,
            supports_realtime_audio: false,
            supports_a2a_facade: true,
            supports_native_app_server: false,
            supports_global_session_mapping: true,
        }
    }
}

pub fn engine_capability_profile(engine_id: &str) -> EngineCapabilityProfile {
    let normalized = engine_id.trim().to_ascii_lowercase();

    if crate::helpers::is_ilhae_native_agent_id(&normalized) {
        return EngineCapabilityProfile {
            engine: crate::helpers::ILHAE_AGENT_ID.to_string(),
            supports_interrupt: true,
            supports_steer: true,
            supports_resume: true,
            supports_fork: true,
            supports_rich_reasoning_deltas: true,
            supports_tool_approval: true,
            supports_terminal_streaming: true,
            supports_realtime_audio: true,
            supports_a2a_facade: true,
            supports_native_app_server: true,
            supports_global_session_mapping: true,
        };
    }

    match normalized.as_str() {
        "gemini" => EngineCapabilityProfile {
            engine: "gemini".to_string(),
            supports_interrupt: true,
            supports_steer: false,
            supports_resume: true,
            supports_fork: false,
            supports_rich_reasoning_deltas: false,
            supports_tool_approval: true,
            supports_terminal_streaming: false,
            supports_realtime_audio: false,
            supports_a2a_facade: true,
            supports_native_app_server: false,
            supports_global_session_mapping: true,
        },
        "claude" => EngineCapabilityProfile {
            engine: "claude".to_string(),
            supports_interrupt: true,
            supports_steer: false,
            supports_resume: true,
            supports_fork: false,
            supports_rich_reasoning_deltas: false,
            supports_tool_approval: true,
            supports_terminal_streaming: false,
            supports_realtime_audio: false,
            supports_a2a_facade: true,
            supports_native_app_server: false,
            supports_global_session_mapping: true,
        },
        other => {
            let mut profile = EngineCapabilityProfile::new(other);
            profile.supports_resume = true;
            profile
        }
    }
}

pub fn engine_capability_matrix() -> Vec<EngineCapabilityProfile> {
    vec![
        engine_capability_profile(crate::helpers::ILHAE_AGENT_ID),
        EngineCapabilityProfile {
            engine: crate::helpers::LEGACY_CODEX_AGENT_ID.to_string(),
            ..engine_capability_profile(crate::helpers::ILHAE_AGENT_ID)
        },
        engine_capability_profile("gemini"),
        engine_capability_profile("claude"),
    ]
}

pub fn engine_capability_profile_json(engine_id: &str) -> Value {
    serde_json::to_value(engine_capability_profile(engine_id)).unwrap_or_else(|_| json!({}))
}

pub fn engine_capability_matrix_json() -> Value {
    serde_json::to_value(engine_capability_matrix()).unwrap_or_else(|_| json!([]))
}

pub fn toggle_skill(name: &str, disabled: bool, agent_id: Option<&str>) -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let settings_path = crate::config::resolve_ilhae_data_dir()
        .join("brain")
        .join("settings")
        .join("app_settings.json");

    let mut val = if let Ok(content) = fs::read_to_string(&settings_path) {
        serde_json::from_str::<Value>(&content).unwrap_or(json!({}))
    } else {
        json!({})
    };

    if !val.is_object() {
        val = json!({});
    }

    if let Some(agent) = agent_id {
        // Team mode per-agent override
        let mut disabled_list: Vec<String> = val
            .pointer(&format!(
                "/agent/team_agent_disabled_capabilities/{}/skills",
                agent
            ))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        if disabled {
            if !disabled_list.contains(&name.to_string()) {
                disabled_list.push(name.to_string());
            }
        } else {
            disabled_list.retain(|x| x != name);
        }

        let agent_obj = val
            .as_object_mut()
            .unwrap()
            .entry("agent")
            .or_insert(json!({}))
            .as_object_mut()
            .unwrap();
        let team_obj = agent_obj
            .entry("team_agent_disabled_capabilities")
            .or_insert(json!({}))
            .as_object_mut()
            .unwrap();
        let role_obj = team_obj
            .entry(agent)
            .or_insert(json!({}))
            .as_object_mut()
            .unwrap();
        role_obj.insert("skills".to_string(), json!(disabled_list));

        let json_str = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
        fs::write(settings_path, json_str).map_err(|e| e.to_string())?;
        return Ok(());
    }

    // Global override (fallback if no agent specified)
    let gemini_settings_path = PathBuf::from(&home)
        .join(".gemini")
        .join("brain")
        .join("settings")
        .join("app_settings.json");
    let mut gval = if let Ok(content) = fs::read_to_string(&gemini_settings_path) {
        serde_json::from_str::<Value>(&content).unwrap_or(json!({}))
    } else {
        json!({})
    };

    let mut disabled_list: Vec<String> = gval
        .pointer("/skills/disabled")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    if disabled {
        if !disabled_list.contains(&name.to_string()) {
            disabled_list.push(name.to_string());
        }
    } else {
        disabled_list.retain(|x| x != name);
    }

    if let Some(obj) = gval.as_object_mut() {
        if !obj.contains_key("skills") {
            obj.insert("skills".to_string(), json!({}));
        }
        if let Some(skills_obj) = obj.get_mut("skills").and_then(|v| v.as_object_mut()) {
            skills_obj.insert("disabled".to_string(), json!(disabled_list));
        }
    }

    let json_str = serde_json::to_string_pretty(&gval).map_err(|e| e.to_string())?;
    fs::write(gemini_settings_path, json_str).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn toggle_mcp(name: &str, disabled: bool, agent_id: Option<&str>) -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());

    if let Some(agent) = agent_id {
        // Team mode per-agent override
        let settings_path = crate::config::resolve_ilhae_data_dir()
            .join("brain")
            .join("settings")
            .join("app_settings.json");
        let mut val = if let Ok(content) = fs::read_to_string(&settings_path) {
            serde_json::from_str::<Value>(&content).unwrap_or(json!({}))
        } else {
            json!({})
        };
        if !val.is_object() {
            val = json!({});
        }

        let mut disabled_list: Vec<String> = val
            .pointer(&format!(
                "/agent/team_agent_disabled_capabilities/{}/mcps",
                agent
            ))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        if disabled {
            if !disabled_list.contains(&name.to_string()) {
                disabled_list.push(name.to_string());
            }
        } else {
            disabled_list.retain(|x| x != name);
        }

        let agent_obj = val
            .as_object_mut()
            .unwrap()
            .entry("agent")
            .or_insert(json!({}))
            .as_object_mut()
            .unwrap();
        let team_obj = agent_obj
            .entry("team_agent_disabled_capabilities")
            .or_insert(json!({}))
            .as_object_mut()
            .unwrap();
        let role_obj = team_obj
            .entry(agent)
            .or_insert(json!({}))
            .as_object_mut()
            .unwrap();
        role_obj.insert("mcps".to_string(), json!(disabled_list));

        let json_str = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
        fs::write(settings_path, json_str).map_err(|e| e.to_string())?;
        return Ok(());
    }

    // Global override (fallback if no agent specified)
    let enable_path = PathBuf::from(&home)
        .join(".gemini")
        .join("mcp-server-enablement.json");
    let mut val = if let Ok(content) = fs::read_to_string(&enable_path) {
        serde_json::from_str::<Value>(&content).unwrap_or(json!({}))
    } else {
        json!({})
    };

    if !val.is_object() {
        val = json!({});
    }

    if let Some(obj) = val.as_object_mut() {
        if !obj.contains_key(name) {
            obj.insert(name.to_string(), json!({}));
        }
        if let Some(mcp_obj) = obj.get_mut(name).and_then(|v| v.as_object_mut()) {
            mcp_obj.insert("enabled".to_string(), json!(!disabled));
        }
    }

    let json_str = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
    fs::write(enable_path, json_str).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn read_gemini_capabilities() -> (Vec<Value>, Vec<Value>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());

    // 1. Read disabled skills & MCPs
    let settings_path = PathBuf::from(&home)
        .join(".gemini")
        .join("brain")
        .join("settings")
        .join("app_settings.json");
    let mut disabled_skills = Vec::new();
    let mut mcps = Vec::new();

    if let Ok(content) = fs::read_to_string(&settings_path) {
        if let Ok(val) = serde_json::from_str::<Value>(&content) {
            // parse disabled skills
            if let Some(ds) = val.pointer("/skills/disabled").and_then(|v| v.as_array()) {
                for s in ds {
                    if let Some(s_str) = s.as_str() {
                        disabled_skills.push(s_str.to_lowercase());
                    }
                }
            }
            // parse mcp servers
            if let Some(servers) = val.get("mcpServers").and_then(|v| v.as_object()) {
                for (name, cfg) in servers {
                    let desc = cfg
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("MCP Server")
                        .to_string();
                    mcps.push(json!({
                        "name": name,
                        "description": desc,
                        "disabled": false
                    }));
                }
            }
        }
    }

    // Read mcp-server-enablement.json
    let mcp_enable_path = PathBuf::from(&home)
        .join(".gemini")
        .join("mcp-server-enablement.json");
    if let Ok(content) = fs::read_to_string(&mcp_enable_path) {
        if let Ok(val) = serde_json::from_str::<Value>(&content) {
            if let Some(obj) = val.as_object() {
                for mcp in mcps.iter_mut() {
                    let name = mcp["name"].as_str().unwrap();
                    if let Some(state) = obj.get(name).and_then(|v| v.as_object()) {
                        if let Some(enabled) = state.get("enabled").and_then(|v| v.as_bool()) {
                            mcp["disabled"] = json!(!enabled);
                        }
                    }
                }
            }
        }
    }

    // 2. Read Skills — brain/skills/ is the single source of truth
    let mut skills = Vec::new();
    let mut seen_names = std::collections::HashSet::<String>::new();
    let brain_skills_dir = crate::config::get_active_vault_dir().join("skills");
    let _ = fs::create_dir_all(&brain_skills_dir);
    for entry in walkdir::WalkDir::new(&brain_skills_dir)
        .min_depth(1)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_dir())
    {
        let path = entry.path();
        let md_path = path.join("SKILL.md");
        if !md_path.exists() {
            continue;
        }

        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if seen_names.contains(&name) {
            continue;
        }

        // Determine source from relative path (e.g., "3rdparty/gemini/skill-name")
        let rel = path
            .strip_prefix(&brain_skills_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let source = if rel.starts_with("3rdparty") {
            "3rdparty"
        } else {
            "brain"
        };
        let is_builtin = rel.starts_with("3rdparty");

        let mut desc = "Brain skill".to_string();
        if let Ok(content) = fs::read_to_string(&md_path) {
            if content.starts_with("---") {
                if let Some(end_idx) = content[3..].find("---") {
                    let fm = &content[3..3 + end_idx];
                    for line in fm.lines() {
                        if line.starts_with("description:") {
                            desc = line.trim_start_matches("description:").trim().to_string();
                            break;
                        }
                    }
                }
            }
        }

        let is_disabled = disabled_skills.contains(&name.to_lowercase());
        seen_names.insert(name.clone());

        skills.push(json!({
            "name": name,
            "description": desc,
            "location": md_path.to_string_lossy().to_string(),
            "disabled": is_disabled,
            "isBuiltin": is_builtin,
            "source": source
        }));
    }

    (skills, mcps)
}

/// Sync ACP-reported skills into ~/ilhae/brain/skills/ for persistence.
/// Builtin skills go under `3rdparty/gemini/`, custom skills go under `custom/`.
pub fn sync_acp_skills_to_brain(skills: &[serde_json::Value]) {
    let brain_skills_dir = crate::config::get_active_vault_dir().join("skills");
    let gemini_dir = brain_skills_dir.join("3rdparty").join("gemini");
    let custom_dir = brain_skills_dir.join("custom");
    let _ = fs::create_dir_all(&gemini_dir);
    let _ = fs::create_dir_all(&custom_dir);

    for skill in skills {
        let name = match skill.get("name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => continue,
        };
        let desc = skill
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("ACP skill");
        let is_builtin = skill
            .get("isBuiltin")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let target_dir = if is_builtin { &gemini_dir } else { &custom_dir };
        let skill_dir = target_dir.join(name);
        let skill_md = skill_dir.join("SKILL.md");

        // Only write if directory doesn't exist yet (don't overwrite user edits)
        if !skill_dir.exists() {
            let _ = fs::create_dir_all(&skill_dir);
            let source_label = if is_builtin { "gemini-cli" } else { "acp" };
            let content = format!(
                "---\ndescription: {}\nsource: {}\nbuiltin: {}\n---\n\n# {}\n\n{}\n",
                desc, source_label, is_builtin, name, desc
            );
            let _ = fs::write(&skill_md, content);
        }
    }
}

/// Copy Gemini CLI built-in skills from source into brain/skills/3rdparty/gemini/
pub fn sync_gemini_builtin_skills_from_source() {
    // Try to find gemini-cli skills in the source tree
    let candidates = [
        // Dev: monorepo source
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("gemini-cli")
            .join("packages")
            .join("core")
            .join("src")
            .join("skills")
            .join("builtin"),
        // Fallback: ~/.gemini/skills
        dirs::home_dir()
            .unwrap_or_default()
            .join(".gemini")
            .join("skills"),
    ];

    let brain_gemini_dir = crate::config::get_active_vault_dir()
        .join("skills")
        .join("3rdparty")
        .join("gemini");
    let _ = fs::create_dir_all(&brain_gemini_dir);

    for candidate in &candidates {
        if !candidate.exists() {
            continue;
        }
        if let Ok(entries) = fs::read_dir(candidate) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let src_skill_md = path.join("SKILL.md");
                let dest_dir = brain_gemini_dir.join(&name);
                let dest_skill_md = dest_dir.join("SKILL.md");

                if src_skill_md.exists() && !dest_dir.exists() {
                    let _ = fs::create_dir_all(&dest_dir);
                    let _ = fs::copy(&src_skill_md, &dest_skill_md);
                }
            }
        }
    }
}
