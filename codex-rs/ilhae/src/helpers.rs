//! Helper functions for ilhae-proxy.
//!
//! Codex config management, relay utilities, attachment handling,
//! browser tool detection, and other shared utilities.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::types::{IlhaeEngineStateNotification, NOTIF_ENGINE_STATE};
use agent_client_protocol_schema::{
    InitializeRequest, NewSessionRequest, NewSessionResponse, ProtocolVersion, SessionNotification,
    SessionUpdate, ToolCall as AcpToolCall, ToolCallUpdate,
};
use base64::Engine;
use sacp::{Agent, Client, Conductor, ConnectionTo, UntypedMessage};
use serde_json::json;
use tokio::sync::RwLock;
use tokio::time::{Duration, Instant, sleep};
use tracing::{debug, info, warn};
use uuid::Uuid;

const CX_CACHE_MAX_ENTRIES: usize = 16;
pub const ILHAE_DIR_NAME: &str = "ilhae";
pub const ILHAE_AGENT_ID: &str = "ilhae";
pub const LEGACY_CODEX_AGENT_ID: &str = "codex";
pub const ILHAE_NATIVE_TRANSPORT_ENV: &str = "ILHAE_NATIVE_TRANSPORT";
pub const LEGACY_CODEX_TRANSPORT_ENV: &str = "ILHAE_CODEX_TRANSPORT";

// ─── Agent ID inference ──────────────────────────────────────────────────

pub fn is_ilhae_native_agent_id(agent_id: &str) -> bool {
    let lower = agent_id.trim().to_ascii_lowercase();
    lower == ILHAE_AGENT_ID || lower == LEGACY_CODEX_AGENT_ID || lower.starts_with("codex-ilhae")
}

pub fn is_ilhae_native_engine_name(engine_name: &str) -> bool {
    is_ilhae_native_agent_id(engine_name)
}

pub fn is_ilhae_native_command(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("codex-ilhae")
        || lower.contains("codex-acp")
        || lower.contains(" codex")
        || lower.starts_with("codex")
        || lower.contains(" ilhae")
        || lower.starts_with("ilhae")
}

pub fn infer_agent_id_from_command(command: &str) -> String {
    let lower = command.to_ascii_lowercase();
    if lower.contains("codex-ilhae") {
        return ILHAE_AGENT_ID.to_string();
    }
    if lower.contains(" ilhae") || lower.starts_with("ilhae") {
        return ILHAE_AGENT_ID.to_string();
    }
    if lower.contains("codex-acp") || lower.contains(" codex") || lower.starts_with("codex") {
        return LEGACY_CODEX_AGENT_ID.to_string();
    }
    if lower.contains("claude-code-acp") || lower.contains(" claude") || lower.starts_with("claude")
    {
        return "claude".to_string();
    }
    if lower.contains("gemini") {
        return "gemini".to_string();
    }
    command
        .split_whitespace()
        .next()
        .and_then(|p| Path::new(p).file_name().and_then(|n| n.to_str()))
        .unwrap_or("agent")
        .to_string()
}

pub fn resolve_engine_command(engine_id: &str, explicit_command: Option<&str>) -> Option<String> {
    if let Some(command) = explicit_command.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(command.to_string());
    }

    match engine_id.trim().to_ascii_lowercase().as_str() {
        "ilhae" => Some("ilhae".to_string()),
        "codex" => Some("codex".to_string()),
        "codex-ilhae" | "codex-ilhae-llama-nemotron" => Some("ilhae".to_string()),
        "gemini" => Some("gemini-ilhae --experimental-acp".to_string()),
        "claude" => Some("claude-code-acp serve".to_string()),
        _ => None,
    }
}

// ─── CxCache — Conductor connection cache ───────────────────────────────

