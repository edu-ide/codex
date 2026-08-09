use codex_protocol::config_types::TrustLevel;
use std::collections::BTreeMap;
use std::fs::File;
use std::fs::OpenOptions;
use std::fs::TryLockError as FileTryLockError;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::TryLockError;
use std::time::Duration;
use std::time::Instant;
use tracing::info;
use tracing::warn;

use crate::settings_store::SettingsStore;
use crate::settings_types::default_advisor_preset;
use crate::settings_types::default_approval_preset;
use crate::settings_types::default_auto_max_turns;
use crate::settings_types::default_auto_pause_on_error;
use crate::settings_types::default_auto_timebox_minutes;
use crate::settings_types::default_knowledge_mode;
use crate::settings_types::default_knowledge_periodic_interval_secs;
use crate::settings_types::default_knowledge_poll_interval_secs;
use crate::settings_types::default_knowledge_report_relative_path;
use crate::settings_types::default_knowledge_report_target;
use crate::settings_types::default_self_improvement_enabled;
use crate::settings_types::default_self_improvement_preset;
use crate::settings_types::default_team_backend;
use crate::settings_types::default_team_max_retries;
use crate::settings_types::default_team_merge_policy;
use crate::settings_types::default_team_pause_on_error;
use crate::settings_types::default_thinking_mode;
use crate::settings_types::normalize_thinking_mode;
use crate::settings_types::thinking_mode_enabled;

pub const ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE: &str = ".config.toml.ilhae-lkg";
pub const ILHAE_CODEX_RUNTIME_CONFIG_LOCK_FILE: &str = ".config.toml.ilhae-runtime.lock";
const ILHAE_CODEX_MODEL_CATALOG_FILE: &str = "model_catalog.json";
const ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY: &str = "ilhae_runtime_system2_projection";
const ILHAE_RUNTIME_SYSTEM2_PROJECTION_SCHEMA_VERSION: i64 = 1;
const DESKTOP_MATERIALIZED_MCP_SERVER_NAME: &str = "office";
const RETIRED_EXCEL_MCP_SERVER_NAME: &str = "excel-mcp";
const ILHAE_CODEX_RUNTIME_CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const ILHAE_CODEX_RUNTIME_CONFIG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);
static ILHAE_CODEX_RUNTIME_CONFIG_LOCK: Mutex<()> = Mutex::new(());

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

fn comparable_config_path(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path).ok();
    }
    let parent = path.parent()?;
    let file_name = path.file_name()?;
    std::fs::canonicalize(parent)
        .ok()
        .map(|canonical_parent| canonical_parent.join(file_name))
}

fn paths_reference_same_config_file(left: &Path, right: &Path) -> bool {
    if comparable_config_path(left)
        .zip(comparable_config_path(right))
        .is_some_and(|(left, right)| left == right)
    {
        return true;
    }

    #[cfg(unix)]
    {
        if let (Ok(left), Ok(right)) = (std::fs::metadata(left), std::fs::metadata(right)) {
            return left.dev() == right.dev() && left.ino() == right.ino();
        }
    }

    false
}

fn runtime_home_aliases_human_config(runtime_home: &Path) -> bool {
    let runtime_config = runtime_home.join("config.toml");
    let primary_source = resolve_ilhae_config_toml_path();
    paths_reference_same_config_file(&primary_source, &runtime_config)
        || resolve_human_ilhae_config_path()
            .filter(|source| source != &primary_source)
            .is_some_and(|source| paths_reference_same_config_file(&source, &runtime_config))
}

