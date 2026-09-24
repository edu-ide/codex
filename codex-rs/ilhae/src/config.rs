mod models;
use models::native_runtime_model_context_window;
pub use models::native_runtime_model_name_from_path;
use models::profile_engine_id;
use models::profile_engine_id_for_display;
use models::profile_runtime_model_name;

mod codex_runtime;
mod profiles;

pub use codex_runtime::ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE;
pub use codex_runtime::ILHAE_CODEX_RUNTIME_CONFIG_LOCK_FILE;
pub use codex_runtime::prepare_ilhae_codex_home;
pub use profiles::IlhaeActiveProfileConfig;
pub use profiles::IlhaeProfileAgentConfig;
pub use profiles::IlhaeProfileConfig;
pub use profiles::IlhaeProfileKnowledgeConfig;
pub use profiles::IlhaeProfileNativeRuntimeConfig;
pub use profiles::IlhaeProfilePermissionsConfig;
pub use profiles::IlhaeProfileScopeConfig;
pub use profiles::IlhaeProfileSidecarConfig;
pub use profiles::IlhaeProfileSystem2Config;
pub use profiles::IlhaeProjectConfig;
pub use profiles::IlhaeTomlConfig;
pub use profiles::ResolvedSystem2TargetConfig;

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use tracing::info;
use tracing::warn;

use crate::settings_store::SettingsStore;
use crate::settings_types::default_advisor_preset;
use crate::settings_types::default_approval_preset;
use crate::settings_types::default_knowledge_mode;
use crate::settings_types::default_knowledge_periodic_interval_secs;
use crate::settings_types::default_knowledge_poll_interval_secs;
use crate::settings_types::default_knowledge_report_relative_path;
use crate::settings_types::default_knowledge_report_target;
use crate::settings_types::default_self_improvement_preset;
use crate::settings_types::default_team_backend;
use crate::settings_types::default_team_merge_policy;
use crate::settings_types::default_thinking_mode;
use crate::settings_types::normalize_thinking_mode;
use crate::settings_types::thinking_mode_enabled;