/// Shared cache for multiple `ConnectionTo<Conductor>` handles.
///
/// Enables broadcasting notifications to all active desktop clients (e.g. multiple windows).
///
/// # Usage
/// - `try_add(cx)` — add a connection to the pool
/// - `notify_desktop(method, payload)` — broadcast extNotification to ALL connected Clients
#[derive(Clone)]
pub struct CxCache {
    inner: Arc<RwLock<Vec<CxCacheEntry>>>,
}

#[derive(Clone)]
struct CxCacheEntry {
    key: String,
    cx: ConnectionTo<Conductor>,
    timeline_subscriptions: HashSet<String>,
}

impl CxCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Vec::new())),
        }
    }

    fn connection_key(cx: &ConnectionTo<Conductor>) -> String {
        format!("{cx:?}")
    }

    fn normalize_timeline_subscriptions(
        session_ids: impl IntoIterator<Item = String>,
    ) -> HashSet<String> {
        session_ids
            .into_iter()
            .filter_map(|session_id| {
                let trimmed = session_id.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
            .collect()
    }

    fn trim_entries(entries: &mut Vec<CxCacheEntry>) {
        if entries.len() > CX_CACHE_MAX_ENTRIES {
            let overflow = entries.len() - CX_CACHE_MAX_ENTRIES;
            entries.drain(0..overflow);
        }
    }

    /// Add a conductor connection to the pool.
    pub async fn try_add(&self, cx: ConnectionTo<Conductor>) {
        let key = Self::connection_key(&cx);
        let mut w = self.inner.write().await;
        if let Some(entry) = w.iter_mut().find(|entry| entry.key == key) {
            entry.cx = cx;
        } else {
            w.push(CxCacheEntry {
                key,
                cx,
                timeline_subscriptions: HashSet::new(),
            });
        }
        Self::trim_entries(&mut w);
    }

    pub async fn set_timeline_subscriptions(
        &self,
        cx: &ConnectionTo<Conductor>,
        session_ids: impl IntoIterator<Item = String>,
    ) {
        let key = Self::connection_key(cx);
        let timeline_subscriptions = Self::normalize_timeline_subscriptions(session_ids);
        let mut w = self.inner.write().await;
        if let Some(entry) = w.iter_mut().find(|entry| entry.key == key) {
            entry.cx = cx.clone();
            entry.timeline_subscriptions = timeline_subscriptions;
        } else {
            w.push(CxCacheEntry {
                key,
                cx: cx.clone(),
                timeline_subscriptions,
            });
        }
        Self::trim_entries(&mut w);
    }

    pub async fn latest(&self) -> Option<ConnectionTo<Conductor>> {
        let r = self.inner.read().await;
        r.last().map(|entry| entry.cx.clone())
    }

    /// Send an extNotification to ALL desktop Clients in the pool.
    /// Automatically removes connections that are no longer active (failed send).
    pub async fn notify_desktop(&self, method: &str, payload: serde_json::Value) {
        let mut conns = self.inner.write().await;
        let mut to_remove = Vec::new();

        match UntypedMessage::new(method, payload) {
            Ok(notif) => {
                for (i, entry) in conns.iter().enumerate() {
                    if let Err(e) = entry.cx.send_notification_to(Client, notif.clone()) {
                        debug!(
                            "[CxCache] Connection {} is stale, marking for removal: {}",
                            i, e
                        );
                        to_remove.push(i);
                    }
                }
            }
            Err(e) => {
                warn!("[CxCache] Failed to build {}: {}", method, e);
                return;
            }
        }

        // Cleanup stale connections in reverse order
        for i in to_remove.into_iter().rev() {
            conns.remove(i);
        }
    }

    pub async fn notify_desktop_for_session(
        &self,
        method: &str,
        payload: serde_json::Value,
        session_id: &str,
    ) {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            self.notify_desktop(method, payload).await;
            return;
        }

        let mut conns = self.inner.write().await;
        let mut to_remove = Vec::new();

        match UntypedMessage::new(method, payload) {
            Ok(notif) => {
                for (i, entry) in conns.iter().enumerate() {
                    let is_legacy_connection = entry.timeline_subscriptions.is_empty();
                    let is_subscribed = entry.timeline_subscriptions.contains(session_id);
                    if !is_legacy_connection && !is_subscribed {
                        continue;
                    }
                    if let Err(e) = entry.cx.send_notification_to(Client, notif.clone()) {
                        debug!(
                            "[CxCache] Connection {} is stale, marking for removal: {}",
                            i, e
                        );
                        to_remove.push(i);
                    }
                }
            }
            Err(e) => {
                warn!("[CxCache] Failed to build {}: {}", method, e);
                return;
            }
        }

        for i in to_remove.into_iter().rev() {
            conns.remove(i);
        }
    }

    /// Poll until at least one connection is available or timeout expires.
    pub async fn wait_for(&self, timeout: Duration) -> Option<ConnectionTo<Conductor>> {
        if let Some(cx) = self.latest().await {
            return Some(cx);
        }
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            sleep(Duration::from_millis(RELAY_DESKTOP_READY_POLL_MS)).await;
            if let Some(cx) = self.latest().await {
                return Some(cx);
            }
        }
        None
    }
}

