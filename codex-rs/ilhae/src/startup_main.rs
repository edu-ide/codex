use crate::*; // Brings in helpers, types, plugins, SharedState, etc.

use crate::mcp_manager::McpManager;

use agent_client_protocol_schema::ContentBlock;
use codex_protocol::user_input::UserInput;
use sacp::DynConnectTo;

use moka::sync::Cache;
use std::collections::{HashMap, HashSet};
use std::sync::{atomic::AtomicU64, Arc, OnceLock};
use std::{process::Stdio, time::Duration};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Signal from pre-spawn to build_agent_transport: leader has been spawned.
static LEADER_READY: tokio::sync::Notify = tokio::sync::Notify::const_new();

use crate::browser_manager::BrowserManager;
use crate::relay_server::{broadcast_event, start_relay_server, RelayEvent, RelayState};
use crate::settings_store::SettingsStore;
use crate::startup::{build_agent_transport, cleanup_redundant_sessions};
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

pub fn subscribe_native_loop_lifecycle(
) -> broadcast::Receiver<crate::IlhaeLoopLifecycleNotification> {
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

pub fn current_native_backend_capability_profile(
) -> Option<crate::capabilities::EngineCapabilityProfile> {
    current_native_backend_engine()
        .map(|engine| crate::capabilities::engine_capability_profile(&engine))
}

async fn native_runtime_healthcheck(url: &str) -> bool {
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
    if prompt.len() > 480 {
        return Err(format!("prompt too long: {} chars", prompt.len()));
    }
    if instructions.len() > 900 {
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
        return Err("instructions must stay within memory_dream tool scope".to_string());
    }
    let score = response.score.unwrap_or(0.0);
    if score <= 0.0 {
        return Err("candidate score did not improve baseline".to_string());
    }
    Ok((prompt.to_string(), instructions.to_string(), score))
}

fn spawn_native_runtime_server(
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
) -> anyhow::Result<()> {
    if config.server_bin.trim().is_empty() {
        anyhow::bail!("native runtime server_bin is required");
    }

    let mut command = std::process::Command::new(&config.server_bin);
    if config.args.is_empty() {
        if !config.model_path.trim().is_empty() {
            command.arg("-m").arg(&config.model_path);
        }
        if !config.chat_template_file.trim().is_empty() {
            command
                .arg("--chat-template-file")
                .arg(&config.chat_template_file);
        }
    } else {
        command.args(&config.args);
    }

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

    let child = command.spawn()?;
    info!(
        pid = child.id(),
        server_bin = %config.server_bin,
        "[NativeRuntime] spawned local model server"
    );
    Ok(())
}

pub async fn ensure_native_runtime_for_cli(profile_id: Option<&str>) -> anyhow::Result<()> {
    let Some((profile_id, config)) = crate::config::get_native_runtime_config(profile_id) else {
        return Ok(());
    };

    if native_runtime_healthcheck(&config.health_url).await {
        if !config.base_url.trim().is_empty() {
            unsafe {
                std::env::set_var("CODEX_OSS_BASE_URL", &config.base_url);
            }
        }
        return Ok(());
    }

    spawn_native_runtime_server(&config)?;

    let timeout_secs = config.startup_timeout_secs.max(1);
    let started = tokio::time::Instant::now();
    loop {
        if native_runtime_healthcheck(&config.health_url).await {
            break;
        }
        if started.elapsed().as_secs() >= timeout_secs {
            anyhow::bail!(
                "native runtime for profile `{profile_id}` did not become healthy within {}s",
                timeout_secs
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    if !config.base_url.trim().is_empty() {
        unsafe {
            std::env::set_var("CODEX_OSS_BASE_URL", &config.base_url);
        }
    }

    info!(
        profile = %profile_id,
        base_url = %config.base_url,
        "[NativeRuntime] local model runtime ready"
    );
    Ok(())
}

pub async fn stop_native_runtime_for_cli(profile_id: Option<&str>) -> anyhow::Result<()> {
    let Some((profile_id, config)) = crate::config::get_native_runtime_config(profile_id) else {
        println!("No active native runtime profile found.");
        return Ok(());
    };

    let path = std::path::Path::new(&config.server_bin);
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        println!(
            "Stopping native runtime: {} (profile: {})",
            name, profile_id
        );

        let mut cmd = std::process::Command::new("killall");
        cmd.arg(name);

        match cmd.status() {
            Ok(status) => {
                if status.success() {
                    println!("Successfully stopped {}.", name);
                } else {
                    println!("Failed to stop {} (maybe it wasn't running).", name);
                }
            }
            Err(e) => {
                println!("Error attempting to stop {}: {}", name, e);
            }
        }
    }

    Ok(())
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
            use context_proxy::{
                ensure_user_agent_server, extract_port_from_endpoint,
                generate_peer_registration_files, load_team_runtime_config, spawn_team_a2a_servers,
                trigger_agent_reload, wait_for_all_team_health,
            };
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
                        if let Some(pid) = child.id() {
                            if let Some(port) = extract_port_from_endpoint(&agent_cfg.endpoint) {
                                process_supervisor::record_pid(&sv, port, pid).await;
                            }
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
            if let Some(ref bw) = store_bg.brain_writer() {
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