/// Resolve the ilhae data directory (~/.ilhae).
pub fn resolve_ilhae_data_dir() -> PathBuf {
    if let Ok(from_env) = std::env::var("ILHAE_DATA_DIR") {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let data_dir = resolve_ilhae_config_dir();
    let legacy_dir = home.join("ilhae");

    if legacy_dir.exists() {
        migrate_legacy_data_dir(&legacy_dir, &data_dir);
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

pub fn resolve_ilhae_codex_home_dir() -> PathBuf {
    if let Ok(from_env) = std::env::var("ILHAE_CODEX_HOME") {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    resolve_ilhae_config_dir().join("codex-home")
}

fn migrate_legacy_data_dir(legacy_dir: &Path, data_dir: &Path) {
    let _ = std::fs::create_dir_all(data_dir);
    for name in LEGACY_MIGRATION_ENTRIES {
        let source = legacy_dir.join(name);
        let dest = data_dir.join(name);
        if !source.exists() || dest.exists() {
            continue;
        }
        if source.is_dir() {
            if copy_dir_missing(&source, &dest).is_ok() {
                info!("Migrated legacy Ilhae directory {:?} -> {:?}", source, dest);
            }
        } else if source.is_file() {
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(&source, &dest).is_ok() {
                info!("Migrated legacy Ilhae file {:?} -> {:?}", source, dest);
            }
        }
    }
}

const LEGACY_MIGRATION_ENTRIES: &[&str] = &[
    "brain",
    "vault",
    "workspace",
    "ws",
    "autonomy-state",
    "settings.json",
    "team.json",
    "tasks.json",
    "schedules.json",
    "kb_workspaces.json",
    "sessions.db",
    "sessions.db-shm",
    "sessions.db-wal",
    "memory.db",
    "memory.db-shm",
    "memory.db-wal",
    "artifacts.db",
    "artifacts.db-shm",
    "artifacts.db-wal",
    "notifications.db",
    "notifications.db-shm",
    "notifications.db-wal",
];

fn copy_dir_missing(source: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if dest_path.exists() {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_missing(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let _ = std::fs::copy(&source_path, &dest_path)?;
        }
    }
    Ok(())
}

pub fn normalize_knowledge_mode(mode: &str) -> String {
    match mode.trim().to_ascii_lowercase().as_str() {
        "worker" => "worker".to_string(),
        "kairos" => "kairos".to_string(),
        "both" => "both".to_string(),
        "off" | "" => default_knowledge_mode(),
        "enabled" | "true" => "kairos".to_string(),
        "worker-only" => "worker".to_string(),
        "kairos-only" => "kairos".to_string(),
        _ => default_knowledge_mode(),
    }
}

pub fn normalize_team_backend(backend: &str) -> String {
    match backend.trim().to_ascii_lowercase().as_str() {
        "remote" => "remote".to_string(),
        "hybrid" => "hybrid".to_string(),
        "local" | "" => default_team_backend(),
        _ => default_team_backend(),
    }
}

pub fn current_thinking_mode() -> String {
    let ilhae_dir = resolve_ilhae_data_dir();
    let settings = crate::settings_store::SettingsStore::new(&ilhae_dir).get();
    let raw = if settings.agent.thinking_mode.trim().is_empty() {
        default_thinking_mode()
    } else {
        settings.agent.thinking_mode
    };
    normalize_thinking_mode(&raw)
}

pub fn current_thinking_enabled() -> bool {
    thinking_mode_enabled(&current_thinking_mode())
}

pub fn team_backend_uses_remote_transport(backend: &str) -> bool {
    matches!(
        normalize_team_backend(backend).as_str(),
        "remote" | "hybrid"
    )
}

pub fn effective_knowledge_mode(profile: &IlhaeProfileConfig) -> String {
    if let Some(knowledge) = profile.knowledge.as_ref() {
        normalize_knowledge_mode(&knowledge.mode)
    } else {
        default_knowledge_mode()
    }
}

pub fn profile_runtime_display_parts(profile: &IlhaeProfileConfig) -> Vec<String> {
    let mut parts = Vec::new();

    if profile.native_runtime.enabled {
        let provider = profile
            .native_runtime
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .unwrap_or("llama-server");
        parts.push(provider.to_string());
    } else if let Some(engine_id) = profile_engine_id_for_display(profile) {
        parts.push(engine_id);
    }

    if let Some(model_name) = profile_runtime_model_name(profile) {
        parts.push(model_name);
    } else if !profile.native_runtime.enabled
        && !native_runtime_effective_base_url(&profile.native_runtime).is_empty()
        && profile
            .native_runtime
            .model_path
            .trim()
            .eq_ignore_ascii_case("default")
    {
        parts.push("server-default".to_string());
    }

    if !profile.native_runtime.enabled
        && !native_runtime_effective_base_url(&profile.native_runtime).is_empty()
    {
        parts.push("remote".to_string());
    }

    parts
}

pub fn knowledge_mode_includes_worker(mode: &str) -> bool {
    matches!(normalize_knowledge_mode(mode).as_str(), "worker" | "both")
}

pub fn knowledge_mode_includes_kairos(mode: &str) -> bool {
    matches!(normalize_knowledge_mode(mode).as_str(), "kairos" | "both")
}

fn settings_for_managed_profile(
    active_profile_name: Option<String>,
    active_profile: &IlhaeProfileConfig,
) -> crate::settings_types::Settings {
    let mut settings = crate::settings_types::Settings::default();
    settings.agent.active_profile = active_profile_name;
    if let Some(command) = active_profile.agent.command.clone() {
        settings.agent.command = command;
    }
    settings.agent.team_mode = active_profile.agent.team_mode;
    settings.agent.team_backend = normalize_team_backend(&active_profile.agent.team_backend);
    settings.agent.team_merge_policy = active_profile.agent.team_merge_policy.clone();
    settings.agent.team_max_retries = active_profile.agent.team_max_retries;
    settings.agent.team_pause_on_error = active_profile.agent.team_pause_on_error;
    settings.agent.autonomous_mode = active_profile.agent.auto_mode;
    settings.agent.advisor_mode = active_profile.agent.advisor;
    settings.agent.advisor_preset = active_profile.agent.advisor_preset.clone();
    settings.agent.auto_max_turns = active_profile.agent.auto_max_turns;
    settings.agent.auto_timebox_minutes = active_profile.agent.auto_timebox_minutes;
    settings.agent.auto_pause_on_error = active_profile.agent.auto_pause_on_error;
    settings.agent.kairos_enabled = active_profile.agent.kairos;
    settings.agent.knowledge_mode = effective_knowledge_mode(active_profile);
    if let Some(knowledge) = active_profile.knowledge.as_ref() {
        settings.agent.knowledge_workspace_id = knowledge.workspace_id.clone();
        settings.agent.knowledge_poll_interval_secs = knowledge.poll_interval_secs.max(1);
        settings.agent.knowledge_periodic_interval_secs = knowledge.periodic_interval_secs.max(1);
        settings.agent.knowledge_report_target = if knowledge.report_target.trim().is_empty() {
            default_knowledge_report_target()
        } else {
            knowledge.report_target.clone()
        };
        settings.agent.knowledge_report_relative_path =
            if knowledge.report_relative_path.trim().is_empty() {
                default_knowledge_report_relative_path()
            } else {
                knowledge.report_relative_path.clone()
            };
    }
    settings.agent.self_improvement_enabled = active_profile.agent.self_improvement;
    settings.agent.self_improvement_preset = if active_profile
        .agent
        .self_improvement_preset
        .trim()
        .is_empty()
    {
        default_self_improvement_preset()
    } else {
        active_profile.agent.self_improvement_preset.clone()
    };
    settings.agent.memory_scope = Some(active_profile.memory.scope.clone());
    settings.agent.task_scope = Some(active_profile.task.scope.clone());
    settings
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
            dream_mode: profile.agent.dream_mode,
            embed_mode: profile.agent.embed_mode,
            team_backend: normalize_team_backend(&profile.agent.team_backend),
            team_merge_policy: profile.agent.team_merge_policy.clone(),
            team_max_retries: profile.agent.team_max_retries,
            team_pause_on_error: profile.agent.team_pause_on_error,
            auto_mode: profile.agent.auto_mode,
            advisor: profile.agent.advisor,
            advisor_preset: profile.agent.advisor_preset.clone(),
            auto_max_turns: profile.agent.auto_max_turns,
            auto_timebox_minutes: profile.agent.auto_timebox_minutes,
            auto_pause_on_error: profile.agent.auto_pause_on_error,
            kairos: profile.agent.kairos,
            self_improvement: profile.agent.self_improvement,
            self_improvement_preset: profile.agent.self_improvement_preset.clone(),
            native_runtime_enabled: profile.native_runtime.enabled,
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
        knowledge: profile
            .knowledge
            .as_ref()
            .map(|knowledge| crate::IlhaeAppProfileKnowledgeDto {
                mode: normalize_knowledge_mode(&knowledge.mode),
                workspace_id: knowledge.workspace_id.clone(),
                poll_interval_secs: knowledge.poll_interval_secs,
                periodic_interval_secs: knowledge.periodic_interval_secs,
                report_target: knowledge.report_target.clone(),
                report_relative_path: knowledge.report_relative_path.clone(),
            }),
    }
}

pub fn dto_to_profile(dto: &crate::IlhaeAppProfileDto) -> IlhaeProfileConfig {
    IlhaeProfileConfig {
        agent: IlhaeProfileAgentConfig {
            engine_id: dto.agent.engine_id.clone(),
            command: dto.agent.command.clone(),
            team_mode: dto.agent.team_mode,
            dream_mode: dto.agent.dream_mode,
            embed_mode: dto.agent.embed_mode,
            team_backend: normalize_team_backend(&dto.agent.team_backend),
            team_merge_policy: if dto.agent.team_merge_policy.trim().is_empty() {
                default_team_merge_policy()
            } else {
                dto.agent.team_merge_policy.clone()
            },
            team_max_retries: dto.agent.team_max_retries.max(1),
            team_pause_on_error: dto.agent.team_pause_on_error,
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
            self_improvement_preset: if dto.agent.self_improvement_preset.trim().is_empty() {
                default_self_improvement_preset()
            } else {
                dto.agent.self_improvement_preset.clone()
            },
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
        knowledge: dto
            .knowledge
            .as_ref()
            .map(|knowledge| IlhaeProfileKnowledgeConfig {
                mode: normalize_knowledge_mode(&knowledge.mode),
                workspace_id: knowledge
                    .workspace_id
                    .clone()
                    .filter(|workspace_id| !workspace_id.trim().is_empty()),
                poll_interval_secs: knowledge.poll_interval_secs.max(1),
                periodic_interval_secs: knowledge.periodic_interval_secs.max(1),
                report_target: if knowledge.report_target.trim().is_empty() {
                    default_knowledge_report_target()
                } else {
                    knowledge.report_target.clone()
                },
                report_relative_path: if knowledge.report_relative_path.trim().is_empty() {
                    default_knowledge_report_relative_path()
                } else {
                    knowledge.report_relative_path.clone()
                },
            }),
        system2: IlhaeProfileSystem2Config::default(),
        native_runtime: IlhaeProfileNativeRuntimeConfig {
            enabled: dto.agent.native_runtime_enabled,
            ..Default::default()
        },
        // Not part of the app DTO; `upsert_ilhae_profile` keeps the stored values.
        backend_profile: None,
        sidecars: Vec::new(),
        stop_when_unused: false,
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

pub fn get_ilhae_profile(
    profile_id: Option<&str>,
) -> (Option<String>, Option<crate::IlhaeAppProfileDto>) {
    let config = load_ilhae_toml_config();
    let active_profile = config.profile.active.clone();
    let target = profile_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| active_profile.clone());
    let profile = target.as_ref().and_then(|id| {
        config
            .profiles
            .get(id)
            .map(|profile| profile_to_dto(id, profile))
    });
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
    let existing_native_runtime = config
        .profiles
        .get(&profile_id)
        .map(|existing| existing.native_runtime.clone())
        .unwrap_or_default();
    let existing_knowledge = config
        .profiles
        .get(&profile_id)
        .and_then(|existing| existing.knowledge.clone());
    let mut persisted = dto_to_profile(&profile);
    persisted.native_runtime = existing_native_runtime;
    persisted.native_runtime.enabled = profile.agent.native_runtime_enabled;
    if let Some(existing) = config.profiles.get(&profile_id) {
        persisted.backend_profile = existing.backend_profile.clone();
        persisted.sidecars = existing.sidecars.clone();
        persisted.stop_when_unused = existing.stop_when_unused;
    }
    persisted.knowledge = profile
        .knowledge
        .as_ref()
        .map(|knowledge| IlhaeProfileKnowledgeConfig {
            mode: normalize_knowledge_mode(&knowledge.mode),
            workspace_id: knowledge
                .workspace_id
                .clone()
                .filter(|workspace_id| !workspace_id.trim().is_empty()),
            poll_interval_secs: knowledge.poll_interval_secs.max(1),
            periodic_interval_secs: knowledge.periodic_interval_secs.max(1),
            report_target: if knowledge.report_target.trim().is_empty() {
                default_knowledge_report_target()
            } else {
                knowledge.report_target.clone()
            },
            report_relative_path: if knowledge.report_relative_path.trim().is_empty() {
                default_knowledge_report_relative_path()
            } else {
                knowledge.report_relative_path.clone()
            },
        })
        .or(existing_knowledge);
    config
        .profiles
        .insert(profile_id.clone(), persisted.clone());
    if activate {
        config.profile.active = Some(profile_id.clone());
    }
    save_ilhae_toml_config(&config)?;
    Ok((
        config.profile.active,
        profile_to_dto(&profile_id, &persisted),
    ))
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

pub fn get_native_runtime_config(
    profile_id: Option<&str>,
) -> Option<(String, IlhaeProfileNativeRuntimeConfig)> {
    let config = load_ilhae_toml_config();
    let target_profile = profile_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or(config.profile.active)?;
    let profile = config.profiles.get(&target_profile)?;
    Some((target_profile, profile.native_runtime.clone()))
}

pub fn get_active_native_runtime_config() -> Option<(String, IlhaeProfileNativeRuntimeConfig)> {
    get_native_runtime_config(None)
}

pub fn get_active_system2_target_config() -> Option<ResolvedSystem2TargetConfig> {
    if std::env::var("ILHAE_RUNTIME").ok().as_deref() == Some("1") {
        return runtime_system2_target_config_from_env();
    }

    let config = load_ilhae_toml_config();
    active_system2_target_config_from(&config)
}

fn runtime_system2_target_config_from_env() -> Option<ResolvedSystem2TargetConfig> {
    if std::env::var("ILHAE_SYSTEM2_ENABLED").ok().as_deref() != Some("1") {
        return None;
    }

    let required = |key| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
    };
    Some(ResolvedSystem2TargetConfig {
        source_profile_id: required("ILHAE_SYSTEM2_SOURCE_PROFILE")?,
        target_profile_id: required("ILHAE_SYSTEM2_PROFILE")?,
        base_url: required("ILHAE_SYSTEM2_BASE_URL")?,
        model_name: required("ILHAE_SYSTEM2_MODEL")?,
    })
}

fn active_system2_target_config_from(
    config: &IlhaeTomlConfig,
) -> Option<ResolvedSystem2TargetConfig> {
    let source_profile_id = config.profile.active.clone()?;
    let source_profile = config.profiles.get(&source_profile_id)?;
    if !source_profile.system2.enabled {
        return None;
    }

    let target_profile_id = source_profile.system2.profile.trim().to_string();
    if target_profile_id.is_empty() {
        return None;
    }

    let target_profile = config.profiles.get(&target_profile_id)?;
    let base_url = native_runtime_effective_base_url(&target_profile.native_runtime);
    if base_url.is_empty() {
        return None;
    }

    let model_name = Path::new(&target_profile.native_runtime.model_path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())?;

    Some(ResolvedSystem2TargetConfig {
        source_profile_id,
        target_profile_id,
        base_url,
        model_name,
    })
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
    let command =
        crate::helpers::resolve_engine_command(&engine_id, profile.agent.command.as_deref())
            .ok_or_else(|| "unknown engine id; provide explicit command".to_string())?;

    settings.set_value("agent.active_profile", serde_json::json!(profile.id))?;
    settings.set_value("agent.command", serde_json::json!(command))?;
    settings.set_value(
        "agent.team_mode",
        serde_json::json!(profile.agent.team_mode),
    )?;
    settings.set_value(
        "agent.team_backend",
        serde_json::json!(normalize_team_backend(&profile.agent.team_backend)),
    )?;
    settings.set_value(
        "agent.team_merge_policy",
        serde_json::json!(if profile.agent.team_merge_policy.trim().is_empty() {
            default_team_merge_policy()
        } else {
            profile.agent.team_merge_policy.clone()
        }),
    )?;
    settings.set_value(
        "agent.team_max_retries",
        serde_json::json!(profile.agent.team_max_retries.max(1)),
    )?;
    settings.set_value(
        "agent.team_pause_on_error",
        serde_json::json!(profile.agent.team_pause_on_error),
    )?;
    settings.set_value(
        "agent.autonomous_mode",
        serde_json::json!(profile.agent.auto_mode),
    )?;
    settings.set_value(
        "agent.advisor_mode",
        serde_json::json!(profile.agent.advisor),
    )?;
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
    let effective_knowledge_mode = effective_knowledge_mode_for_profile(profile);
    settings.set_value(
        "agent.kairos_enabled",
        serde_json::json!(profile.agent.kairos),
    )?;
    settings.set_value(
        "agent.thinking_mode",
        serde_json::json!(if profile.agent.native_runtime_enabled {
            "on"
        } else {
            "off"
        }),
    )?;
    settings.set_value(
        "agent.native_runtime_enabled",
        serde_json::json!(profile.agent.native_runtime_enabled),
    )?;
    settings.set_value(
        "agent.knowledge_mode",
        serde_json::json!(effective_knowledge_mode),
    )?;
    settings.set_value(
        "agent.knowledge_workspace_id",
        serde_json::json!(
            profile
                .knowledge
                .as_ref()
                .and_then(|knowledge| knowledge.workspace_id.clone())
        ),
    )?;
    settings.set_value(
        "agent.knowledge_poll_interval_secs",
        serde_json::json!(
            profile
                .knowledge
                .as_ref()
                .map(|knowledge| knowledge.poll_interval_secs.max(1))
                .unwrap_or_else(default_knowledge_poll_interval_secs)
        ),
    )?;
    settings.set_value(
        "agent.knowledge_periodic_interval_secs",
        serde_json::json!(
            profile
                .knowledge
                .as_ref()
                .map(|knowledge| knowledge.periodic_interval_secs.max(1))
                .unwrap_or_else(default_knowledge_periodic_interval_secs)
        ),
    )?;
    settings.set_value(
        "agent.knowledge_report_target",
        serde_json::json!(
            profile
                .knowledge
                .as_ref()
                .map(|knowledge| {
                    if knowledge.report_target.trim().is_empty() {
                        default_knowledge_report_target()
                    } else {
                        knowledge.report_target.clone()
                    }
                })
                .unwrap_or_else(default_knowledge_report_target)
        ),
    )?;
    settings.set_value(
        "agent.knowledge_report_relative_path",
        serde_json::json!(
            profile
                .knowledge
                .as_ref()
                .map(|knowledge| {
                    if knowledge.report_relative_path.trim().is_empty() {
                        default_knowledge_report_relative_path()
                    } else {
                        knowledge.report_relative_path.clone()
                    }
                })
                .unwrap_or_else(default_knowledge_report_relative_path)
        ),
    )?;
    settings.set_value(
        "agent.self_improvement_enabled",
        serde_json::json!(profile.agent.self_improvement),
    )?;
    settings.set_value(
        "agent.self_improvement_preset",
        serde_json::json!(if profile.agent.self_improvement_preset.trim().is_empty() {
            default_self_improvement_preset()
        } else {
            profile.agent.self_improvement_preset.clone()
        }),
    )?;
    settings.set_value(
        "agent.memory_scope",
        serde_json::json!(profile.memory.scope),
    )?;
    settings.set_value("agent.task_scope", serde_json::json!(profile.task.scope))?;
    settings.set_value(
        "permissions.approval_preset",
        serde_json::json!(profile.permissions.approval_preset),
    )?;

    let mut enabled_engines = settings.get().agent.enabled_engines;
    if !enabled_engines
        .iter()
        .any(|existing| existing == &engine_id)
    {
        enabled_engines.push(engine_id);
        settings.set_value("agent.enabled_engines", serde_json::json!(enabled_engines))?;
    }

    Ok(())
}

pub fn apply_active_ilhae_profile_projection(settings: &SettingsStore) -> Result<(), String> {
    let (_, profile) = get_ilhae_profile(None);
    if let Some(profile) = profile {
        apply_ilhae_profile_projection(settings, &profile)?;
    }
    Ok(())
}

fn effective_knowledge_mode_for_profile(profile: &crate::IlhaeAppProfileDto) -> String {
    match profile.knowledge.as_ref() {
        Some(knowledge) => normalize_knowledge_mode(&knowledge.mode),
        None => default_knowledge_mode(),
    }
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

pub(crate) fn resolve_ilhae_profile_model_name(
    profile_id: &str,
    profile: &IlhaeProfileConfig,
) -> String {
    models::resolve_profile_model_name(profile_id, profile, || {
        codex_runtime::codex_model_from_root_config(profile_id)
    })
}

pub fn native_runtime_effective_base_url(runtime: &IlhaeProfileNativeRuntimeConfig) -> String {
    crate::native_runtime_endpoint::effective_proxy_base_url(runtime)
        .or_else(|| crate::native_runtime_endpoint::runtime_upstream_base_url(runtime))
        .unwrap_or_default()
}

pub fn native_runtime_effective_health_url(runtime: &IlhaeProfileNativeRuntimeConfig) -> String {
    if let Some(health_url) = crate::native_runtime_endpoint::effective_proxy_health_url(runtime) {
        return health_url;
    }
    crate::native_runtime_endpoint::runtime_upstream_health_url(runtime)
}

pub(crate) fn native_runtime_health_url_from_base_url(base_url: &str) -> String {
    if let Ok(mut parsed) = url::Url::parse(&base_url) {
        let mut path = parsed.path().trim_end_matches('/').to_string();
        if path.ends_with("/v1") {
            path = path.strip_suffix("/v1").unwrap_or("").to_string();
        }
        if path.is_empty() {
            path = "/health".to_string();
        } else {
            path.push_str("/health");
        }
        parsed.set_path(&path);
        parsed.set_query(None);
        parsed.set_fragment(None);
        return parsed.to_string();
    }

    let fallback_base = base_url.trim_end_matches('/');
    if fallback_base.ends_with("/v1") {
        return format!(
            "{}/health",
            fallback_base
                .trim_end_matches('/')
                .trim_end_matches("v1")
                .trim_end_matches('/')
        );
    }
    format!("{}/health", fallback_base)
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