// ─── Relay utilities ─────────────────────────────────────────────────────

pub const RELAY_DESKTOP_READY_TIMEOUT_MS: u64 = 10_000;
pub const RELAY_DESKTOP_READY_POLL_MS: u64 = 120;

pub fn relay_wait_timeout_from_payload(payload: &serde_json::Value, default_ms: u64) -> Duration {
    let requested = payload
        .get("wait_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(default_ms);
    Duration::from_millis(requested.clamp(100, 60_000))
}

/// Legacy wrapper — delegates to CxCache.wait_for().
/// Kept for backward compatibility in main.rs relay command handler.
#[allow(dead_code)]
pub async fn wait_for_relay_conductor_connection(
    cx_cache: &CxCache,
    timeout: Duration,
) -> Option<ConnectionTo<Conductor>> {
    cx_cache.wait_for(timeout).await
}

#[allow(dead_code)]
pub async fn notify_desktop_settings_changed(
    cx_cache: &CxCache,
    key: &str,
    value: &serde_json::Value,
) {
    cx_cache
        .notify_desktop(
            "ilhae/settings_changed",
            json!({
                "key": key,
                "value": value,
            }),
        )
        .await;
}

/// Push the current engine state to all desktop clients (SSoT).
/// Called on: Initialize, settings change (agent.command), team mode toggle.
#[allow(dead_code)]
pub async fn notify_engine_state(
    cx_cache: &CxCache,
    settings_store: &crate::settings_store::SettingsStore,
) {
    let settings = settings_store.get();
    let engine = infer_agent_id_from_command(&settings.agent.command);
    let resolved_engine = crate::engine_env::resolve_engine_env(&engine);
    let team_mode = settings.agent.team_mode;
    let team_backend = crate::config::normalize_team_backend(&settings.agent.team_backend);
    let use_remote_team =
        team_mode && crate::config::team_backend_uses_remote_transport(&team_backend);

    let endpoint = if use_remote_team {
        let ep = settings.agent.a2a_endpoint.trim();
        if ep.is_empty() {
            format!("http://127.0.0.1:{}", crate::port_config::team_base_port())
        } else {
            ep.to_string()
        }
    } else {
        let ep = settings.agent.a2a_endpoint.trim();
        if ep.is_empty() {
            format!("http://127.0.0.1:{}", resolved_engine.default_port())
        } else {
            ep.to_string()
        }
    };

    if settings.agent.embed_mode {
        std::env::set_var("ILHAE_EMBED_MODE", "1");
    } else {
        std::env::set_var("ILHAE_EMBED_MODE", "0");
    }

    info!(
        "[SSoT] Pushing engine_state: engine={}, endpoint={}, team_mode={}, team_backend={}",
        engine, endpoint, team_mode, team_backend
    );

    cx_cache
        .notify_desktop(
            NOTIF_ENGINE_STATE,
            json!(IlhaeEngineStateNotification {
                engine: engine.clone(),
                endpoint,
                team_mode,
                team_backend: team_backend.clone(),
                team_merge_policy: settings.agent.team_merge_policy.clone(),
                team_max_retries: settings.agent.team_max_retries,
                team_pause_on_error: settings.agent.team_pause_on_error,
                auto_mode: settings.agent.autonomous_mode,
                advisor_mode: settings.agent.advisor_mode,
                advisor_preset: settings.agent.advisor_preset.clone(),
                auto_max_turns: settings.agent.auto_max_turns,
                auto_timebox_minutes: settings.agent.auto_timebox_minutes,
                auto_pause_on_error: settings.agent.auto_pause_on_error,
                kairos_enabled: settings.agent.kairos_enabled,
                self_improvement_enabled: settings.agent.self_improvement_enabled,
                self_improvement_preset: settings.agent.self_improvement_preset.clone(),
                active_profile: settings.agent.active_profile.clone(),
                memory_scope: settings.agent.memory_scope.clone(),
                task_scope: settings.agent.task_scope.clone(),
                knowledge_mode: settings.agent.knowledge_mode.clone(),
                knowledge_workspace_id: settings.agent.knowledge_runtime.last_workspace_id.clone(),
                knowledge_last_result: (!settings
                    .agent
                    .knowledge_runtime
                    .last_result
                    .trim()
                    .is_empty())
                .then(|| settings.agent.knowledge_runtime.last_result.clone()),
                knowledge_last_driver: settings.agent.knowledge_runtime.last_driver.clone(),
                knowledge_last_issue_count: settings.agent.knowledge_runtime.last_issue_count,
                knowledge_last_run_reason: settings.agent.knowledge_runtime.last_run_reason.clone(),
                approval_preset: settings.permissions.approval_preset.clone(),
                command: settings.agent.command.clone(),
                capabilities: crate::capabilities::engine_capability_profile_json(&engine),
                capability_matrix: crate::capabilities::engine_capability_matrix_json(),
            }),
        )
        .await;
}

// ─── Mobile attachments ──────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RelayAttachmentPayload {
    pub name: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    pub data_base64: String,
}