/// Human intent and the validated Codex runtime snapshot must never share a
/// file identity. If an explicit home aliases the human config directory (even
/// through a symlink or hardlink), move only the generated runtime state into a
/// stable child directory and leave the source bytes untouched.
fn resolve_non_aliasing_ilhae_codex_home_dir() -> Result<PathBuf, String> {
    let requested = resolve_ilhae_codex_home_dir();
    if !runtime_home_aliases_human_config(&requested) {
        return Ok(requested);
    }

    let isolated = if requested.is_file() {
        requested
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".ilhae-runtime-home")
    } else {
        requested.join(".ilhae-runtime-home")
    };
    if runtime_home_aliases_human_config(&isolated) {
        return Err("Human configuration and Codex runtime storage could not be isolated".into());
    }
    warn!("Separated human configuration from generated Codex runtime storage");
    Ok(isolated)
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IlhaeProfileNativeRuntimeConfig {
    pub enabled: bool,
    pub provider: Option<String>,
    pub health_url: String,
    pub url: Option<String>,
    pub base_url: String,
    pub proxy_base_url: Option<String>,
    pub proxy_control_url: Option<String>,
    pub proxy_control_token_env: Option<String>,
    pub server_bin: String,
    pub model_path: String,
    pub chat_template_file: String,
    pub log_file: String,
    pub env: std::collections::BTreeMap<String, String>,
    pub query_params: Option<BTreeMap<String, String>>,
    pub http_headers: Option<BTreeMap<String, String>>,
    pub env_http_headers: Option<BTreeMap<String, String>>,
    pub request_max_retries: Option<u64>,
    pub stream_max_retries: Option<u64>,
    pub context_window: Option<u64>,
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
            url: None,
            base_url: String::new(),
            proxy_base_url: None,
            proxy_control_url: None,
            proxy_control_token_env: None,
            server_bin: String::new(),
            model_path: String::new(),
            chat_template_file: String::new(),
            log_file: String::new(),
            env: std::collections::BTreeMap::new(),
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            context_window: None,
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

fn system2_projection_table_from(config: &IlhaeTomlConfig) -> Option<toml::value::Table> {
    let system2 = active_system2_target_config_from(config)?;
    let mut projection = toml::value::Table::new();
    projection.insert(
        "schema_version".to_string(),
        toml::Value::Integer(ILHAE_RUNTIME_SYSTEM2_PROJECTION_SCHEMA_VERSION),
    );
    projection.insert(
        "source_profile_id".to_string(),
        toml::Value::String(system2.source_profile_id),
    );
    projection.insert(
        "target_profile_id".to_string(),
        toml::Value::String(system2.target_profile_id),
    );
    projection.insert(
        "base_url".to_string(),
        toml::Value::String(system2.base_url),
    );
    projection.insert(
        "model_name".to_string(),
        toml::Value::String(system2.model_name),
    );
    Some(projection)
}

fn system2_projection_from_runtime_document(
    document: &toml::Value,
) -> Result<Option<ResolvedSystem2TargetConfig>, String> {
    let Some(desktop) = document.get("desktop") else {
        return Ok(None);
    };
    let desktop = desktop
        .as_table()
        .ok_or_else(|| "Generated Codex runtime desktop metadata is invalid".to_string())?;
    let Some(projection) = desktop.get(ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY) else {
        return Ok(None);
    };
    let projection = projection
        .as_table()
        .ok_or_else(|| "Generated Codex runtime System2 projection is invalid".to_string())?;
    if projection
        .get("schema_version")
        .and_then(toml::Value::as_integer)
        != Some(ILHAE_RUNTIME_SYSTEM2_PROJECTION_SCHEMA_VERSION)
    {
        return Err("Generated Codex runtime System2 projection schema is invalid".to_string());
    }
    let required = |key| {
        projection
            .get(key)
            .and_then(toml::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("Generated Codex runtime System2 projection is missing {key}"))
    };
    Ok(Some(ResolvedSystem2TargetConfig {
        source_profile_id: required("source_profile_id")?,
        target_profile_id: required("target_profile_id")?,
        base_url: required("base_url")?,
        model_name: required("model_name")?,
    }))
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

fn parse_context_window_from_native_args(args: &[String]) -> Option<u64> {
    let mut idx = 0usize;
    while idx < args.len() {
        let arg = args[idx].trim();
        if matches!(
            arg,
            "-c" | "--ctx-size" | "--context-size" | "--context-length"
        ) {
            if let Some(value) = args.get(idx + 1).and_then(|next| next.parse::<u64>().ok()) {
                return Some(value);
            }
        } else if let Some(value) = arg
            .strip_prefix("--ctx-size=")
            .or_else(|| arg.strip_prefix("--context-size="))
            .or_else(|| arg.strip_prefix("--context-length="))
            .and_then(|value| value.parse::<u64>().ok())
        {
            return Some(value);
        }
        idx += 1;
    }
    None
}

fn parse_context_window_from_native_query_params(
    query_params: Option<&std::collections::BTreeMap<String, String>>,
) -> Option<u64> {
    let query_params = query_params?;
    let mut normalized_lookup =
        |key: &str| -> Option<u64> { query_params.get(key).and_then(|value| value.parse().ok()) };

    if let Some(value) = normalized_lookup("context-size") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("context_size") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("ctx-size") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("ctx_size") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("context-length") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("context_length") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("num-ctx") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("num_ctx") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("n-ctx") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("n_ctx") {
        return Some(value);
    }

    for (raw_key, value) in query_params {
        match raw_key
            .trim()
            .to_ascii_lowercase()
            .replace('_', "-")
            .as_str()
        {
            "context-size" | "ctx-size" | "context-length" | "num-ctx" | "n-ctx" => {
                if let Ok(value) = value.parse() {
                    return Some(value);
                }
            }
            _ => {}
        }
    }

    None
}

fn native_runtime_model_context_window(runtime: &IlhaeProfileNativeRuntimeConfig) -> u64 {
    if let Some(context_window) = runtime
        .context_window
        .filter(|context_window| *context_window > 0)
    {
        return context_window;
    }

    if let Some(context_window) = parse_context_window_from_native_args(&runtime.args) {
        return context_window;
    }

    if let Some(context_window) =
        parse_context_window_from_native_query_params(runtime.query_params.as_ref())
    {
        return context_window;
    }

    32_768
}

fn profile_engine_id_for_display(profile: &IlhaeProfileConfig) -> Option<String> {
    profile
        .agent
        .engine_id
        .as_deref()
        .map(|engine_id| engine_id.trim())
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

    native_runtime_model_name_from_path(raw_model_path)
}

fn codex_model_from_root_config(profile_id: &str) -> Option<String> {
    let config_path = dirs::home_dir().map(|h| h.join(".codex/config.toml"))?;
    if !config_path.exists() {
        return None;
    }

    let config = std::fs::read_to_string(config_path).ok()?;
    let config = config.parse::<toml::Value>().ok()?;

    config
        .get("profiles")
        .and_then(|profiles| profiles.get(profile_id))
        .and_then(|profile| profile.get("model"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .map(str::to_string)
        .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("default"))
        .or_else(|| {
            config
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .map(str::to_string)
                .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("default"))
        })
}

fn fallback_profile_name(profile_id: &str) -> String {
    let trimmed = profile_id.trim();
    if trimmed.is_empty() {
        "ilhae".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn resolve_ilhae_profile_model_name(
    profile_id: &str,
    profile: &IlhaeProfileConfig,
) -> String {
    if let Some(model_name) =
        native_runtime_model_name_from_path(&profile.native_runtime.model_path)
    {
        return model_name;
    }

    if let Some(model_name) = codex_model_from_root_config(profile_id) {
        return model_name;
    }

    if profile
        .agent
        .engine_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|engine_id| engine_id.eq_ignore_ascii_case("openai"))
    {
        return "gpt-5.5".to_string();
    }

    if let Some(engine_id) = profile
        .agent
        .engine_id
        .as_deref()
        .map(str::trim)
        .filter(|engine_id| {
            !engine_id.is_empty()
                && !engine_id.eq_ignore_ascii_case("default")
                && !engine_id.eq_ignore_ascii_case("codex")
                && !engine_id.eq_ignore_ascii_case("openai")
        })
    {
        return engine_id.to_string();
    }

    if let Some(command) = profile
        .agent
        .command
        .as_deref()
        .map(str::trim)
        .filter(|command| {
            !command.is_empty()
                && !command.eq_ignore_ascii_case("default")
                && !command.eq_ignore_ascii_case("codex")
                && !command.eq_ignore_ascii_case("openai")
        })
    {
        return command.to_string();
    }

    fallback_profile_name(profile_id)
}

pub fn native_runtime_model_name_from_path(raw_model_path: &str) -> Option<String> {
    let path = Path::new(raw_model_path.trim());
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let use_stem = matches!(
        extension.as_deref(),
        Some("gguf" | "safetensors" | "bin" | "pt" | "pth" | "onnx" | "ckpt")
    );
    let name = if use_stem {
        path.file_stem().or_else(|| path.file_name())
    } else {
        path.file_name().or_else(|| path.file_stem())
    }?;
    let model_name = name.to_string_lossy().trim().to_string();
    if model_name.is_empty() || model_name.eq_ignore_ascii_case("default") {
        None
    } else {
        Some(model_name)
    }
}

fn string_map_as_toml_value(values: &BTreeMap<String, String>) -> toml::Value {
    let mut table = toml::value::Table::new();
    for (key, value) in values {
        table.insert(key.trim().to_string(), toml::Value::String(value.clone()));
    }
    toml::Value::Table(table)
}

fn native_runtime_effective_query_params(
    runtime: &IlhaeProfileNativeRuntimeConfig,
) -> BTreeMap<String, String> {
    let mut query_params = BTreeMap::new();
    if let Some(overrides) = runtime.query_params.as_ref() {
        for (key, value) in overrides {
            let key = key.trim();
            if !key.is_empty() {
                query_params.insert(key.to_string(), value.clone());
            }
        }
    }
    query_params
}

pub fn native_runtime_effective_base_url(runtime: &IlhaeProfileNativeRuntimeConfig) -> String {
    runtime
        .proxy_base_url
        .as_ref()
        .map(|base_url| base_url.trim())
        .filter(|base_url| !base_url.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            let base_url = runtime.base_url.trim();
            if base_url.is_empty() {
                None
            } else {
                Some(base_url.to_string())
            }
        })
        .or_else(|| {
            let base_url = runtime.url.as_ref().map(|url| url.trim())?;
            if base_url.is_empty() {
                None
            } else {
                Some(base_url.to_string())
            }
        })
        .unwrap_or_else(|| runtime.base_url.trim().to_string())
}

pub fn native_runtime_effective_health_url(runtime: &IlhaeProfileNativeRuntimeConfig) -> String {
    let explicit = runtime.health_url.trim();
    if !explicit.is_empty() {
        return explicit.to_string();
    }

    let base_url = native_runtime_effective_base_url(runtime);
    if base_url.is_empty() {
        return String::new();
    }

    native_runtime_health_url_from_base_url(&base_url)
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

fn native_runtime_for_profile(
    profile: &IlhaeProfileConfig,
) -> Option<&IlhaeProfileNativeRuntimeConfig> {
    Some(&profile.native_runtime)
}

fn native_model_provider_id_for_profile(profile_id: &str) -> String {
    format!("ilhae-native-{profile_id}")
}

fn native_model_provider_table(runtime: &IlhaeProfileNativeRuntimeConfig) -> toml::value::Table {
    let mut table = toml::value::Table::new();
    let base_url = native_runtime_effective_base_url(runtime);
    table.insert(
        "name".to_string(),
        toml::Value::String(
            runtime
                .provider
                .as_deref()
                .map(str::trim)
                .filter(|provider| !provider.is_empty())
                .unwrap_or("llama-server")
                .to_string(),
        ),
    );
    table.insert("base_url".to_string(), toml::Value::String(base_url));
    table.insert(
        "wire_api".to_string(),
        toml::Value::String("responses".to_string()),
    );
    table.insert(
        "requires_openai_auth".to_string(),
        toml::Value::Boolean(false),
    );
    let query_params = native_runtime_effective_query_params(runtime);
    if !query_params.is_empty() {
        table.insert(
            "query_params".to_string(),
            string_map_as_toml_value(&query_params),
        );
    }
    if let Some(http_headers) = runtime.http_headers.as_ref()
        && !http_headers.is_empty()
    {
        table.insert(
            "http_headers".to_string(),
            string_map_as_toml_value(http_headers),
        );
    }
    let mut env_http_headers = runtime.env_http_headers.clone().unwrap_or_default();
    if let Some(token_env) = runtime
        .proxy_control_token_env
        .as_deref()
        .map(str::trim)
        .filter(|token_env| !token_env.is_empty())
    {
        env_http_headers.insert("X-Ilhae-Runtime-Token".to_string(), token_env.to_string());
    }
    if !env_http_headers.is_empty() {
        table.insert(
            "env_http_headers".to_string(),
            string_map_as_toml_value(&env_http_headers),
        );
    }
    if let Some(request_max_retries) = runtime.request_max_retries {
        if let Ok(request_max_retries) = i64::try_from(request_max_retries) {
            table.insert(
                "request_max_retries".to_string(),
                toml::Value::Integer(request_max_retries),
            );
        }
    }
    if let Some(stream_max_retries) = runtime.stream_max_retries {
        if let Ok(stream_max_retries) = i64::try_from(stream_max_retries) {
            table.insert(
                "stream_max_retries".to_string(),
                toml::Value::Integer(stream_max_retries),
            );
        }
    }
    table
}

fn insert_native_model_provider(
    model_providers: &mut toml::value::Table,
    profile_id: &str,
    profile: &IlhaeProfileConfig,
) {
    let Some(runtime) = native_runtime_for_profile(profile) else {
        return;
    };
    if native_runtime_effective_base_url(runtime).is_empty() {
        return;
    }
    model_providers.insert(
        native_model_provider_id_for_profile(profile_id),
        toml::Value::Table(native_model_provider_table(runtime)),
    );
}

fn codex_profile_table_for_ilhae_profile(
    profile_id: &str,
    profile: &IlhaeProfileConfig,
    user_model: Option<&str>,
    model_catalog_path: &Path,
) -> toml::value::Table {
    let native = native_runtime_for_profile(profile);
    let engine = profile_engine_id(profile);
    let mut table = toml::value::Table::new();
    let model_name = resolve_ilhae_profile_model_name(profile_id, profile);
    let model_context_window = native
        .map(native_runtime_model_context_window)
        .unwrap_or(32_768);
    let model_provider = native
        .filter(|runtime| !native_runtime_effective_base_url(runtime).is_empty())
        .map(|_| native_model_provider_id_for_profile(profile_id))
        .or_else(|| {
            native
                .and_then(|runtime| runtime.provider.clone())
                .filter(|provider| !provider.trim().is_empty())
        })
        .unwrap_or_else(|| {
            if engine == "ilhae" || engine == "codex" {
                "llama-server".to_string()
            } else {
                engine.clone()
            }
        });

    // Final safety check: if the resolved provider is "ilhae" or "codex",
    // it MUST be mapped to "llama-server" because "ilhae" is not a valid
    // provider ID in the core engine (it's the engine name).
    let model_provider = if model_provider == "ilhae" || model_provider == "codex" {
        "llama-server".to_string()
    } else {
        model_provider
    };

    table.insert("model".to_string(), toml::Value::String(model_name));
    table.insert(
        "model_context_window".to_string(),
        toml::Value::Integer(model_context_window as i64),
    );
    table.insert(
        "model_provider".to_string(),
        toml::Value::String(model_provider),
    );
    if !native_runtime_effective_base_url(&profile.native_runtime).is_empty() {
        table.insert(
            "model_catalog_json".to_string(),
            toml::Value::String(model_catalog_path.display().to_string()),
        );
    }

    table
}

fn user_mcp_servers_for_managed_config(user_config: &toml::Value) -> toml::value::Table {
    let mut servers = toml::value::Table::new();
    let mut excluded_invalid_count = 0usize;
    for (name, server) in user_table_for_managed_config(user_config, "mcp_servers") {
        // The desktop owns the exact per-turn `office` endpoint; carrying a
        // human snapshot here can make that binding stale. The old Excel alias
        // is intentionally retired. The human source itself remains untouched.
        if name == DESKTOP_MATERIALIZED_MCP_SERVER_NAME || name == RETIRED_EXCEL_MCP_SERVER_NAME {
            continue;
        }
        match server.clone().try_into::<codex_config::McpServerConfig>() {
            Ok(config) if mcp_server_transport_is_semantically_valid(&config) => {
                servers.insert(name, server);
            }
            Ok(_) | Err(_) => excluded_invalid_count += 1,
        }
    }
    if excluded_invalid_count > 0 {
        warn!(
            "Excluded {excluded_invalid_count} invalid MCP configuration entries from the Codex runtime snapshot"
        );
    }

    disable_duplicate_legacy_fortune_mcp_server(&mut servers);
    servers
}

fn mcp_server_transport_is_semantically_valid(config: &codex_config::McpServerConfig) -> bool {
    match &config.transport {
        codex_config::McpServerTransportConfig::Stdio { command, .. } => !command.trim().is_empty(),
        codex_config::McpServerTransportConfig::StreamableHttp { url, .. } => {
            if url.is_empty() || url.trim() != url {
                return false;
            }
            let Ok(parsed) = url::Url::parse(url) else {
                return false;
            };
            matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some()
        }
    }
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

fn user_model_providers_for_managed_config(user_config: &toml::Value) -> toml::value::Table {
    user_table_for_managed_config(user_config, "model_providers")
}

fn user_features_for_managed_config(user_config: &toml::Value) -> toml::value::Table {
    user_table_for_managed_config(user_config, "features")
}

fn user_web_search_for_managed_config(user_config: &toml::Value) -> Option<toml::Value> {
    user_config_value_for_managed_config(user_config, "web_search")
}

fn user_model_for_managed_config(user_config: &toml::Value) -> Option<String> {
    user_config_value_for_managed_config(user_config, "model")
        .and_then(|value| value.as_str().map(str::trim).map(str::to_string))
        .filter(|model| !model.is_empty())
}

fn user_tools_for_managed_config(user_config: &toml::Value) -> toml::value::Table {
    let user_tools = user_table_for_managed_config(user_config, "tools");
    let mut tools = toml::value::Table::new();
    for key in ["web_search", "view_image"] {
        if let Some(value) = user_tools.get(key).cloned() {
            tools.insert(key.to_string(), value);
        }
    }
    tools
}

fn user_table_for_managed_config(user_config: &toml::Value, key: &str) -> toml::value::Table {
    user_config_value_for_managed_config(user_config, key)
        .and_then(|value| value.as_table().cloned())
        .unwrap_or_default()
}

fn user_config_value_for_managed_config(
    user_config: &toml::Value,
    key: &str,
) -> Option<toml::Value> {
    user_config.get(key).cloned()
}

fn default_ilhae_codex_home_table(
    config: &IlhaeTomlConfig,
    user_config: &toml::Value,
    model_catalog_path: &Path,
) -> toml::value::Table {
    let user_model = user_model_for_managed_config(user_config);
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
    let active_profile_provider_id = active_profile_name
        .as_deref()
        .unwrap_or("ilhae-active")
        .to_string();

    let mut root = toml::value::Table::new();

    root.insert(
        "approval_policy".to_string(),
        toml::Value::String("never".to_string()),
    );
    root.insert(
        "sandbox_mode".to_string(),
        toml::Value::String("danger-full-access".to_string()),
    );
    // Keep credentials in a file: the keyring backend is unavailable in the
    // headless/desktop-spawned sessions this config is generated for.
    root.insert(
        "cli_auth_credentials_store".to_string(),
        toml::Value::String("file".to_string()),
    );
    let active_codex_profile = codex_profile_table_for_ilhae_profile(
        &active_profile_provider_id,
        &active_profile,
        user_model.as_deref(),
        model_catalog_path,
    );
    for key in [
        "model",
        "model_provider",
        "model_context_window",
        "model_catalog_json",
    ] {
        if let Some(value) = active_codex_profile.get(key).cloned() {
            root.insert(key.to_string(), value);
        }
    }
    if let Some(web_search) = user_web_search_for_managed_config(user_config) {
        root.insert("web_search".to_string(), web_search);
    }
    let user_tools = user_tools_for_managed_config(user_config);
    if !user_tools.is_empty() {
        root.insert("tools".to_string(), toml::Value::Table(user_tools));
    }

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
    features.insert(
        "apply_patch_freeform".to_string(),
        toml::Value::Boolean(true),
    );
    features.insert(
        "apply_patch_streaming_events".to_string(),
        toml::Value::Boolean(true),
    );
    features.insert("fast_mode".to_string(), toml::Value::Boolean(true));
    features.insert("multi_agent".to_string(), toml::Value::Boolean(true));
    for (key, value) in user_features_for_managed_config(user_config) {
        features.insert(key, value);
    }
    root.insert("features".to_string(), toml::Value::Table(features));
    root.insert(
        "mcp_oauth_credentials_store".to_string(),
        toml::Value::String("file".to_string()),
    );
    if let Some(system2_projection) = system2_projection_table_from(config) {
        let mut desktop = toml::value::Table::new();
        desktop.insert(
            ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY.to_string(),
            toml::Value::Table(system2_projection),
        );
        root.insert("desktop".to_string(), toml::Value::Table(desktop));
    }

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

    for (name, server) in user_mcp_servers_for_managed_config(user_config) {
        mcp_servers.insert(name, server);
    }

    root.insert("mcp_servers".to_string(), toml::Value::Table(mcp_servers));

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

    let mut model_providers = user_model_providers_for_managed_config(user_config);
    for (profile_id, profile) in &config.profiles {
        insert_native_model_provider(&mut model_providers, profile_id, profile);
    }
    insert_native_model_provider(
        &mut model_providers,
        &active_profile_provider_id,
        &active_profile,
    );
    if !model_providers.is_empty() {
        root.insert(
            "model_providers".to_string(),
            toml::Value::Table(model_providers),
        );
    }

    let mut profiles = toml::value::Table::new();
    for (profile_id, profile) in &config.profiles {
        profiles.insert(
            profile_id.clone(),
            toml::Value::Table(codex_profile_table_for_ilhae_profile(
                profile_id,
                profile,
                user_model.as_deref(),
                model_catalog_path,
            )),
        );
    }
    profiles.insert(
        "ilhae-active".to_string(),
        toml::Value::Table(active_codex_profile),
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

#[derive(Debug)]
struct HumanIlhaeConfigSnapshot {
    typed: IlhaeTomlConfig,
    document: toml::Value,
}

impl Default for HumanIlhaeConfigSnapshot {
    fn default() -> Self {
        Self {
            typed: IlhaeTomlConfig::default(),
            document: toml::Value::Table(toml::value::Table::new()),
        }
    }
}

fn resolve_human_ilhae_config_path() -> Option<PathBuf> {
    let primary = resolve_ilhae_config_toml_path();
    if primary.exists() {
        return Some(primary);
    }
    let legacy = resolve_ilhae_data_dir().join("config.toml");
    legacy.exists().then_some(legacy)
}

/// Reads the human-managed source exactly once. Projection always derives from
/// this immutable generation, so a concurrent editor cannot mix MCP, model,
/// feature, and profile values from different file generations.
fn load_human_ilhae_config_snapshot() -> Result<HumanIlhaeConfigSnapshot, String> {
    let Some(path) = resolve_human_ilhae_config_path() else {
        return Ok(HumanIlhaeConfigSnapshot::default());
    };
    let bytes = std::fs::read(&path).map_err(|error| {
        format!(
            "Failed to read human-managed Ilhae config snapshot ({}): {error}",
            path.display()
        )
    })?;
    let content = std::str::from_utf8(&bytes).map_err(|error| {
        format!(
            "Human-managed Ilhae config is not valid UTF-8 ({}): {error}",
            path.display()
        )
    })?;
    let document = content.parse::<toml::Value>().map_err(|error| {
        format!(
            "Human-managed Ilhae config is not valid TOML ({}): {error}",
            path.display()
        )
    })?;
    let typed = document
        .clone()
        .try_into::<IlhaeTomlConfig>()
        .map_err(|error| {
            format!(
                "Human-managed Ilhae config has an invalid typed shape ({}): {error}",
                path.display()
            )
        })?;
    Ok(HumanIlhaeConfigSnapshot { typed, document })
}

fn render_ilhae_codex_runtime_candidate(
    snapshot: &HumanIlhaeConfigSnapshot,
    model_catalog_path: &Path,
) -> Result<String, String> {
    let root =
        default_ilhae_codex_home_table(&snapshot.typed, &snapshot.document, model_catalog_path);
    let rendered =
        toml::to_string_pretty(&toml::Value::Table(root)).map_err(|error| error.to_string())?;
    validate_ilhae_codex_runtime_config(&rendered)?;
    Ok(rendered)
}

fn native_model_catalog(
    config: &IlhaeTomlConfig,
) -> Option<codex_protocol::openai_models::ModelsResponse> {
    let mut models_by_slug = BTreeMap::new();
    for (profile_id, profile) in &config.profiles {
        if native_runtime_effective_base_url(&profile.native_runtime).is_empty() {
            continue;
        }
        let slug = resolve_ilhae_profile_model_name(profile_id, profile);
        models_by_slug
            .entry(slug.clone())
            .or_insert_with(|| codex_models_manager::model_info::model_info_from_slug(&slug));
    }
    (!models_by_slug.is_empty()).then(|| codex_protocol::openai_models::ModelsResponse {
        models: models_by_slug.into_values().collect(),
    })
}

fn serialize_native_model_catalog(
    catalog: &codex_protocol::openai_models::ModelsResponse,
) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(catalog)
        .map_err(|error| format!("Failed to serialize native model catalog: {error}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn transition_native_model_catalog(
    catalog_path: &Path,
    current: &codex_protocol::openai_models::ModelsResponse,
) -> codex_protocol::openai_models::ModelsResponse {
    let mut models_by_slug = std::fs::read(catalog_path)
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<codex_protocol::openai_models::ModelsResponse>(&bytes).ok()
        })
        .unwrap_or_default()
        .models
        .into_iter()
        .map(|model| (model.slug.clone(), model))
        .collect::<BTreeMap<_, _>>();
    for model in &current.models {
        models_by_slug.insert(model.slug.clone(), model.clone());
    }
    codex_protocol::openai_models::ModelsResponse {
        models: models_by_slug.into_values().collect(),
    }
}

fn absolute_native_model_catalog_path(codex_home: &Path) -> Result<PathBuf, String> {
    let path = codex_home.join(ILHAE_CODEX_MODEL_CATALOG_FILE);
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|current_dir| current_dir.join(path))
        .map_err(|error| format!("Failed to resolve Codex runtime directory: {error}"))
}

fn validate_ilhae_codex_runtime_config(content: &str) -> Result<(), String> {
    parse_validated_ilhae_codex_runtime_config(content).map(drop)
}

fn parse_validated_ilhae_codex_runtime_config(
    content: &str,
) -> Result<Option<ResolvedSystem2TargetConfig>, String> {
    let parsed = toml::from_str::<codex_config::config_toml::ConfigToml>(content)
        .map_err(|error| format!("Generated Codex runtime config is invalid: {error}"))?;
    for (name, server) in &parsed.mcp_servers {
        if !mcp_server_transport_is_semantically_valid(server) {
            return Err(format!(
                "Generated Codex runtime config contains an unusable MCP transport: {name}"
            ));
        }
    }
    let document = content
        .parse::<toml::Value>()
        .map_err(|_| "Generated Codex runtime config is not valid TOML".to_string())?;
    system2_projection_from_runtime_document(&document)
}

fn read_valid_ilhae_codex_runtime_config(path: &Path) -> Option<Vec<u8>> {
    read_valid_ilhae_codex_runtime_config_with_system2(path).map(|(bytes, _)| bytes)
}

fn read_valid_ilhae_codex_runtime_config_with_system2(
    path: &Path,
) -> Option<(Vec<u8>, Option<ResolvedSystem2TargetConfig>)> {
    let bytes = std::fs::read(path).ok()?;
    let content = std::str::from_utf8(&bytes).ok()?;
    let system2 = parse_validated_ilhae_codex_runtime_config(content).ok()?;
    Some((bytes, system2))
}

struct IlhaeCodexRuntimeConfigLockGuard {
    _process: MutexGuard<'static, ()>,
    _file: File,
}

fn acquire_ilhae_codex_runtime_config_lock(
    codex_home: &Path,
) -> Result<Option<IlhaeCodexRuntimeConfigLockGuard>, String> {
    let deadline = Instant::now() + ILHAE_CODEX_RUNTIME_CONFIG_LOCK_TIMEOUT;
    let process_guard = loop {
        match ILHAE_CODEX_RUNTIME_CONFIG_LOCK.try_lock() {
            Ok(guard) => break guard,
            Err(TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(ILHAE_CODEX_RUNTIME_CONFIG_LOCK_POLL_INTERVAL);
            }
        }
    };

    let lock_path = codex_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LOCK_FILE);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(&lock_path).map_err(|error| {
        format!(
            "Failed to open Codex runtime config lock ({}): {error}",
            lock_path.display()
        )
    })?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| {
            format!(
                "Failed to secure Codex runtime config lock ({}): {error}",
                lock_path.display()
            )
        })?;

    loop {
        match file.try_lock() {
            Ok(()) => {
                return Ok(Some(IlhaeCodexRuntimeConfigLockGuard {
                    _process: process_guard,
                    _file: file,
                }));
            }
            Err(FileTryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(ILHAE_CODEX_RUNTIME_CONFIG_LOCK_POLL_INTERVAL);
            }
            Err(FileTryLockError::Error(error)) => {
                return Err(format!(
                    "Failed to acquire Codex runtime config lock ({}): {error}",
                    lock_path.display()
                ));
            }
        }
    }
}

fn write_ilhae_codex_runtime_file_atomically(
    destination: &Path,
    bytes: &[u8],
) -> Result<(), String> {
    let parent = destination.parent().ok_or_else(|| {
        format!(
            "Codex runtime config path has no parent: {}",
            destination.display()
        )
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Failed to create Codex runtime config directory ({}): {error}",
            parent.display()
        )
    })?;
    let temporary_path = parent.join(format!(
        ".ilhae-runtime.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| -> Result<(), String> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut temporary = options.open(&temporary_path).map_err(|error| {
            format!(
                "Failed to create Codex runtime config temporary file ({}): {error}",
                temporary_path.display()
            )
        })?;
        temporary.write_all(bytes).map_err(|error| {
            format!(
                "Failed to write Codex runtime config temporary file ({}): {error}",
                temporary_path.display()
            )
        })?;
        temporary.sync_all().map_err(|error| {
            format!(
                "Failed to sync Codex runtime config temporary file ({}): {error}",
                temporary_path.display()
            )
        })?;
        drop(temporary);
        replace_ilhae_codex_runtime_file(&temporary_path, destination).map_err(|error| {
            format!(
                "Failed to atomically replace Codex runtime config ({}): {error}",
                destination.display()
            )
        })?;
        File::open(destination)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                format!(
                    "Failed to sync Codex runtime config ({}): {error}",
                    destination.display()
                )
            })?;
        sync_ilhae_codex_runtime_config_parent(parent)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(unix)]
