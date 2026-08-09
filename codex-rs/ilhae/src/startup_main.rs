use crate::*; // Brings in helpers, types, plugins, SharedState, etc.

use crate::mcp_manager::McpManager;

use agent_client_protocol_schema::ContentBlock;
use codex_protocol::user_input::UserInput;
use sacp::DynConnectTo;

use moka::sync::Cache;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::info;
use tracing::warn;

/// Signal from pre-spawn to build_agent_transport: leader has been spawned.
static LEADER_READY: tokio::sync::Notify = tokio::sync::Notify::const_new();

use crate::browser_manager::BrowserManager;
use crate::relay_server::RelayEvent;
use crate::relay_server::RelayState;
use crate::relay_server::broadcast_event;
use crate::relay_server::start_relay_server;
use crate::settings_store::SettingsStore;
use crate::startup::build_agent_transport;
use crate::startup::cleanup_redundant_sessions;
use tokio::sync::broadcast;

// ═════════════════════════════════════════════════════════════════════════

use crate::process_lifecycle::enforce_singleton_proxy;

#[allow(unused_imports)]
use crate::config::*;
#[allow(unused_imports)]
use crate::shared_state::SharedState;

// IlhaeProxy handlers moved to relay_proxy.rs

// ═════════════════════════════════════════════════════════════════════════
// main
// ═════════════════════════════════════════════════════════════════════════
#[derive(Clone)]
pub struct BootstrappedIlhaeRuntime {
    pub ilhae_dir: std::path::PathBuf,
    pub settings_store: Arc<SettingsStore>,
    pub brain: Arc<brain_rs::BrainService>,
    pub cx_cache: CxCache,
}

static NATIVE_RUNTIME_CONTEXT: OnceLock<BootstrappedIlhaeRuntime> = OnceLock::new();
static NATIVE_RUNTIME_BACKGROUND_WORKERS_STARTED: OnceLock<()> = OnceLock::new();
static NATIVE_LOOP_LIFECYCLE_BUS: OnceLock<
    broadcast::Sender<crate::IlhaeLoopLifecycleNotification>,
> = OnceLock::new();
const DEFAULT_GEPA_OPTIMIZER_INTERVAL_SECS: u64 = 1800;
const ILHAE_NATIVE_THINKING_MODE_ENV: &str = "ILHAE_NATIVE_THINKING_MODE";
const ILHAE_NATIVE_RUNTIME_OWNER_TOKEN_ENV: &str = "ILHAE_NATIVE_RUNTIME_OWNER_TOKEN";
const NATIVE_RUNTIME_OWNERSHIP_SCHEMA_VERSION: u32 = 1;
const NATIVE_RUNTIME_OWNERSHIP_FILE: &str = "native-runtime-owner.json";

pub fn native_runtime_context() -> Option<BootstrappedIlhaeRuntime> {
    NATIVE_RUNTIME_CONTEXT.get().cloned()
}

fn native_loop_lifecycle_bus() -> &'static broadcast::Sender<crate::IlhaeLoopLifecycleNotification>
{
    NATIVE_LOOP_LIFECYCLE_BUS.get_or_init(|| {
        let (tx, _rx) = broadcast::channel(256);
        tx
    })
}

pub fn subscribe_native_loop_lifecycle()
-> broadcast::Receiver<crate::IlhaeLoopLifecycleNotification> {
    native_loop_lifecycle_bus().subscribe()
}

pub fn emit_native_loop_lifecycle(notification: crate::IlhaeLoopLifecycleNotification) {
    let _ = native_loop_lifecycle_bus().send(notification);
}

fn spawn_native_runtime_background_workers(runtime: &BootstrappedIlhaeRuntime) {
    if NATIVE_RUNTIME_BACKGROUND_WORKERS_STARTED.set(()).is_err() {
        return;
    }

    let ilhae_dir_for_knowledge_worker = runtime.ilhae_dir.clone();
    let settings_for_knowledge_worker = runtime.settings_store.clone();
    tokio::spawn(async move {
        knowledge_loop::run_worker_loop(
            settings_for_knowledge_worker,
            ilhae_dir_for_knowledge_worker,
        )
        .await;
    });

    let ilhae_dir_for_super_loop = runtime.ilhae_dir.clone();
    let settings_for_super_loop = runtime.settings_store.clone();
    let brain_for_super_loop = runtime.brain.clone();
    let autonomous_sessions_for_super_loop: Arc<
        Cache<String, context_proxy::autonomy::state::AutonomousSessionState>,
    > = Arc::new(
        moka::sync::Cache::builder()
            .time_to_idle(std::time::Duration::from_secs(3600))
            .build(),
    );
    tokio::spawn(async move {
        crate::super_loop::run_worker_loop(
            brain_for_super_loop,
            settings_for_super_loop,
            autonomous_sessions_for_super_loop,
            ilhae_dir_for_super_loop,
        )
        .await;
    });
}

pub fn current_native_backend_engine() -> Option<String> {
    native_runtime_context()
        .map(|runtime| infer_agent_id_from_command(&runtime.settings_store.get().agent.command))
}

pub fn current_native_backend_capability_profile()
-> Option<crate::capabilities::EngineCapabilityProfile> {
    current_native_backend_engine()
        .map(|engine| crate::capabilities::engine_capability_profile(&engine))
}

/// Low-level HTTP liveness probe retained for callers that only need transport
/// availability. Runtime activation must use [`native_runtime_readiness`],
/// which also verifies that the configured model is the one being served.
pub async fn native_runtime_healthcheck(url: &str) -> bool {
    if url.trim().is_empty() {
        return false;
    }

    match reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct NativeRuntimeStatusSnapshot {
    pub profile: String,
    pub enabled: bool,
    pub provider: Option<String>,
    pub model_path: String,
    pub health_url: String,
    pub base_url: String,
    /// True only when both the health endpoint and exact model identity match.
    pub healthy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeRuntimeReadinessError {
    message: String,
    listener_confirmed: bool,
}

impl NativeRuntimeReadinessError {
    fn configuration(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            listener_confirmed: false,
        }
    }

    fn listener(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            listener_confirmed: true,
        }
    }
}

impl fmt::Display for NativeRuntimeReadinessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Debug, serde::Deserialize)]
struct NativeRuntimeModelList {
    #[serde(default)]
    data: Vec<NativeRuntimeModelIdentity>,
    #[serde(default)]
    models: Vec<NativeRuntimeModelIdentity>,
}

#[derive(Debug, serde::Deserialize)]
struct NativeRuntimeModelIdentity {
    id: Option<String>,
    model: Option<String>,
    name: Option<String>,
}