pub fn sanitize_attachment_filename(raw: &str) -> String {
    let file_name = Path::new(raw)
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("attachment");
    let mut sanitized = file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        sanitized = "attachment".to_string();
    }
    sanitized
}

pub fn save_mobile_attachments_to_cwd(
    session_id: &str,
    session_cwd: &str,
    attachments: &[RelayAttachmentPayload],
) -> Result<Vec<PathBuf>, String> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }

    let cwd = session_cwd.trim();
    if cwd.is_empty() {
        return Err("session cwd is empty".to_string());
    }

    let upload_dir = PathBuf::from(cwd).join("ilhae-uploads").join(session_id);
    std::fs::create_dir_all(&upload_dir)
        .map_err(|e| format!("failed to create upload directory (cwd={}): {}", cwd, e))?;

    let mut saved_paths = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let _mime_type = attachment.mime_type.as_deref().unwrap_or("");
        let normalized_name = sanitize_attachment_filename(&attachment.name);
        let file_name = format!("{}-{}", Uuid::new_v4(), normalized_name);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(attachment.data_base64.trim())
            .map_err(|e| format!("invalid attachment base64 ({}): {}", normalized_name, e))?;
        let file_path = upload_dir.join(file_name);
        std::fs::write(&file_path, bytes).map_err(|e| {
            format!(
                "failed to write attachment (cwd={}, name={}): {}",
                cwd, normalized_name, e
            )
        })?;
        saved_paths.push(file_path);
    }
    Ok(saved_paths)
}

// ─── Session bootstrap ──────────────────────────────────────────────────

pub fn is_initialize_related_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("not initialized")
        || lower.contains("initialize")
        || lower.contains("initialize/proxy")
}