fn sync_ilhae_codex_runtime_config_parent(parent: &Path) -> Result<(), String> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "Failed to sync Codex runtime config directory ({}): {error}",
                parent.display()
            )
        })
}

#[cfg(not(unix))]
fn sync_ilhae_codex_runtime_config_parent(_parent: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
fn replace_ilhae_codex_runtime_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_ilhae_codex_runtime_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    if !destination.exists() {
        return std::fs::rename(temporary, destination);
    }

    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced_file_name: *const u16,
            replacement_file_name: *const u16,
            backup_file_name: *const u16,
            replace_flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }

    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let temporary_wide = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        ReplaceFileW(
            destination_wide.as_ptr(),
            temporary_wide.as_ptr(),
            ptr::null(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn install_ilhae_codex_runtime_snapshot_locked(
    codex_home: &Path,
    rendered: &str,
) -> Result<(), String> {
    validate_ilhae_codex_runtime_config(rendered)?;
    let lkg_path = codex_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE);
    let config_path = codex_home.join("config.toml");
    write_ilhae_codex_runtime_file_atomically(&config_path, rendered.as_bytes())?;
    write_ilhae_codex_runtime_file_atomically(&lkg_path, rendered.as_bytes())
}

fn install_ilhae_codex_runtime_generation_locked(
    codex_home: &Path,
    rendered: &str,
    model_catalog: Option<&codex_protocol::openai_models::ModelsResponse>,
) -> Result<(), String> {
    let catalog_path = absolute_native_model_catalog_path(codex_home)?;
    let Some(model_catalog) = model_catalog else {
        install_ilhae_codex_runtime_snapshot_locked(codex_home, rendered)?;
        match std::fs::remove_file(&catalog_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => warn!(
                "Failed to remove unused native model catalog ({}): {error}",
                catalog_path.display()
            ),
        }
        return Ok(());
    };

    // First publish the union of the old and new catalogs. Both the currently
    // active config and the incoming config remain loadable if the process is
    // interrupted between the independently atomic file replacements.
    let transition_catalog = transition_native_model_catalog(&catalog_path, model_catalog);
    let transition_bytes = serialize_native_model_catalog(&transition_catalog)?;
    write_ilhae_codex_runtime_file_atomically(&catalog_path, &transition_bytes)?;
    install_ilhae_codex_runtime_snapshot_locked(codex_home, rendered)?;

    // Active and LKG now describe the same generation, so the catalog can be
    // pruned back to exactly the native models present in that generation.
    let current_bytes = serialize_native_model_catalog(model_catalog)?;
    if let Err(error) = write_ilhae_codex_runtime_file_atomically(&catalog_path, &current_bytes) {
        warn!("Kept the compatible transition model catalog after final pruning failed: {error}");
    }
    Ok(())
}

fn preserve_valid_active_runtime_locked(codex_home: &Path) -> bool {
    let config_path = codex_home.join("config.toml");
    read_valid_ilhae_codex_runtime_config(&config_path).is_some()
}

fn restore_valid_runtime_lkg_locked(codex_home: &Path) -> Result<bool, String> {
    let lkg_path = codex_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE);
    let Some(lkg) = read_valid_ilhae_codex_runtime_config(&lkg_path) else {
        return Ok(false);
    };
    write_ilhae_codex_runtime_file_atomically(&codex_home.join("config.toml"), &lkg)?;
    Ok(true)
}

