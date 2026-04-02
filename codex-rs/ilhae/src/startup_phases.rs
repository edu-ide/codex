//! Startup phase functions extracted from main().
//!
//! - `start_health_server`: Axum HTTP health/admin API
//! - `spawn_background_workers`: A2A proxy, channel bots, relay, team observer
//! - `run_conductor`: ACP conductor chain + stdio/daemon run loop

use std::sync::Arc;
use tracing::{info, warn};

use crate::a2a_persistence;
use crate::admin_proxy;
use crate::agent_router;
use crate::channel_bots;
use crate::context_proxy;
use crate::helpers::{parse_host_port, probe_tcp};
use crate::persistence_proxy;
use crate::process_supervisor;
use crate::relay_commands;
use crate::relay_proxy;
use crate::relay_server;
use crate::shared_state::SharedState;
use crate::startup::{build_agent_transport, resolve_team_main_target};
use crate::tools_proxy;

use sacp_conductor::snoop::SnooperComponent;
use sacp_conductor::{ConductorImpl, McpBridgeMode, ProxiesAndAgent};

/// Static handoff: pre-bound A2A persistence proxy listener + routing table.
/// Set by pre-spawn (main.rs), consumed by `spawn_background_workers`.
static A2A_PROXY_PREBOUND: std::sync::Mutex<
    Option<(tokio::net::TcpListener, Vec<(String, String, bool)>)>,
> = std::sync::Mutex::new(None);

/// Expose so main.rs can set it during pre-spawn.
pub fn set_a2a_proxy_prebound(val: (tokio::net::TcpListener, Vec<(String, String, bool)>)) {
    A2A_PROXY_PREBOUND
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(val);
}