pub async fn send_new_session_with_bootstrap(
    cx: &ConnectionTo<Conductor>,
    cwd: &str,
) -> Result<NewSessionResponse, sacp::Error> {
    let request = NewSessionRequest::new(PathBuf::from(cwd));
    match cx.send_request_to(Agent, request).block_task().await {
        Ok(response) => Ok(response),
        Err(err) => {
            if !is_initialize_related_error(&err.to_string()) {
                return Err(err);
            }

            info!("[Relay] NewSessionRequest hit initialize guard. Retrying after initialize.");
            let init_req = InitializeRequest::new(ProtocolVersion::LATEST);
            cx.send_request_to(Agent, init_req).block_task().await?;

            cx.send_request_to(Agent, NewSessionRequest::new(PathBuf::from(cwd)))
                .block_task()
                .await
        }
    }
}

// ─── Config builders (re-exported from config_builder.rs) ────────────────

pub use crate::config_builder::{
    apply_codex_profile_to_config, build_codex_config_options, build_dynamic_instructions,
    build_gemini_config_options, enrich_response_with_config_options, read_codex_runtime_options,
    write_codex_runtime_option,
};

// ─── Browser Tool Detection ─────────────────────────────────────────────

/// Patterns that uniquely identify browser automation tools.
/// Uses prefix match for "browser_" and contains for patterns unlikely to appear in non-browser tools.
const BROWSER_TOOL_PREFIXES: &[&str] = &["browser_"];
const BROWSER_TOOL_EXACT_PATTERNS: &[&str] = &[
    "screenshot",
    "select_option",
    "close_page",
    "wait_for_selector",
    "press_key_combination",
    "drag_and_drop",
];

pub fn is_browser_tool(tool_name: &str) -> bool {
    let lower = tool_name.to_lowercase();
    BROWSER_TOOL_PREFIXES.iter().any(|p| lower.starts_with(p))
        || BROWSER_TOOL_EXACT_PATTERNS
            .iter()
            .any(|p| lower.contains(p))
}

pub fn detect_browser_tool_in_update(update: &SessionUpdate) -> Option<String> {
    match update {
        SessionUpdate::ToolCall(tc) => {
            if is_browser_tool(&tc.title) {
                Some(tc.title.clone())
            } else {
                None
            }
        }
        _ => None,
    }
}

// ─── Synthetic tool call ─────────────────────────────────────────────────

/// Send a synthetic `tool_call` SessionNotification to the client.
pub fn send_synthetic_tool_call(
    session_id: &agent_client_protocol_schema::SessionId,
    tool_call_update: &ToolCallUpdate,
    cx: &ConnectionTo<Conductor>,
) {
    match AcpToolCall::try_from(tool_call_update.clone()) {
        Ok(tc) => {
            let notif = SessionNotification::new(session_id.clone(), SessionUpdate::ToolCall(tc));
            if let Err(e) = cx.send_notification_to(Client, notif) {
                warn!("Failed to send synthetic tool_call to client: {}", e);
            }
        }
        Err(e) => {
            debug!(
                "Could not convert ToolCallUpdate to ToolCall ({}), sending as update",
                e
            );
            let notif = SessionNotification::new(
                session_id.clone(),
                SessionUpdate::ToolCallUpdate(tool_call_update.clone()),
            );
            if let Err(e) = cx.send_notification_to(Client, notif) {
                warn!("Failed to send synthetic tool_call_update to client: {}", e);
            }
        }
    }
}

/// Parse host and port from a URL string.
pub fn parse_host_port(url: &str) -> (String, u16) {
    let stripped = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host_port = stripped.split('/').next().unwrap_or(stripped);
    if let Some(colon) = host_port.rfind(':') {
        let host = host_port[..colon].to_string();
        let port = host_port[colon + 1..].parse::<u16>().unwrap_or(80);
        (host, port)
    } else if url.starts_with("https://") {
        (host_port.to_string(), 443)
    } else {
        (host_port.to_string(), 80)
    }
}