fn exact_model_basename(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    std::path::Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn native_runtime_models_url(base_url: &str) -> Result<url::Url, NativeRuntimeReadinessError> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err(NativeRuntimeReadinessError::configuration(
            "native runtime base_url is required for exact model readiness",
        ));
    }

    let mut url = url::Url::parse(trimmed).map_err(|error| {
        NativeRuntimeReadinessError::configuration(format!(
            "native runtime base_url is invalid: {error}"
        ))
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(NativeRuntimeReadinessError::configuration(
            "native runtime base_url must be an http(s) URL with a host",
        ));
    }

    let base_path = url.path().trim_end_matches('/');
    let models_path = if base_path.is_empty() {
        "/models".to_string()
    } else {
        format!("{base_path}/models")
    };
    url.set_path(&models_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn model_list_contains_exact_basename(
    response: &NativeRuntimeModelList,
    expected_basename: &str,
) -> (bool, Vec<String>) {
    let mut observed = Vec::new();
    for identity in response.data.iter().chain(response.models.iter()) {
        for value in [
            identity.id.as_deref(),
            identity.model.as_deref(),
            identity.name.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(basename) = exact_model_basename(value) {
                if basename == expected_basename {
                    return (true, observed);
                }
                if !observed.contains(&basename) {
                    observed.push(basename);
                }
            }
        }
    }
    (false, observed)
}

fn native_runtime_expected_model_identity(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> Option<String> {
    let mut alias = None;
    let mut args = config.args.iter();
    while let Some(arg) = args.next() {
        if matches!(arg.as_str(), "--alias" | "-a") {
            alias = args.next().map(String::as_str);
            continue;
        }
        if let Some(value) = arg
            .strip_prefix("--alias=")
            .or_else(|| arg.strip_prefix("-a="))
        {
            alias = Some(value);
        }
    }

    alias
        .and_then(exact_model_basename)
        .or_else(|| exact_model_basename(&config.model_path))
}

fn native_runtime_readiness_headers(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> Result<reqwest::header::HeaderMap, NativeRuntimeReadinessError> {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(configured) = config.http_headers.as_ref() {
        for (raw_name, raw_value) in configured {
            let name = reqwest::header::HeaderName::from_bytes(raw_name.trim().as_bytes())
                .map_err(|error| {
                    NativeRuntimeReadinessError::configuration(format!(
                        "native runtime HTTP header name `{raw_name}` is invalid: {error}"
                    ))
                })?;
            let value = reqwest::header::HeaderValue::from_str(raw_value).map_err(|error| {
                NativeRuntimeReadinessError::configuration(format!(
                    "native runtime HTTP header `{raw_name}` has an invalid value: {error}"
                ))
            })?;
            headers.insert(name, value);
        }
    }

    let mut env_headers = config.env_http_headers.clone().unwrap_or_default();
    if let Some(token_env) = config
        .proxy_control_token_env
        .as_deref()
        .map(str::trim)
        .filter(|token_env| !token_env.is_empty())
    {
        env_headers.insert("X-Ilhae-Runtime-Token".to_string(), token_env.to_string());
    }
    for (raw_name, raw_env_name) in env_headers {
        let env_name = raw_env_name.trim();
        let raw_value = std::env::var(env_name).map_err(|_| {
            NativeRuntimeReadinessError::configuration(format!(
                "native runtime HTTP header environment variable `{env_name}` is not set"
            ))
        })?;
        let name = reqwest::header::HeaderName::from_bytes(raw_name.trim().as_bytes()).map_err(
            |error| {
                NativeRuntimeReadinessError::configuration(format!(
                    "native runtime HTTP header name `{raw_name}` is invalid: {error}"
                ))
            },
        )?;
        let value = reqwest::header::HeaderValue::from_str(&raw_value).map_err(|error| {
            NativeRuntimeReadinessError::configuration(format!(
                "native runtime HTTP header `{raw_name}` from `{env_name}` has an invalid value: {error}"
            ))
        })?;
        headers.insert(name, value);
    }
    Ok(headers)
}

async fn probe_native_runtime_readiness(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> Result<(), NativeRuntimeReadinessError> {
    let health_url = native_runtime_readiness_health_url(config);
    let health_url = health_url.trim();
    if health_url.is_empty() {
        return Err(NativeRuntimeReadinessError::configuration(
            "native runtime health_url is required",
        ));
    }
    let expected_basename = native_runtime_expected_model_identity(config).ok_or_else(|| {
        NativeRuntimeReadinessError::configuration(
            "native runtime model_path or --alias must identify an exact model",
        )
    })?;
    let base_url = crate::config::native_runtime_effective_base_url(config);
    let models_url = native_runtime_models_url(&base_url)?;
    let client = reqwest::Client::new();
    let headers = native_runtime_readiness_headers(config)?;

    let health_response = client
        .get(health_url)
        .headers(headers.clone())
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|error| {
            NativeRuntimeReadinessError::configuration(format!(
                "native runtime health endpoint is unavailable: {error}"
            ))
        })?;
    if !health_response.status().is_success() {
        return Err(NativeRuntimeReadinessError::listener(format!(
            "native runtime health endpoint returned {}",
            health_response.status()
        )));
    }

    let models_response = client
        .get(models_url.clone())
        .headers(headers)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|error| {
            NativeRuntimeReadinessError::listener(format!(
                "native runtime models endpoint `{models_url}` is unavailable: {error}"
            ))
        })?;
    if !models_response.status().is_success() {
        return Err(NativeRuntimeReadinessError::listener(format!(
            "native runtime models endpoint returned {}",
            models_response.status()
        )));
    }
    let model_list = models_response
        .json::<NativeRuntimeModelList>()
        .await
        .map_err(|error| {
            NativeRuntimeReadinessError::listener(format!(
                "native runtime models response is malformed: {error}"
            ))
        })?;
    let (matches, observed) = model_list_contains_exact_basename(&model_list, &expected_basename);
    if !matches {
        let observed = if observed.is_empty() {
            "none".to_string()
        } else {
            observed.join(", ")
        };
        return Err(NativeRuntimeReadinessError::listener(format!(
            "native runtime model mismatch: expected exact basename `{expected_basename}`, observed `{observed}`"
        )));
    }
    Ok(())
}

fn native_runtime_readiness_health_url(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> String {
    if crate::native_runtime_endpoint::uses_remote_proxy(config)
        && let Some(proxy_base_url) =
            crate::native_runtime_endpoint::effective_proxy_base_url(config)
    {
        return crate::config::native_runtime_health_url_from_base_url(&proxy_base_url);
    }
    crate::config::native_runtime_effective_health_url(config)
}

/// Returns true only when the configured endpoint is healthy and serves the
/// exact configured model basename. Suffix and substring matches are rejected.
pub async fn native_runtime_readiness(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> bool {
    probe_native_runtime_readiness(config).await.is_ok()
}

pub async fn native_runtime_status_snapshot(
    profile_id: Option<&str>,
) -> Option<NativeRuntimeStatusSnapshot> {
    let (profile, config) = crate::config::get_native_runtime_config(profile_id)?;
    let activated = config.enabled || crate::native_runtime_endpoint::uses_remote_proxy(&config);
    let healthy = activated && native_runtime_readiness(&config).await;
    Some(NativeRuntimeStatusSnapshot {
        profile,
        enabled: activated,
        provider: config.provider.clone(),
        model_path: config.model_path.clone(),
        health_url: native_runtime_readiness_health_url(&config),
        base_url: crate::config::native_runtime_effective_base_url(&config),
        healthy,
    })
}

fn parse_positive_env_secs(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn gepa_optimizer_interval_secs() -> u64 {
    parse_positive_env_secs(
        "ILHAE_GEPA_OPTIMIZER_INTERVAL_SECS",
        DEFAULT_GEPA_OPTIMIZER_INTERVAL_SECS,
    )
}

fn gepa_auto_approve_enabled() -> bool {
    matches!(
        std::env::var("ILHAE_GEPA_AUTO_APPROVE")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn build_gepa_optimizer_request(
    preset: &str,
    subject: &str,
    detail: &str,
    group_count: usize,
    top_paths: Vec<String>,
) -> crate::super_loop::GepaSidecarRequest {
    let base_spec = crate::super_loop::default_self_improvement_followup_spec_for_runtime();
    crate::super_loop::GepaSidecarRequest {
        kind: "self_improvement_followup_offline".to_string(),
        preset: preset.to_string(),
        subject: subject.to_string(),
        detail: detail.to_string(),
        prompt: base_spec.prompt,
        instructions: base_spec.instructions,
        task_history: Vec::new(),
        top_paths,
        group_count: Some(group_count),
    }
}

fn gate_gepa_optimizer_candidate(
    response: &crate::super_loop::GepaSidecarResponse,
) -> Result<(String, String, f64), String> {
    let prompt = response
        .optimized_prompt
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    let instructions = response
        .optimized_instructions
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if prompt.is_empty() || instructions.is_empty() {
        return Err("missing optimized prompt or instructions".to_string());
    }
    if prompt.len() > 720 {
        return Err(format!("prompt too long: {} chars", prompt.len()));
    }
    if instructions.len() > 1400 {
        return Err(format!(
            "instructions too long: {} chars",
            instructions.len()
        ));
    }

    let prompt_lower = prompt.to_ascii_lowercase();
    let instructions_lower = instructions.to_ascii_lowercase();
    if !(prompt_lower.contains("review") || prompt_lower.contains("summarize")) {
        return Err("prompt must keep review/summarize intent".to_string());
    }
    if !instructions_lower.contains("memory_dream_") {
        return Err("instructions must preserve memory_dream tool scope".to_string());
    }
    if !instructions_lower.contains("skill_upsert") {
        return Err("instructions must preserve skill_upsert guidance".to_string());
    }
    let score = response.score.unwrap_or(0.0);
    if score <= 0.0 {
        return Err("candidate score did not improve baseline".to_string());
    }
    Ok((prompt.to_string(), instructions.to_string(), score))
}

struct NativeRuntimeStartLock {
    file: std::fs::File,
}

impl Drop for NativeRuntimeStartLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;

            // SAFETY: file descriptor is owned by this guard and remains valid until drop completes.
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

fn native_runtime_start_lock_path(
    _profile_id: &str,
    _config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> std::path::PathBuf {
    crate::config::resolve_ilhae_data_dir()
        .join("run")
        .join("native-runtime-start.lock")
}

fn acquire_native_runtime_start_lock(
    profile_id: &str,
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<NativeRuntimeStartLock> {
    let path = native_runtime_start_lock_path(profile_id, config);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).append(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // Deny all sharing while the guard owns the file handle.
        options.share_mode(0);
    }
    let file = options.open(&path)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;

        // SAFETY: flock is called on a valid file descriptor and the guard unlocks it on drop.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            return Err(anyhow::Error::from(std::io::Error::last_os_error()));
        }
    }
    Ok(NativeRuntimeStartLock { file })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct NativeRuntimeOwnershipRecord {
    schema_version: u32,
    profile_id: String,
    pid: u32,
    process_start_ticks: u64,
    owner_token: String,
    server_executable: String,
    model_path: String,
    #[serde(default)]
    runtime_config_sha256: Option<String>,
}

fn native_runtime_ownership_path() -> std::path::PathBuf {
    crate::config::resolve_ilhae_data_dir()
        .join("run")
        .join(NATIVE_RUNTIME_OWNERSHIP_FILE)
}

fn read_native_runtime_ownership_record() -> anyhow::Result<Option<NativeRuntimeOwnershipRecord>> {
    let path = native_runtime_ownership_path();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let record =
        serde_json::from_slice::<NativeRuntimeOwnershipRecord>(&bytes).map_err(|error| {
            anyhow::anyhow!(
                "invalid native runtime ownership record `{}`: {error}",
                path.display()
            )
        })?;
    if record.schema_version != NATIVE_RUNTIME_OWNERSHIP_SCHEMA_VERSION {
        anyhow::bail!(
            "unsupported native runtime ownership schema {} in `{}`",
            record.schema_version,
            path.display()
        );
    }
    Ok(Some(record))
}

fn write_native_runtime_ownership_record(
    record: &NativeRuntimeOwnershipRecord,
) -> anyhow::Result<()> {
    let path = native_runtime_ownership_path();
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("native runtime ownership path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let temporary_path = parent.join(format!(
        ".{NATIVE_RUNTIME_OWNERSHIP_FILE}.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> anyhow::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary_path)?;
        let bytes = serde_json::to_vec(record)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        std::io::Write::write_all(&mut file, b"\n")?;
        file.sync_all()?;
        std::fs::rename(&temporary_path, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

fn clear_native_runtime_ownership_record_if_matches(record: &NativeRuntimeOwnershipRecord) {
    let Ok(Some(current)) = read_native_runtime_ownership_record() else {
        return;
    };
    if current == *record {
        let _ = std::fs::remove_file(native_runtime_ownership_path());
    }
}

fn process_start_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let command_end = stat.rfind(')')?;
    let fields = stat.get(command_end + 1..)?.split_whitespace();
    // The remainder starts at proc(5) field 3; starttime is field 22.
    fields.skip(19).next()?.parse().ok()
}

fn read_proc_executable(pid: u32) -> Option<std::path::PathBuf> {
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    std::fs::canonicalize(&executable).ok().or(Some(executable))
}

fn proc_environ_contains_owner_token(pid: u32, owner_token: &str) -> bool {
    let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    let expected = format!("{ILHAE_NATIVE_RUNTIME_OWNER_TOKEN_ENV}={owner_token}");
    environ
        .split(|byte| *byte == 0)
        .any(|entry| entry == expected.as_bytes())
}

fn resolve_server_executable(server_bin: &str) -> anyhow::Result<std::path::PathBuf> {
    let trimmed = server_bin.trim();
    if trimmed.is_empty() {
        anyhow::bail!("native runtime server_bin is required");
    }
    let configured = std::path::Path::new(trimmed);
    let candidate = if configured.is_absolute() || configured.components().count() > 1 {
        configured.to_path_buf()
    } else {
        std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join(configured))
                    .find(|candidate| candidate.is_file())
            })
            .ok_or_else(|| anyhow::anyhow!("native runtime server `{trimmed}` was not found"))?
    };
    std::fs::canonicalize(&candidate).map_err(|error| {
        anyhow::anyhow!(
            "failed to resolve native runtime server `{}`: {error}",
            candidate.display()
        )
    })
}

fn attest_native_runtime_process(record: &NativeRuntimeOwnershipRecord) -> anyhow::Result<()> {
    let actual_start = process_start_ticks(record.pid).ok_or_else(|| {
        anyhow::anyhow!("managed native runtime PID {} is not running", record.pid)
    })?;
    if actual_start != record.process_start_ticks {
        anyhow::bail!(
            "managed native runtime PID {} was reused (expected start {}, observed {})",
            record.pid,
            record.process_start_ticks,
            actual_start
        );
    }
    let actual_executable = read_proc_executable(record.pid).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot attest executable for managed native runtime PID {}",
            record.pid
        )
    })?;
    if actual_executable != std::path::Path::new(&record.server_executable) {
        anyhow::bail!(
            "managed native runtime executable mismatch for PID {}",
            record.pid
        );
    }
    if !proc_environ_contains_owner_token(record.pid, &record.owner_token) {
        anyhow::bail!(
            "managed native runtime owner token mismatch for PID {}",
            record.pid
        );
    }
    let cmdline = read_proc_cmdline(record.pid).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot attest command line for managed native runtime PID {}",
            record.pid
        )
    })?;
    if !cmdline_contains_model_path(&cmdline, &record.model_path) {
        anyhow::bail!(
            "managed native runtime model argument mismatch for PID {}",
            record.pid
        );
    }
    Ok(())
}

fn live_managed_native_runtime_record_any() -> anyhow::Result<Option<NativeRuntimeOwnershipRecord>>
{
    let Some(record) = read_native_runtime_ownership_record()? else {
        return Ok(None);
    };

    let Some(actual_start) = process_start_ticks(record.pid) else {
        clear_native_runtime_ownership_record_if_matches(&record);
        return Ok(None);
    };
    if actual_start != record.process_start_ticks {
        // The recorded process is gone and its PID was reused. The replacement
        // is never considered owned and must not receive a signal.
        clear_native_runtime_ownership_record_if_matches(&record);
        return Ok(None);
    }

    attest_native_runtime_process(&record)?;
    Ok(Some(record))
}

fn live_managed_native_runtime_record(
    profile_id: &str,
) -> anyhow::Result<Option<NativeRuntimeOwnershipRecord>> {
    let Some(record) = live_managed_native_runtime_record_any()? else {
        return Ok(None);
    };
    if record.profile_id != profile_id {
        anyhow::bail!(
            "live managed native runtime belongs to profile `{}`, not `{profile_id}`; refusing to signal it",
            record.profile_id
        );
    }
    Ok(Some(record))
}

fn spawn_native_runtime_server_locked(
    profile_id: &str,
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
    thinking_mode: &str,
) -> anyhow::Result<u32> {
    if profile_id.trim().is_empty() {
        anyhow::bail!("native runtime profile id is required");
    }
    let model_path = config.model_path.trim();
    if model_path.is_empty() {
        anyhow::bail!("native runtime model_path is required");
    }
    if live_managed_native_runtime_record(profile_id)?.is_some() {
        anyhow::bail!("native runtime profile `{profile_id}` is already managed and running");
    }

    let server_executable = resolve_server_executable(&config.server_bin)?;
    let owner_token = uuid::Uuid::new_v4().to_string();
    let mut command = std::process::Command::new(&server_executable);
    for (key, value) in effective_native_runtime_env(config, thinking_mode) {
        command.env(key, value);
    }
    // This is a supervisor-owned attestation secret, not profile-controlled
    // configuration. Set it last so a profile cannot replace the token that is
    // persisted in the ownership record.
    command.env(ILHAE_NATIVE_RUNTIME_OWNER_TOKEN_ENV, &owner_token);
    if config.args.is_empty() {
        command.arg("-m").arg(model_path);
        if !config.chat_template_file.trim().is_empty() {
            command
                .arg("--chat-template-file")
                .arg(&config.chat_template_file);
        }
        if native_runtime_supports_reasoning_flag(config) {
            command.arg("--reasoning").arg(&thinking_mode);
        }
    } else {
        command.args(effective_native_runtime_args(config, thinking_mode));
    }

    #[cfg(unix)]
    command.process_group(0);
    command.stdin(Stdio::null());

    if config.log_file.trim().is_empty() {
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
    } else {
        let log_path = std::path::Path::new(&config.log_file);
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let stdout = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        let stderr = stdout.try_clone()?;
        command.stdout(Stdio::from(stdout));
        command.stderr(Stdio::from(stderr));
    }

    let mut child = command.spawn()?;
    let pid = child.id();
    let attestation_deadline = std::time::Instant::now() + Duration::from_secs(1);
    let record = loop {
        if let (Some(process_start_ticks), Some(actual_executable)) =
            (process_start_ticks(pid), read_proc_executable(pid))
        {
            let candidate = NativeRuntimeOwnershipRecord {
                schema_version: NATIVE_RUNTIME_OWNERSHIP_SCHEMA_VERSION,
                profile_id: profile_id.to_string(),
                pid,
                process_start_ticks,
                owner_token: owner_token.clone(),
                server_executable: server_executable.to_string_lossy().into_owned(),
                model_path: model_path.to_string(),
                runtime_config_sha256: Some(
                    native_runtime_execution_fingerprint_with_thinking_mode(config, thinking_mode),
                ),
            };
            if actual_executable == server_executable
                && attest_native_runtime_process(&candidate).is_ok()
            {
                break candidate;
            }
        }
        if let Some(status) = child.try_wait()? {
            anyhow::bail!(
                "native runtime for profile `{profile_id}` exited before ownership attestation: {status}"
            );
        }
        if std::time::Instant::now() >= attestation_deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "native runtime for profile `{profile_id}` failed executable/model/owner/start-time attestation"
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    if let Err(error) = write_native_runtime_ownership_record(&record) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(anyhow::anyhow!(
            "failed to persist native runtime ownership: {error}"
        ));
    }

    let reaper_record = record.clone();
    std::thread::spawn(move || {
        match child.wait() {
            Ok(status) => {
                if status.success() {
                    info!(pid, "[NativeRuntime] local model server exited normally");
                } else {
                    warn!(pid, status = %status, "[NativeRuntime] local model server exited with error");
                }
            }
            Err(error) => {
                warn!(pid, error = %error, "[NativeRuntime] error waiting for local model server");
            }
        }
        clear_native_runtime_ownership_record_if_matches(&reaper_record);
    });

    info!(
        profile = %profile_id,
        pid,
        server_bin = %config.server_bin,
        "[NativeRuntime] spawned attested local model server"
    );
    Ok(pid)
}

pub fn spawn_native_runtime_server(
    profile_id: &str,
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<u32> {
    let thinking_mode = crate::config::current_thinking_mode();
    let _start_lock = acquire_native_runtime_start_lock(profile_id, config)?;
    if configured_runtime_listener_present(config) {
        anyhow::bail!(
            "native runtime endpoint for profile `{profile_id}` is already occupied; refusing to replace an unattested listener"
        );
    }
    spawn_native_runtime_server_locked(profile_id, config, &thinking_mode)
}

fn native_runtime_supports_reasoning_flag(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> bool {
    if let Some(provider) = config.provider.as_deref().map(str::trim)
        && !provider.is_empty()
    {
        return provider == "llama-server";
    }

    std::path::Path::new(&config.server_bin)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == "llama-server")
        .unwrap_or(false)
}

fn effective_native_runtime_args(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
    thinking_mode: &str,
) -> Vec<String> {
    if !native_runtime_supports_reasoning_flag(config) {
        return config.args.clone();
    }

    let mut args = Vec::with_capacity(config.args.len() + 2);
    let mut iter = config.args.iter();
    while let Some(arg) = iter.next() {
        if matches!(arg.as_str(), "-rea" | "--reasoning") {
            let _ = iter.next();
            continue;
        }
        if arg.starts_with("-rea=") || arg.starts_with("--reasoning=") {
            continue;
        }
        args.push(arg.clone());
    }
    args.push("--reasoning".to_string());
    args.push(thinking_mode.to_string());
    args
}

fn effective_native_runtime_env(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
    thinking_mode: &str,
) -> Vec<(String, String)> {
    let mut env = Vec::with_capacity(config.env.len() + 1);
    env.push((
        ILHAE_NATIVE_THINKING_MODE_ENV.to_string(),
        thinking_mode.to_string(),
    ));
    env.extend(
        config
            .env
            .iter()
            .filter(|(key, _)| !key.trim().is_empty())
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    env
}

#[derive(serde::Serialize)]
struct NativeRuntimeExecutionSpec<'a> {
    enabled: bool,
    provider: Option<&'a str>,
    health_url: String,
    base_url: String,
    server_bin: &'a str,
    model_path: &'a str,
    chat_template_file: &'a str,
    log_file: &'a str,
    env: Vec<(String, String)>,
    args: Vec<String>,
}

pub fn native_runtime_execution_fingerprint(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> String {
    let thinking_mode = crate::config::current_thinking_mode();
    native_runtime_execution_fingerprint_with_thinking_mode(config, &thinking_mode)
}

pub(crate) fn native_runtime_execution_fingerprint_with_thinking_mode(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
    thinking_mode: &str,
) -> String {
    use sha2::Digest;

    let spec = NativeRuntimeExecutionSpec {
        enabled: config.enabled,
        provider: config.provider.as_deref().map(str::trim),
        health_url: crate::config::native_runtime_effective_health_url(config),
        base_url: crate::config::native_runtime_effective_base_url(config),
        server_bin: config.server_bin.trim(),
        model_path: config.model_path.trim(),
        chat_template_file: config.chat_template_file.trim(),
        log_file: config.log_file.trim(),
        env: effective_native_runtime_env(config, thinking_mode),
        args: effective_native_runtime_args(config, thinking_mode),
    };
    let encoded = serde_json::to_vec(&spec).expect("native runtime execution spec must serialize");
    format!("{:x}", sha2::Sha256::digest(encoded))
}

fn native_runtime_configs_equivalent(
    left: &crate::config::IlhaeProfileNativeRuntimeConfig,
    right: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> bool {
    native_runtime_execution_fingerprint(left) == native_runtime_execution_fingerprint(right)
        && crate::native_runtime_endpoint::effective_proxy_control_url(left)
            == crate::native_runtime_endpoint::effective_proxy_control_url(right)
        && crate::native_runtime_endpoint::ssh_host(left)
            == crate::native_runtime_endpoint::ssh_host(right)
        && crate::native_runtime_endpoint::ssh_local_port(left)
            == crate::native_runtime_endpoint::ssh_local_port(right)
        && crate::native_runtime_endpoint::ssh_remote_port(left)
            == crate::native_runtime_endpoint::ssh_remote_port(right)
        && left.proxy_control_token_env.as_deref().map(str::trim)
            == right.proxy_control_token_env.as_deref().map(str::trim)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeRuntimeReusePolicy {
    AllowExactUnmanaged,
    RequireManagedExecutionSpec,
}

fn read_proc_cmdline(pid: u32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(
        bytes
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).to_string())
            .collect(),
    )
}

fn cmdline_contains_model_path(cmdline: &[String], model_path: &str) -> bool {
    cmdline.iter().any(|arg| {
        arg == model_path
            || arg
                .strip_prefix("--model=")
                .or_else(|| arg.strip_prefix("-m="))
                .is_some_and(|value| value == model_path)
    })
}

fn extract_port_from_config(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> Option<u16> {
    if let Some(port) = crate::native_runtime_endpoint::runtime_port_from_args(config) {
        return Some(port);
    }

    let health_url = crate::config::native_runtime_effective_health_url(config);
    let base_url = crate::config::native_runtime_effective_base_url(config);
    [&health_url, &base_url]
        .into_iter()
        .filter_map(|value| url::Url::parse(value.trim()).ok())
        .find_map(|url| url.port_or_known_default())
}

fn find_pid_by_port(port: u16) -> Option<u32> {
    // Only consider the listener on the runtime port. Tools such as fuser also
    // report clients connected to the port, which can include the Responses API
    // compatibility proxy that must survive native-runtime preemption.
    if let Ok(output) = std::process::Command::new("lsof")
        .args(["-nP", "-t", "-sTCP:LISTEN", "-iTCP", &format!(":{port}")])
        .output()
        && output.status.success()
    {
        let s = String::from_utf8_lossy(&output.stdout);
        if let Some(pid_str) = s.trim().lines().next()
            && let Ok(pid) = pid_str.trim().parse::<u32>()
        {
            return Some(pid);
        }
    }

    // Try ss as fallback.
    if let Ok(output) = std::process::Command::new("ss")
        .args(["-lptn", &format!("sport = :{}", port)])
        .output()
        && output.status.success()
    {
        let s = String::from_utf8_lossy(&output.stdout);
        // Example: LISTEN 0 128 *:8082 *:* users:(("llama-server",pid=701443,fd=16))
        if let Some(pos) = s.find("pid=") {
            let rest = &s[pos + 4..];
            if let Some(end) = rest.find(',')
                && let Ok(pid) = rest[..end].parse::<u32>()
            {
                return Some(pid);
            }
        }
    }

    None
}

fn configured_runtime_listener_present(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> bool {
    let Some(configured_port) = extract_port_from_config(config) else {
        return false;
    };
    if find_pid_by_port(configured_port).is_some() {
        return true;
    }

    let mut addresses = Vec::new();
    for value in [&config.health_url, &config.base_url] {
        let Ok(url) = url::Url::parse(value.trim()) else {
            continue;
        };
        let Some(host) = url.host_str() else {
            continue;
        };
        let Some(port) = url.port_or_known_default() else {
            continue;
        };
        if let Ok(resolved) = std::net::ToSocketAddrs::to_socket_addrs(&(host, port)) {
            addresses.extend(resolved);
        }
    }
    if addresses.is_empty() {
        addresses.push(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            configured_port,
        )));
    }
    addresses.into_iter().any(|address| {
        std::net::TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok()
    })
}

fn native_runtime_cmdline_matches(
    cmdline: &[String],
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> bool {
    let server_name = std::path::Path::new(&config.server_bin)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .trim();
    if server_name.is_empty() {
        return false;
    }

    let server_matches = cmdline.first().is_some_and(|arg| {
        std::path::Path::new(arg)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name == server_name)
            .unwrap_or(false)
    });
    if !server_matches {
        return false;
    }

    let model_path = config.model_path.trim();
    if model_path.is_empty() {
        return true;
    }

    cmdline_contains_model_path(cmdline, model_path)
}

pub fn find_native_runtime_pids(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> Vec<u32> {
    let current_pid = std::process::id();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
        })
        .filter(|pid| *pid != current_pid)
        .filter(|pid| {
            read_proc_cmdline(*pid)
                .map(|cmdline| native_runtime_cmdline_matches(&cmdline, config))
                .unwrap_or(false)
        })
        .collect()
}

fn send_native_runtime_signal(pid: u32, signal: i32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: PID and signal are validated scalar values; ownership is
        // attested immediately before this helper is called.
        let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
        if result == 0 {
            return Ok(());
        }
        return Err(std::io::Error::last_os_error().into());
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, signal);
        anyhow::bail!("managed native runtime signaling is unsupported on this platform");
    }
}