fn recover_or_bootstrap_ilhae_codex_runtime_locked(
    codex_home: &Path,
    reason: &str,
) -> Result<(), String> {
    warn!("{reason}");
    if preserve_valid_active_runtime_locked(codex_home) {
        return Ok(());
    }
    if restore_valid_runtime_lkg_locked(codex_home)? {
        return Ok(());
    }

    let model_catalog_path = absolute_native_model_catalog_path(codex_home)?;
    let safe = render_ilhae_codex_runtime_candidate(
        &HumanIlhaeConfigSnapshot::default(),
        &model_catalog_path,
    )
    .map_err(|error| format!("Failed to build safe Codex runtime bootstrap: {error}"))?;
    install_ilhae_codex_runtime_generation_locked(codex_home, &safe, None)
}

fn prepare_ilhae_codex_runtime_config_locked(codex_home: &Path) -> Result<(), String> {
    let snapshot = match load_human_ilhae_config_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => {
            recover_or_bootstrap_ilhae_codex_runtime_locked(
                codex_home,
                "Human configuration was unreadable; keeping a validated Codex runtime snapshot",
            )?;
            return Ok(());
        }
    };
    let model_catalog_path = absolute_native_model_catalog_path(codex_home)?;
    let candidate = match render_ilhae_codex_runtime_candidate(&snapshot, &model_catalog_path) {
        Ok(candidate) => candidate,
        Err(_) => {
            recover_or_bootstrap_ilhae_codex_runtime_locked(
                codex_home,
                "Ignored invalid human configuration overrides; keeping a validated Codex runtime snapshot",
            )?;
            return Ok(());
        }
    };

    let model_catalog = native_model_catalog(&snapshot.typed);
    if let Err(error) = install_ilhae_codex_runtime_generation_locked(
        codex_home,
        &candidate,
        model_catalog.as_ref(),
    ) {
        if preserve_valid_active_runtime_locked(codex_home) {
            warn!(
                "Keeping the previous validated Codex runtime snapshot after install failure: {error}"
            );
            return Ok(());
        }
        if restore_valid_runtime_lkg_locked(codex_home)? {
            warn!("Restored the validated Codex runtime LKG after install failure: {error}");
            return Ok(());
        }
        return Err(error);
    }
    Ok(())
}

