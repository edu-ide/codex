use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::settings_store::SettingsStore;
use crate::settings_types::{
    default_advisor_preset, default_approval_preset, default_auto_max_turns,
    default_auto_pause_on_error, default_auto_timebox_minutes,
};

/// Resolve the ilhae data directory (~⁄ilhae), using the generic name.
pub fn resolve_ilhae_data_dir() -> PathBuf {
    if let Ok(from_env) = std::env::var("ILHAE_DATA_DIR") {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let data_dir = home.join("ilhae");
    let legacy_dir = home.join(crate::helpers::ILHAE_DIR_NAME);

    if legacy_dir.exists() {
        let _ = std::fs::create_dir_all(&data_dir);
        if let Ok(entries) = std::fs::read_dir(&legacy_dir) {
            for entry in entries.flatten() {
                let dest = data_dir.join(entry.file_name());
                if !dest.exists() && entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    info!("Migrating {:?} → {:?}", entry.path(), dest);
                    let _ = std::fs::copy(entry.path(), &dest);
                }
            }
        }
    }

    data_dir
}

/// Resolve the human-managed ilhae config directory (~/.ilhae).
pub fn resolve_ilhae_config_dir() -> PathBuf {
    if let Ok(from_env) = std::env::var("ILHAE_CONFIG_DIR") {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ilhae")
}

pub fn resolve_ilhae_config_toml_path() -> PathBuf {
    resolve_ilhae_config_dir().join("config.toml")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeTomlConfig {
    pub profile: IlhaeActiveProfileConfig,
    pub profiles: BTreeMap<String, IlhaeProfileConfig>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeActiveProfileConfig {
    pub active: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeProfileConfig {
    pub agent: IlhaeProfileAgentConfig,
    pub permissions: IlhaeProfilePermissionsConfig,
    pub memory: IlhaeProfileScopeConfig,
    pub task: IlhaeProfileScopeConfig,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeProfileAgentConfig {
    #[serde(rename = "engine")]
    pub engine_id: Option<String>,
    pub command: Option<String>,
    pub team_mode: bool,
    pub auto_mode: bool,
    pub advisor: bool,
    #[serde(default = "default_advisor_preset")]
    pub advisor_preset: String,
    #[serde(default = "default_auto_max_turns")]
    pub auto_max_turns: u32,
    #[serde(default = "default_auto_timebox_minutes")]
    pub auto_timebox_minutes: u32,
    #[serde(default = "default_auto_pause_on_error")]
    pub auto_pause_on_error: bool,
    pub kairos: bool,
    pub self_improvement: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IlhaeProfilePermissionsConfig {
    pub approval_preset: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IlhaeProfileScopeConfig {
    pub scope: String,
}

impl Default for IlhaeProfilePermissionsConfig {
    fn default() -> Self {
        Self {
            approval_preset: default_approval_preset(),
        }
    }
}

impl Default for IlhaeProfileScopeConfig {
    fn default() -> Self {
        Self {
            scope: "default".to_string(),
        }
    }
}

pub fn load_ilhae_toml_config() -> IlhaeTomlConfig {
    let primary = resolve_ilhae_config_toml_path();
    let legacy = resolve_ilhae_data_dir().join("config.toml");

    let path = if primary.exists() {
        primary
    } else if legacy.exists() {
        legacy
    } else {
        return IlhaeTomlConfig::default();
    };

    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| toml::from_str::<IlhaeTomlConfig>(&content).ok())
        .unwrap_or_default()
}

pub fn save_ilhae_toml_config(config: &IlhaeTomlConfig) -> Result<(), String> {
    let path = resolve_ilhae_config_toml_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| e.to_string())
}

pub fn profile_to_dto(id: &str, profile: &IlhaeProfileConfig) -> crate::IlhaeAppProfileDto {
    crate::IlhaeAppProfileDto {
        id: id.to_string(),
        agent: crate::IlhaeAppProfileAgentDto {
            engine_id: profile.agent.engine_id.clone(),
            command: profile.agent.command.clone(),
            team_mode: profile.agent.team_mode,
            auto_mode: profile.agent.auto_mode,
            advisor: profile.agent.advisor,
            advisor_preset: profile.agent.advisor_preset.clone(),
            auto_max_turns: profile.agent.auto_max_turns,
            auto_timebox_minutes: profile.agent.auto_timebox_minutes,
            auto_pause_on_error: profile.agent.auto_pause_on_error,
            kairos: profile.agent.kairos,
            self_improvement: profile.agent.self_improvement,
        },
        permissions: crate::IlhaeAppProfilePermissionsDto {
            approval_preset: profile.permissions.approval_preset.clone(),
        },
        memory: crate::IlhaeAppProfileScopeDto {
            scope: profile.memory.scope.clone(),
        },
        task: crate::IlhaeAppProfileScopeDto {
            scope: profile.task.scope.clone(),
        },
    }
}

pub fn dto_to_profile(dto: &crate::IlhaeAppProfileDto) -> IlhaeProfileConfig {
    IlhaeProfileConfig {
        agent: IlhaeProfileAgentConfig {
            engine_id: dto.agent.engine_id.clone(),
            command: dto.agent.command.clone(),
            team_mode: dto.agent.team_mode,
            auto_mode: dto.agent.auto_mode,
            advisor: dto.agent.advisor,
            advisor_preset: if dto.agent.advisor_preset.trim().is_empty() {
                default_advisor_preset()
            } else {
                dto.agent.advisor_preset.clone()
            },
            auto_max_turns: dto.agent.auto_max_turns.max(1),
            auto_timebox_minutes: dto.agent.auto_timebox_minutes.max(1),
            auto_pause_on_error: dto.agent.auto_pause_on_error,
            kairos: dto.agent.kairos,
            self_improvement: dto.agent.self_improvement,
        },
        permissions: IlhaeProfilePermissionsConfig {
            approval_preset: if dto.permissions.approval_preset.trim().is_empty() {
                default_approval_preset()
            } else {
                dto.permissions.approval_preset.clone()
            },
        },
        memory: IlhaeProfileScopeConfig {
            scope: if dto.memory.scope.trim().is_empty() {
                "default".to_string()
            } else {
                dto.memory.scope.clone()
            },
        },
        task: IlhaeProfileScopeConfig {
            scope: if dto.task.scope.trim().is_empty() {
                "default".to_string()
            } else {
                dto.task.scope.clone()
            },
        },
    }
}

pub fn list_ilhae_profiles() -> (Option<String>, Vec<crate::IlhaeAppProfileDto>) {
    let config = load_ilhae_toml_config();
    let profiles = config
        .profiles
        .iter()
        .map(|(id, profile)| profile_to_dto(id, profile))
        .collect();
    (config.profile.active, profiles)
}

pub fn get_ilhae_profile(profile_id: Option<&str>) -> (Option<String>, Option<crate::IlhaeAppProfileDto>) {
    let config = load_ilhae_toml_config();
    let active_profile = config.profile.active.clone();
    let target = profile_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| active_profile.clone());
    let profile = target
        .as_ref()
        .and_then(|id| config.profiles.get(id).map(|profile| profile_to_dto(id, profile)));
    (active_profile, profile)
}

pub fn upsert_ilhae_profile(
    profile: crate::IlhaeAppProfileDto,
    activate: bool,
) -> Result<(Option<String>, crate::IlhaeAppProfileDto), String> {
    let profile_id = profile.id.trim().to_string();
    if profile_id.is_empty() {
        return Err("profile id is required".to_string());
    }

    let mut config = load_ilhae_toml_config();
    let persisted = dto_to_profile(&profile);
    config.profiles.insert(profile_id.clone(), persisted.clone());
    if activate {
        config.profile.active = Some(profile_id.clone());
    }
    save_ilhae_toml_config(&config)?;
    Ok((config.profile.active, profile_to_dto(&profile_id, &persisted)))
}

pub fn set_active_ilhae_profile(profile_id: &str) -> Result<crate::IlhaeAppProfileDto, String> {
    let profile_id = profile_id.trim();
    if profile_id.is_empty() {
        return Err("profile id is required".to_string());
    }

    let mut config = load_ilhae_toml_config();
    let Some(profile) = config.profiles.get(profile_id).cloned() else {
        return Err(format!("unknown profile id: {profile_id}"));
    };
    config.profile.active = Some(profile_id.to_string());
    save_ilhae_toml_config(&config)?;
    Ok(profile_to_dto(profile_id, &profile))
}

pub fn apply_ilhae_profile_projection(
    settings: &SettingsStore,
    profile: &crate::IlhaeAppProfileDto,
) -> Result<(), String> {
    let engine_id = profile
        .agent
        .engine_id
        .clone()
        .or_else(|| {
            profile
                .agent
                .command
                .as_deref()
                .map(crate::helpers::infer_agent_id_from_command)
        })
        .unwrap_or_else(|| "gemini".to_string());
    let command = crate::helpers::resolve_engine_command(&engine_id, profile.agent.command.as_deref())
        .ok_or_else(|| "unknown engine id; provide explicit command".to_string())?;

    settings.set_value("agent.active_profile", serde_json::json!(profile.id))?;
    settings.set_value("agent.command", serde_json::json!(command))?;
    settings.set_value("agent.team_mode", serde_json::json!(profile.agent.team_mode))?;
    settings.set_value("agent.autonomous_mode", serde_json::json!(profile.agent.auto_mode))?;
    settings.set_value("agent.advisor_mode", serde_json::json!(profile.agent.advisor))?;
    settings.set_value(
        "agent.advisor_preset",
        serde_json::json!(if profile.agent.advisor_preset.trim().is_empty() {
            default_advisor_preset()
        } else {
            profile.agent.advisor_preset.clone()
        }),
    )?;
    settings.set_value(
        "agent.auto_max_turns",
        serde_json::json!(profile.agent.auto_max_turns.max(1)),
    )?;
    settings.set_value(
        "agent.auto_timebox_minutes",
        serde_json::json!(profile.agent.auto_timebox_minutes.max(1)),
    )?;
    settings.set_value(
        "agent.auto_pause_on_error",
        serde_json::json!(profile.agent.auto_pause_on_error),
    )?;
    settings.set_value("agent.kairos_enabled", serde_json::json!(profile.agent.kairos))?;
    settings.set_value(
        "agent.self_improvement_enabled",
        serde_json::json!(profile.agent.self_improvement),
    )?;
    settings.set_value("agent.memory_scope", serde_json::json!(profile.memory.scope))?;
    settings.set_value("agent.task_scope", serde_json::json!(profile.task.scope))?;
    settings.set_value(
        "permissions.approval_preset",
        serde_json::json!(profile.permissions.approval_preset),
    )?;

    let mut enabled_engines = settings.get().agent.enabled_engines;
    if !enabled_engines.iter().any(|existing| existing == &engine_id) {
        enabled_engines.push(engine_id);
        settings.set_value("agent.enabled_engines", serde_json::json!(enabled_engines))?;
    }

    Ok(())
}

#[derive(serde::Deserialize)]
struct MinimalSettings {
    #[serde(default)]
    vault: VaultConfig,
}

#[derive(serde::Deserialize, Default)]
struct VaultConfig {
    #[serde(default)]
    active_vault: Option<String>,
}

/// Get the currently active vault directory path.
/// Defaults to `~/ilhae/brain` if not configured.
pub fn get_active_vault_dir() -> PathBuf {
    let ilhae_dir = resolve_ilhae_data_dir();
    let default_vault = ilhae_dir.join("brain");

    let settings_path = ilhae_dir
        .join("brain")
        .join("settings")
        .join("app_settings.json");
    if let Ok(content) = std::fs::read_to_string(&settings_path) {
        if let Ok(settings) = serde_json::from_str::<MinimalSettings>(&content) {
            if let Some(active) = settings.vault.active_vault {
                if !active.trim().is_empty() {
                    return PathBuf::from(active);
                }
            }
        }
    }
    default_vault
}

/// Copy codex auth files from ~/.codex to the workspace CODEX_HOME directory.
pub fn sync_codex_auth_to_workspace(home: &str, workspace: &PathBuf) {
    let source_dir = PathBuf::from(home).join(".codex");
    if !source_dir.exists() {
        return;
    }

    if let Err(err) = std::fs::create_dir_all(workspace) {
        warn!(
            "Failed to create CODEX_HOME workspace directory {:?}: {}",
            workspace, err
        );
        return;
    }

    for file in ["auth.json", "config.toml", ".credentials.json"] {
        let src = source_dir.join(file);
        if !src.exists() {
            continue;
        }
        let dst = workspace.join(file);
        if let Err(err) = std::fs::copy(&src, &dst) {
            warn!("Failed to copy {:?} -> {:?}: {}", src, dst, err);
        }
    }
}

/// Build the context prefix from IDENTITY.md, SOUL.md, USER.md, and memory/global/ folder files.
pub fn build_context_prefix(_ilhae_dir_unused: &Path) -> String {
    let vault_dir = get_active_vault_dir();
    let global_dir = vault_dir.join("memory").join("global");
    let legacy_context_dir = vault_dir.join("context"); // legacy fallback
    let ilhae_dir = resolve_ilhae_data_dir();

    // Core identity files: memory/global/ > legacy context/ > active vault/ > ilhae root
    let read_with_fallback = |name: &str| -> String {
        std::fs::read_to_string(global_dir.join(name))
            .or_else(|_| std::fs::read_to_string(legacy_context_dir.join(name)))
            .or_else(|_| std::fs::read_to_string(vault_dir.join(name)))
            .or_else(|_| std::fs::read_to_string(ilhae_dir.join(name)))
            .unwrap_or_default()
    };

    let system = read_with_fallback("SYSTEM.md");
    let identity = read_with_fallback("IDENTITY.md");
    let soul = read_with_fallback("SOUL.md");
    let user = read_with_fallback("USER.md");

    // Collect additional memory/global/ folder .md files (exclude core + README)
    let mut context_parts = Vec::new();
    for dir in [&global_dir, &legacy_context_dir] {
        if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                let excluded = [
                    "SYSTEM.md",
                    "IDENTITY.md",
                    "SOUL.md",
                    "USER.md",
                    "README.md",
                ];
                let mut paths: Vec<_> = entries
                    .flatten()
                    .filter(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        name.ends_with(".md") && !excluded.contains(&name.as_str())
                    })
                    .collect();
                paths.sort_by_key(|e| e.file_name());
                for entry in paths {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        if !content.trim().is_empty() {
                            context_parts.push(content);
                        }
                    }
                }
            }
        }
    }

    let context_section = if context_parts.is_empty() {
        String::new()
    } else {
        format!("\n### CONTEXT\n{}\n", context_parts.join("\n---\n"))
    };

    let system_section = if system.trim().is_empty() {
        String::new()
    } else {
        format!("### SYSTEM\n{}\n", system)
    };

    format!(
        "\n<agent_context>\n{}### IDENTITY\n{}\n### SOUL\n{}\n### USER\n{}{}\n</agent_context>\n\n",
        system_section, identity, soul, user, context_section
    )
}