async fn stop_managed_native_runtime_record_locked(
    record: NativeRuntimeOwnershipRecord,
) -> anyhow::Result<()> {
    // Full token/executable/model/start-time attestation occurs again at the
    // destructive boundary, after the caller has acquired the global lock.
    attest_native_runtime_process(&record)?;
    info!(
        profile = %record.profile_id,
        pid = record.pid,
        "[NativeRuntime] terminating owned local model server"
    );
    send_native_runtime_signal(record.pid, libc::SIGTERM)?;

    let graceful_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        match process_start_ticks(record.pid) {
            None => {
                clear_native_runtime_ownership_record_if_matches(&record);
                break;
            }
            Some(start_ticks) if start_ticks != record.process_start_ticks => {
                // PID reuse is success for stopping the old process, but the
                // replacement is explicitly outside our ownership boundary.
                clear_native_runtime_ownership_record_if_matches(&record);
                break;
            }
            Some(_) if tokio::time::Instant::now() >= graceful_deadline => {
                attest_native_runtime_process(&record)?;
                send_native_runtime_signal(record.pid, libc::SIGKILL)?;
                let kill_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                loop {
                    match process_start_ticks(record.pid) {
                        None => break,
                        Some(start_ticks) if start_ticks != record.process_start_ticks => break,
                        Some(_) if tokio::time::Instant::now() >= kill_deadline => {
                            anyhow::bail!(
                                "owned native runtime PID {} did not exit after SIGKILL",
                                record.pid
                            );
                        }
                        Some(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                    }
                }
                clear_native_runtime_ownership_record_if_matches(&record);
                break;
            }
            Some(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }

    info!(
        profile = %record.profile_id,
        pid = record.pid,
        "[NativeRuntime] stopped owned local model server"
    );
    Ok(())
}

async fn stop_managed_native_runtime_server_locked(
    profile_id: &str,
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<()> {
    let Some(record) = live_managed_native_runtime_record(profile_id)? else {
        if configured_runtime_listener_present(config) {
            anyhow::bail!(
                "native runtime endpoint for profile `{profile_id}` is occupied by an unattested process; refusing to signal it"
            );
        }
        info!(
            profile = %profile_id,
            "[NativeRuntime] no owned local model server to stop"
        );
        return Ok(());
    };

    stop_managed_native_runtime_record_locked(record).await
}

pub async fn stop_native_runtime_server_for_config(
    profile_id: &str,
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<()> {
    let _start_lock = acquire_native_runtime_start_lock(profile_id, config)?;
    stop_managed_native_runtime_server_locked(profile_id, config).await
}

async fn ensure_native_runtime_for_config_with_policy(
    profile_id: &str,
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
    thinking_mode: &str,
    reuse_policy: NativeRuntimeReusePolicy,
) -> anyhow::Result<()> {
    crate::gpu_queue::start_gate::wait_for_native_runtime_start_gate().await?;

    let desired_fingerprint =
        native_runtime_execution_fingerprint_with_thinking_mode(config, thinking_mode);
    let _start_lock = acquire_native_runtime_start_lock(profile_id, config)?;
    if let Some(record) = live_managed_native_runtime_record_any()? {
        let exact_execution_spec = record.profile_id == profile_id
            && record.runtime_config_sha256.as_deref() == Some(desired_fingerprint.as_str());
        if exact_execution_spec && probe_native_runtime_readiness(config).await.is_ok() {
            return Ok(());
        }
        stop_managed_native_runtime_record_locked(record).await?;
    } else if let Err(readiness_error) = probe_native_runtime_readiness(config).await {
        let matching_unowned_pids = find_native_runtime_pids(config);
        if readiness_error.listener_confirmed
            || configured_runtime_listener_present(config)
            || !matching_unowned_pids.is_empty()
        {
            anyhow::bail!(
                "native runtime profile `{profile_id}` is not exact-model ready ({readiness_error}), but its endpoint/process is not owned by Ilhae; refusing to kill or replace it"
            );
        }
    } else if reuse_policy == NativeRuntimeReusePolicy::AllowExactUnmanaged {
        return Ok(());
    } else {
        anyhow::bail!(
            "native runtime profile `{profile_id}` is exact-model ready but is not owned by Ilhae; refusing to claim that its complete execution specification matches"
        );
    }

    let runtime_pid = spawn_native_runtime_server_locked(profile_id, config, thinking_mode)?;

    let timeout_secs = config.startup_timeout_secs.max(1);
    let started = tokio::time::Instant::now();
    loop {
        let last_readiness_error = match probe_native_runtime_readiness(config).await {
            Ok(()) => break,
            Err(error) => error,
        };
        if process_start_ticks(runtime_pid).is_none() {
            let log_hint = if config.log_file.trim().is_empty() {
                "no native runtime log_file is configured".to_string()
            } else {
                format!("see {}", config.log_file)
            };
            anyhow::bail!(
                "native runtime for profile `{profile_id}` exited before exact-model readiness; {log_hint}"
            );
        }
        if started.elapsed().as_secs() >= timeout_secs {
            let last_error = last_readiness_error.to_string();
            if let Err(error) = stop_managed_native_runtime_server_locked(profile_id, config).await
            {
                warn!(
                    profile = %profile_id,
                    error = %error,
                    "[NativeRuntime] failed to clean up runtime after readiness timeout"
                );
            }
            anyhow::bail!(
                "native runtime for profile `{profile_id}` did not reach exact-model readiness within {timeout_secs}s: {last_error}"
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    info!(
        profile = %profile_id,
        model = %config.model_path,
        base_url = %crate::config::native_runtime_effective_base_url(config),
        "[NativeRuntime] exact local model runtime ready"
    );
    Ok(())
}

pub async fn ensure_native_runtime_for_config(
    profile_id: &str,
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<()> {
    let thinking_mode = crate::config::current_thinking_mode();
    ensure_native_runtime_for_config_with_thinking_mode(profile_id, config, &thinking_mode).await
}

pub(crate) async fn ensure_native_runtime_for_config_with_thinking_mode(
    profile_id: &str,
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
    thinking_mode: &str,
) -> anyhow::Result<()> {
    ensure_native_runtime_for_config_with_policy(
        profile_id,
        config,
        thinking_mode,
        NativeRuntimeReusePolicy::RequireManagedExecutionSpec,
    )
    .await
}

pub(crate) async fn ensure_native_runtime_for_proxy_with_thinking_mode(
    profile_id: &str,
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
    thinking_mode: &str,
) -> anyhow::Result<()> {
    if config.enabled {
        ensure_native_runtime_for_config_with_policy(
            profile_id,
            config,
            thinking_mode,
            NativeRuntimeReusePolicy::AllowExactUnmanaged,
        )
        .await
    } else {
        probe_native_runtime_readiness(config).await.map_err(|error| {
            anyhow::anyhow!(
                "native runtime profile `{profile_id}` is configured for connection-only mode but is not exact-model ready: {error}"
            )
        })
    }
}

pub async fn ensure_native_runtime_for_cli(profile_id: Option<&str>) -> anyhow::Result<()> {
    let Some((profile_id, config)) = crate::config::get_native_runtime_config(profile_id) else {
        return Ok(());
    };

    if crate::native_runtime_endpoint::uses_remote_proxy(&config) {
        crate::native_runtime_proxy::ensure_remote_native_runtime(&profile_id, &config).await?;
    } else if config.enabled {
        let thinking_mode = crate::config::current_thinking_mode();
        ensure_native_runtime_for_config_with_policy(
            &profile_id,
            &config,
            &thinking_mode,
            NativeRuntimeReusePolicy::AllowExactUnmanaged,
        )
        .await?;
    } else {
        return Ok(());
    }

    let base_url = crate::config::native_runtime_effective_base_url(&config);
    if !base_url.trim().is_empty() {
        unsafe {
            std::env::set_var("CODEX_OSS_BASE_URL", &base_url);
        }
    }
    Ok(())
}

pub async fn switch_native_runtime_for_cli(
    previous_profile_id: Option<&str>,
    next_profile_id: Option<&str>,
) -> anyhow::Result<()> {
    let previous =
        previous_profile_id.and_then(|id| crate::config::get_native_runtime_config(Some(id)));
    let next = next_profile_id.and_then(|id| crate::config::get_native_runtime_config(Some(id)));

    if let Some((previous_id, previous_config)) = previous.as_ref()
        && (previous_config.enabled
            || crate::native_runtime_endpoint::uses_remote_proxy(previous_config))
    {
        let should_stop_previous = next
            .as_ref()
            .map(|(_, next_config)| {
                !native_runtime_configs_equivalent(previous_config, next_config)
            })
            .unwrap_or(true);
        if should_stop_previous {
            stop_configured_native_runtime(previous_id, previous_config).await?;
        }
    }

    if let Some((next_id, _)) = next {
        ensure_native_runtime_for_cli(Some(&next_id)).await?;
    } else {
        unsafe {
            std::env::remove_var("CODEX_OSS_BASE_URL");
        }
    }

    Ok(())
}

pub async fn stop_native_runtime_for_cli(profile_id: Option<&str>) -> anyhow::Result<()> {
    let Some((profile_id, config)) = crate::config::get_native_runtime_config(profile_id) else {
        println!("No active native runtime profile found.");
        return Ok(());
    };

    if !config.enabled && !crate::native_runtime_endpoint::uses_remote_proxy(&config) {
        println!("Native runtime profile {profile_id} is connection-only; nothing to stop.");
        return Ok(());
    }

    println!("Stopping native runtime profile: {}", profile_id);
    stop_configured_native_runtime(&profile_id, &config).await
}

async fn stop_configured_native_runtime(
    profile_id: &str,
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<()> {
    if crate::native_runtime_endpoint::uses_remote_proxy(config) {
        crate::native_runtime_proxy::stop_remote_native_runtime(profile_id, config).await?;
        Ok(())
    } else {
        stop_native_runtime_server_for_config(profile_id, config).await
    }
}

fn flatten_prompt_blocks_to_text(blocks: Vec<ContentBlock>) -> String {
    blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) if !text.text.trim().is_empty() => Some(text.text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn extract_user_text_from_inputs(items: &[UserInput]) -> String {
    items
        .iter()
        .filter_map(|item| match item {
            UserInput::Text { text, .. } if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_task_scope(task_scope: Option<&str>) -> Option<String> {
    let trimmed = task_scope.unwrap_or("").trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("default")
        || trimmed.eq_ignore_ascii_case("all")
        || trimmed == "*"
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn compact_runtime_text(text: &str, max_chars: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

fn normalize_loop_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn basename_for_runtime(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn extract_recommended_summarize_paths(analysis: &serde_json::Value) -> HashSet<String> {
    analysis
        .pointer("/recommended_actions/summarize_paths")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(normalize_loop_path)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default()
}

pub async fn prepare_session_turn_inputs(
    global_session_id: &str,
    current_agent_id: &str,
    items: Vec<UserInput>,
) -> anyhow::Result<Vec<UserInput>> {
    let Some(runtime) = native_runtime_context() else {
        return Ok(items);
    };

    let context_deps = crate::session_context_service::SessionPromptContextDeps {
        brain: runtime.brain.clone(),
        settings_store: runtime.settings_store.clone(),
        ilhae_dir: runtime.ilhae_dir.clone(),
        reverse_session_map: None,
        active_session_id: None,
    };
    let recall_deps = crate::session_recall_service::SessionRecallDeps {
        brain: runtime.brain.clone(),
    };
    let user_text = extract_user_text_from_inputs(&items);

    let prepared_context = crate::session_context_service::prepare_session_prompt_context(
        &context_deps,
        global_session_id,
        false,
    )
    .await?;
    let recall_blocks = crate::session_recall_service::prepare_prompt_recall_blocks(
        &recall_deps,
        global_session_id,
        false,
        current_agent_id,
        &user_text,
    )
    .await;

    let mut prelude = flatten_prompt_blocks_to_text(prepared_context.prompt_blocks);
    let recall_text = flatten_prompt_blocks_to_text(recall_blocks);
    if !recall_text.is_empty() {
        if !prelude.is_empty() {
            prelude.push_str("\n\n");
        }
        prelude.push_str(&recall_text);
    }

    if prelude.trim().is_empty() {
        return Ok(items);
    }

    let mut combined = Vec::with_capacity(items.len() + 1);
    combined.push(UserInput::Text {
        text: prelude,
        text_elements: Vec::new(),
    });
    combined.extend(items);
    Ok(combined)
}

pub async fn prepare_native_turn_inputs(
    local_thread_id: &str,
    items: Vec<UserInput>,
) -> anyhow::Result<Vec<UserInput>> {
    let Some(runtime) = native_runtime_context() else {
        return Ok(items);
    };
    let global_session_id = runtime
        .brain
        .session_find_by_engine_ref(ILHAE_AGENT_ID, local_thread_id)?
        .unwrap_or_else(|| local_thread_id.to_string());
    let _ = runtime.brain.session_upsert_engine_ref(
        &global_session_id,
        ILHAE_AGENT_ID,
        local_thread_id,
    );
    let current_agent_id = infer_agent_id_from_command(&runtime.settings_store.get().agent.command);
    prepare_session_turn_inputs(&global_session_id, &current_agent_id, items).await
}

pub async fn bootstrap_ilhae_runtime() -> anyhow::Result<BootstrappedIlhaeRuntime> {
    let ilhae_dir = resolve_ilhae_data_dir();
    std::fs::create_dir_all(&ilhae_dir).ok();
    crate::superpowers_skills::provision_superpowers_skills();
    tokio::task::spawn_blocking(|| {
        tracing::info!("Running brain init (syncing tools/skills)...");
        let _ = brain_rs::sync::run_sync();
    });
    crate::mock_provider::init_mock_mode(false);
    let settings_store = Arc::new(SettingsStore::new(&ilhae_dir));
    if let Err(err) = crate::config::apply_active_ilhae_profile_projection(&settings_store) {
        warn!(
            "[Startup] Failed to apply active profile projection: {}",
            err
        );
    }
    let mock_enabled = match std::env::var("ILHAE_MOCK") {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => settings_store.get().agent.mock_mode,
    };
    crate::mock_provider::init_mock_mode(mock_enabled);

    let brain_dir = crate::config::get_active_vault_dir();
    let brain_writer = brain_session_rs::brain_session_writer::BrainSessionWriter::new(&brain_dir);
    let brain_service =
        brain_rs::BrainService::new_with_brain_writer(&ilhae_dir, None, brain_writer)
            .expect("Failed to initialize BrainService");
    let brain_service = Arc::new(brain_service);
    let cx_cache = CxCache::new();

    let runtime = BootstrappedIlhaeRuntime {
        ilhae_dir: ilhae_dir.clone(),
        settings_store: settings_store.clone(),
        brain: brain_service,
        cx_cache,
    };
    let _ = NATIVE_RUNTIME_CONTEXT.set(runtime.clone());
    spawn_native_runtime_background_workers(&runtime);

    Ok(runtime)
}

fn foreground_loop_item(
    id: String,
    title: &str,
    summary: String,
    status: crate::types::LoopLifecycleStatus,
    reason: &str,
    duration_ms: Option<i64>,
) -> crate::types::LoopLifecycleItem {
    crate::types::LoopLifecycleItem {
        id,
        kind: crate::types::LoopLifecycleKind::SuperLoop,
        title: title.to_string(),
        summary,
        detail: Some("foreground loop cycle".to_string()),
        status,
        reason: Some(reason.to_string()),
        counts: None,
        error: None,
        duration_ms,
        target_profile: None,
    }
}

fn foreground_loop_item_id(name: &str) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{name}:foreground:{millis}")
}

async fn run_foreground_loop_cycle_with_runtime(
    settings_store: Arc<SettingsStore>,
    brain: Arc<brain_rs::BrainService>,
    ilhae_dir: std::path::PathBuf,
    force_super_loop: bool,
) -> anyhow::Result<bool> {
    let settings = settings_store.get();
    if !force_super_loop
        && crate::session_context_service::build_runtime_loop_developer_instructions(&settings)
            .is_none()
    {
        return Ok(false);
    }
    let knowledge_worker =
        crate::config::knowledge_mode_includes_worker(&settings.agent.knowledge_mode);
    let knowledge_kairos =
        crate::config::knowledge_mode_includes_kairos(&settings.agent.knowledge_mode);
    if knowledge_worker || knowledge_kairos {
        let item_id = foreground_loop_item_id("knowledge_loop");
        let started = std::time::Instant::now();
        emit_native_loop_lifecycle(crate::IlhaeLoopLifecycleNotification::Started {
            session_id: "native-runtime".to_string(),
            item: foreground_loop_item(
                item_id.clone(),
                "Running Knowledge Loop",
                "Knowledge loop foreground cycle started".to_string(),
                crate::types::LoopLifecycleStatus::InProgress,
                "knowledge_foreground_started",
                None,
            ),
        });
        if knowledge_worker {
            knowledge_loop::maybe_run_cycle(
                knowledge_loop::KnowledgeLoopDriver::Worker,
                settings_store.clone(),
                ilhae_dir.clone(),
            )
            .await;
        }
        if knowledge_kairos {
            knowledge_loop::maybe_run_cycle(
                knowledge_loop::KnowledgeLoopDriver::Kairos,
                settings_store.clone(),
                ilhae_dir.clone(),
            )
            .await;
        }
        emit_native_loop_lifecycle(crate::IlhaeLoopLifecycleNotification::Completed {
            session_id: "native-runtime".to_string(),
            item: foreground_loop_item(
                item_id,
                "Running Knowledge Loop",
                "Knowledge loop foreground cycle completed".to_string(),
                crate::types::LoopLifecycleStatus::Completed,
                "knowledge_foreground_completed",
                Some(started.elapsed().as_millis() as i64),
            ),
        });
    }

    let autonomous_sessions: Arc<
        Cache<String, context_proxy::autonomy::state::AutonomousSessionState>,
    > = Arc::new(
        moka::sync::Cache::builder()
            .time_to_idle(std::time::Duration::from_secs(3600))
            .build(),
    );
    if force_super_loop {
        crate::super_loop::run_goal_cycle(
            crate::super_loop::SuperLoopDriver::Worker,
            brain,
            settings_store,
            autonomous_sessions,
            ilhae_dir,
        )
        .await;
    } else {
        crate::super_loop::maybe_run_cycle(
            crate::super_loop::SuperLoopDriver::Worker,
            brain,
            settings_store,
            autonomous_sessions,
            ilhae_dir,
        )
        .await;
    }

    Ok(true)
}

pub async fn run_exec_foreground_loop_cycle(
    settings: crate::settings_types::Settings,
) -> anyhow::Result<()> {
    let ilhae_dir = resolve_ilhae_data_dir();
    std::fs::create_dir_all(&ilhae_dir).ok();
    let settings_store = Arc::new(SettingsStore::new_with_snapshot(
        &ilhae_dir,
        settings.clone(),
    ));
    let brain_dir = crate::config::get_active_vault_dir();
    let brain_writer = brain_session_rs::brain_session_writer::BrainSessionWriter::new(&brain_dir);
    let brain = Arc::new(
        brain_rs::BrainService::new_with_brain_writer(&ilhae_dir, None, brain_writer).map_err(
            |err| anyhow::anyhow!("failed to initialize brain for foreground loop: {err}"),
        )?,
    );

    run_foreground_loop_cycle_with_runtime(
        settings_store,
        brain,
        ilhae_dir,
        /*force_super_loop*/ false,
    )
    .await
    .map(|_| ())
}

pub async fn run_active_foreground_loop_cycle() -> anyhow::Result<bool> {
    let Some(runtime) = native_runtime_context() else {
        return Ok(false);
    };
    run_foreground_loop_cycle_with_runtime(
        runtime.settings_store,
        runtime.brain,
        runtime.ilhae_dir,
        /*force_super_loop*/ false,
    )
    .await
}

pub async fn run_active_goal_foreground_loop_cycle() -> anyhow::Result<bool> {
    let Some(runtime) = native_runtime_context() else {
        return Ok(false);
    };
    run_foreground_loop_cycle_with_runtime(
        runtime.settings_store,
        runtime.brain,
        runtime.ilhae_dir,
        /*force_super_loop*/ true,
    )
    .await
}

pub async fn run_active_foreground_loop_cycle_collecting_lifecycle()
-> anyhow::Result<Vec<crate::IlhaeLoopLifecycleNotification>> {
    let mut lifecycle_rx = subscribe_native_loop_lifecycle();
    run_active_foreground_loop_cycle().await?;
    let mut notifications = Vec::new();
    while let Ok(notification) = lifecycle_rx.try_recv() {
        notifications.push(notification);
    }
    Ok(notifications)
}

pub async fn run_active_goal_foreground_loop_cycle_collecting_lifecycle()
-> anyhow::Result<Vec<crate::IlhaeLoopLifecycleNotification>> {
    let mut lifecycle_rx = subscribe_native_loop_lifecycle();
    run_active_goal_foreground_loop_cycle().await?;
    let mut notifications = Vec::new();
    while let Ok(notification) = lifecycle_rx.try_recv() {
        notifications.push(notification);
    }
    Ok(notifications)
}

pub async fn run_ilhae_proxy() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let bootstrapped = bootstrap_ilhae_runtime().await?;
    let ilhae_dir = bootstrapped.ilhae_dir.clone();
    let settings_store = bootstrapped.settings_store.clone();
    let brain_service = bootstrapped.brain.clone();
    let runtime_cx_cache = bootstrapped.cx_cache.clone();
    let mock_enabled = crate::mock_provider::is_mock_mode();

    let daemon_mode = std::env::args().any(|arg| arg == "--daemon")
        || matches!(
            std::env::var("ILHAE_PROXY_DAEMON")
                .ok()
                .as_deref()
                .map(|v| v.trim().to_ascii_lowercase()),
            Some(v) if matches!(v.as_str(), "1" | "true" | "yes" | "on")
        );

    info!("ilhae-proxy starting. Workspace: {:?}", ilhae_dir);

    // ── PID-based Process Lifecycle ────────────────────────────────────────
    // Kill previous proxy session using PID files (deterministic, no pkill/pgrep)
    let (pk, ck) =
        crate::process_lifecycle::kill_previous_session_for_mode(&ilhae_dir, daemon_mode);
    if pk > 0 || ck > 0 {
        info!(
            "[PID] Cleaned previous session: proxy={}, children={}",
            pk, ck
        );
    }
    // Aggressively find and kill any other ilhae-proxy processes
    let z_killed = enforce_singleton_proxy(!daemon_mode);
    if z_killed > 0 {
        info!(
            "[ZombieSweep] Killed {} zombie ilhae-proxy processes",
            z_killed
        );
    }

    // Port-based zombie resolution: kill orphans holding known ports.
    // MUST run synchronously before relay/screencast server start to prevent
    // "Address already in use" on ports 18790/18791/41241/41242.
    process_supervisor::startup_cleanup(daemon_mode);
    // Write our PID for the next session to clean up
    crate::process_lifecycle::write_proxy_pid_for_mode(&ilhae_dir, daemon_mode);
    let supervisor_handle = process_supervisor::create_supervisor(settings_store.clone());
    {
        let settings = settings_store.get();
        let team_backend = crate::config::normalize_team_backend(&settings.agent.team_backend);
        let use_remote_team = settings.agent.team_mode
            && crate::config::team_backend_uses_remote_transport(&team_backend);
        info!(
            "[Startup] team_mode={}, team_backend={}, a2a_endpoint={}",
            settings.agent.team_mode, team_backend, settings.agent.a2a_endpoint
        );
        if !use_remote_team {
            if mock_enabled {
                info!("[Startup] Solo/local-team mode + mock mode → skipping A2A server spawning");
            } else {
                info!("[Startup] Entering solo/local-team branch");
                // Solo mode: initial spawn of both A2A servers (supervisor will keep them alive)
                let gemini_port = {
                    let ep = settings.agent.a2a_endpoint.trim();
                    if ep.is_empty() {
                        crate::port_config::gemini_a2a_port()
                    } else {
                        parse_host_port(ep).1
                    }
                };
                let codex_port = crate::port_config::codex_a2a_port();

                let sv_gemini = supervisor_handle.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        crate::process_supervisor::ensure_agent_healthy(&sv_gemini, gemini_port)
                            .await
                    {
                        warn!("[Supervisor] Failed initial Gemini spawn: {}", e);
                    }
                });

                let sv_codex = supervisor_handle.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        crate::process_supervisor::ensure_agent_healthy(&sv_codex, codex_port).await
                    {
                        warn!("[Supervisor] Failed initial Codex spawn: {}", e);
                    }
                });
            }
        } else if mock_enabled {
            info!(
                "[Startup] Remote/hybrid team mode + mock mode → skipping real team A2A pre-spawn"
            );
        } else {
            // Team mode: pre-spawn + register with supervisor
            use context_proxy::ensure_user_agent_server;
            use context_proxy::extract_port_from_endpoint;
            use context_proxy::generate_peer_registration_files;
            use context_proxy::load_team_runtime_config;
            use context_proxy::spawn_team_a2a_servers;
            use context_proxy::trigger_agent_reload;
            use context_proxy::wait_for_all_team_health;
            let dir = ilhae_dir.clone();

            // Auto-generate team.json from default preset if missing
            let team_path = dir.join("team.json");
            if !team_path.exists() {
                info!("[TeamPreSpawn] team.json missing, auto-generating from default preset");
                let default_cfg = crate::admin_proxy::default_team_config();
                if let Ok(content) = serde_json::to_string_pretty(&default_cfg) {
                    if let Err(e) = std::fs::write(&team_path, &content) {
                        warn!("[TeamPreSpawn] Failed to write default team.json: {}", e);
                    } else {
                        info!(
                            "[TeamPreSpawn] Created default team.json at {:?}",
                            team_path
                        );
                    }
                }
            }

            if let Some(team) = load_team_runtime_config(&dir) {
                // Register team agents with supervisor for health monitoring
                let team_entries: Vec<(String, u16, String)> = team
                    .agents
                    .iter()
                    .filter_map(|a| {
                        extract_port_from_endpoint(&a.endpoint)
                            .map(|port| (a.role.clone(), port, a.engine.clone()))
                    })
                    .collect();
                let sv = supervisor_handle.clone();

                let agent_count = team.agents.len();
                // Signal: leader is ready for build_agent_transport to connect
                LEADER_READY.notify_waiters();
                tokio::spawn(async move {
                    // ── A2A Persistence Proxy: start reverse proxy for inter-agent recording ──
                    let routing_table = a2a_persistence::build_routing_table(&team);
                    let proxy_base_url = if !routing_table.is_empty() {
                        // Note: start_a2a_persistence needs SharedState, but it's not
                        // available yet. We'll use a simpler approach: just build the URL
                        // pattern and start the proxy after SharedState (via static handoff).
                        // For now, pre-bind the port and store routing table for Phase 2.
                        if let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await {
                            let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                            let url = format!("http://127.0.0.1:{}", port);
                            info!("[TeamPreSpawn] A2A proxy pre-bound at {}", url);
                            // Store listener and routing table for Phase 2
                            crate::startup_phases::set_a2a_proxy_prebound((
                                listener,
                                routing_table,
                            ));
                            Some(url)
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // Generate peer files with proxy URL so agents route through the proxy
                    let workspace_map =
                        generate_peer_registration_files(&team, proxy_base_url.as_deref());

                    process_supervisor::register_team_processes(&sv, &team_entries, &workspace_map)
                        .await;

                    // Refresh agent cards from brain-rs registry before spawning
                    tokio::task::spawn_blocking(|| {
                        let (refreshed, errors) = brain_rs::sync::startup_refresh_agents();
                        if refreshed > 0 {
                            info!(
                                "[TeamPreSpawn] Refreshed {} agent cards from running agents",
                                refreshed
                            );
                        }
                        for err in &errors {
                            warn!("[TeamPreSpawn] Agent refresh error: {}", err);
                        }
                    })
                    .await
                    .ok();

                    info!("[TeamPreSpawn] Pre-spawning {} team agents...", agent_count);
                    let children =
                        spawn_team_a2a_servers(&team, &workspace_map, None, "pre-spawn").await;

                    // Spawn user_agent AFTER team agents (it has a 30s health wait
                    // that would delay team spawn and cause leader timeout)
                    tokio::spawn({
                        let dir = dir.clone();
                        let proxy_base_url = proxy_base_url.clone();
                        async move {
                            let _ = ensure_user_agent_server(
                                &dir,
                                proxy_base_url.as_deref(),
                                "auto-user-agent",
                            )
                            .await;
                        }
                    });

                    // Register spawned PIDs with supervisor for accurate lifecycle management
                    for (child, agent_cfg) in children.iter().zip(team.agents.iter()) {
                        if let Some(pid) = child.id()
                            && let Some(port) = extract_port_from_endpoint(&agent_cfg.endpoint)
                        {
                            process_supervisor::record_pid(&sv, port, pid).await;
                        }
                    }

                    // Signal build_agent_transport that the leader is spawned
                    // (it may not be healthy yet, but wait_for_server handles that)
                    LEADER_READY.notify_waiters();

                    match wait_for_all_team_health(&team).await {
                        Ok(()) => {
                            info!("[TeamPreSpawn] All {} team agents ready", agent_count);

                            // Root cause fix: mark all team agents as healthy in supervisor
                            // so that TeamMonitor can immediately start observers.
                            // Without this, last_healthy stays None until the supervisor's
                            // background health loop runs (10s+ delay), causing observers
                            // to never start due to a race condition.
                            {
                                let mut sv_guard = sv.write().await;
                                for (role, _port, _engine) in &team_entries {
                                    let key = format!("team-{}", role.to_lowercase());
                                    if let Some(proc) = sv_guard.processes.get_mut(&key) {
                                        proc.last_healthy = Some(std::time::Instant::now());
                                        info!(
                                            "[TeamPreSpawn] Marked {} as healthy in supervisor",
                                            key
                                        );
                                    }
                                }
                            }

                            use context_proxy::add_brain_directories;
                            // Critical: trigger AgentRegistry reload so each server
                            // discovers its peers via .gemini/agents/{peer}.md files.
                            // Without this, the LLM never gets peer tools and can't delegate.
                            info!("[TeamPreSpawn] Triggering agent reload for peer discovery...");
                            trigger_agent_reload(&team).await;
                            info!("[TeamPreSpawn] Agent reload complete — peers discoverable");

                            // Dynamically add brain/ directory to each agent's workspace
                            // context via JSON-RPC, enabling skill discovery and file access.
                            let ilhae_root = dir.clone();
                            info!("[TeamPreSpawn] Adding brain directory to all agents...");
                            add_brain_directories(&team, &ilhae_root).await;
                            info!("[TeamPreSpawn] Brain directory registration complete");
                        }
                        Err(e) => warn!("[TeamPreSpawn] Some agents failed health check: {}", e),
                    }
                });
            } else {
                info!("[TeamPreSpawn] team_mode enabled but no valid team.json found");
            }
        }
    }
    // Start the supervisor health-check loop
    process_supervisor::spawn_supervisor_loop(supervisor_handle.clone(), None);

    // ── Periodic Agent Health Monitor (30s interval) ───────────────────
    // Monitors all registered brain-rs agents, fires webhooks on status changes,
    // persists snapshots for historical metrics, and checks for new agent files.
    tokio::spawn(async move {
        // Wait 15s before first check (let agents start up)
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        loop {
            tokio::task::spawn_blocking(|| {
                // Check if agents/ directory changed (new .md files added externally)
                if brain_rs::sync::agents_dir_changed() {
                    info!("[AgentMonitor] agents/ directory changed — re-syncing cards");
                    let report = brain_rs::sync::run_sync_agent_cards();
                    info!(
                        "[AgentMonitor] Synced: found={}, synced={}",
                        report.agent_cards_found, report.agent_cards_synced
                    );
                }
                // Health check + webhook for status changes
                let snapshot = brain_rs::sync::monitor_agents_with_webhook();
                let total = snapshot.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
                let online = snapshot.get("online").and_then(|v| v.as_u64()).unwrap_or(0);
                if total > 0 {
                    tracing::debug!("[AgentMonitor] {}/{} agents online", online, total);
                }
            })
            .await
            .ok();
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });

    // ── Brain Service (unified store initialization) ─────────────────────
    // Clone stores from BrainService for SharedState backward compatibility
    let store = brain_service.sessions().clone();

    // One-time migration: export existing SQLite sessions to markdown.
    // Run in background — migration is non-critical and can be slow on large DBs.
    {
        let store_bg = store.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(bw) = store_bg.brain_writer() {
                match bw.migrate_from_db(&store_bg) {
                    Ok(n) if n > 0 => info!(
                        "[BrainSessionWriter] Migrated {} existing sessions to markdown",
                        n
                    ),
                    Ok(_) => {}
                    Err(e) => warn!("[BrainSessionWriter] Migration failed (non-fatal): {}", e),
                }
            }
        });
    }
    let schedule_store = brain_service.schedules().clone();

    // ── Migrate legacy data ──────────────────────────────────────────────
    schedule_store.import_missions(&ilhae_dir);
    schedule_store.import_cron(&ilhae_dir);

    // MCP Manager
    let mcp_mgr = Arc::new(McpManager::new());
    {
        let active: Vec<_> = store
            .list_presets()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false))
            .collect();
        mcp_mgr.sync_with_presets(active).await;
    }

    // Browser manager (lazy-launch: browser starts on first tool call, not at startup)
    fn build_cache<K, V>() -> Arc<moka::sync::Cache<K, V>>
    where
        K: std::hash::Hash + Eq + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
    {
        Arc::new(
            moka::sync::Cache::builder()
                .time_to_idle(std::time::Duration::from_secs(3600))
                .build(),
        )
    }

    let browser_mgr = Arc::new(BrowserManager::new(&ilhae_dir));
    let assistant_buffers: Arc<Cache<String, crate::AssistantBuffer>> = build_cache();
    let instructions_version = Arc::new(AtomicU64::new(1));
    let cancel_version = Arc::new(AtomicU64::new(0));
    let session_instructions_ver: Arc<Cache<String, u64>> = build_cache();
    let session_cancel_ver: Arc<Cache<String, u64>> = build_cache();
    let pending_history: Arc<Cache<String, String>> = build_cache();
    let active_session_id: Arc<RwLock<String>> = Arc::new(RwLock::new(String::new()));
    let autonomous_sessions: Arc<
        Cache<String, context_proxy::autonomy::state::AutonomousSessionState>,
    > = build_cache();
    let channel_memory: Arc<RwLock<HashMap<String, HashMap<String, serde_json::Value>>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let session_turn_seq: Arc<Cache<String, u64>> = build_cache();
    let session_id_map: Arc<Cache<String, String>> = build_cache();
    let reverse_session_map: Arc<Cache<String, String>> = build_cache();
    let relay_conductor_cx = runtime_cx_cache;
    let approval_manager = approval_manager::ApprovalManager::new();
    let terminal_manager = Arc::new(context_proxy::terminal_handlers::TerminalManager::new());
    let cached_config_options: Arc<RwLock<Vec<serde_json::Value>>> =
        Arc::new(RwLock::new(Vec::new()));
    let shared_task_pool: Arc<RwLock<Vec<crate::types::SharedTaskDto>>> =
        Arc::new(RwLock::new(Vec::new()));

    // ── Notification store ───────────────────────────────────────────────
    let notification_db_path = ilhae_dir.join("notifications.db");
    let notification_store = Arc::new(
        notification_store::NotificationStore::open(&notification_db_path)
            .expect("Failed to open notifications.db"),
    );
    info!("Notification store ready at {:?}", notification_db_path);

    // ── Relay server (mobile monitoring) ─────────────────────────────────
    let (command_tx, command_rx) = tokio::sync::mpsc::channel(64);
    let (relay_state, relay_tx) =
        RelayState::new(store.clone(), schedule_store.clone(), command_tx);
    let relay_port: u16 = std::env::var("RELAY_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(crate::port_config::sacp_port());
    tokio::spawn(start_relay_server(relay_state.clone(), relay_port));

    // ── Task change → Relay bridge ───────────────────────────────────────
    // Subscribe to ScheduleStore broadcast events and forward to relay (mobile/web)
    {
        let mut task_rx = schedule_store.subscribe();
        let relay_tx_schedules = relay_tx.clone();
        tokio::spawn(async move {
            loop {
                match task_rx.recv().await {
                    Ok(event) => {
                        info!("[TaskBridge] Task change: {:?}", event);
                        broadcast_event(&relay_tx_schedules, RelayEvent::TasksChanged { event });
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[TaskBridge] Skipped {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    // ── Relay command handler — spawned below after shared state is built ──

    cleanup_redundant_sessions(&store);

    let memory_store = brain_service.memory().clone();

    // ── Memory reindex + embedding worker (moved from init_memory_store) ─
    {
        let ms = memory_store.clone();
        let vault_dir = crate::config::get_active_vault_dir();
        tokio::task::spawn_blocking(move || match ms.reindex_all(&vault_dir) {
            Ok(n) => info!("Memory: indexed {} new chunks from vault files", n),
            Err(e) => warn!("Memory: reindex failed: {}", e),
        });
    }
    {
        let ms = memory_store.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            loop {
                match tokio::task::spawn_blocking({
                    let ms = ms.clone();
                    move || ms.embed_pending()
                })
                .await
                {
                    Ok(Ok(count)) if count > 0 => {
                        info!("Embedding worker: vectorized {} pending chunks", count);
                    }
                    Ok(Err(e)) => warn!("Embedding worker error: {}", e),
                    Err(e) => warn!("Embedding worker join error: {}", e),
                    _ => {}
                }
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
    }

    // ── SubAgent GC Worker ───────────────────────────────────────────────
    {
        let s_store = store.clone();
        tokio::spawn(async move {
            // Run GC every 1 hour (3600 seconds)
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                interval.tick().await;
                match tokio::task::spawn_blocking({
                    let st = s_store.clone();
                    // Hard-delete subagent sessions older than 1 hour (3600 seconds)
                    move || st.cleanup_subagent_contexts(3600)
                })
                .await
                {
                    Ok(Ok(count)) if count > 0 => {
                        info!("🧹 [GC] Cleaned up {} expired subagent contexts", count);
                    }
                    Ok(Err(e)) => warn!("🧹 [GC] Failed to cleanup subagent contexts: {}", e),
                    Err(e) => warn!("🧹 [GC] Worker join error: {}", e),
                    _ => {}
                }
            }
        });
    }

    // ── Kairos proactive scheduling loop ────────────────────────────────
    {
        let brain_for_kairos = brain_service.clone();
        let settings_for_kairos = settings_store.clone();
        let autonomous_sessions_for_kairos = autonomous_sessions.clone();
        let notif_store_for_kairos = notification_store.clone();
        let relay_tx_for_kairos = relay_tx.clone();
        let ilhae_dir_for_kairos = ilhae_dir.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;

                let settings_snapshot = settings_for_kairos.get();
                let run_task_kairos = settings_snapshot.agent.kairos_enabled;
                let run_kb_kairos = crate::config::knowledge_mode_includes_kairos(
                    &settings_snapshot.agent.knowledge_mode,
                );
                let run_hygiene_kairos = crate::hygiene_loop::hygiene_mode_includes_kairos(
                    &settings_snapshot.agent.hygiene_mode,
                );
                if !run_task_kairos && !run_kb_kairos && !run_hygiene_kairos {
                    continue;
                }

                if run_kb_kairos {
                    knowledge_loop::maybe_run_cycle(
                        knowledge_loop::KnowledgeLoopDriver::Kairos,
                        settings_for_kairos.clone(),
                        ilhae_dir_for_kairos.clone(),
                    )
                    .await;
                }

                crate::super_loop::maybe_run_cycle(
                    crate::super_loop::SuperLoopDriver::Kairos,
                    brain_for_kairos.clone(),
                    settings_for_kairos.clone(),
                    autonomous_sessions_for_kairos.clone(),
                    ilhae_dir_for_kairos.clone(),
                )
                .await;

                if !run_task_kairos {
                    continue;
                }

                let task_scope =
                    normalize_task_scope(settings_snapshot.agent.task_scope.as_deref());
                let triggered =
                    brain_for_kairos.schedule_run_with_scope(task_scope.as_deref(), None);

                if triggered.is_empty() {
                    continue;
                }

                let preview = triggered
                    .iter()
                    .take(3)
                    .map(|task| task.title.clone())
                    .collect::<Vec<_>>();
                let message = if triggered.len() == 1 {
                    format!("[Kairos] Triggered scheduled task: {}", preview[0])
                } else {
                    let suffix = if triggered.len() > preview.len() {
                        format!(" 외 {}개", triggered.len() - preview.len())
                    } else {
                        String::new()
                    };
                    format!(
                        "[Kairos] Triggered {} scheduled tasks: {}{}",
                        triggered.len(),
                        preview.join(", "),
                        suffix
                    )
                };

                info!("{}", message);
                if let Err(e) = notif_store_for_kairos.add(&message, "info", "kairos") {
                    warn!("[Kairos] Failed to persist notification: {}", e);
                }
                broadcast_event(
                    &relay_tx_for_kairos,
                    RelayEvent::UiNotification {
                        message,
                        level: "info".to_string(),
                        source: Some("kairos".to_string()),
                    },
                );
            }
        });
    }

    {
        let settings_for_knowledge_worker = settings_store.clone();
        let ilhae_dir_for_knowledge_worker = ilhae_dir.clone();
        tokio::spawn(async move {
            knowledge_loop::run_worker_loop(
                settings_for_knowledge_worker,
                ilhae_dir_for_knowledge_worker,
            )
            .await;
        });
    }

    {
        let brain_for_super_loop = brain_service.clone();
        let settings_for_super_loop = settings_store.clone();
        let autonomous_sessions_for_super_loop = autonomous_sessions.clone();
        let ilhae_dir_for_super_loop = ilhae_dir.clone();
        tokio::spawn(async move {
            crate::super_loop::run_worker_loop(
                brain_for_super_loop,
                settings_for_super_loop,
                autonomous_sessions_for_super_loop,
                ilhae_dir_for_super_loop,
            )
            .await;
        });
    }

    // ── Self-improvement review loop ────────────────────────────────────
    {
        let brain_for_self_improvement = brain_service.clone();
        let settings_for_self_improvement = settings_store.clone();
        let notif_store_for_self_improvement = notification_store.clone();
        let relay_tx_for_self_improvement = relay_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            let mut last_reported_groups: usize = 0;
            let mut last_review_signature: Option<String> = None;
            let mut applied_group_signatures: HashSet<String> = HashSet::new();
            loop {
                interval.tick().await;

                let settings_snapshot = settings_for_self_improvement.get();
                if !settings_snapshot.agent.self_improvement_enabled {
                    last_reported_groups = 0;
                    last_review_signature = None;
                    applied_group_signatures.clear();
                    continue;
                }
                let self_improvement_preset = settings_snapshot
                    .agent
                    .self_improvement_preset
                    .trim()
                    .to_ascii_lowercase();
                let auto_summarize_enabled = matches!(
                    self_improvement_preset.as_str(),
                    "safe_summarize" | "safe_apply"
                );

                let Ok(preview) = brain_for_self_improvement.memory_dream_preview(5) else {
                    continue;
                };
                let groups = preview
                    .get("groups")
                    .and_then(|value| value.as_array())
                    .cloned()
                    .unwrap_or_default();
                let group_count = preview
                    .get("group_count")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0) as usize;

                if group_count == 0 {
                    last_reported_groups = 0;
                    last_review_signature = None;
                    applied_group_signatures.clear();
                    continue;
                }

                let top_paths = groups
                    .iter()
                    .take(3)
                    .filter_map(|group| group.get("path").and_then(|value| value.as_str()))
                    .map(basename_for_runtime)
                    .collect::<Vec<_>>();

                if self_improvement_preset == "gepa_sidecar" {
                    let now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let optimizer_interval_secs = gepa_optimizer_interval_secs();
                    let last_run_at = settings_snapshot
                        .agent
                        .self_improvement_runtime
                        .last_run_at
                        .unwrap_or(0);
                    if now_secs.saturating_sub(last_run_at) >= optimizer_interval_secs {
                        let subject = settings_snapshot
                            .agent
                            .active_profile
                            .clone()
                            .unwrap_or_else(|| "default".to_string());
                        let detail = if top_paths.is_empty() {
                            format!(
                                "{} dream groups pending under preset {}",
                                group_count, self_improvement_preset
                            )
                        } else {
                            format!(
                                "{} dream groups pending under preset {} ({})",
                                group_count,
                                self_improvement_preset,
                                top_paths.join(", ")
                            )
                        };
                        let request = build_gepa_optimizer_request(
                            &self_improvement_preset,
                            &subject,
                            &detail,
                            group_count,
                            top_paths.clone(),
                        );
                        let mut runtime_status =
                            settings_snapshot.agent.self_improvement_runtime.clone();
                        runtime_status.last_run_at = Some(now_secs);

                        match crate::super_loop::run_gepa_self_improvement_sidecar(&request) {
                            Ok(response) => match gate_gepa_optimizer_candidate(&response) {
                                Ok((prompt, instructions, score)) => {
                                    let optimizer = response
                                        .optimizer
                                        .clone()
                                        .unwrap_or_else(|| "gepa_sidecar".to_string());
                                    let reason = response.reason.clone();
                                    let auto_approved = gepa_auto_approve_enabled();

                                    runtime_status.last_result = if auto_approved {
                                        "approved".to_string()
                                    } else {
                                        "candidate_ready".to_string()
                                    };
                                    runtime_status.last_optimizer = Some(optimizer.clone());
                                    runtime_status.last_success_at = Some(now_secs);
                                    runtime_status.last_error = None;
                                    runtime_status.last_reason = reason.clone();
                                    runtime_status.candidate_prompt = Some(prompt.clone());
                                    runtime_status.candidate_instructions =
                                        Some(instructions.clone());
                                    runtime_status.candidate_score = Some(score);
                                    runtime_status.candidate_generated_at = Some(now_secs);
                                    if auto_approved {
                                        runtime_status.approved_prompt = Some(prompt);
                                        runtime_status.approved_instructions = Some(instructions);
                                        runtime_status.approved_score = Some(score);
                                        runtime_status.approved_at = Some(now_secs);
                                    }

                                    let persist_result = settings_for_self_improvement.set_value(
                                        "agent.self_improvement_runtime",
                                        serde_json::to_value(&runtime_status)
                                            .unwrap_or(serde_json::Value::Null),
                                    );
                                    if let Err(error) = persist_result {
                                        warn!(
                                            "[Self-Improvement] Failed to persist offline optimizer runtime status: {}",
                                            error
                                        );
                                    } else {
                                        let action_label = if auto_approved {
                                            "approved"
                                        } else {
                                            "prepared candidate"
                                        };
                                        let suffix = if top_paths.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" ({})", top_paths.join(", "))
                                        };
                                        let message = format!(
                                            "[Self-Improvement] Offline optimizer {} for {} dream groups{} [score={:.2}, optimizer={}]",
                                            action_label, group_count, suffix, score, optimizer
                                        );
                                        info!("{}", message);
                                        if let Err(error) = notif_store_for_self_improvement.add(
                                            &message,
                                            "info",
                                            "self-improvement",
                                        ) {
                                            warn!(
                                                "[Self-Improvement] Failed to persist optimizer notification: {}",
                                                error
                                            );
                                        }
                                        broadcast_event(
                                            &relay_tx_for_self_improvement,
                                            RelayEvent::UiNotification {
                                                message,
                                                level: "info".to_string(),
                                                source: Some("self-improvement".to_string()),
                                            },
                                        );
                                    }
                                }
                                Err(error) => {
                                    runtime_status.last_result = "candidate_rejected".to_string();
                                    runtime_status.last_optimizer = response.optimizer.clone();
                                    runtime_status.last_error =
                                        Some(format!("hard gate rejected candidate: {}", error));
                                    runtime_status.last_reason = response.reason.clone();
                                    if let Err(persist_error) = settings_for_self_improvement
                                        .set_value(
                                            "agent.self_improvement_runtime",
                                            serde_json::to_value(&runtime_status)
                                                .unwrap_or(serde_json::Value::Null),
                                        )
                                    {
                                        warn!(
                                            "[Self-Improvement] Failed to persist rejected optimizer runtime status: {}",
                                            persist_error
                                        );
                                    }
                                }
                            },
                            Err(error) => {
                                let mut runtime_status =
                                    settings_snapshot.agent.self_improvement_runtime.clone();
                                runtime_status.last_run_at = Some(now_secs);
                                runtime_status.last_result = "error".to_string();
                                runtime_status.last_error = Some(error.clone());
                                if let Err(persist_error) = settings_for_self_improvement.set_value(
                                    "agent.self_improvement_runtime",
                                    serde_json::to_value(&runtime_status)
                                        .unwrap_or(serde_json::Value::Null),
                                ) {
                                    warn!(
                                        "[Self-Improvement] Failed to persist optimizer error runtime status: {}",
                                        persist_error
                                    );
                                }
                                warn!(
                                    "[Self-Improvement] Offline optimizer failed for preset gepa_sidecar: {}",
                                    error
                                );
                            }
                        }
                    }
                }

                let mut candidate_dirs = Vec::new();
                for group in &groups {
                    let Some(path) = group.get("path").and_then(|value| value.as_str()) else {
                        continue;
                    };
                    let Some(parent) = std::path::Path::new(path).parent() else {
                        continue;
                    };
                    let normalized_parent = normalize_loop_path(parent.to_string_lossy().as_ref());
                    if !candidate_dirs
                        .iter()
                        .any(|existing| existing == &normalized_parent)
                    {
                        candidate_dirs.push(normalized_parent);
                    }
                    if candidate_dirs.len() >= 3 {
                        break;
                    }
                }

                let mut summarize_paths: HashSet<String> = HashSet::new();
                for dir in &candidate_dirs {
                    let Ok(analysis) = brain_for_self_improvement
                        .memory_dream_analyze(std::path::Path::new(dir), 6)
                    else {
                        continue;
                    };
                    summarize_paths.extend(extract_recommended_summarize_paths(&analysis));
                }

                let mut auto_summarized = Vec::new();
                if auto_summarize_enabled {
                    for group in &groups {
                        let Some(path) = group.get("path").and_then(|value| value.as_str()) else {
                            continue;
                        };
                        let normalized_path = normalize_loop_path(path);
                        if !summarize_paths.contains(&normalized_path) {
                            continue;
                        }

                        let ids = group
                            .get("chunk_ids")
                            .and_then(|value| value.as_array())
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(|item| item.as_i64())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let chunk_count = group
                            .get("chunk_count")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(ids.len() as u64)
                            as usize;

                        if ids.is_empty() || chunk_count < 2 {
                            continue;
                        }

                        let signature = format!(
                            "{}#{}",
                            normalized_path,
                            ids.iter()
                                .map(|id| id.to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        );
                        if applied_group_signatures.contains(&signature) {
                            continue;
                        }

                        match brain_for_self_improvement.memory_dream_summarize(&ids) {
                            Ok(_) => {
                                applied_group_signatures.insert(signature);
                                auto_summarized.push((normalized_path, chunk_count));
                            }
                            Err(error) => {
                                warn!(
                                    "[Self-Improvement] Failed to auto-summarize dream group {}: {}",
                                    path, error
                                );
                            }
                        }
                    }
                }

                if !auto_summarized.is_empty() {
                    let follow_up = brain_for_self_improvement.memory_dream_preview(5).ok();
                    let remaining_groups = follow_up
                        .as_ref()
                        .and_then(|value| value.get("group_count"))
                        .and_then(|value| value.as_u64())
                        .unwrap_or(group_count as u64)
                        as usize;
                    last_reported_groups = remaining_groups;
                    last_review_signature = None;

                    let preview_paths = auto_summarized
                        .iter()
                        .take(3)
                        .map(|(path, _)| basename_for_runtime(path))
                        .collect::<Vec<_>>();
                    let suffix = if auto_summarized.len() > preview_paths.len() {
                        format!(" 외 {}개", auto_summarized.len() - preview_paths.len())
                    } else {
                        String::new()
                    };
                    let message = format!(
                        "[Self-Improvement] Auto-summarized {} dream groups (remaining: {}): {}{}",
                        auto_summarized.len(),
                        remaining_groups,
                        preview_paths.join(", "),
                        suffix
                    );
                    info!("{}", message);
                    if let Err(e) =
                        notif_store_for_self_improvement.add(&message, "info", "self-improvement")
                    {
                        warn!(
                            "[Self-Improvement] Failed to persist auto-apply notification: {}",
                            e
                        );
                    }
                    broadcast_event(
                        &relay_tx_for_self_improvement,
                        RelayEvent::UiNotification {
                            message,
                            level: "info".to_string(),
                            source: Some("self-improvement".to_string()),
                        },
                    );
                    continue;
                }

                let review_signature = format!("{}:{}", group_count, top_paths.join("|"));
                if group_count == last_reported_groups
                    && last_review_signature.as_deref() == Some(review_signature.as_str())
                {
                    continue;
                }
                last_reported_groups = group_count;
                last_review_signature = Some(review_signature);

                let suffix = if top_paths.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", top_paths.join(", "))
                };
                let message = format!(
                    "[Self-Improvement] {} dream groups need review after auto-analysis{}",
                    group_count, suffix
                );

                info!("{}", message);
                if let Err(e) =
                    notif_store_for_self_improvement.add(&message, "info", "self-improvement")
                {
                    warn!("[Self-Improvement] Failed to persist notification: {}", e);
                }
                broadcast_event(
                    &relay_tx_for_self_improvement,
                    RelayEvent::UiNotification {
                        message,
                        level: "info".to_string(),
                        source: Some("self-improvement".to_string()),
                    },
                );
            }
        });
    }

    // ── Build shared state BEFORE agent transport ─────────────────────
    // SharedState does not depend on the AI agent connection, only on
    // infrastructure (relay, stores, supervisor, etc.). Building it early
    // lets health server + relay command handlers start immediately,
    // even while build_agent_transport blocks waiting for AI server.
    let agent_spawner = Arc::new(crate::adapters::RealAgentSpawner);
    let session_mcp_servers = build_cache();
    let (agent_refresh_tx, agent_refresh_rx) = tokio::sync::mpsc::unbounded_channel();
    let session_state = crate::shared_state::SessionState {
        instructions_ver: session_instructions_ver,
        cancel_ver: session_cancel_ver,
        turn_seq: session_turn_seq,
        id_map: session_id_map,
        reverse_map: reverse_session_map,
        delegation_mode: build_cache(),
        mcp_servers: session_mcp_servers,
        assistant_buffers: assistant_buffers.clone(),
        instructions_version: instructions_version.clone(),
        cancel_version: cancel_version.clone(),
        pending_history: pending_history.clone(),
        connection_sessions: build_cache(),
        active_session_id: active_session_id.clone(),
        autonomous_sessions: autonomous_sessions.clone(),
    };
    let (event_tx, _) = tokio::sync::broadcast::channel(1000);
    let team_state = crate::shared_state::TeamState {
        supervisor: supervisor_handle.clone(),
        agent_pool: Arc::new(agent_pool::AgentPool::new()),
        a2a_routing_map: None,
        delegation_metrics: crate::process_supervisor::create_metrics(),
        comms: crate::shared_state::TeamCommsChannel::new(),
        channel_memory: channel_memory.clone(),
        event_tx,
        agent_spawner: agent_spawner.clone(),
    };
    let infra_context = crate::shared_state::InfraContext {
        brain: brain_service.clone(),
        settings_store: settings_store.clone(),
        browser_mgr: browser_mgr.clone(),
        mcp_mgr: mcp_mgr.clone(),
        notification_store: notification_store.clone(),
        relay_state: relay_state.clone(),
        relay_tx: relay_tx.clone(),
        relay_conductor_cx: relay_conductor_cx.clone(),
        approval_manager: approval_manager.clone(),
        ilhae_dir: ilhae_dir.clone(),
        terminal_manager: terminal_manager.clone(),
        cached_config_options: cached_config_options.clone(),
        shared_task_pool: shared_task_pool.clone(),
        agent_refresh_tx: agent_refresh_tx.clone(),
    };

    let shared = Arc::new(SharedState {
        sessions: session_state,
        team: team_state,
        infra: infra_context,
    });

    // ── Lightweight HTTP health server (available immediately) ──
    crate::startup_phases::start_health_server(shared.clone()).await?;

    // ── Background workers (relay command handler starts immediately) ──
    crate::startup_phases::spawn_background_workers(shared.clone(), command_rx).await;

    // ── Build Agent (may block waiting for AI server connection) ──────
    // This is intentionally AFTER health + relay are ready, so CLI can
    // query status even while the agent transport is still connecting.
    let (agent, a2a_server_child) = if mock_enabled {
        info!("[Startup] Mock mode → Using MockAgent");
        (DynConnectTo::new(crate::mock_agent::MockAgent::new()), None)
    } else {
        build_agent_transport(&settings_store, supervisor_handle.clone()).await?
    };
    let agent_child_slot = Arc::new(tokio::sync::Mutex::new(a2a_server_child));

    // ── Build and run conductor ──
    crate::startup_phases::run_conductor(
        shared.clone(),
        agent,
        agent_child_slot.clone(),
        agent_refresh_rx,
        ilhae_dir.clone(),
        daemon_mode,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::get;

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: tests scope process env mutations and restore values on drop.
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
                if let Some(previous) = self.previous.as_ref() {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    async fn start_readiness_server(
        models_body: Option<&str>,
    ) -> (
        crate::config::IlhaeProfileNativeRuntimeConfig,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind readiness test server");
        let address = listener.local_addr().expect("readiness test address");
        let app = Router::new().route("/health", get(|| async { StatusCode::OK }));
        let app = if let Some(models_body) = models_body {
            let models_body = models_body.to_string();
            app.route(
                "/v1/models",
                get(move || {
                    let body = models_body.clone();
                    async move { ([("content-type", "application/json")], body) }
                }),
            )
        } else {
            app
        };
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve readiness test responses");
        });
        let config = crate::config::IlhaeProfileNativeRuntimeConfig {
            enabled: true,
            health_url: format!("http://{address}/health"),
            base_url: format!("http://{address}/v1"),
            model_path: "/models/Fable-Fusion.gguf".to_string(),
            ..Default::default()
        };
        (config, server)
    }

    #[tokio::test]
    async fn exact_model_readiness_accepts_absolute_model_id() {
        let (config, server) =
            start_readiness_server(Some(r#"{"data":[{"id":"/models/Fable-Fusion.gguf"}]}"#)).await;

        assert!(native_runtime_readiness(&config).await);
        server.abort();
    }

    #[tokio::test]
    async fn exact_model_readiness_accepts_exact_basename() {
        let (config, server) =
            start_readiness_server(Some(r#"{"models":[{"name":"Fable-Fusion.gguf"}]}"#)).await;

        assert!(native_runtime_readiness(&config).await);
        server.abort();
    }

    #[tokio::test]
    async fn exact_model_readiness_accepts_configured_alias() {
        let (mut config, server) =
            start_readiness_server(Some(r#"{"data":[{"id":"fable-remote"}]}"#)).await;
        config.args = vec!["--alias=fable-remote".to_string()];

        assert!(native_runtime_readiness(&config).await);
        server.abort();
    }

    #[tokio::test]
    async fn proxy_connection_only_mode_reuses_exact_existing_runtime() {
        let (mut config, server) =
            start_readiness_server(Some(r#"{"data":[{"id":"Fable-Fusion.gguf"}]}"#)).await;
        config.enabled = false;

        ensure_native_runtime_for_proxy_with_thinking_mode("fable-test", &config, "on")
            .await
            .expect("connection-only proxy mode should reuse the exact existing runtime");

        server.abort();
    }

    #[tokio::test]
    async fn remote_model_readiness_uses_the_proxy_health_route() {
        let (mut config, server) =
            start_readiness_server(Some(r#"{"data":[{"id":"Fable-Fusion.gguf"}]}"#)).await;
        config.health_url = "http://127.0.0.1:9/health".to_string();
        config.proxy_base_url = Some(config.base_url.clone());
        config.proxy_control_url =
            Some("http://127.0.0.1:18083/_ilhae/native-runtime/ensure".to_string());

        assert!(native_runtime_readiness(&config).await);
        server.abort();
    }

    #[tokio::test]
    async fn native_runtime_readiness_sends_configured_http_headers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind authenticated readiness server");
        let address = listener.local_addr().expect("readiness server address");
        let authorized = |headers: axum::http::HeaderMap| {
            headers
                .get("x-runtime-auth")
                .is_some_and(|value| value == "ready")
        };
        let app = Router::new()
            .route(
                "/health",
                get(move |headers| async move {
                    if authorized(headers) {
                        StatusCode::OK
                    } else {
                        StatusCode::UNAUTHORIZED
                    }
                }),
            )
            .route(
                "/v1/models",
                get(move |headers| async move {
                    if authorized(headers) {
                        (
                            StatusCode::OK,
                            [("content-type", "application/json")],
                            r#"{"data":[{"id":"Fable-Fusion.gguf"}]}"#,
                        )
                    } else {
                        (
                            StatusCode::UNAUTHORIZED,
                            [("content-type", "application/json")],
                            "{}",
                        )
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve authenticated readiness responses");
        });
        let config = crate::config::IlhaeProfileNativeRuntimeConfig {
            enabled: true,
            health_url: format!("http://{address}/health"),
            base_url: format!("http://{address}/v1"),
            model_path: "/models/Fable-Fusion.gguf".to_string(),
            http_headers: Some(std::collections::BTreeMap::from([(
                "X-Runtime-Auth".to_string(),
                "ready".to_string(),
            )])),
            ..Default::default()
        };

        assert!(native_runtime_readiness(&config).await);
        server.abort();
    }

    #[tokio::test]
    async fn exact_model_readiness_rejects_wrong_or_suffix_model() {
        let (config, server) = start_readiness_server(Some(
            r#"{"data":[{"id":"/models/Fable-Fusion.gguf.backup"},{"id":"Other.gguf"}]}"#,
        ))
        .await;

        assert!(!native_runtime_readiness(&config).await);
        server.abort();
    }

    #[tokio::test]
    async fn exact_model_readiness_rejects_malformed_models_response() {
        let (config, server) = start_readiness_server(Some("{not-json")).await;

        assert!(!native_runtime_readiness(&config).await);
        server.abort();
    }

    #[tokio::test]
    async fn exact_model_readiness_rejects_health_only_listener() {
        let (config, server) = start_readiness_server(None).await;

        assert!(native_runtime_healthcheck(&config.health_url).await);
        assert!(!native_runtime_readiness(&config).await);
        server.abort();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[serial_test::serial]
    async fn stop_refuses_unowned_listener_without_closing_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind unowned listener");
        let address = listener.local_addr().expect("unowned listener address");
        let config = crate::config::IlhaeProfileNativeRuntimeConfig {
            enabled: true,
            health_url: format!("http://{address}/health"),
            base_url: format!("http://{address}/v1"),
            server_bin: "/bin/false".to_string(),
            model_path: "/models/Fable-Fusion.gguf".to_string(),
            ..Default::default()
        };

        let error = stop_native_runtime_server_for_config("fable-test", &config)
            .await
            .expect_err("unowned listener must not be signaled");

        assert!(error.to_string().contains("unattested process"));
        assert!(tokio::net::TcpStream::connect(address).await.is_ok());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn switching_away_from_connection_only_profile_leaves_listener_running() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _config_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
        let _data_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind connection-only listener");
        let address = listener
            .local_addr()
            .expect("connection-only listener address");
        let mut config = crate::config::IlhaeTomlConfig::default();
        config.profile.active = Some("connection-only".to_string());
        config.profiles.insert(
            "connection-only".to_string(),
            crate::config::IlhaeProfileConfig {
                native_runtime: crate::config::IlhaeProfileNativeRuntimeConfig {
                    enabled: false,
                    health_url: format!("http://{address}/health"),
                    base_url: format!("http://{address}/v1"),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        crate::config::save_ilhae_toml_config(&config).expect("save Ilhae config");

        switch_native_runtime_for_cli(Some("connection-only"), /*next_profile_id*/ None)
            .await
            .expect("connection-only profile should not be stopped");

        assert!(tokio::net::TcpStream::connect(address).await.is_ok());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[serial_test::serial]
    async fn stop_never_kills_unowned_same_name_and_model_process() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path());
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn unowned lookalike");
        let pid = child.id();
        let config = crate::config::IlhaeProfileNativeRuntimeConfig {
            enabled: true,
            server_bin: "/bin/sleep".to_string(),
            model_path: "30".to_string(),
            ..Default::default()
        };

        let was_detected_as_lookalike = find_native_runtime_pids(&config).contains(&pid);
        let stop_result = stop_native_runtime_server_for_config("fable-test", &config).await;
        let remained_alive = child.try_wait().expect("query lookalike process").is_none();
        let _ = child.kill();
        let _ = child.wait();

        assert!(was_detected_as_lookalike);
        assert!(stop_result.is_ok());
        assert!(remained_alive, "unowned lookalike received a signal");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[serial_test::serial]
    async fn stop_terminates_fully_attested_owned_process() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path());
        let owner_token = uuid::Uuid::new_v4().to_string();
        let server_executable = resolve_server_executable("/bin/sleep").expect("resolve sleep");
        let mut child = std::process::Command::new(&server_executable)
            .arg("30")
            .env(ILHAE_NATIVE_RUNTIME_OWNER_TOKEN_ENV, &owner_token)
            .spawn()
            .expect("spawn owned process");
        let pid = child.id();
        let start_ticks = (0..100)
            .find_map(|_| {
                let ticks = process_start_ticks(pid);
                if ticks.is_none() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                ticks
            })
            .expect("owned process start ticks");
        let record = NativeRuntimeOwnershipRecord {
            schema_version: NATIVE_RUNTIME_OWNERSHIP_SCHEMA_VERSION,
            profile_id: "fable-test".to_string(),
            pid,
            process_start_ticks: start_ticks,
            owner_token,
            server_executable: server_executable.to_string_lossy().into_owned(),
            model_path: "30".to_string(),
            runtime_config_sha256: None,
        };
        write_native_runtime_ownership_record(&record).expect("write ownership record");
        attest_native_runtime_process(&record).expect("pre-stop ownership attestation");

        // Reap concurrently so the stop loop observes /proc disappearing after
        // SIGTERM instead of waiting on a zombie owned by this test process.
        let reaper = std::thread::spawn(move || child.wait().expect("reap owned process"));
        let config = crate::config::IlhaeProfileNativeRuntimeConfig {
            enabled: true,
            server_bin: server_executable.to_string_lossy().into_owned(),
            model_path: "30".to_string(),
            ..Default::default()
        };

        stop_native_runtime_server_for_config("fable-test", &config)
            .await
            .expect("fully attested owned process should stop");
        let status = reaper.join().expect("join owned process reaper");

        assert!(
            !status.success(),
            "owned process should receive termination"
        );
        assert!(
            read_native_runtime_ownership_record()
                .expect("read ownership record after stop")
                .is_none(),
            "ownership record should be cleared after the owned process exits"
        );
    }

    #[test]
    #[serial_test::serial]
    fn native_runtime_start_lock_path_is_global_for_all_profiles() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path());
        let mut first = crate::config::IlhaeProfileNativeRuntimeConfig::default();
        first.base_url = "http://127.0.0.1:8081/v1".to_string();
        first.health_url = "http://127.0.0.1:8081/health".to_string();
        let mut second = first.clone();
        second.base_url = "http://127.0.0.1:8082/v1".to_string();
        second.health_url = "http://127.0.0.1:8082/health".to_string();

        assert_eq!(
            native_runtime_start_lock_path("qwen-local", &first),
            native_runtime_start_lock_path("nemotron-local", &second)
        );
    }

    #[test]
    fn native_runtime_argument_parsing_accepts_inline_model_and_port() {
        let config = crate::config::IlhaeProfileNativeRuntimeConfig {
            model_path: "/models/fable.gguf".to_string(),
            args: vec![
                "--model=/models/fable.gguf".to_string(),
                "--port=18081".to_string(),
            ],
            ..Default::default()
        };

        assert!(cmdline_contains_model_path(
            &config.args,
            &config.model_path
        ));
        assert_eq!(extract_port_from_config(&config), Some(18081));
    }

    #[test]
    fn effective_native_runtime_env_includes_config_env_and_thinking_mode() {
        let mut config = crate::config::IlhaeProfileNativeRuntimeConfig::default();
        config
            .env
            .insert("TURBO_AUTO_ASYMMETRIC".to_string(), "0".to_string());

        let env = effective_native_runtime_env(&config, "on");

        assert!(
            env.iter()
                .any(|(key, value)| { key == "ILHAE_NATIVE_THINKING_MODE" && value == "on" })
        );
        assert!(
            env.iter()
                .any(|(key, value)| { key == "TURBO_AUTO_ASYMMETRIC" && value == "0" })
        );
    }

    #[test]
    #[serial_test::serial]
    fn native_runtime_fingerprint_tracks_execution_spec_but_not_proxy_request_options() {
        let _thinking_mode = EnvVarGuard::set(ILHAE_NATIVE_THINKING_MODE_ENV, "on");
        let mut config = crate::config::IlhaeProfileNativeRuntimeConfig {
            provider: Some("llama-server".to_string()),
            health_url: "http://127.0.0.1:8081/health".to_string(),
            base_url: "http://127.0.0.1:8081/v1".to_string(),
            server_bin: "/opt/llama-server".to_string(),
            model_path: "/models/fable.gguf".to_string(),
            args: vec!["-c".to_string(), "131072".to_string()],
            ..Default::default()
        };
        let original = native_runtime_execution_fingerprint(&config);

        config.query_params = Some(std::collections::BTreeMap::from([(
            "draft".to_string(),
            "mtp".to_string(),
        )]));
        config.proxy_control_url = Some("http://127.0.0.1:8083/control".to_string());
        config.proxy_control_token_env = Some("ILHAE_RUNTIME_PROXY_TOKEN".to_string());
        assert_eq!(native_runtime_execution_fingerprint(&config), original);

        config.args.push("--flash-attn".to_string());
        assert_ne!(native_runtime_execution_fingerprint(&config), original);

        config.args.pop();
        config.enabled = true;
        assert_ne!(native_runtime_execution_fingerprint(&config), original);
    }

    #[test]
    fn native_runtime_fingerprint_tracks_transmitted_thinking_mode() {
        let config = crate::config::IlhaeProfileNativeRuntimeConfig {
            provider: Some("llama-server".to_string()),
            health_url: "http://127.0.0.1:8081/health".to_string(),
            base_url: "http://127.0.0.1:8081/v1".to_string(),
            server_bin: "/opt/llama-server".to_string(),
            model_path: "/models/fable.gguf".to_string(),
            args: vec![
                "-m".to_string(),
                "/models/fable.gguf".to_string(),
                "--reasoning".to_string(),
                "on".to_string(),
            ],
            ..Default::default()
        };

        assert_ne!(
            native_runtime_execution_fingerprint_with_thinking_mode(&config, "on"),
            native_runtime_execution_fingerprint_with_thinking_mode(&config, "off")
        );
        assert_eq!(
            effective_native_runtime_args(&config, "off"),
            vec![
                "-m".to_string(),
                "/models/fable.gguf".to_string(),
                "--reasoning".to_string(),
                "off".to_string(),
            ]
        );
    }
}