/// Quick TCP probe — returns true if something is listening.
pub fn probe_tcp(host: &str, port: u16) -> bool {
    let addr_str = format!("{}:{}", host, port);
    if let Ok(addr) = addr_str.parse::<std::net::SocketAddr>() {
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(150)).is_ok()
    } else {
        use std::net::ToSocketAddrs;
        addr_str
            .to_socket_addrs()
            .ok()
            .and_then(|mut addrs| addrs.next())
            .map(|addr| {
                std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(150))
                    .is_ok()
            })
            .unwrap_or(false)
    }
}

/// Ultra-fast TCP probe (50ms timeout).
pub fn probe_tcp_fast(host: &str, port: u16) -> bool {
    let addr_str = format!("{}:{}", host, port);
    if let Ok(addr) = addr_str.parse::<std::net::SocketAddr>() {
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(50)).is_ok()
    } else {
        false
    }
}

/// Re-export from ports (single source of truth).
pub use crate::ports::TeamSpawnEnv;

/// Spawn a local a2a-server process on the given port.
pub async fn spawn_local_a2a_server(
    port: u16,
    agent_cmd: &str,
    card_name: Option<&str>,
    team_env: Option<TeamSpawnEnv>,
) -> anyhow::Result<tokio::process::Child> {
    let inferred = infer_agent_id_from_command(agent_cmd);
    let engine_name = if is_ilhae_native_agent_id(&inferred) {
        ILHAE_AGENT_ID
    } else if inferred == "codex" {
        LEGACY_CODEX_AGENT_ID
    } else {
        "gemini"
    };

    // ── Engine dispatch (single source of truth) ──
    let engine_env = crate::engine_env::resolve_engine_env(engine_name);
    let mut cmd = engine_env.build_spawn_command(port, "solo");

    let default_card = format!("{} Agent", engine_env.label());
    cmd.env("AGENT_CARD_NAME", card_name.unwrap_or(&default_card));

    crate::engine_env::apply_engine_env(&mut cmd, engine_name).await;

    if let Some(ref te) = team_env {
        cmd.env(
            "GEMINI_CLI_HOME",
            te.workspace_path.to_string_lossy().as_ref(),
        );
        let gemini_config_dir = te.workspace_path.join(".gemini");
        cmd.env(
            "GEMINI_CONFIG_DIR",
            gemini_config_dir.to_string_lossy().as_ref(),
        );
        cmd.env("CODEX_HOME", te.workspace_path.to_string_lossy().as_ref());
        cmd.env("CODER_AGENT_NAME", te.role.to_lowercase());
        cmd.env("CODER_AGENT_ENABLE_AGENTS", "true");
        cmd.env("CODER_AGENT_IGNORE_WORKSPACE_SETTINGS", "true");
        if let Ok(cwd) = std::env::current_dir() {
            let mut current = cwd.clone();
            let mut project_root = cwd.clone();
            while let Some(parent) = current.parent() {
                if current.join(".git").exists() {
                    project_root = current.clone();
                    break;
                }
                current = parent.to_path_buf();
            }
            cmd.env(
                "CODER_AGENT_WORKSPACE_PATH",
                project_root.to_string_lossy().as_ref(),
            );
        }
        if let Some(team_ws_dir) = te.workspace_path.parent() {
            if let Some(ilhae_root) = team_ws_dir.parent() {
                let brain_dir = ilhae_root.join("brain");
                if brain_dir.is_dir() {
                    cmd.env(
                        "CODER_AGENT_INCLUDE_DIRS",
                        brain_dir.to_string_lossy().as_ref(),
                    );
                }
            }
        }
        info!(
            "[Supervisor] Team env set for {} (workspace: {:?})",
            te.role, te.workspace_path
        );
    }

    cmd.stdin(std::process::Stdio::null());
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let log_dir = std::path::PathBuf::from(&home).join(".gemini").join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_file = log_dir.join(format!("a2a-server-daemon-{}.log", port));
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
    {
        if let Ok(file_clone) = file.try_clone() {
            cmd.stdout(file_clone);
        } else {
            cmd.stdout(std::process::Stdio::null());
        }
        cmd.stderr(file);
    } else {
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
    }

    #[cfg(unix)]
    {
        #[allow(unused_imports)]
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd.spawn()?;
    info!(pid = ?child.id(), port, log = %log_file.display(), "Local a2a-server process spawned as daemon");
    if let Some(pid) = child.id() {
        let ilhae_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("ilhae");
        crate::append_child_pid(&ilhae_dir, pid);
    }
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_store;

    // ── infer_agent_id_from_command ──────────────────────────────

    #[test]
    fn infer_codex() {
        assert_eq!(infer_agent_id_from_command("codex-acp server"), "codex");
        assert_eq!(infer_agent_id_from_command("npx codex --help"), "codex");
        assert_eq!(
            infer_agent_id_from_command("codex-ilhae-llama-nemotron"),
            "ilhae"
        );
    }

    #[test]
    fn infer_gemini() {
        assert_eq!(infer_agent_id_from_command("gemini-cli start"), "gemini");
    }

    #[test]
    fn infer_claude() {
        assert_eq!(
            infer_agent_id_from_command("claude-code-acp serve"),
            "claude"
        );
    }

    #[test]
    fn infer_unknown_fallback() {
        let result = infer_agent_id_from_command("my-custom-agent --port 8080");
        assert_eq!(result, "my-custom-agent");
    }

    // ── sanitize_attachment_filename ─────────────────────────────

    #[test]
    fn sanitize_normal_filename() {
        assert_eq!(sanitize_attachment_filename("photo.jpg"), "photo.jpg");
    }

    #[test]
    fn sanitize_special_chars() {
        assert_eq!(
            sanitize_attachment_filename("hello world!.png"),
            "hello_world_.png"
        );
    }

    #[test]
    fn sanitize_path_traversal() {
        assert_eq!(sanitize_attachment_filename("../../etc/passwd"), "passwd");
    }

    // ── is_initialize_related_error ─────────────────────────────

    #[test]
    fn init_error_detected() {
        assert!(is_initialize_related_error("Session not initialized"));
        assert!(is_initialize_related_error("must initialize first"));
    }

    #[test]
    fn non_init_error_ignored() {
        assert!(!is_initialize_related_error("timeout waiting for response"));
    }

    // ── parse_host_port ─────────────────────────────────────────

    #[test]
    fn parse_host_port_with_port() {
        let (host, port) = parse_host_port("http://localhost:4321/api");
        assert_eq!(host, "localhost");
        assert_eq!(port, 4321);
    }

    #[test]
    fn parse_host_port_https_default() {
        let (host, port) = parse_host_port("https://example.com/path");
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn parse_host_port_http_default() {
        let (host, port) = parse_host_port("http://example.com");
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
    }

    // ── is_browser_tool ─────────────────────────────────────────

    #[test]
    fn browser_tool_detected() {
        assert!(is_browser_tool("browser_click"));
        assert!(is_browser_tool("browser_navigate"));
        assert!(is_browser_tool("screenshot"));
    }

    #[test]
    fn non_browser_tool() {
        assert!(!is_browser_tool("read_file"));
        assert!(!is_browser_tool("shell"));
    }

    #[test]
    fn browser_plugin_does_not_inject_browser_priority_prompt() {
        let mut settings = settings_store::Settings::default();
        settings.plugins.insert("browser".to_string(), true);

        let instructions = build_dynamic_instructions(&settings);

        assert!(!instructions.contains("## 브라우저 자동화 도구 우선 사용"));
        assert!(
            !instructions.contains(
                "외부 MCP 브라우저 도구(chrome-devtools 등)보다 이 도구를 우선 사용하세요."
            )
        );
    }
}
