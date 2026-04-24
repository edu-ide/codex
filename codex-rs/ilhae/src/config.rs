use codex_protocol::config_types::TrustLevel;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::settings_store::SettingsStore;
use crate::settings_types::{
    default_advisor_preset, default_approval_preset, default_auto_max_turns,
    default_auto_pause_on_error, default_auto_timebox_minutes, default_knowledge_mode,
    default_knowledge_periodic_interval_secs, default_knowledge_poll_interval_secs,
    default_knowledge_report_relative_path, default_knowledge_report_target,
    default_self_improvement_enabled, default_self_improvement_preset, default_team_backend,
    default_team_max_retries, default_team_merge_policy, default_team_pause_on_error,
    default_thinking_mode, normalize_thinking_mode, thinking_mode_enabled,
};

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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeTomlConfig {
    pub profile: IlhaeActiveProfileConfig,
    pub profiles: BTreeMap<String, IlhaeProfileConfig>,
    pub projects: BTreeMap<String, IlhaeProjectConfig>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<IlhaeProfileKnowledgeConfig>,
    pub system2: IlhaeProfileSystem2Config,
    pub native_runtime: IlhaeProfileNativeRuntimeConfig,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeProjectConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_level: Option<TrustLevel>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IlhaeProfileAgentConfig {
    #[serde(rename = "engine")]
    pub engine_id: Option<String>,
    pub command: Option<String>,
    pub team_mode: bool,
    #[serde(default)]
    pub dream_mode: bool,
    #[serde(default)]
    pub embed_mode: bool,
    #[serde(default = "default_team_backend")]
    pub team_backend: String,
    #[serde(default = "default_team_merge_policy")]
    pub team_merge_policy: String,
    #[serde(default = "default_team_max_retries")]
    pub team_max_retries: u32,
    #[serde(default = "default_team_pause_on_error")]
    pub team_pause_on_error: bool,
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
    #[serde(default = "default_self_improvement_enabled")]
    pub self_improvement: bool,
    #[serde(default = "default_self_improvement_preset")]
    pub self_improvement_preset: String,
}

fn default_native_runtime_startup_timeout_secs() -> u64 {
    120
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IlhaeProfileNativeRuntimeConfig {
    pub enabled: bool,
    pub provider: Option<String>,
    pub health_url: String,
    pub base_url: String,
    pub server_bin: String,
    pub model_path: String,
    pub chat_template_file: String,
    pub log_file: String,
    #[serde(default = "default_native_runtime_startup_timeout_secs")]
    pub startup_timeout_secs: u64,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct IlhaeProfileSystem2Config {
    pub enabled: bool,
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSystem2TargetConfig {
    pub source_profile_id: String,
    pub target_profile_id: String,
    pub base_url: String,
    pub model_name: String,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IlhaeProfileKnowledgeConfig {
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub poll_interval_secs: u64,
    pub periodic_interval_secs: u64,
    pub report_target: String,
    pub report_relative_path: String,
}

impl Default for IlhaeProfilePermissionsConfig {
    fn default() -> Self {
        Self {
            approval_preset: default_approval_preset(),
        }
    }
}

impl Default for IlhaeProfileAgentConfig {
    fn default() -> Self {
        Self {
            engine_id: None,
            command: None,
            team_mode: false,
            dream_mode: false,
            embed_mode: false,
            team_backend: default_team_backend(),
            team_merge_policy: default_team_merge_policy(),
            team_max_retries: default_team_max_retries(),
            team_pause_on_error: default_team_pause_on_error(),
            auto_mode: false,
            advisor: false,
            advisor_preset: default_advisor_preset(),
            auto_max_turns: default_auto_max_turns(),
            auto_timebox_minutes: default_auto_timebox_minutes(),
            auto_pause_on_error: default_auto_pause_on_error(),
            kairos: false,
            self_improvement: default_self_improvement_enabled(),
            self_improvement_preset: default_self_improvement_preset(),
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

impl Default for IlhaeProfileKnowledgeConfig {
    fn default() -> Self {
        Self {
            mode: default_knowledge_mode(),
            workspace_id: None,
            poll_interval_secs: default_knowledge_poll_interval_secs(),
            periodic_interval_secs: default_knowledge_periodic_interval_secs(),
            report_target: default_knowledge_report_target(),
            report_relative_path: default_knowledge_report_relative_path(),
        }
    }
}

impl Default for IlhaeProfileNativeRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: None,
            health_url: String::new(),
            base_url: String::new(),
            server_bin: String::new(),
            model_path: String::new(),
            chat_template_file: String::new(),
            log_file: String::new(),
            startup_timeout_secs: default_native_runtime_startup_timeout_secs(),
            args: Vec::new(),
        }
    }
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
        && !profile.native_runtime.base_url.trim().is_empty()
        && profile
            .native_runtime
            .model_path
            .trim()
            .eq_ignore_ascii_case("default")
    {
        parts.push("server-default".to_string());
    }

    if !profile.native_runtime.enabled && !profile.native_runtime.base_url.trim().is_empty() {
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
        native_runtime: IlhaeProfileNativeRuntimeConfig::default(),
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
    if !profile.native_runtime.enabled {
        return None;
    }
    Some((target_profile, profile.native_runtime.clone()))
}

pub fn get_active_native_runtime_config() -> Option<(String, IlhaeProfileNativeRuntimeConfig)> {
    get_native_runtime_config(None)
}

pub fn get_active_system2_target_config() -> Option<ResolvedSystem2TargetConfig> {
    let config = load_ilhae_toml_config();
    let source_profile_id = config.profile.active?;
    let source_profile = config.profiles.get(&source_profile_id)?;
    if !source_profile.system2.enabled {
        return None;
    }

    let target_profile_id = source_profile.system2.profile.trim().to_string();
    if target_profile_id.is_empty() {
        return None;
    }

    let target_profile = config.profiles.get(&target_profile_id)?;
    let base_url = target_profile.native_runtime.base_url.trim().to_string();
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

fn parse_context_window_from_native_args(args: &[String]) -> u64 {
    let mut idx = 0usize;
    while idx < args.len() {
        let arg = args[idx].trim();
        if matches!(arg, "-c" | "--ctx-size" | "--context-size") {
            if let Some(value) = args.get(idx + 1).and_then(|next| next.parse::<u64>().ok()) {
                return value;
            }
        } else if let Some(value) = arg
            .strip_prefix("--ctx-size=")
            .or_else(|| arg.strip_prefix("--context-size="))
            .and_then(|value| value.parse::<u64>().ok())
        {
            return value;
        }
        idx += 1;
    }
    32_768
}

fn profile_engine_id_for_display(profile: &IlhaeProfileConfig) -> Option<String> {
    profile
        .agent
        .engine_id
        .as_deref()
        .map(str::trim)
        .filter(|engine_id| !engine_id.is_empty())
        .map(str::to_string)
        .or_else(|| {
            profile
                .agent
                .command
                .as_deref()
                .map(crate::helpers::infer_agent_id_from_command)
                .map(|engine_id| engine_id.trim().to_string())
                .filter(|engine_id| !engine_id.is_empty())
        })
}

fn profile_engine_id(profile: &IlhaeProfileConfig) -> String {
    profile_engine_id_for_display(profile).unwrap_or_else(|| "ilhae".to_string())
}

fn profile_runtime_model_name(profile: &IlhaeProfileConfig) -> Option<String> {
    let raw_model_path = profile.native_runtime.model_path.trim();
    if raw_model_path.is_empty() || raw_model_path.eq_ignore_ascii_case("default") {
        return None;
    }

    let model_name = Path::new(raw_model_path)
        .file_stem()
        .or_else(|| Path::new(raw_model_path).file_name())
        .map(|name| name.to_string_lossy().trim().to_string())
        .filter(|name| !name.is_empty())?;

    if model_name.eq_ignore_ascii_case("default") {
        None
    } else {
        Some(model_name)
    }
}

fn native_runtime_for_profile(
    profile: &IlhaeProfileConfig,
) -> Option<&IlhaeProfileNativeRuntimeConfig> {
    if profile.native_runtime.enabled {
        Some(&profile.native_runtime)
    } else {
        None
    }
}

fn codex_profile_table_for_ilhae_profile(profile: &IlhaeProfileConfig) -> toml::value::Table {
    let native = native_runtime_for_profile(profile);
    let mut table = toml::value::Table::new();
    let model_name = native
        .and_then(|runtime| {
            Path::new(&runtime.model_path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })
        .or_else(|| profile.agent.command.clone())
        .or_else(|| profile.agent.engine_id.clone())
        .unwrap_or_else(|| "ilhae".to_string());
    let model_context_window = native
        .map(|runtime| parse_context_window_from_native_args(&runtime.args))
        .unwrap_or(32_768);
    let model_provider = native
        .and_then(|runtime| runtime.provider.clone())
        .filter(|provider| !provider.trim().is_empty())
        .unwrap_or_else(|| {
            if native.is_some() {
                "llama-server".to_string()
            } else {
                profile_engine_id(profile)
            }
        });

    table.insert("model".to_string(), toml::Value::String(model_name));
    table.insert(
        "model_context_window".to_string(),
        toml::Value::Integer(model_context_window as i64),
    );
    table.insert(
        "model_provider".to_string(),
        toml::Value::String(model_provider),
    );

    if let Some(url) = native
        .map(|runtime| runtime.base_url.trim())
        .filter(|url| !url.is_empty())
    {
        table.insert("url".to_string(), toml::Value::String(url.to_string()));
    }

    table
}

fn user_mcp_servers_for_managed_config() -> toml::value::Table {
    let Ok(config_str) = std::fs::read_to_string(resolve_ilhae_config_toml_path()) else {
        return toml::value::Table::new();
    };
    let Ok(config) = config_str.parse::<toml::Value>() else {
        return toml::value::Table::new();
    };
    let mut servers = config
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default();

    disable_duplicate_legacy_fortune_mcp_server(&mut servers);
    servers
}

fn is_fortune_mcp_server(value: &toml::Value) -> bool {
    let Some(table) = value.as_table() else {
        return false;
    };
    table
        .get("url")
        .and_then(toml::Value::as_str)
        .is_some_and(|url| url.trim_end_matches('/') == "https://fortune.ugot.uk/mcp")
        || table
            .get("oauth_resource")
            .and_then(toml::Value::as_str)
            .is_some_and(|url| url.trim_end_matches('/') == "https://fortune.ugot.uk/mcp")
}

fn disable_duplicate_legacy_fortune_mcp_server(servers: &mut toml::value::Table) {
    let has_canonical_fortune = servers
        .get("ugot_fortune")
        .is_some_and(is_fortune_mcp_server);
    let has_legacy_fortune = servers.get("fortune").is_some_and(is_fortune_mcp_server);

    if has_canonical_fortune
        && has_legacy_fortune
        && let Some(table) = servers
            .get_mut("fortune")
            .and_then(toml::Value::as_table_mut)
    {
        table.insert("enabled".to_string(), toml::Value::Boolean(false));
    }
}

fn user_model_providers_for_managed_config() -> toml::value::Table {
    let Ok(config_str) = std::fs::read_to_string(resolve_ilhae_config_toml_path()) else {
        return toml::value::Table::new();
    };
    let Ok(config) = config_str.parse::<toml::Value>() else {
        return toml::value::Table::new();
    };
    config
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default()
}

fn default_ilhae_codex_home_table() -> toml::value::Table {
    let config = load_ilhae_toml_config();
    let active_profile_name = config
        .profile
        .active
        .clone()
        .filter(|value| config.profiles.contains_key(value));
    let active_profile = active_profile_name
        .as_ref()
        .and_then(|id| config.profiles.get(id))
        .cloned()
        .unwrap_or_default();

    let mut root = toml::value::Table::new();

    root.insert(
        "approval_policy".to_string(),
        toml::Value::String("never".to_string()),
    );
    root.insert(
        "profile".to_string(),
        toml::Value::String(
            active_profile_name
                .clone()
                .unwrap_or_else(|| "ilhae-active".to_string()),
        ),
    );
    root.insert(
        "sandbox_mode".to_string(),
        toml::Value::String("danger-full-access".to_string()),
    );

    let mut agent = toml::value::Table::new();
    agent.insert(
        "active_profile".to_string(),
        toml::Value::String(
            active_profile_name
                .clone()
                .unwrap_or_else(|| "ilhae-active".to_string()),
        ),
    );
    if let Some(command) = active_profile.agent.command.clone() {
        agent.insert("command".to_string(), toml::Value::String(command));
    }
    agent.insert(
        "team_mode".to_string(),
        toml::Value::Boolean(active_profile.agent.team_mode),
    );
    agent.insert(
        "team_backend".to_string(),
        toml::Value::String(normalize_team_backend(&active_profile.agent.team_backend)),
    );
    agent.insert(
        "team_merge_policy".to_string(),
        toml::Value::String(active_profile.agent.team_merge_policy.clone()),
    );
    agent.insert(
        "team_max_retries".to_string(),
        toml::Value::Integer(active_profile.agent.team_max_retries as i64),
    );
    agent.insert(
        "team_pause_on_error".to_string(),
        toml::Value::Boolean(active_profile.agent.team_pause_on_error),
    );
    agent.insert(
        "autonomous_mode".to_string(),
        toml::Value::Boolean(active_profile.agent.auto_mode),
    );
    agent.insert(
        "advisor_mode".to_string(),
        toml::Value::Boolean(active_profile.agent.advisor),
    );
    agent.insert(
        "advisor_preset".to_string(),
        toml::Value::String(active_profile.agent.advisor_preset.clone()),
    );
    agent.insert(
        "auto_max_turns".to_string(),
        toml::Value::Integer(active_profile.agent.auto_max_turns as i64),
    );
    agent.insert(
        "auto_timebox_minutes".to_string(),
        toml::Value::Integer(active_profile.agent.auto_timebox_minutes as i64),
    );
    agent.insert(
        "auto_pause_on_error".to_string(),
        toml::Value::Boolean(active_profile.agent.auto_pause_on_error),
    );
    agent.insert(
        "kairos_enabled".to_string(),
        toml::Value::Boolean(active_profile.agent.kairos),
    );
    agent.insert(
        "self_improvement_enabled".to_string(),
        toml::Value::Boolean(active_profile.agent.self_improvement),
    );
    agent.insert(
        "self_improvement_preset".to_string(),
        toml::Value::String(active_profile.agent.self_improvement_preset.clone()),
    );
    root.insert("agent".to_string(), toml::Value::Table(agent));
    let managed_settings =
        settings_for_managed_profile(active_profile_name.clone(), &active_profile);
    if let Some(developer_instructions) =
        crate::session_context_service::build_runtime_loop_developer_instructions(&managed_settings)
    {
        root.insert(
            "developer_instructions".to_string(),
            toml::Value::String(developer_instructions),
        );
    }

    let mut features = toml::value::Table::new();
    features.insert("fast_mode".to_string(), toml::Value::Boolean(true));
    features.insert("multi_agent".to_string(), toml::Value::Boolean(true));
    root.insert("features".to_string(), toml::Value::Table(features));
    root.insert(
        "mcp_oauth_credentials_store".to_string(),
        toml::Value::String("file".to_string()),
    );

    let mut mcp_servers = toml::value::Table::new();

    if std::env::var("ILHAE_DREAM_MODE").is_err() {
        let mut brain = toml::value::Table::new();
        brain.insert(
            "command".to_string(),
            toml::Value::String("brain".to_string()),
        );
        brain.insert(
            "args".to_string(),
            toml::Value::Array(vec![toml::Value::String("mcp".to_string())]),
        );
        mcp_servers.insert("brain".to_string(), toml::Value::Table(brain));

        let mut browser = toml::value::Table::new();
        browser.insert(
            "command".to_string(),
            toml::Value::String("browser".to_string()),
        );
        browser.insert(
            "args".to_string(),
            toml::Value::Array(vec![toml::Value::String("mcp".to_string())]),
        );
        mcp_servers.insert("browser".to_string(), toml::Value::Table(browser));

        let mut computer = toml::value::Table::new();
        computer.insert(
            "command".to_string(),
            toml::Value::String("computer".to_string()),
        );
        computer.insert(
            "args".to_string(),
            toml::Value::Array(vec![toml::Value::String("mcp".to_string())]),
        );
        mcp_servers.insert("computer".to_string(), toml::Value::Table(computer));

        let mut email = toml::value::Table::new();
        email.insert(
            "command".to_string(),
            toml::Value::String("email".to_string()),
        );
        email.insert(
            "args".to_string(),
            toml::Value::Array(vec![toml::Value::String("mcp".to_string())]),
        );
        mcp_servers.insert("email".to_string(), toml::Value::Table(email));
    }

    for (name, server) in user_mcp_servers_for_managed_config() {
        mcp_servers.insert(name, server);
    }

    root.insert("mcp_servers".to_string(), toml::Value::Table(mcp_servers));

    let model_providers = user_model_providers_for_managed_config();
    if !model_providers.is_empty() {
        root.insert(
            "model_providers".to_string(),
            toml::Value::Table(model_providers),
        );
    }

    let mut plugins = toml::value::Table::new();
    for plugin in [
        "canva@openai-curated",
        "github@openai-curated",
        "gmail@openai-curated",
    ] {
        let mut entry = toml::value::Table::new();
        entry.insert("enabled".to_string(), toml::Value::Boolean(true));
        plugins.insert(plugin.to_string(), toml::Value::Table(entry));
    }
    root.insert("plugins".to_string(), toml::Value::Table(plugins));

    let mut profiles = toml::value::Table::new();
    for (profile_id, profile) in &config.profiles {
        profiles.insert(
            profile_id.clone(),
            toml::Value::Table(codex_profile_table_for_ilhae_profile(profile)),
        );
    }
    profiles.insert(
        "ilhae-active".to_string(),
        toml::Value::Table(codex_profile_table_for_ilhae_profile(&active_profile)),
    );
    root.insert("profiles".to_string(), toml::Value::Table(profiles));

    let mut projects = toml::value::Table::new();
    for (project_path, project) in &config.projects {
        let Some(trust_level) = project.trust_level else {
            continue;
        };
        let mut project_table = toml::value::Table::new();
        project_table.insert(
            "trust_level".to_string(),
            toml::Value::String(trust_level.to_string()),
        );
        projects.insert(project_path.clone(), toml::Value::Table(project_table));
    }
    if !projects.is_empty() {
        root.insert("projects".to_string(), toml::Value::Table(projects));
    }

    root
}

pub fn prepare_ilhae_codex_home() -> Result<PathBuf, String> {
    let codex_home = resolve_ilhae_codex_home_dir();
    std::fs::create_dir_all(&codex_home).map_err(|err| err.to_string())?;

    let ilhae_config_dir = resolve_ilhae_config_dir();
    let codex_config_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex");
    let auth_source_dirs = [ilhae_config_dir, codex_config_dir];
    for auth_file in ["auth.json", ".credentials.json"] {
        let current_auth = codex_home.join(auth_file);
        if current_auth.exists() {
            continue;
        }
        for source_dir in &auth_source_dirs {
            let source_auth = source_dir.join(auth_file);
            if !source_auth.exists() {
                continue;
            }
            if let Err(err) = std::fs::copy(&source_auth, &current_auth) {
                warn!(
                    "Failed to seed {:?} -> {:?}: {}",
                    source_auth, current_auth, err
                );
            }
            break;
        }
    }

    let root = default_ilhae_codex_home_table();
    let rendered =
        toml::to_string_pretty(&toml::Value::Table(root)).map_err(|err| err.to_string())?;
    std::fs::write(codex_home.join("managed_config.toml"), rendered)
        .map_err(|err| err.to_string())?;

    // SAFETY: ilhae sets CODEX_HOME once during single-threaded CLI startup,
    // before any worker threads or async tasks that could concurrently depend
    // on environment mutation are spawned.
    unsafe {
        std::env::set_var("CODEX_HOME", &codex_home);
        std::env::set_var("ILHAE_RUNTIME", "1");
    }
    if let Some(system2) = get_active_system2_target_config() {
        unsafe {
            std::env::set_var("ILHAE_SYSTEM2_ENABLED", "1");
            std::env::set_var("ILHAE_SYSTEM2_PROFILE", system2.target_profile_id);
            std::env::set_var("ILHAE_SYSTEM2_BASE_URL", system2.base_url);
            std::env::set_var("ILHAE_SYSTEM2_MODEL", system2.model_name);
        }
    } else {
        unsafe {
            std::env::remove_var("ILHAE_SYSTEM2_ENABLED");
            std::env::remove_var("ILHAE_SYSTEM2_PROFILE");
            std::env::remove_var("ILHAE_SYSTEM2_BASE_URL");
            std::env::remove_var("ILHAE_SYSTEM2_MODEL");
        }
    }
    Ok(codex_home)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: tests mutate env in a scoped, single-process context and restore it on drop.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: tests restore process env to its previous value before exiting scope.
            unsafe {
                if let Some(previous) = self.previous.as_deref() {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[test]
    fn prepare_ilhae_codex_home_projects_named_profiles_into_managed_config() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("nemotron-local".to_string());

        let mut nemotron = IlhaeProfileConfig::default();
        nemotron.agent.engine_id = Some("ilhae".to_string());
        nemotron.agent.auto_mode = true;
        nemotron.agent.auto_max_turns = 8;
        nemotron.native_runtime.enabled = true;
        nemotron.native_runtime.base_url = "http://127.0.0.1:8081/v1".to_string();
        nemotron.native_runtime.model_path = "/models/gemma-4-26b.gguf".to_string();
        nemotron.native_runtime.args = vec!["--ctx-size".to_string(), "65536".to_string()];
        config
            .profiles
            .insert("nemotron-local".to_string(), nemotron.clone());

        let mut review = IlhaeProfileConfig::default();
        review.agent.engine_id = Some("openai".to_string());
        review.agent.command = Some("codex".to_string());
        config.profiles.insert("review".to_string(), review.clone());

        save_ilhae_toml_config(&config).expect("save config");
        let config_path = tmp.path().join("config.toml");
        let mut config_toml = std::fs::read_to_string(&config_path).expect("read config");
        config_toml.push_str(
            r#"
[mcp_servers.fortune]
url = "https://fortune.ugot.uk/mcp"
enabled = true
scopes = ["openid", "profile", "email", "mcp.read", "mcp.write"]
oauth_resource = "https://fortune.ugot.uk/mcp"

[model_providers.minimax-turboquant]
name = "MiniMax local"
base_url = "http://127.0.0.1:8080/v1"
wire_api = "responses"
requires_openai_auth = false
"#,
        );
        std::fs::write(config_path, config_toml).expect("write config with mcp");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let codex_home = tmp.path().join("codex-home");
        let managed = std::fs::read_to_string(codex_home.join("managed_config.toml"))
            .expect("read managed config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse managed config");
        let root = parsed.as_table().expect("root table");

        assert_eq!(
            root.get("profile").and_then(toml::Value::as_str),
            Some("nemotron-local")
        );
        assert_eq!(
            root.get("mcp_oauth_credentials_store")
                .and_then(toml::Value::as_str),
            Some("file")
        );
        let agent = root
            .get("agent")
            .and_then(toml::Value::as_table)
            .expect("agent table");
        assert_eq!(
            agent.get("active_profile").and_then(toml::Value::as_str),
            Some("nemotron-local")
        );
        assert_eq!(
            agent.get("autonomous_mode").and_then(toml::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            agent
                .get("auto_max_turns")
                .and_then(toml::Value::as_integer),
            Some(8)
        );

        let profiles = root
            .get("profiles")
            .and_then(toml::Value::as_table)
            .expect("profiles table");
        assert!(profiles.contains_key("nemotron-local"));
        assert!(profiles.contains_key("review"));
        assert!(profiles.contains_key("ilhae-active"));

        let nemotron_profile = profiles
            .get("nemotron-local")
            .and_then(toml::Value::as_table)
            .expect("nemotron profile");
        assert_eq!(
            nemotron_profile.get("model").and_then(toml::Value::as_str),
            Some("gemma-4-26b")
        );
        assert_eq!(
            nemotron_profile
                .get("model_provider")
                .and_then(toml::Value::as_str),
            Some("llama-server")
        );
        assert_eq!(
            nemotron_profile.get("url").and_then(toml::Value::as_str),
            Some("http://127.0.0.1:8081/v1")
        );
        assert_eq!(
            nemotron_profile
                .get("model_context_window")
                .and_then(toml::Value::as_integer),
            Some(65_536)
        );

        let review_profile = profiles
            .get("review")
            .and_then(toml::Value::as_table)
            .expect("review profile");
        assert_eq!(
            review_profile
                .get("model_provider")
                .and_then(toml::Value::as_str),
            Some("openai")
        );
        assert!(review_profile.get("url").is_none());

        let mcp_servers = root
            .get("mcp_servers")
            .and_then(toml::Value::as_table)
            .expect("mcp servers table");
        let fortune = mcp_servers
            .get("fortune")
            .and_then(toml::Value::as_table)
            .expect("fortune mcp server");
        assert_eq!(
            fortune.get("url").and_then(toml::Value::as_str),
            Some("https://fortune.ugot.uk/mcp")
        );

        let model_providers = root
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .expect("model providers table");
        let minimax_provider = model_providers
            .get("minimax-turboquant")
            .and_then(toml::Value::as_table)
            .expect("minimax provider");
        assert_eq!(
            minimax_provider
                .get("requires_openai_auth")
                .and_then(toml::Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn prepare_ilhae_codex_home_projects_foreground_loop_instructions_into_managed_config() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("foreground-local".to_string());

        let mut foreground = IlhaeProfileConfig::default();
        foreground.agent.command = Some("ilhae".to_string());
        foreground.agent.kairos = true;
        foreground.agent.self_improvement = true;
        foreground.agent.self_improvement_preset = "foreground".to_string();
        foreground.knowledge = Some(IlhaeProfileKnowledgeConfig {
            mode: "both".to_string(),
            ..IlhaeProfileKnowledgeConfig::default()
        });
        config
            .profiles
            .insert("foreground-local".to_string(), foreground);

        save_ilhae_toml_config(&config).expect("save config");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/managed_config.toml"))
            .expect("read managed config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse managed config");
        let instructions = parsed
            .get("developer_instructions")
            .and_then(toml::Value::as_str)
            .expect("developer instructions");

        assert!(instructions.contains("ILHAE RUNTIME LOOP STATE"));
        assert!(instructions.contains("- Knowledge loop: enabled (both)"));
        assert!(instructions.contains("- Preset: foreground"));
        assert!(instructions.contains("SELF-IMPROVEMENT SKILL LOOP"));
    }

    #[test]
    fn prepare_ilhae_codex_home_defaults_self_improvement_foreground_loop() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        save_ilhae_toml_config(&IlhaeTomlConfig::default()).expect("save default config");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/managed_config.toml"))
            .expect("read managed config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse managed config");
        let agent = parsed
            .get("agent")
            .and_then(toml::Value::as_table)
            .expect("agent table");
        let instructions = parsed
            .get("developer_instructions")
            .and_then(toml::Value::as_str)
            .expect("developer instructions");

        assert_eq!(
            agent
                .get("self_improvement_enabled")
                .and_then(toml::Value::as_bool),
            Some(true)
        );
        assert!(instructions.contains("- Self-improvement: enabled"));
        assert!(instructions.contains("- Preset: foreground"));
        assert!(instructions.contains("SELF-IMPROVEMENT SKILL LOOP"));
    }

    #[test]
    fn get_native_runtime_config_accepts_non_ilhae_engine_profiles() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("qwen3.6-local".to_string());

        let mut local = IlhaeProfileConfig::default();
        local.agent.engine_id = Some("llama-server".to_string());
        local.agent.command = Some("ilhae".to_string());
        local.native_runtime.enabled = true;
        local.native_runtime.base_url = "http://127.0.0.1:8081/v1".to_string();
        local.native_runtime.health_url = "http://127.0.0.1:8081/health".to_string();
        local.native_runtime.model_path = "/models/Qwen3.6-35B.gguf".to_string();
        config.profiles.insert("qwen3.6-local".to_string(), local);

        save_ilhae_toml_config(&config).expect("save config");

        let (profile_id, runtime) =
            get_native_runtime_config(None).expect("native runtime config for local profile");
        assert_eq!(profile_id, "qwen3.6-local");
        assert_eq!(runtime.base_url, "http://127.0.0.1:8081/v1");
        assert_eq!(runtime.health_url, "http://127.0.0.1:8081/health");
    }

    #[test]
    fn prepare_ilhae_codex_home_projects_non_ilhae_native_profiles_into_managed_config() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("qwen3.6-local".to_string());

        let mut local = IlhaeProfileConfig::default();
        local.agent.engine_id = Some("llama-server".to_string());
        local.agent.command = Some("ilhae".to_string());
        local.native_runtime.enabled = true;
        local.native_runtime.base_url = "http://127.0.0.1:8081/v1".to_string();
        local.native_runtime.provider = Some("sglang".to_string());
        local.native_runtime.model_path = "/models/Qwen3.6-35B-A3B.gguf".to_string();
        config.profiles.insert("qwen3.6-local".to_string(), local);

        save_ilhae_toml_config(&config).expect("save config");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/managed_config.toml"))
            .expect("read managed config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse managed config");
        let profiles = parsed
            .get("profiles")
            .and_then(toml::Value::as_table)
            .expect("profiles table");
        let local_profile = profiles
            .get("qwen3.6-local")
            .and_then(toml::Value::as_table)
            .expect("local profile");

        assert_eq!(
            local_profile.get("model").and_then(toml::Value::as_str),
            Some("Qwen3.6-35B-A3B")
        );
        assert_eq!(
            local_profile
                .get("model_provider")
                .and_then(toml::Value::as_str),
            Some("sglang")
        );
        assert_eq!(
            local_profile.get("url").and_then(toml::Value::as_str),
            Some("http://127.0.0.1:8081/v1")
        );
    }

    #[test]
    fn profile_runtime_display_parts_identifies_native_provider_and_model() {
        let mut local = IlhaeProfileConfig::default();
        local.agent.engine_id = Some("ilhae".to_string());
        local.native_runtime.enabled = true;
        local.native_runtime.model_path =
            "/models/Qwen3.6-27B-GGUF/Qwen3.6-27B-UD-Q4_K_XL.gguf".to_string();

        assert_eq!(
            profile_runtime_display_parts(&local),
            vec!["llama-server", "Qwen3.6-27B-UD-Q4_K_XL"]
        );

        local.native_runtime.provider = Some("luce-dflash".to_string());

        assert_eq!(
            profile_runtime_display_parts(&local),
            vec!["luce-dflash", "Qwen3.6-27B-UD-Q4_K_XL"]
        );
    }

    #[test]
    fn profile_runtime_display_parts_keeps_remote_engine_and_model_metadata() {
        let mut minimax = IlhaeProfileConfig::default();
        minimax.agent.engine_id = Some("minimax-turboquant".to_string());
        minimax.native_runtime.base_url = "http://192.168.219.113:8080/v1".to_string();
        minimax.native_runtime.model_path =
            "/models/MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf".to_string();

        assert_eq!(
            profile_runtime_display_parts(&minimax),
            vec![
                "minimax-turboquant",
                "MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003",
                "remote",
            ]
        );

        let mut sglang = IlhaeProfileConfig::default();
        sglang.agent.engine_id = Some("sglang".to_string());
        sglang.native_runtime.base_url = "http://192.168.219.113:30000/v1".to_string();
        sglang.native_runtime.model_path = "default".to_string();

        assert_eq!(
            profile_runtime_display_parts(&sglang),
            vec!["sglang", "server-default", "remote"]
        );
    }

    #[test]
    fn current_thinking_mode_reads_persisted_setting() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        assert_eq!(current_thinking_mode(), "on");

        let settings_store = crate::settings_store::SettingsStore::new(&tmp.path().join("data"));
        settings_store
            .set_value("agent.thinking_mode", serde_json::json!("off"))
            .expect("persist thinking mode");

        assert_eq!(current_thinking_mode(), "off");
        assert!(!current_thinking_enabled());
    }

    #[test]
    fn prepare_ilhae_codex_home_preserves_oauth_credentials() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        std::fs::write(tmp.path().join("config.toml"), "[profile]\n").expect("write config");
        std::fs::write(tmp.path().join(".credentials.json"), r#"{"mcp":"token"}"#)
            .expect("write credentials");
        std::fs::write(tmp.path().join("auth.json"), r#"{"auth":"token"}"#).expect("write auth");

        prepare_ilhae_codex_home().expect("prepare codex home");

        assert_eq!(
            std::fs::read_to_string(tmp.path().join(".credentials.json"))
                .expect("read credentials"),
            r#"{"mcp":"token"}"#
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("codex-home/.credentials.json"))
                .expect("read codex credentials"),
            r#"{"mcp":"token"}"#
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("auth.json")).expect("read auth"),
            r#"{"auth":"token"}"#
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("codex-home/auth.json"))
                .expect("read codex auth"),
            r#"{"auth":"token"}"#
        );
    }

    #[test]
    fn prepare_ilhae_codex_home_disables_duplicate_legacy_fortune_server() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        save_ilhae_toml_config(&IlhaeTomlConfig::default()).expect("save config");
        let config_path = tmp.path().join("config.toml");
        let mut config_toml = std::fs::read_to_string(&config_path).expect("read config");
        config_toml.push_str(
            r#"
[mcp_servers.fortune]
url = "https://fortune.ugot.uk/mcp"
enabled = true
scopes = ["openid", "profile", "email", "mcp.read", "mcp.write"]
oauth_resource = "https://fortune.ugot.uk/mcp"

[mcp_servers.ugot_fortune]
url = "https://fortune.ugot.uk/mcp"
enabled = true
scopes = ["openid", "profile", "email", "offline_access", "mcp.read", "mcp.write"]
oauth_resource = "https://fortune.ugot.uk/mcp"
"#,
        );
        std::fs::write(config_path, config_toml).expect("write config with duplicate fortune");

        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/managed_config.toml"))
            .expect("read managed config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse managed config");
        let mcp_servers = parsed
            .get("mcp_servers")
            .and_then(toml::Value::as_table)
            .expect("mcp servers table");
        let legacy = mcp_servers
            .get("fortune")
            .and_then(toml::Value::as_table)
            .expect("legacy fortune server");
        let canonical = mcp_servers
            .get("ugot_fortune")
            .and_then(toml::Value::as_table)
            .expect("canonical fortune server");

        assert_eq!(
            legacy.get("enabled").and_then(toml::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            canonical.get("enabled").and_then(toml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn prepare_ilhae_codex_home_seeds_auth_from_codex_home_fallback() {
        let tmp = tempdir().expect("tempdir");
        let ilhae_dir = tmp.path().join(".ilhae");
        let home_dir = tmp.path().join("home");
        let codex_dir = home_dir.join(".codex");
        std::fs::create_dir_all(&ilhae_dir).expect("create ilhae dir");
        std::fs::create_dir_all(&codex_dir).expect("create codex dir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &ilhae_dir);
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _home_guard = EnvVarGuard::set("HOME", &home_dir);

        std::fs::write(ilhae_dir.join("config.toml"), "[profile]\n").expect("write config");
        std::fs::write(codex_dir.join("auth.json"), r#"{"codex":"auth"}"#)
            .expect("write codex auth");

        prepare_ilhae_codex_home().expect("prepare codex home");

        assert_eq!(
            std::fs::read_to_string(ilhae_dir.join("codex-home/auth.json"))
                .expect("read seeded auth"),
            r#"{"codex":"auth"}"#
        );
    }

    #[test]
    fn prepare_ilhae_codex_home_preserves_existing_codex_home_oauth_credentials() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        std::fs::write(tmp.path().join("config.toml"), "[profile]\n").expect("write config");
        let codex_home = tmp.path().join("codex-home");
        std::fs::create_dir_all(&codex_home).expect("create codex home");
        std::fs::write(tmp.path().join(".credentials.json"), r#"{"root":"mcp"}"#)
            .expect("write root credentials");
        std::fs::write(codex_home.join(".credentials.json"), r#"{"codex":"mcp"}"#)
            .expect("write codex credentials");

        prepare_ilhae_codex_home().expect("prepare codex home");

        assert_eq!(
            std::fs::read_to_string(codex_home.join(".credentials.json"))
                .expect("read codex credentials"),
            r#"{"codex":"mcp"}"#
        );
    }

    #[test]
    fn prepare_ilhae_codex_home_carries_project_trust_into_managed_config() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let config_toml = r#"
[profile]
active = "qwen-35b-sglang"

[profiles.qwen-35b-sglang.agent]
engine = "sglang"
command = "ilhae"

[projects."/mnt/nvme0n1p2/workspace/monorepo"]
trust_level = "trusted"

[projects."/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent"]
trust_level = "untrusted"
"#;
        std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/managed_config.toml"))
            .expect("read managed config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse managed config");
        let projects = parsed
            .get("projects")
            .and_then(toml::Value::as_table)
            .expect("projects table");

        let trusted = projects
            .get("/mnt/nvme0n1p2/workspace/monorepo")
            .and_then(toml::Value::as_table)
            .expect("trusted project entry");
        assert_eq!(
            trusted.get("trust_level").and_then(toml::Value::as_str),
            Some("trusted")
        );

        let untrusted = projects
            .get("/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent")
            .and_then(toml::Value::as_table)
            .expect("untrusted project entry");
        assert_eq!(
            untrusted.get("trust_level").and_then(toml::Value::as_str),
            Some("untrusted")
        );
    }

    #[test]
    fn prepare_ilhae_codex_home_sets_system2_env_projection() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let config_toml = r#"
[profile]
active = "qwen3.6-local"

[profiles."qwen3.6-local".agent]
engine = "ilhae"
command = "ilhae"

[profiles."qwen3.6-local".system2]
enabled = true
profile = "minimax-m2.7-turboquant"

[profiles."minimax-m2.7-turboquant".agent]
engine = "ilhae"
command = "ilhae"

[profiles."minimax-m2.7-turboquant".native_runtime]
enabled = false
base_url = "http://192.168.219.113:8080/v1"
model_path = "/home/sk/models/MiniMax-M2.7-GGUF/UD-IQ3_XXS/MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf"
"#;
        std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

        prepare_ilhae_codex_home().expect("prepare codex home");

        assert!(
            std::env::var("ILHAE_SYSTEM2_ENABLED").ok().as_deref() == Some("1"),
            "system2 should be enabled"
        );
        assert_eq!(
            std::env::var("ILHAE_SYSTEM2_PROFILE").ok().as_deref(),
            Some("minimax-m2.7-turboquant")
        );
        assert_eq!(
            std::env::var("ILHAE_SYSTEM2_BASE_URL").ok().as_deref(),
            Some("http://192.168.219.113:8080/v1")
        );
        assert_eq!(
            std::env::var("ILHAE_SYSTEM2_MODEL").ok().as_deref(),
            Some("MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf")
        );
    }

    #[test]
    fn active_profile_system2_resolves_target_profile() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let config_toml = r#"
[profile]
active = "qwen3.6-local"

[profiles."qwen3.6-local".agent]
engine = "ilhae"
command = "ilhae"

[profiles."qwen3.6-local".system2]
enabled = true
profile = "minimax-m2.7-turboquant"

[profiles."minimax-m2.7-turboquant".agent]
engine = "minimax-turboquant"
command = "ilhae"

[profiles."minimax-m2.7-turboquant".native_runtime]
enabled = false
base_url = "http://192.168.219.113:8080/v1"
model_path = "/home/sk/models/MiniMax-M2.7-GGUF/UD-IQ3_XXS/MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf"
"#;
        std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

        let resolved = get_active_system2_target_config().expect("system2 target");
        assert_eq!(resolved.source_profile_id, "qwen3.6-local");
        assert_eq!(resolved.target_profile_id, "minimax-m2.7-turboquant");
        assert_eq!(resolved.base_url, "http://192.168.219.113:8080/v1");
        assert_eq!(
            resolved.model_name,
            "MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf"
        );
    }
}