pub async fn start_health_server(shared: Arc<SharedState>) -> anyhow::Result<()> {
    use axum::Json;
    use axum::extract::State as AxState;
    use axum::routing::get;

    fn resolve_agent_endpoint(state: &SharedState) -> (String, bool) {
        let settings = state.infra_context().settings_store.get();
        let mock_mode = settings.agent.mock_mode
            || std::env::var("ILHAE_MOCK")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);
        let endpoint = if settings.agent.team_mode {
            let ep = settings.agent.a2a_endpoint.trim();
            if ep.is_empty() {
                resolve_team_main_target(&state.infra_context().ilhae_dir)
                    .map(|(_, endpoint)| endpoint)
                    .unwrap_or_else(|| {
                        format!("http://127.0.0.1:{}", crate::port_config::team_base_port())
                    })
            } else {
                ep.to_string()
            }
        } else {
            let ep = settings.agent.a2a_endpoint.trim();
            if ep.is_empty() {
                let port = if settings.agent.command.contains("codex") {
                    crate::port_config::codex_a2a_port()
                } else {
                    crate::port_config::gemini_a2a_port()
                };
                format!("http://127.0.0.1:{port}")
            } else {
                ep.to_string()
            }
        };
        (endpoint, mock_mode)
    }

    async fn health(AxState(state): AxState<Arc<SharedState>>) -> Json<serde_json::Value> {
        let desktop_ready = state.infra_context().relay_conductor_cx.latest().await.is_some();
        let (agent_endpoint, mock_mode) = resolve_agent_endpoint(&state);
        let (host, port) = parse_host_port(&agent_endpoint);
        let agent_ready = if mock_mode {
            true
        } else {
            probe_tcp(&host, port)
        };
        let sv = state.team_state().supervisor.read().await;
        let team_agents = sv
            .processes
            .iter()
            .map(|(name, proc)| {
                serde_json::json!({
                    "name": name,
                    "port": proc.port,
                    "alive": proc.last_healthy.is_some(),
                })
            })
            .collect::<Vec<_>>();

        Json(serde_json::json!({
            "relay_listen": true,
            "desktop_ready": desktop_ready,
            "agent_ready": agent_ready,
            "mock_mode": mock_mode,
            "agent_endpoint": agent_endpoint,
            "team_agents": team_agents,
        }))
    }

    async fn ready(AxState(state): AxState<Arc<SharedState>>) -> axum::http::StatusCode {
        let (agent_endpoint, mock_mode) = resolve_agent_endpoint(&state);
        if mock_mode {
            return axum::http::StatusCode::OK;
        }
        let (host, port) = parse_host_port(&agent_endpoint);
        if probe_tcp(&host, port) {
            axum::http::StatusCode::OK
        } else {
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        }
    }

    // ── A2A Push Notification webhook handler ──────────────────────────
    // Per A2A spec, when push_notification_config is set in a request,
    // the agent server POSTs task status updates to this webhook URL
    // when the task completes asynchronously.
    //
    // Payload: { "id": "...", "result": { Task } } or { "id": "...", "error": {...} }
    async fn handle_push_notification(
        AxState(state): AxState<Arc<SharedState>>,
        axum::extract::Path(session_id): axum::extract::Path<String>,
        axum::extract::Json(payload): axum::extract::Json<serde_json::Value>,
    ) -> axum::http::StatusCode {
        tracing::info!(
            "[A2A:Push] Received push notification for session={}: {}",
            session_id,
            serde_json::to_string(&payload)
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect::<String>()
        );

        // Extract task from push payload
        let task = match payload.get("result") {
            Some(t) => t,
            None => {
                if let Some(err) = payload.get("error") {
                    tracing::warn!(
                        "[A2A:Push] Push error for session={}: {:?}",
                        session_id,
                        err
                    );
                }
                return axum::http::StatusCode::OK;
            }
        };

        let task_id = task.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
        let task_state = task
            .pointer("/status/state")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        // Extract agent role from the push notification config id (format: "role:session_id")
        let push_id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let agent_role = push_id.split(':').next().unwrap_or("unknown");

        // Extract response text from status message or artifacts
        let preview = task
            .pointer("/status/message/parts")
            .and_then(|v| v.as_array())
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        tracing::info!(
            "[A2A:Push] Task update: session={}, task={}, state={}, agent={}, preview={}B",
            session_id,
            task_id,
            task_state,
            agent_role,
            preview.len()
        );

        // ── Relay to desktop UI via SACP ──
        let maybe_cx: Option<sacp::ConnectionTo<sacp::Conductor>> =
            state.infra_context().relay_conductor_cx.latest().await;
        if let Some(cx) = maybe_cx {
            // Send ilhae/a2a_task_update
            let update_payload = serde_json::json!({
                "sessionId": session_id,
                "agentRole": agent_role,
                "taskId": task_id,
                "state": task_state,
                "preview": preview.chars().take(200).collect::<String>(),
                "eventCount": 0,
                "source": "push_notification",
            });
            if let Ok(notif) = sacp::UntypedMessage::new("ilhae/a2a_task_update", update_payload) {
                let _ = cx.send_notification_to(sacp::Client, notif);
            }

            // For terminal states, also send background_task_completed
            let is_terminal = matches!(
                task_state,
                "completed" | "failed" | "canceled" | "cancelled"
            );
            if is_terminal {
                if let Ok(notif) = sacp::UntypedMessage::new(
                    crate::types::NOTIF_BACKGROUND_TASK_COMPLETED,
                    serde_json::json!({
                        "taskId": task_id,
                        "agentRole": agent_role,
                        "sessionId": session_id,
                    }),
                ) {
                    let _ = cx.send_notification_to(sacp::Client, notif);
                }
            }
        }

        // ── Persist to session store ──
        if !session_id.is_empty() {
            use crate::team_timeline::{persist_events, task_status_event};
            let preview_text = if preview.is_empty() {
                "(push notification)"
            } else {
                &preview
            };
            persist_events(
                state.infra_context().brain.sessions(),
                &session_id,
                [task_status_event(
                    agent_role,
                    task_id,
                    preview_text,
                    task_state,
                    None,
                )],
            );
        }

        axum::http::StatusCode::OK
    }

    // ── Admin Dashboard API ───────────────────────────────────────────
    async fn api_metrics(AxState(state): AxState<Arc<SharedState>>) -> Json<serde_json::Value> {
        let metrics = crate::process_supervisor::get_delegation_metrics(
            &state.team_state().delegation_metrics,
        )
        .await;
        Json(serde_json::json!({
            "total_delegations": metrics.total,
            "success": metrics.success,
            "failed": metrics.failed,
            "avg_duration_ms": if metrics.total > 0 { metrics.total_duration_ms / metrics.total } else { 0 },
            "per_agent": metrics.per_agent.iter().map(|(name, m)| {
                serde_json::json!({
                    "name": name,
                    "delegations": m.delegations,
                    "success_rate": format!("{:.1}%", m.success_rate()),
                    "avg_duration_ms": m.avg_duration_ms(),
                    "consecutive_failures": m.consecutive_failures,
                    "circuit": format!("{:?}", m.circuit),
                })
            }).collect::<Vec<_>>(),
        }))
    }

    async fn api_team_status(AxState(state): AxState<Arc<SharedState>>) -> Json<serde_json::Value> {
        let sv = state.team_state().supervisor.read().await;
        let agents: Vec<_> = sv
            .processes
            .iter()
            .map(|(name, proc)| {
                serde_json::json!({
                    "name": name,
                    "port": proc.port,
                    "enabled": proc.enabled,
                    "alive": proc.last_healthy.is_some(),
                    "restart_count": proc.restart_count,
                    "is_leader": proc.is_leader,
                    "has_agent_card": proc.cached_agent_card.is_some(),
                })
            })
            .collect();
        let leader = sv
            .processes
            .iter()
            .find(|(_, p)| p.is_leader)
            .map(|(n, _)| n.clone())
            .unwrap_or_default();
        Json(serde_json::json!({
            "agent_count": agents.len(),
            "leader": leader,
            "agents": agents,
        }))
    }

    async fn api_team_comms(AxState(state): AxState<Arc<SharedState>>) -> Json<serde_json::Value> {
        let events = state.team_state().comms.recent_events(20).await;
        let items: Vec<_> = events
            .iter()
            .map(|(ts, evt)| {
                serde_json::json!({
                    "timestamp": ts,
                    "event": evt,
                })
            })
            .collect();
        Json(serde_json::json!({ "events": items }))
    }

    // ── Agent Hot-Add: add a new team agent at runtime ──
    #[derive(serde::Deserialize)]
    struct HotAddAgent {
        role: String,
        endpoint: String,
        #[serde(default)]
        engine: Option<String>,
    }

    async fn api_add_agent(
        AxState(state): AxState<Arc<SharedState>>,
        axum::extract::Json(body): axum::extract::Json<HotAddAgent>,
    ) -> Json<serde_json::Value> {
        let role = body.role.to_lowercase();
        let engine = body.engine.unwrap_or_else(|| "gemini".to_string());

        // Add to supervisor
        let mut sv = state.team_state().supervisor.write().await;
        if sv.processes.contains_key(&role) {
            return Json(serde_json::json!({ "ok": false, "error": "Agent already exists" }));
        }

        sv.processes.insert(
            role.clone(),
            crate::process_supervisor::ManagedProcess {
                name: role.clone(),
                port: 0,
                engine: engine.clone(),
                card_name: None,
                pid: None,
                restart_count: 0,
                last_healthy: None,
                enabled: true,
                workspace_path: None,
                role: Some(role.clone()),
                is_leader: false,
                original_leader: false,
                cached_agent_card: None,
                card_last_fetched: None,
            },
        );

        info!(
            "[HotAdd] Added agent: role={}, endpoint={}, engine={}",
            role, body.endpoint, engine
        );

        Json(serde_json::json!({
            "ok": true,
            "agent": role,
            "endpoint": body.endpoint,
            "engine": engine,
        }))
    }

    let health_app = axum::Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route(
            "/a2a/push/:session_id",
            axum::routing::post(handle_push_notification),
        )
        .route("/api/metrics", get(api_metrics))
        .route("/api/team/status", get(api_team_status))
        .route("/api/team/comms", get(api_team_comms))
        .route("/api/team/agents", axum::routing::post(api_add_agent))
        .with_state(shared.clone());

    let listener = tokio::net::TcpListener::bind((
        std::net::Ipv4Addr::LOCALHOST,
        crate::port_config::health_port(),
    ))
    .await?;
    info!(
        "[Health] HTTP server listening at http://127.0.0.1:{}",
        crate::port_config::health_port()
    );
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, health_app).await {
            warn!("[Health] HTTP server error: {}", e);
        }
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn spawn_background_workers(
    shared: Arc<SharedState>,
    mut command_rx: tokio::sync::mpsc::Receiver<relay_server::RelayCommandWithClient>,
) {
    // ── Phase 2: Start A2A persistence proxy chain (if pre-bound) ──
    if let Some((listener, routing_table)) = A2A_PROXY_PREBOUND
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        info!("[A2aProxy] Starting AgentExecutor proxy chain");
        let cx_for_proxy = shared.infra_context().relay_conductor_cx.clone();
        let event_tx_for_proxy = shared.team_state().event_tx.clone();
        tokio::spawn(async move {
            let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
            let base_url = format!("http://127.0.0.1:{}", port);

            // Shared delegation response cache: sub-agents write, leader reads
            let delegation_cache: a2a_persistence::DelegationResponseCache =
                std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

            let (app, _routing_map) = a2a_persistence::build_proxy_router(
                &routing_table,
                event_tx_for_proxy,
                cx_for_proxy.clone(),
                delegation_cache,
                &base_url,
            );

            if let Err(e) = axum::serve(listener, app).await {
                warn!("[A2aProxy] Server error: {}", e);
            }
        });
    }

    // ── Start Delegation Tracker Daemon ──
    {
        let shared_tracker = shared.clone();
        let rx = shared.team_state().event_tx.subscribe();
        crate::a2a_persistence::delegation_tracker::spawn_tracker_daemon(shared_tracker, rx).await;
    }

    // ── Start Channel Bots (deferred — non-blocking) ──────────────────
    // Spawned in background so they don't block the ACP conductor startup.
    {
        let shared_bots = shared.clone();
        tokio::spawn(async move {
            tokio::join!(
                channel_bots::start_telegram_if_enabled(&shared_bots),
                channel_bots::start_discord_if_enabled(&shared_bots),
                channel_bots::start_slack_if_enabled(&shared_bots),
                channel_bots::start_kakao_if_enabled(&shared_bots),
                channel_bots::start_line_if_enabled(&shared_bots),
                channel_bots::start_whatsapp_if_enabled(&shared_bots),
            );
        });
    }

    // ── Spawn relay command handler ──────────────────────────────────────
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            while let Some(relay_cmd) = command_rx.recv().await {
                relay_commands::dispatch(&shared, relay_cmd).await;
            }
        });
    }

    // ── Spawn Team Agent Native Stream Observer (A2A Full Mesh) ──────────
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut observing_agents = std::collections::HashSet::new();

            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;

                // Wait until we have a conductor connection to send UI patches
                let Some(cx) = shared.infra_context().relay_conductor_cx.latest().await else {
                    continue;
                };

                let Some(team_cfg) = context_proxy::team_a2a::load_team_runtime_config(
                    &shared.infra_context().ilhae_dir,
                ) else {
                    continue;
                };

                // Check supervisor for healthy agents
                // (last_healthy is set by TeamPreSpawn after wait_for_all_team_health succeeds,
                //  and kept up-to-date by the supervisor health loop.)
                let sv = shared.team_state().supervisor.read().await;
                let mut needs_init = false;
                for agent in &team_cfg.agents {
                    let role_lower = agent.role.to_lowercase();
                    if observing_agents.contains(&role_lower) {
                        continue;
                    }

                    let key = format!("team-{}", role_lower);
                    if let Some(proc) = sv.processes.get(&key) {
                        if proc.last_healthy.is_some() {
                            needs_init = true;
                        }
                    }
                }
                drop(sv);

                if !needs_init {
                    continue;
                }

                // Ensure AgentPool connects to all team agents
                shared
                    .team_state()
                    .agent_pool
                    .init_from_team_config(&team_cfg)
                    .await;

                for agent in &team_cfg.agents {
                    let role_lower = agent.role.to_lowercase();
                    if observing_agents.contains(&role_lower) {
                        continue;
                    }

                    if let Ok(rx) = shared
                        .team_state()
                        .agent_pool
                        .subscribe_updates(&role_lower)
                        .await
                    {
                        tracing::info!(
                            "[TeamMonitor] Spawning native A2A observer for {}",
                            role_lower
                        );
                        crate::a2a_client::spawn_a2a_observer(
                            &role_lower,
                            "", // Generic cross-session streams don't enforce a hardcoded sessionId
                            &cx,
                            rx,
                        );
                        observing_agents.insert(role_lower);
                    }
                }
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_conductor(
    shared: Arc<SharedState>,
    agent: impl sacp::ConnectTo<sacp::Client> + 'static,
    agent_child_slot: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
    mut agent_refresh_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    ilhae_dir: std::path::PathBuf,
    daemon_mode: bool,
) -> anyhow::Result<()> {
    // ── Build Conductor + Proxy chain ────────────────────────────────────
    let admin = admin_proxy::AdminProxy {
        state: shared.clone(),
    };
    let tools = tools_proxy::ToolsProxy {
        state: shared.clone(),
    };
    let persist = persistence_proxy::PersistenceProxy {
        state: shared.clone(),
    };
    let ctx = context_proxy::ContextProxy {
        state: shared.clone(),
    };
    let relay = relay_proxy::RelayProxy {
        state: shared.clone(),
    };

    let router = agent_router::AgentRouter {
        state: shared.clone(),
    };

    // Feature: SnooperComponent — ILHAE_SNOOP=1 으로 모든 ACP 메시지 tracing 로깅
    let chain = if std::env::var("ILHAE_SNOOP").is_ok() {
        info!("ACP message snooping enabled (ILHAE_SNOOP)");
        let snooped = SnooperComponent::new(
            relay,
            |msg| {
                tracing::debug!(direction = "incoming", ?msg, "ACP snoop");
                Ok(())
            },
            |msg| {
                tracing::debug!(direction = "outgoing", ?msg, "ACP snoop");
                Ok(())
            },
        );
        ProxiesAndAgent::new(agent)
            .proxy(admin)
            .proxy(tools)
            .proxy(persist)
            .proxy(ctx)
            .proxy(snooped)
            .proxy(router)
    } else {
        ProxiesAndAgent::new(agent)
            .proxy(admin)
            .proxy(tools)
            .proxy(persist)
            .proxy(ctx)
            .proxy(relay)
            .proxy(router)
    };

    // Feature: MCP Bridge HTTP mode — ILHAE_MCP_HTTP=1 으로 HTTP 브릿지 사용
    let mcp_bridge_mode = if std::env::var("ILHAE_MCP_HTTP").is_ok() {
        info!("MCP bridge mode: HTTP");
        McpBridgeMode::Http
    } else {
        McpBridgeMode::default()
    };

    let (mut conductor, agent_swap_handle) =
        ConductorImpl::new_agent_with_swap_handle("ilhae-conductor", chain, mcp_bridge_mode);

    // 2. Desktop bridge: SettingsEvent → SACP conductor notification
    {
        let mut rx = shared.infra_context().settings_store.subscribe();
        let cx = shared.infra_context().relay_conductor_cx.clone();
        let ss = shared.infra_context().settings_store.clone();
        let refresh_tx = shared.infra_context().agent_refresh_tx.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        cx.notify_desktop(
                            "ilhae/settings_changed",
                            serde_json::json!({
                                "key": event.key,
                                "value": event.value,
                            }),
                        )
                        .await;

                        if event.key == "agent.command" || event.key == "agent.team_mode" {
                            crate::notify_engine_state(&cx, &ss).await;
                            let _ = refresh_tx.send(());
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[SettingsBridge/Desktop] Skipped {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    {
        let ss = shared.infra_context().settings_store.clone();
        let cx = shared.infra_context().relay_conductor_cx.clone();
        let child_slot = agent_child_slot.clone();
        let swap_handle = agent_swap_handle.clone();
        let sv_handle = shared.team_state().supervisor.clone();

        let shared_for_refresh = shared.clone();
        tokio::spawn(async move {
            while agent_refresh_rx.recv().await.is_some() {
                if crate::mock_provider::is_mock_mode() {
                    continue;
                }

                // Determine the new engine from current settings
                let new_engine = {
                    let settings = ss.get();
                    if settings.agent.team_mode {
                        // Team mode handles its own engine switching
                        String::new()
                    } else {
                        settings.agent.command.clone()
                    }
                };

                // Switch supervisor's solo A2A server if engine changed
                if !new_engine.is_empty() {
                    crate::process_supervisor::switch_solo_engine(&sv_handle, &new_engine).await;
                }

                crate::notify_engine_state(&cx, &ss).await;
                match build_agent_transport(&ss, shared_for_refresh.team_state().supervisor.clone())
                    .await
                {
                    Ok((agent, new_child)) => {
                        if let Err(err) = swap_handle.swap_dyn_agent(agent) {
                            warn!("[AgentRefresh] Agent swap failed: {}", err);
                            continue;
                        }

                        let mut child_guard = child_slot.lock().await;
                        if let Some(mut old_child) = child_guard.take() {
                            let _ = old_child.kill().await;
                            let _ = old_child.wait().await;
                        }
                        *child_guard = new_child;
                        info!("[AgentRefresh] Swapped upstream agent transport");
                    }
                    Err(err) => {
                        warn!("[AgentRefresh] Failed to rebuild agent transport: {}", err);
                    }
                }
            }
        });
    }

    // Feature: DebugLogger — ILHAE_DEBUG_DIR=/path/to/dir 로 컴포넌트별 상세 디버그 로그
    let debug_logger = if let Ok(debug_dir) = std::env::var("ILHAE_DEBUG_DIR") {
        match sacp_conductor::debug_logger::DebugLogger::new(
            Some(std::path::PathBuf::from(&debug_dir)),
            &["ilhae-proxy".to_string()],
        )
        .await
        {
            Ok(logger) => {
                info!(dir = %debug_dir, "Debug logger enabled — logs at {}/", debug_dir);
                Some(logger)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to create debug logger, continuing without");
                None
            }
        }
    } else {
        None
    };

    // Feature: Trace Viewer — ILHAE_TRACE_PATH=/path/to/trace.jsons 로 시퀀스 다이어그램 기록
    if let Ok(trace_path) = std::env::var("ILHAE_TRACE_PATH") {
        conductor = conductor
            .trace_to_path(&trace_path)
            .map_err(|e| sacp::util::internal_error(format!("Failed to open trace file: {e}")))?;
        info!(path = %trace_path, "Trace viewer enabled — view with: sacp-trace-viewer {}", trace_path);
    }

    if daemon_mode {
        info!("Proxy daemon mode: starting internal loopback SACP conductor...");

        // Create an in-memory duplex channel to act as a virtual stdio transport.
        // conductor_side <-> client_side form a bidirectional pipe.
        let (client_side, conductor_side) = tokio::io::duplex(64 * 1024);

        // Split the conductor side into read/write halves for ByteStreams
        let (conductor_read, conductor_write) = tokio::io::split(conductor_side);

        use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
        let transport =
            sacp::ByteStreams::new(conductor_write.compat_write(), conductor_read.compat());

        // Run conductor in background — it will process SACP messages from the loopback
        let conductor_handle = tokio::spawn(async move {
            if let Err(e) = conductor.run(transport).await {
                warn!("[DaemonConductor] Conductor exited with error: {}", e);
            }
        });

        // Send initialize handshake from the client side to bootstrap the proxy chain.
        // This causes tools_proxy/admin_proxy to call relay_conductor_cx.try_add(cx),
        // populating CxCache and enabling chat_message to use the full SACP path.
        {
            let (client_read, mut client_write) = tokio::io::split(client_side);

            // Build ACP InitializeRequest JSON-RPC message
            let init_msg = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-10-30",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "ilhae-daemon-loopback",
                        "version": "1.0.0"
                    }
                }
            });
            let init_line = format!("{}\n", init_msg);

            use tokio::io::AsyncWriteExt;
            if let Err(e) = client_write.write_all(init_line.as_bytes()).await {
                warn!("[DaemonConductor] Failed to send initialize: {}", e);
            } else {
                info!("[DaemonConductor] Sent initialize handshake to conductor");
            }

            // Read the initialize response (and discard it)
            use tokio::io::AsyncBufReadExt;
            let mut reader = tokio::io::BufReader::new(client_read);
            let mut response_line = String::new();
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                reader.read_line(&mut response_line),
            )
            .await
            {
                Ok(Ok(_)) => {
                    info!(
                        "[DaemonConductor] Initialize response received ({} bytes), SACP chain active",
                        response_line.len()
                    );
                }
                Ok(Err(e)) => warn!("[DaemonConductor] Error reading init response: {}", e),
                Err(_) => warn!("[DaemonConductor] Timeout waiting for init response"),
            }

            // Inject bridge into relay_state for path-based routing (/acp).
            // Desktop connects to ws://127.0.0.1:{relay_port}/acp with AcpWebSocketTransport.
            let bridge = crate::acp_ws_server::AcpWsBridge::spawn(client_write, reader);
            shared
                .infra_context()
                .relay_state
                .set_acp_bridge(bridge)
                .await;
        }

        info!(
            "[DaemonConductor] SACP loopback conductor running. Desktop: ws://127.0.0.1:{}/acp",
            crate::port_config::sacp_port()
        );

        // Wait for shutdown signals
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sighup = signal(SignalKind::hangup())?;
            tokio::spawn(async move {
                while sighup.recv().await.is_some() {
                    info!("[Shutdown] Ignoring SIGHUP in daemon mode");
                }
            });
            let mut sigterm = signal(SignalKind::terminate())?;
            let mut sigint = signal(SignalKind::interrupt())?;
            tokio::select! {
                _ = sigterm.recv() => {
                    info!("[Shutdown] Received SIGTERM in daemon mode");
                }
                _ = sigint.recv() => {
                    info!("[Shutdown] Received SIGINT in daemon mode");
                }
                _ = conductor_handle => {
                    info!("[Shutdown] Conductor exited in daemon mode");
                }
            }
        }
        #[cfg(not(unix))]
        {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = conductor_handle => {
                    info!("[Shutdown] Conductor exited in daemon mode");
                }
            }
        }

        // 12e: Notify connected Desktop clients of shutdown via ACP bridge
        {
            let bridge = shared
                .infra_context()
                .relay_state
                .acp_bridge
                .read()
                .await
                .clone();
            if let Some(bridge) = bridge {
                let shutdown_msg = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/daemon_shutdown",
                    "params": { "reason": "daemon_exit" }
                });
                let _ = bridge.from_conductor.send(shutdown_msg.to_string());
                info!("[Shutdown] Sent daemon_shutdown notification to Desktop clients");
                // Brief delay to allow WS flush
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    } else {
        // Stdio with optional debug callback
        let stdio = if let Some(ref logger) = debug_logger {
            sacp_tokio::Stdio::new().with_debug(logger.create_callback("C".to_string()))
        } else {
            sacp_tokio::Stdio::new()
        };

        info!("Proxy server listening on stdio...");
        conductor.run(stdio).await?;
    }

    // Graceful shutdown: kill all managed A2A server processes
    info!("[Shutdown] Proxy exiting, cleaning up managed processes...");
    let supervisor_handle = shared.team_state().supervisor.clone();
    process_supervisor::shutdown_all(&supervisor_handle).await;
    crate::process_lifecycle::cleanup_pid_files_for_mode(&ilhae_dir, daemon_mode);
    info!("[Shutdown] PID files cleaned up");

    Ok(())
}