pub fn prepare_ilhae_codex_home() -> Result<PathBuf, String> {
    let codex_home = resolve_non_aliasing_ilhae_codex_home_dir()?;
    std::fs::create_dir_all(&codex_home).map_err(|err| err.to_string())?;

    let runtime_system2 = {
        match acquire_ilhae_codex_runtime_config_lock(&codex_home)? {
            Some(_guard) => {
                prepare_ilhae_codex_runtime_config_locked(&codex_home)?;
                let config_path = codex_home.join("config.toml");
                read_valid_ilhae_codex_runtime_config_with_system2(&config_path)
                    .map(|(_, system2)| system2)
                    .ok_or_else(|| {
                        format!(
                            "The prepared Codex runtime snapshot is not valid ({})",
                            config_path.display()
                        )
                    })?
            }
            None => {
                let config_path = codex_home.join("config.toml");
                let (_, system2) = read_valid_ilhae_codex_runtime_config_with_system2(&config_path)
                    .ok_or_else(|| {
                        format!(
                            "Timed out waiting for the Codex runtime config lock and the latest active snapshot is not valid ({})",
                            config_path.display()
                        )
                    })?;
                warn!(
                    "Timed out waiting for the Codex runtime config lock; using the latest validated active snapshot"
                );
                system2
            }
        }
    };
    let _ = std::fs::remove_file(codex_home.join("managed_config.toml"));

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

    // SAFETY: ilhae sets CODEX_HOME once during single-threaded CLI startup,
    // before any worker threads or async tasks that could concurrently depend
    // on environment mutation are spawned.
    unsafe {
        std::env::set_var("CODEX_HOME", &codex_home);
        std::env::set_var("ILHAE_RUNTIME", "1");
    }
    // System2 routing is projected from the final validated active snapshot,
    // never from a separately re-read human config generation.
    if let Some(system2) = runtime_system2 {
        unsafe {
            std::env::set_var("ILHAE_SYSTEM2_ENABLED", "1");
            std::env::set_var("ILHAE_SYSTEM2_SOURCE_PROFILE", system2.source_profile_id);
            std::env::set_var("ILHAE_SYSTEM2_PROFILE", system2.target_profile_id);
            std::env::set_var("ILHAE_SYSTEM2_BASE_URL", system2.base_url);
            std::env::set_var("ILHAE_SYSTEM2_MODEL", system2.model_name);
        }
    } else {
        unsafe {
            std::env::remove_var("ILHAE_SYSTEM2_ENABLED");
            std::env::remove_var("ILHAE_SYSTEM2_SOURCE_PROFILE");
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
    use sha2::Digest;
    use std::sync::Arc;
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

        fn preserve(key: &'static str) -> Self {
            Self {
                key,
                previous: std::env::var(key).ok(),
            }
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

    #[derive(Clone)]
    struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedLogWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture log mutex")
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn capture_warnings<T>(action: impl FnOnce() -> T) -> (T, String) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer_bytes = Arc::clone(&bytes);
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || CapturedLogWriter(Arc::clone(&writer_bytes)))
            .finish();
        let result = tracing::subscriber::with_default(subscriber, action);
        let output = String::from_utf8(bytes.lock().expect("capture log mutex").clone())
            .expect("captured warnings are UTF-8");
        (result, output)
    }

    fn sha256(bytes: &[u8]) -> Vec<u8> {
        sha2::Sha256::digest(bytes).to_vec()
    }

    fn preserve_runtime_environment() -> Vec<EnvVarGuard> {
        [
            "CODEX_HOME",
            "ILHAE_RUNTIME",
            "ILHAE_SYSTEM2_ENABLED",
            "ILHAE_SYSTEM2_SOURCE_PROFILE",
            "ILHAE_SYSTEM2_PROFILE",
            "ILHAE_SYSTEM2_BASE_URL",
            "ILHAE_SYSTEM2_MODEL",
        ]
        .into_iter()
        .map(EnvVarGuard::preserve)
        .collect()
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_uses_explicit_runtime_home() {
        let tmp = tempdir().expect("tempdir");
        let config_dir = tmp.path().join("human");
        let data_dir = tmp.path().join("data");
        let runtime_home = tmp.path().join("explicit-runtime-home");
        std::fs::create_dir_all(&config_dir).expect("create human config directory");
        std::fs::write(config_dir.join("config.toml"), "[profile]\n").expect("write human config");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
        let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &runtime_home);
        let _runtime_environment = preserve_runtime_environment();

        let prepared = prepare_ilhae_codex_home().expect("prepare explicit runtime home");

        assert_eq!(prepared, runtime_home);
        let active = std::fs::read(runtime_home.join("config.toml")).expect("read active snapshot");
        let lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
            .expect("read LKG snapshot");
        assert_eq!(active, lkg);
        validate_ilhae_codex_runtime_config(
            std::str::from_utf8(&active).expect("active snapshot UTF-8"),
        )
        .expect("active snapshot validates");
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_separates_an_aliased_human_config_directory() {
        let tmp = tempdir().expect("tempdir");
        let config_dir = tmp.path().join("shared");
        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&config_dir).expect("create shared config directory");
        let source = config_dir.join("config.toml");
        let source_bytes = b"[profile]\nactive = \"fable\"\n";
        std::fs::write(&source, source_bytes).expect("write human config");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
        let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &config_dir);
        let _runtime_environment = preserve_runtime_environment();

        let prepared = prepare_ilhae_codex_home().expect("prepare isolated runtime home");

        assert_eq!(prepared, config_dir.join(".ilhae-runtime-home"));
        assert_eq!(
            std::fs::read(&source).expect("read unchanged human config"),
            source_bytes
        );
        let active = std::fs::read(prepared.join("config.toml")).expect("read active snapshot");
        validate_ilhae_codex_runtime_config(
            std::str::from_utf8(&active).expect("active snapshot UTF-8"),
        )
        .expect("isolated active snapshot validates");
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_separates_an_empty_aliased_human_config_directory() {
        let tmp = tempdir().expect("tempdir");
        let config_dir = tmp.path().join("shared");
        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&config_dir).expect("create empty shared config directory");
        let source = config_dir.join("config.toml");
        assert!(!source.exists());
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
        let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &config_dir);
        let _runtime_environment = preserve_runtime_environment();

        let prepared = prepare_ilhae_codex_home().expect("prepare isolated runtime home");

        assert_eq!(prepared, config_dir.join(".ilhae-runtime-home"));
        assert!(
            !source.exists(),
            "generated runtime must not create the human config file"
        );
        let active = std::fs::read(prepared.join("config.toml")).expect("read active snapshot");
        let lkg = std::fs::read(prepared.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
            .expect("read LKG snapshot");
        assert_eq!(active, lkg);
        validate_ilhae_codex_runtime_config(
            std::str::from_utf8(&active).expect("active snapshot UTF-8"),
        )
        .expect("isolated active snapshot validates");
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_separates_symlink_and_hardlink_aliases() {
        use std::os::unix::fs::symlink;

        let tmp = tempdir().expect("tempdir");
        let config_dir = tmp.path().join("human");
        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&config_dir).expect("create human config directory");
        let source = config_dir.join("config.toml");
        let source_bytes = b"[profile]\nactive = \"fable\"\n";
        std::fs::write(&source, source_bytes).expect("write human config");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
        let _runtime_environment = preserve_runtime_environment();

        let symlink_home = tmp.path().join("runtime-symlink");
        symlink(&config_dir, &symlink_home).expect("symlink runtime home to human config");
        {
            let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &symlink_home);
            let prepared = prepare_ilhae_codex_home().expect("separate symlink alias");
            assert_eq!(prepared, symlink_home.join(".ilhae-runtime-home"));
        }
        assert_eq!(
            std::fs::read(&source).expect("read human config after symlink case"),
            source_bytes
        );

        let hardlink_home = tmp.path().join("runtime-hardlink");
        std::fs::create_dir_all(&hardlink_home).expect("create hardlink runtime home");
        std::fs::hard_link(&source, hardlink_home.join("config.toml"))
            .expect("hardlink runtime config to human source");
        {
            let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &hardlink_home);
            let prepared = prepare_ilhae_codex_home().expect("separate hardlink alias");
            assert_eq!(prepared, hardlink_home.join(".ilhae-runtime-home"));
        }
        assert_eq!(
            std::fs::read(&source).expect("read human config after hardlink case"),
            source_bytes
        );
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_filters_reserved_and_poisoned_mcp_entries() {
        let tmp = tempdir().expect("tempdir");
        let config_dir = tmp.path().join("human");
        let data_dir = tmp.path().join("data");
        let runtime_home = tmp.path().join("runtime");
        std::fs::create_dir_all(&config_dir).expect("create human config directory");
        let source = config_dir.join("config.toml");
        let source_bytes = br#"
[profile]

[mcp_servers.valid_stdio]
command = "node"
args = ["server.js"]

[mcp_servers.valid_http]
url = "https://example.com/mcp"

[mcp_servers.office]
command = "node"

[mcp_servers."excel-mcp"]
command = "node"

[mcp_servers.mcpb_custom]
command = "node"

[mcp_servers.mixed_poison]
command = "node"
url = "https://example.com/mcp"

[mcp_servers.missing_transport]
enabled = true

[mcp_servers.empty_stdio]
command = ""

[mcp_servers.whitespace_stdio]
command = "   "

[mcp_servers.empty_http]
url = ""

[mcp_servers.whitespace_http]
url = "   "

[mcp_servers.padded_http]
url = " https://example.com/mcp "

[mcp_servers.ftp_http]
url = "ftp://example.com/mcp"
"#;
        std::fs::write(&source, source_bytes).expect("write human config");
        let source_hash = sha256(source_bytes);
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
        let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &runtime_home);
        let _runtime_environment = preserve_runtime_environment();

        let (result, warnings) = capture_warnings(prepare_ilhae_codex_home);
        result.expect("poisoned MCP entries must not stop runtime preparation");

        let source_after = std::fs::read(&source).expect("read unchanged human config");
        assert_eq!(source_after, source_bytes);
        assert_eq!(sha256(&source_after), source_hash);

        let active = std::fs::read(runtime_home.join("config.toml")).expect("read active snapshot");
        let lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
            .expect("read LKG snapshot");
        assert_eq!(active, lkg);
        let active_text = std::str::from_utf8(&active).expect("active snapshot UTF-8");
        validate_ilhae_codex_runtime_config(active_text).expect("active snapshot validates");
        let parsed = toml::from_str::<codex_config::config_toml::ConfigToml>(active_text)
            .expect("parse exact Codex config type");
        for preserved in ["valid_stdio", "valid_http", "mcpb_custom"] {
            assert!(
                parsed.mcp_servers.contains_key(preserved),
                "valid human MCP intent must be preserved: {preserved}"
            );
        }
        for excluded in [
            "office",
            "excel-mcp",
            "mixed_poison",
            "missing_transport",
            "empty_stdio",
            "whitespace_stdio",
            "empty_http",
            "whitespace_http",
            "padded_http",
            "ftp_http",
        ] {
            assert!(
                !parsed.mcp_servers.contains_key(excluded),
                "{excluded} must not reach the runtime snapshot"
            );
        }
        assert!(warnings.contains("invalid MCP configuration entries"));
        for secret in [
            "mixed_poison",
            "url is not supported for stdio",
            source.to_string_lossy().as_ref(),
        ] {
            assert!(
                !warnings.contains(secret),
                "warning leaked poison diagnostic: {secret}"
            );
        }
    }

    #[test]
    #[serial_test::serial]
    fn invalid_human_config_preserves_active_and_lkg_without_diagnostic_leaks() {
        let tmp = tempdir().expect("tempdir");
        let config_dir = tmp.path().join("human");
        let data_dir = tmp.path().join("data");
        let runtime_home = tmp.path().join("runtime");
        std::fs::create_dir_all(&config_dir).expect("create human config directory");
        let source = config_dir.join("config.toml");
        std::fs::write(&source, "[profile]\n").expect("write baseline human config");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
        let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &runtime_home);
        let _runtime_environment = preserve_runtime_environment();
        prepare_ilhae_codex_home().expect("prepare baseline runtime snapshot");
        let baseline_active =
            std::fs::read(runtime_home.join("config.toml")).expect("read baseline active");
        let baseline_lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
            .expect("read baseline LKG");

        for invalid_source in [
            b"[mcp_servers.poison_fixture\nurl = \"fixture-secret\"\n".to_vec(),
            vec![0xff, 0xfe, b'p', b'o', b'i', b's', b'o', b'n'],
        ] {
            std::fs::write(&source, &invalid_source).expect("write invalid human config");
            let source_hash = sha256(&invalid_source);
            // The preserved active generation has no System2 projection, so no
            // stale environment value may survive recovery.
            unsafe {
                std::env::set_var("ILHAE_SYSTEM2_ENABLED", "1");
                std::env::set_var("ILHAE_SYSTEM2_SOURCE_PROFILE", "stale-source");
                std::env::set_var("ILHAE_SYSTEM2_PROFILE", "stale-profile");
                std::env::set_var("ILHAE_SYSTEM2_BASE_URL", "https://stale.invalid/v1");
                std::env::set_var("ILHAE_SYSTEM2_MODEL", "stale-model");
            }

            let (result, warnings) = capture_warnings(prepare_ilhae_codex_home);
            result.expect("invalid human config must not stop runtime preparation");

            let source_after = std::fs::read(&source).expect("read unchanged invalid source");
            assert_eq!(source_after, invalid_source);
            assert_eq!(sha256(&source_after), source_hash);
            assert_eq!(
                std::fs::read(runtime_home.join("config.toml")).expect("read preserved active"),
                baseline_active
            );
            assert_eq!(
                std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
                    .expect("read preserved LKG"),
                baseline_lkg
            );
            for key in [
                "ILHAE_SYSTEM2_ENABLED",
                "ILHAE_SYSTEM2_SOURCE_PROFILE",
                "ILHAE_SYSTEM2_PROFILE",
                "ILHAE_SYSTEM2_BASE_URL",
                "ILHAE_SYSTEM2_MODEL",
            ] {
                assert!(
                    std::env::var_os(key).is_none(),
                    "stale System2 environment survived recovery: {key}"
                );
            }
            assert!(warnings.contains("Human configuration was unreadable"));
            for secret in [
                "poison_fixture",
                "fixture-secret",
                "url is not supported for stdio",
                source.to_string_lossy().as_ref(),
            ] {
                assert!(
                    !warnings.contains(secret),
                    "warning leaked human config diagnostic: {secret}"
                );
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn invalid_first_start_override_bootstraps_valid_runtime_snapshot() {
        let tmp = tempdir().expect("tempdir");
        let config_dir = tmp.path().join("human");
        let data_dir = tmp.path().join("data");
        let runtime_home = tmp.path().join("runtime");
        std::fs::create_dir_all(&config_dir).expect("create human config directory");
        let source = config_dir.join("config.toml");
        let invalid_source = b"[features]\nmulti_agent = \"fixture-secret\"\n";
        std::fs::write(&source, invalid_source).expect("write invalid feature override");
        let source_hash = sha256(invalid_source);
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
        let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &runtime_home);
        let _runtime_environment = preserve_runtime_environment();

        let (result, warnings) = capture_warnings(prepare_ilhae_codex_home);
        result.expect("invalid first-start override must fall back to safe defaults");

        let source_after = std::fs::read(&source).expect("read unchanged human config");
        assert_eq!(source_after, invalid_source);
        assert_eq!(sha256(&source_after), source_hash);
        let active = std::fs::read(runtime_home.join("config.toml")).expect("read active snapshot");
        let lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
            .expect("read LKG snapshot");
        assert_eq!(active, lkg);
        let active_text = std::str::from_utf8(&active).expect("active snapshot UTF-8");
        validate_ilhae_codex_runtime_config(active_text).expect("safe snapshot validates");
        let active_toml = active_text
            .parse::<toml::Value>()
            .expect("parse safe snapshot TOML");
        assert_eq!(
            active_toml
                .get("features")
                .and_then(toml::Value::as_table)
                .and_then(|features| features.get("multi_agent"))
                .and_then(toml::Value::as_bool),
            Some(true)
        );
        assert!(!active_text.contains("fixture-secret"));
        assert!(warnings.contains("Ignored invalid human configuration overrides"));
        assert!(!warnings.contains("fixture-secret"));
        assert!(!warnings.contains(source.to_string_lossy().as_ref()));
    }

    #[test]
    fn rejected_candidate_cannot_replace_valid_active_or_lkg_snapshot() {
        let tmp = tempdir().expect("tempdir");
        let runtime_home = tmp.path().join("runtime");
        let valid = render_ilhae_codex_runtime_candidate(
            &HumanIlhaeConfigSnapshot::default(),
            &runtime_home.join(ILHAE_CODEX_MODEL_CATALOG_FILE),
        )
        .expect("render valid snapshot");
        install_ilhae_codex_runtime_snapshot_locked(&runtime_home, &valid)
            .expect("install valid snapshot");
        let baseline_active =
            std::fs::read(runtime_home.join("config.toml")).expect("read baseline active");
        let baseline_lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
            .expect("read baseline LKG");

        let poison = "[mcp_servers.poison]\ncommand = \"   \"\n";
        assert!(install_ilhae_codex_runtime_snapshot_locked(&runtime_home, poison).is_err());
        assert_eq!(
            std::fs::read(runtime_home.join("config.toml")).expect("read unchanged active"),
            baseline_active
        );
        assert_eq!(
            std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
                .expect("read unchanged LKG"),
            baseline_lkg
        );
    }

    #[test]
    fn valid_dynamic_active_does_not_replace_base_lkg_and_invalid_active_restores_base() {
        let tmp = tempdir().expect("tempdir");
        let runtime_home = tmp.path().join("runtime");
        let candidate_a = render_ilhae_codex_runtime_candidate(
            &HumanIlhaeConfigSnapshot::default(),
            &runtime_home.join(ILHAE_CODEX_MODEL_CATALOG_FILE),
        )
        .expect("render candidate A");
        install_ilhae_codex_runtime_snapshot_locked(&runtime_home, &candidate_a)
            .expect("install candidate A");

        let mut candidate_b_toml = candidate_a
            .parse::<toml::Value>()
            .expect("parse candidate A");
        let mut office = toml::value::Table::new();
        office.insert(
            "url".to_string(),
            toml::Value::String("http://127.0.0.1:43123/mcp".to_string()),
        );
        candidate_b_toml
            .get_mut("mcp_servers")
            .and_then(toml::Value::as_table_mut)
            .expect("candidate MCP table")
            .insert("office".to_string(), toml::Value::Table(office));
        let candidate_b = toml::to_string_pretty(&candidate_b_toml).expect("render candidate B");
        validate_ilhae_codex_runtime_config(&candidate_b).expect("candidate B validates");

        std::fs::write(runtime_home.join("config.toml"), candidate_b.as_bytes())
            .expect("simulate independently updated valid active snapshot");
        assert!(preserve_valid_active_runtime_locked(&runtime_home));
        assert_eq!(
            std::fs::read(runtime_home.join("config.toml")).expect("read preserved active"),
            candidate_b.as_bytes()
        );
        assert_eq!(
            std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
                .expect("read unchanged base LKG"),
            candidate_a.as_bytes()
        );

        std::fs::write(
            runtime_home.join("config.toml"),
            b"[mcp_servers.poison]\ncommand = \"   \"\n",
        )
        .expect("simulate corrupted active snapshot");
        recover_or_bootstrap_ilhae_codex_runtime_locked(
            &runtime_home,
            "Recovering a corrupted active runtime snapshot",
        )
        .expect("restore valid LKG");
        assert_eq!(
            std::fs::read(runtime_home.join("config.toml")).expect("read restored active"),
            candidate_a.as_bytes()
        );
        assert_eq!(
            std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
                .expect("read unchanged LKG"),
            candidate_a.as_bytes()
        );
    }

    #[test]
    fn concurrent_runtime_snapshot_writers_do_not_tear_active_or_lkg() {
        let tmp = tempdir().expect("tempdir");
        let runtime_home = tmp.path().join("runtime");
        std::fs::create_dir_all(&runtime_home).expect("create runtime home");
        let candidate_a = render_ilhae_codex_runtime_candidate(
            &HumanIlhaeConfigSnapshot::default(),
            &runtime_home.join(ILHAE_CODEX_MODEL_CATALOG_FILE),
        )
        .expect("render candidate A");
        let mut candidate_b_toml = candidate_a
            .parse::<toml::Value>()
            .expect("parse candidate A");
        candidate_b_toml
            .as_table_mut()
            .expect("candidate root table")
            .insert(
                "model_context_window".to_string(),
                toml::Value::Integer(65_536),
            );
        let candidate_b = toml::to_string_pretty(&candidate_b_toml).expect("render candidate B");
        validate_ilhae_codex_runtime_config(&candidate_b).expect("candidate B validates");

        let runtime_home = Arc::new(runtime_home);
        let handles = (0..12)
            .map(|index| {
                let runtime_home = Arc::clone(&runtime_home);
                let candidate = if index % 2 == 0 {
                    candidate_a.clone()
                } else {
                    candidate_b.clone()
                };
                std::thread::spawn(move || {
                    let _guard = acquire_ilhae_codex_runtime_config_lock(&runtime_home)
                        .expect("acquire runtime snapshot lock")
                        .expect("runtime snapshot lock before timeout");
                    install_ilhae_codex_runtime_snapshot_locked(&runtime_home, &candidate)
                        .expect("install serialized runtime snapshot");
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().expect("runtime writer thread");
        }

        let active = std::fs::read(runtime_home.join("config.toml")).expect("read final active");
        let lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
            .expect("read final LKG");
        assert_eq!(active, lkg);
        assert!(active == candidate_a.as_bytes() || active == candidate_b.as_bytes());
        validate_ilhae_codex_runtime_config(
            std::str::from_utf8(&active).expect("final active UTF-8"),
        )
        .expect("final active validates");
        let temporary_files = std::fs::read_dir(runtime_home.as_ref())
            .expect("read runtime home")
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with(".ilhae-runtime.") && name.ends_with(".tmp")
            })
            .count();
        assert_eq!(temporary_files, 0);
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_projects_named_profiles_into_managed_config() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

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
        let managed =
            std::fs::read_to_string(codex_home.join("config.toml")).expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
        let root = parsed.as_table().expect("root table");

        assert!(root.get("profile").is_none());
        assert_eq!(
            root.get("mcp_oauth_credentials_store")
                .and_then(toml::Value::as_str),
            Some("file")
        );
        assert_eq!(
            root.get("model").and_then(toml::Value::as_str),
            Some("gemma-4-26b")
        );
        assert_eq!(
            root.get("model_provider").and_then(toml::Value::as_str),
            Some("ilhae-native-nemotron-local")
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
            Some("ilhae-native-nemotron-local")
        );
        assert!(nemotron_profile.get("url").is_none());
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

        let codex_config =
            std::fs::read_to_string(codex_home.join("config.toml")).expect("read codex config");
        let codex_config: toml::Value = toml::from_str(&codex_config).expect("parse codex config");
        let codex_mcp_servers = codex_config
            .get("mcp_servers")
            .and_then(toml::Value::as_table)
            .expect("codex mcp servers table");
        let projected_fortune = codex_mcp_servers
            .get("fortune")
            .and_then(toml::Value::as_table)
            .expect("projected fortune mcp server");
        assert_eq!(projected_fortune, fortune);

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
        let nemotron_provider = model_providers
            .get("ilhae-native-nemotron-local")
            .and_then(toml::Value::as_table)
            .expect("nemotron provider");
        assert_eq!(
            nemotron_provider
                .get("base_url")
                .and_then(toml::Value::as_str),
            Some("http://127.0.0.1:8081/v1")
        );
        assert_eq!(
            nemotron_provider
                .get("requires_openai_auth")
                .and_then(toml::Value::as_bool),
            Some(false)
        );
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_projects_foreground_loop_instructions_into_managed_config() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

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

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
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
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_defaults_self_improvement_foreground_loop() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

        save_ilhae_toml_config(&IlhaeTomlConfig::default()).expect("save default config");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
        let agent = parsed
            .get("agent")
            .and_then(toml::Value::as_table)
            .expect("agent table");
        let features = parsed
            .get("features")
            .and_then(toml::Value::as_table)
            .expect("features table");
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
        assert_eq!(
            features
                .get("apply_patch_freeform")
                .and_then(toml::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            features
                .get("apply_patch_streaming_events")
                .and_then(toml::Value::as_bool),
            Some(true)
        );
        assert!(instructions.contains("- Self-improvement: enabled"));
        assert!(instructions.contains("- Preset: foreground"));
        assert!(instructions.contains("SELF-IMPROVEMENT SKILL LOOP"));
    }

    #[test]
    #[serial_test::serial]
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
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_scopes_static_catalog_to_native_profiles() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("qwen-local".to_string());

        let mut qwen = IlhaeProfileConfig::default();
        qwen.agent.engine_id = Some("llama-server".to_string());
        qwen.native_runtime.base_url = "http://127.0.0.1:8081/v1".to_string();
        qwen.native_runtime.model_path = "qwen3.6-27b".to_string();
        config.profiles.insert("qwen-local".to_string(), qwen);

        let mut review = IlhaeProfileConfig::default();
        review.agent.engine_id = Some("openai".to_string());
        review.agent.command = Some("codex".to_string());
        config.profiles.insert("review".to_string(), review);

        save_ilhae_toml_config(&config).expect("save config");
        let codex_home = prepare_ilhae_codex_home().expect("prepare codex home");
        let catalog_path = codex_home.join(ILHAE_CODEX_MODEL_CATALOG_FILE);
        let managed =
            std::fs::read_to_string(codex_home.join("config.toml")).expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
        let profiles = parsed
            .get("profiles")
            .and_then(toml::Value::as_table)
            .expect("profiles table");

        assert_eq!(
            parsed
                .get("model_catalog_json")
                .and_then(toml::Value::as_str),
            catalog_path.to_str()
        );
        assert_eq!(
            profiles
                .get("qwen-local")
                .and_then(|profile| profile.get("model_catalog_json"))
                .and_then(toml::Value::as_str),
            catalog_path.to_str()
        );
        assert!(
            profiles
                .get("review")
                .and_then(|profile| profile.get("model_catalog_json"))
                .is_none()
        );

        let catalog = serde_json::from_slice::<codex_protocol::openai_models::ModelsResponse>(
            &std::fs::read(&catalog_path).expect("read native model catalog"),
        )
        .expect("parse native model catalog");
        assert_eq!(
            catalog,
            codex_protocol::openai_models::ModelsResponse {
                models: vec![codex_models_manager::model_info::model_info_from_slug(
                    "qwen3.6-27b"
                )],
            }
        );

        config.profile.active = Some("review".to_string());
        save_ilhae_toml_config(&config).expect("save OpenAI-active config");
        prepare_ilhae_codex_home().expect("prepare OpenAI-active codex home");
        let managed =
            std::fs::read_to_string(codex_home.join("config.toml")).expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
        let profiles = parsed
            .get("profiles")
            .and_then(toml::Value::as_table)
            .expect("profiles table");

        assert!(parsed.get("model_catalog_json").is_none());
        assert!(
            profiles
                .get("review")
                .and_then(|profile| profile.get("model_catalog_json"))
                .is_none()
        );
        assert_eq!(
            profiles
                .get("qwen-local")
                .and_then(|profile| profile.get("model_catalog_json"))
                .and_then(toml::Value::as_str),
            catalog_path.to_str()
        );
    }

    #[test]
    fn native_runtime_effective_urls_fall_back_to_url_alias() {
        let mut config = IlhaeProfileNativeRuntimeConfig::default();
        config.url = Some("http://127.0.0.1:8085/v1".to_string());
        config.health_url = String::new();
        config.base_url = String::new();

        assert_eq!(
            native_runtime_effective_base_url(&config),
            "http://127.0.0.1:8085/v1"
        );
        assert_eq!(
            native_runtime_effective_health_url(&config),
            "http://127.0.0.1:8085/health"
        );
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_projects_non_ilhae_native_profiles_into_managed_config() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

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

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
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
            Some("ilhae-native-qwen3.6-local")
        );
        assert!(local_profile.get("url").is_none());
        let model_providers = parsed
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .expect("model providers table");
        let local_provider = model_providers
            .get("ilhae-native-qwen3.6-local")
            .and_then(toml::Value::as_table)
            .expect("local provider");
        assert_eq!(
            local_provider.get("name").and_then(toml::Value::as_str),
            Some("sglang")
        );
        assert_eq!(
            local_provider.get("base_url").and_then(toml::Value::as_str),
            Some("http://127.0.0.1:8081/v1")
        );
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_projects_external_runtime_into_root_model_config() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();
        let mut query_params = BTreeMap::new();
        query_params.insert("draft".to_string(), "mtp".to_string());
        query_params.insert("ngram-mode".to_string(), "1".to_string());
        let mut http_headers = BTreeMap::new();
        http_headers.insert("X-Test-Header".to_string(), "test-value".to_string());
        let mut env_http_headers = BTreeMap::new();
        env_http_headers.insert("X-Test-Env".to_string(), "TEST_ENV_HEADER".to_string());

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("remote-llama".to_string());

        let mut remote = IlhaeProfileConfig::default();
        remote.agent.engine_id = Some("ilhae".to_string());
        remote.agent.command = Some("ilhae".to_string());
        remote.native_runtime.enabled = false;
        remote.native_runtime.provider = Some("llama-server".to_string());
        remote.native_runtime.base_url = "http://tripleyoung.synology.me:8082/v1".to_string();
        remote.native_runtime.health_url = "http://tripleyoung.synology.me:8082/health".to_string();
        remote.native_runtime.query_params = Some(query_params);
        remote.native_runtime.http_headers = Some(http_headers);
        remote.native_runtime.env_http_headers = Some(env_http_headers);
        config.profiles.insert("remote-llama".to_string(), remote);

        save_ilhae_toml_config(&config).expect("save config");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");

        assert_eq!(
            parsed.get("model").and_then(toml::Value::as_str),
            Some("ilhae")
        );
        assert_eq!(
            parsed.get("model_provider").and_then(toml::Value::as_str),
            Some("ilhae-native-remote-llama")
        );

        let model_providers = parsed
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .expect("model providers");
        let remote_provider = model_providers
            .get("ilhae-native-remote-llama")
            .and_then(toml::Value::as_table)
            .expect("remote provider");
        assert_eq!(
            remote_provider
                .get("query_params")
                .and_then(toml::Value::as_table)
                .and_then(|query| query.get("draft"))
                .and_then(toml::Value::as_str),
            Some("mtp")
        );
        assert_eq!(
            remote_provider
                .get("query_params")
                .and_then(toml::Value::as_table)
                .and_then(|query| query.get("ngram-mode"))
                .and_then(toml::Value::as_str),
            Some("1")
        );
        assert_eq!(
            remote_provider
                .get("http_headers")
                .and_then(toml::Value::as_table)
                .and_then(|headers| headers.get("X-Test-Header"))
                .and_then(toml::Value::as_str),
            Some("test-value")
        );
        assert_eq!(
            remote_provider
                .get("env_http_headers")
                .and_then(toml::Value::as_table)
                .and_then(|headers| headers.get("X-Test-Env"))
                .and_then(toml::Value::as_str),
            Some("TEST_ENV_HEADER")
        );
    }

    #[test]
    fn prepare_ilhae_codex_home_keeps_runtime_args_out_of_request_query_params() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("remote-args".to_string());

        let mut remote = IlhaeProfileConfig::default();
        remote.agent.engine_id = Some("ilhae".to_string());
        remote.agent.command = Some("ilhae".to_string());
        remote.native_runtime.enabled = false;
        remote.native_runtime.provider = Some("llama-server".to_string());
        remote.native_runtime.base_url = "http://127.0.0.1:8082/v1".to_string();
        remote.native_runtime.health_url = "http://127.0.0.1:8082/health".to_string();
        remote.native_runtime.args = vec![
            "draft".to_string(),
            "mtp".to_string(),
            "ngram-mode".to_string(),
            "1".to_string(),
        ];
        config.profiles.insert("remote-args".to_string(), remote);

        save_ilhae_toml_config(&config).expect("save config");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");

        let model_providers = parsed
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .expect("model providers");
        let remote_provider = model_providers
            .get("ilhae-native-remote-args")
            .and_then(toml::Value::as_table)
            .expect("remote provider");
        assert!(remote_provider.get("query_params").is_none());
    }

    #[test]
    fn prepare_ilhae_codex_home_remote_runtime_context_size_from_query_params() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("remote-ctx".to_string());

        let mut query_params = BTreeMap::new();
        query_params.insert("context_size".to_string(), "131072".to_string());

        let mut remote = IlhaeProfileConfig::default();
        remote.agent.engine_id = Some("ilhae".to_string());
        remote.agent.command = Some("ilahe".to_string());
        remote.native_runtime.enabled = false;
        remote.native_runtime.provider = Some("llama-server".to_string());
        remote.native_runtime.base_url = "http://127.0.0.1:8082/v1".to_string();
        remote.native_runtime.health_url = "http://127.0.0.1:8082/health".to_string();
        remote.native_runtime.query_params = Some(query_params);
        config.profiles.insert("remote-ctx".to_string(), remote);

        save_ilhae_toml_config(&config).expect("save config");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");

        let profile = parsed
            .get("profiles")
            .and_then(toml::Value::as_table)
            .and_then(|profiles| profiles.get("remote-ctx"))
            .and_then(toml::Value::as_table)
            .expect("remote profile");
        assert_eq!(
            profile
                .get("model_context_window")
                .and_then(toml::Value::as_integer),
            Some(131_072)
        );

        let model_providers = parsed
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .expect("model providers");
        let remote_provider = model_providers
            .get("ilhae-native-remote-ctx")
            .and_then(toml::Value::as_table)
            .expect("remote provider");
        assert_eq!(
            remote_provider
                .get("query_params")
                .and_then(toml::Value::as_table)
                .and_then(|query| query.get("context_size"))
                .and_then(toml::Value::as_str),
            Some("131072")
        );
    }

    #[test]
    fn prepare_ilhae_codex_home_keeps_context_window_out_of_request_query_params() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("remote-context-window".to_string());

        let mut remote = IlhaeProfileConfig::default();
        remote.agent.engine_id = Some("ilhae".to_string());
        remote.agent.command = Some("ilhae".to_string());
        remote.native_runtime.provider = Some("llama-server".to_string());
        remote.native_runtime.base_url = "http://127.0.0.1:8082/v1".to_string();
        remote.native_runtime.context_window = Some(16_384);
        config
            .profiles
            .insert("remote-context-window".to_string(), remote);

        save_ilhae_toml_config(&config).expect("save config");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");

        let profile = parsed
            .get("profiles")
            .and_then(toml::Value::as_table)
            .and_then(|profiles| profiles.get("remote-context-window"))
            .and_then(toml::Value::as_table)
            .expect("remote profile");
        assert_eq!(
            profile
                .get("model_context_window")
                .and_then(toml::Value::as_integer),
            Some(16_384)
        );

        let provider = parsed
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .and_then(|providers| providers.get("ilhae-native-remote-context-window"))
            .and_then(toml::Value::as_table)
            .expect("remote provider");
        assert!(provider.get("query_params").is_none());
    }

    #[test]
    fn prepare_ilhae_codex_home_remote_turboquant_query_params() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("remote-turboquant".to_string());

        let mut query_params = BTreeMap::new();
        query_params.insert("draft".to_string(), "mtp".to_string());
        query_params.insert("ngram-mode".to_string(), "1".to_string());
        query_params.insert("cache-type-k".to_string(), "turbo4_0".to_string());
        query_params.insert("cache-type-v".to_string(), "turbo4_0".to_string());

        let mut remote = IlhaeProfileConfig::default();
        remote.agent.engine_id = Some("minimax-turboquant".to_string());
        remote.agent.command = Some("minimax-turboquant".to_string());
        remote.native_runtime.enabled = false;
        remote.native_runtime.provider = Some("minimax-turboquant".to_string());
        remote.native_runtime.base_url = "http://127.0.0.1:8082/v1".to_string();
        remote.native_runtime.health_url = "http://127.0.0.1:8082/health".to_string();
        remote.native_runtime.query_params = Some(query_params);
        config
            .profiles
            .insert("remote-turboquant".to_string(), remote);

        save_ilhae_toml_config(&config).expect("save config");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");

        let model_providers = parsed
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .expect("model providers");
        let provider = model_providers
            .get("ilhae-native-remote-turboquant")
            .and_then(toml::Value::as_table)
            .expect("turboquant provider");
        let provider_query_params = provider
            .get("query_params")
            .and_then(toml::Value::as_table)
            .expect("turboquant query params");

        assert_eq!(
            provider_query_params
                .get("draft")
                .and_then(toml::Value::as_str),
            Some("mtp")
        );
        assert_eq!(
            provider_query_params
                .get("ngram-mode")
                .and_then(toml::Value::as_str),
            Some("1")
        );
        assert_eq!(
            provider_query_params
                .get("cache-type-k")
                .and_then(toml::Value::as_str),
            Some("turbo4_0")
        );
        assert_eq!(
            provider_query_params
                .get("cache-type-v")
                .and_then(toml::Value::as_str),
            Some("turbo4_0")
        );

        let profiles = parsed
            .get("profiles")
            .and_then(toml::Value::as_table)
            .expect("profiles table");
        let profile = profiles
            .get("remote-turboquant")
            .and_then(toml::Value::as_table)
            .expect("remote profile");
        assert_eq!(
            profile.get("model_provider").and_then(toml::Value::as_str),
            Some("ilhae-native-remote-turboquant")
        );
    }

    #[test]
    fn prepare_ilhae_codex_home_uses_proxy_base_url_for_remote_runtime() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("remote-proxy".to_string());

        let mut remote = IlhaeProfileConfig::default();
        remote.agent.engine_id = Some("ilhae".to_string());
        remote.agent.command = Some("ilhae".to_string());
        remote.native_runtime.enabled = false;
        remote.native_runtime.provider = Some("llama-server".to_string());
        remote.native_runtime.base_url = String::new();
        remote.native_runtime.proxy_base_url = Some("http://127.0.0.1:8082/v1".to_string());
        remote.native_runtime.proxy_control_token_env =
            Some("ILHAE_RUNTIME_PROXY_TOKEN".to_string());
        remote.native_runtime.health_url = "http://127.0.0.1:8082/health".to_string();
        remote.native_runtime.model_path = "/models/Qwen3.6-27B-Fable-Fusion.gguf".to_string();
        config.profiles.insert("remote-proxy".to_string(), remote);

        save_ilhae_toml_config(&config).expect("save config");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
        let model_providers = parsed
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .expect("model providers");
        let remote_provider = model_providers
            .get("ilhae-native-remote-proxy")
            .and_then(toml::Value::as_table)
            .expect("remote provider");
        assert_eq!(
            remote_provider
                .get("base_url")
                .and_then(toml::Value::as_str),
            Some("http://127.0.0.1:8082/v1")
        );
        assert_eq!(
            remote_provider
                .get("env_http_headers")
                .and_then(toml::Value::as_table)
                .and_then(|headers| headers.get("X-Ilhae-Runtime-Token"))
                .and_then(toml::Value::as_str),
            Some("ILHAE_RUNTIME_PROXY_TOKEN")
        );
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_preserves_sglang_directory_model_names() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

        let mut config = IlhaeTomlConfig::default();
        config.profile.active = Some("qwen3.6-sglang".to_string());

        let mut sglang = IlhaeProfileConfig::default();
        sglang.agent.engine_id = Some("sglang".to_string());
        sglang.agent.command = Some("ilhae".to_string());
        sglang.native_runtime.enabled = true;
        sglang.native_runtime.base_url = "http://192.168.219.113:30000/v1".to_string();
        sglang.native_runtime.provider = Some("sglang".to_string());
        sglang.native_runtime.model_path = "/home/sk/ws/llm/models/Qwen3.6-27B".to_string();
        sglang.native_runtime.args = vec!["--context-length".to_string(), "262144".to_string()];
        config.profiles.insert("qwen3.6-sglang".to_string(), sglang);

        save_ilhae_toml_config(&config).expect("save config");
        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
        let profile = parsed
            .get("profiles")
            .and_then(toml::Value::as_table)
            .and_then(|profiles| profiles.get("qwen3.6-sglang"))
            .and_then(toml::Value::as_table)
            .expect("sglang profile");

        assert_eq!(
            profile.get("model").and_then(toml::Value::as_str),
            Some("Qwen3.6-27B")
        );
        assert_eq!(
            profile
                .get("model_context_window")
                .and_then(toml::Value::as_integer),
            Some(262144)
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
    #[serial_test::serial]
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
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_preserves_oauth_credentials() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

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
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_disables_duplicate_legacy_fortune_server() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

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

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
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
    #[serial_test::serial]
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
        let _runtime_environment = preserve_runtime_environment();

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
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_preserves_existing_codex_home_oauth_credentials() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

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
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_removes_stale_runtime_overrides() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

        let config_toml = r#"
[profile]
active = "qwen3.6-27b-mtp-ud-q4-k-xl"

[profiles."qwen3.6-27b-mtp-ud-q4-k-xl".native_runtime]
enabled = true
model_path = "/models/Qwen3.6-27B-MTP-UD-Q4_K_XL.gguf"
"#;
        std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

        let codex_home = tmp.path().join("codex-home");
        std::fs::create_dir_all(&codex_home).expect("create codex home");
        std::fs::write(
            codex_home.join("config.toml"),
            r#"model = "Qwen3.6-27B-UD-Q4_K_XL"
model_provider = "ilhae-native-qwen3.6-27b-ud-q4-k-xl"
profile = "qwen3.6-27b-ud-q4-k-xl"

[projects."/mnt/nvme0n1p2/workspace/monorepo"]
trust_level = "trusted"
"#,
        )
        .expect("write stale codex config");

        prepare_ilhae_codex_home().expect("prepare codex home");

        let sanitized =
            std::fs::read_to_string(codex_home.join("config.toml")).expect("read codex config");
        let parsed: toml::Value = toml::from_str(&sanitized).expect("parse managed codex config");
        assert_eq!(
            parsed.get("model").and_then(toml::Value::as_str),
            Some("Qwen3.6-27B-MTP-UD-Q4_K_XL")
        );
        assert_eq!(
            parsed.get("model_provider").and_then(toml::Value::as_str),
            Some("llama-server")
        );
        assert!(parsed.get("profile").is_none());
        assert!(!sanitized.contains("Qwen3.6-27B-UD-Q4_K_XL"));
        assert!(!sanitized.contains("ilhae-native-qwen3.6-27b-ud-q4-k-xl"));
        assert!(parsed.get("projects").is_none());
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_carries_project_trust_into_managed_config() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

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

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
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
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_carries_web_search_into_managed_config() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

        let config_toml = r#"
web_search = "live"

[tools.web_search]
engine = "duckduckgo"
use_duckduckgo_fallback = true

[profile]
active = "qwen-local"

[profiles.qwen-local.agent]
engine = "ilhae"
command = "ilhae"
"#;
        std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
        assert_eq!(
            parsed.get("web_search").and_then(toml::Value::as_str),
            Some("live")
        );

        let web_search = parsed
            .get("tools")
            .and_then(toml::Value::as_table)
            .and_then(|tools| tools.get("web_search"))
            .and_then(toml::Value::as_table)
            .expect("tools.web_search table");
        assert_eq!(
            web_search.get("engine").and_then(toml::Value::as_str),
            Some("duckduckgo")
        );
        assert_eq!(
            web_search
                .get("use_duckduckgo_fallback")
                .and_then(toml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_carries_features_into_managed_config() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

        let config_toml = r#"
[features]
goals = true
fast_mode = false

[profile]
active = "qwen-local"

[profiles.qwen-local.agent]
engine = "ilhae"
command = "ilhae"
"#;
        std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

        prepare_ilhae_codex_home().expect("prepare codex home");

        let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
            .expect("read generated config");
        let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
        let features = parsed
            .get("features")
            .and_then(toml::Value::as_table)
            .expect("features table");

        assert_eq!(
            features.get("goals").and_then(toml::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            features.get("fast_mode").and_then(toml::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            features
                .get("apply_patch_freeform")
                .and_then(toml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    #[serial_test::serial]
    fn prepare_ilhae_codex_home_persists_and_recovers_system2_projection() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

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

        let codex_home = prepare_ilhae_codex_home().expect("prepare codex home");
        let expected = ResolvedSystem2TargetConfig {
            source_profile_id: "qwen3.6-local".to_string(),
            target_profile_id: "minimax-m2.7-turboquant".to_string(),
            base_url: "http://192.168.219.113:8080/v1".to_string(),
            model_name: "MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf".to_string(),
        };

        assert!(
            std::env::var("ILHAE_SYSTEM2_ENABLED").ok().as_deref() == Some("1"),
            "system2 should be enabled"
        );
        assert_eq!(
            std::env::var("ILHAE_SYSTEM2_SOURCE_PROFILE")
                .ok()
                .as_deref(),
            Some("qwen3.6-local")
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
        assert_eq!(get_active_system2_target_config(), Some(expected.clone()));

        let active_path = codex_home.join("config.toml");
        let lkg_path = codex_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE);
        let baseline_active = std::fs::read(&active_path).expect("read System2 active snapshot");
        let baseline_lkg = std::fs::read(&lkg_path).expect("read System2 LKG snapshot");
        assert_eq!(baseline_active, baseline_lkg);
        let (_, persisted_projection) =
            read_valid_ilhae_codex_runtime_config_with_system2(&active_path)
                .expect("read validated System2 snapshot");
        assert_eq!(persisted_projection, Some(expected.clone()));

        let active_document = std::str::from_utf8(&baseline_active)
            .expect("System2 active snapshot UTF-8")
            .parse::<toml::Value>()
            .expect("parse System2 active snapshot");
        let projection = active_document
            .get("desktop")
            .and_then(toml::Value::as_table)
            .and_then(|desktop| desktop.get(ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY))
            .and_then(toml::Value::as_table)
            .expect("opaque desktop System2 projection");
        assert_eq!(
            projection
                .get("schema_version")
                .and_then(toml::Value::as_integer),
            Some(ILHAE_RUNTIME_SYSTEM2_PROJECTION_SCHEMA_VERSION)
        );
        assert_eq!(
            projection
                .get("source_profile_id")
                .and_then(toml::Value::as_str),
            Some("qwen3.6-local")
        );
        assert_eq!(
            projection
                .get("target_profile_id")
                .and_then(toml::Value::as_str),
            Some("minimax-m2.7-turboquant")
        );
        assert_eq!(
            projection.get("base_url").and_then(toml::Value::as_str),
            Some("http://192.168.219.113:8080/v1")
        );
        assert_eq!(
            projection.get("model_name").and_then(toml::Value::as_str),
            Some("MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf")
        );

        // A failed human generation must keep both validated runtime files and
        // repopulate every environment value from that final active generation.
        std::fs::write(
            tmp.path().join("config.toml"),
            b"[profile\ninvalid = true\n",
        )
        .expect("write invalid human config");
        unsafe {
            std::env::set_var("ILHAE_SYSTEM2_ENABLED", "1");
            std::env::set_var("ILHAE_SYSTEM2_SOURCE_PROFILE", "stale-source");
            std::env::set_var("ILHAE_SYSTEM2_PROFILE", "stale-target");
            std::env::set_var("ILHAE_SYSTEM2_BASE_URL", "https://stale.invalid/v1");
            std::env::set_var("ILHAE_SYSTEM2_MODEL", "stale-model");
        }
        prepare_ilhae_codex_home().expect("recover System2 projection from final active");

        assert_eq!(
            std::fs::read(&active_path).expect("read recovered active snapshot"),
            baseline_active
        );
        assert_eq!(
            std::fs::read(&lkg_path).expect("read unchanged System2 LKG snapshot"),
            baseline_lkg
        );
        assert_eq!(get_active_system2_target_config(), Some(expected));
    }

    #[test]
    #[serial_test::serial]
    fn active_profile_system2_resolves_target_profile() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();
        // This test exercises the human-config API, not the managed runtime projection.
        unsafe {
            std::env::remove_var("ILHAE_RUNTIME");
        }

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

    #[test]
    #[serial_test::serial]
    fn runtime_system2_getter_requires_complete_env_without_human_fallback() {
        let tmp = tempdir().expect("tempdir");
        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
        let _runtime_environment = preserve_runtime_environment();

        let config_toml = r#"
[profile]
active = "human-source"

[profiles.human-source.system2]
enabled = true
profile = "human-target"

[profiles.human-target.native_runtime]
base_url = "https://human.invalid/v1"
model_path = "/models/human-model.gguf"
"#;
        std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write human config");

        unsafe {
            std::env::set_var("ILHAE_RUNTIME", "1");
            std::env::remove_var("ILHAE_SYSTEM2_ENABLED");
            std::env::remove_var("ILHAE_SYSTEM2_SOURCE_PROFILE");
            std::env::remove_var("ILHAE_SYSTEM2_PROFILE");
            std::env::remove_var("ILHAE_SYSTEM2_BASE_URL");
            std::env::remove_var("ILHAE_SYSTEM2_MODEL");
        }
        assert_eq!(get_active_system2_target_config(), None);

        unsafe {
            std::env::set_var("ILHAE_SYSTEM2_ENABLED", "1");
            std::env::set_var("ILHAE_SYSTEM2_SOURCE_PROFILE", "runtime-source");
            std::env::set_var("ILHAE_SYSTEM2_PROFILE", "runtime-target");
            std::env::set_var("ILHAE_SYSTEM2_BASE_URL", "https://runtime.test/v1");
        }
        assert_eq!(
            get_active_system2_target_config(),
            None,
            "an incomplete runtime projection must not fall back to human config"
        );

        unsafe {
            std::env::set_var("ILHAE_SYSTEM2_MODEL", "runtime-model.gguf");
        }
        assert_eq!(
            get_active_system2_target_config(),
            Some(ResolvedSystem2TargetConfig {
                source_profile_id: "runtime-source".to_string(),
                target_profile_id: "runtime-target".to_string(),
                base_url: "https://runtime.test/v1".to_string(),
                model_name: "runtime-model.gguf".to_string(),
            })
        );
    }
}
