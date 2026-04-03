//! ProcessSupervisor — manages A2A server lifecycles with PID tracking,
//! health checks, auto-restart, and graceful shutdown.
//!
//! The supervisor owns a set of "managed processes" (solo gemini/codex A2A servers
//! and team A2A servers). A background tokio task periodically probes each managed
//! endpoint via TCP and re-spawns any that have died.
//!
//! ## Architecture
//! - **PID Tracking**: Each spawned process's PID is stored, enabling direct kill
//!   instead of relying on `fuser` port scanning.
//! - **Graceful Shutdown**: `shutdown_all()` kills all tracked PIDs when the proxy exits.
//! - **Engine Hot-Swap**: `restart_team_agents()` detects engine changes, kills old
//!   processes by PID, updates the registry, and lets the health loop re-spawn.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::helpers::{infer_agent_id_from_command, is_ilhae_native_agent_id};
use crate::ports::AgentSpawner;
use crate::settings_store::SettingsStore;

// ── Types ────────────────────────────────────────────────────────────────────

/// Describes one managed A2A server process.
#[derive(Debug)]
pub struct ManagedProcess {
    /// Human-readable name, e.g. "gemini-solo", "codex-solo", "team-leader"
    pub name: String,
    /// TCP port to probe
    pub port: u16,
    /// Engine command hint for spawn (e.g. "gemini", "codex")
    pub engine: String,
    /// A2A card name override (e.g. "Leader (Codex)"). None = use default.
    pub card_name: Option<String>,
    /// Tracked PID of the spawned child process. None if not yet spawned.
    pub pid: Option<u32>,
    /// Restart counter (for backoff)
    pub restart_count: u32,
    /// Last successful health check
    pub last_healthy: Option<Instant>,
    /// Whether this process is enabled (solo processes might be disabled in team mode)
    pub enabled: bool,
    /// Team agent workspace path (for GEMINI_CLI_HOME, CODEX_HOME etc.)
    /// None for solo processes.
    pub workspace_path: Option<PathBuf>,
    /// Team agent role name (e.g. "Researcher"). None for solo processes.
    pub role: Option<String>,
    /// Whether this agent is currently acting as team leader.
    pub is_leader: bool,
    /// Whether this agent was originally configured as leader (is_main in team config).
    pub original_leader: bool,
    /// Cached Agent Card JSON from `.well-known/agent.json`.
    pub cached_agent_card: Option<serde_json::Value>,
    /// When the agent card was last successfully fetched.
    pub card_last_fetched: Option<Instant>,
}

// ── Delegation Metrics + Circuit Breaker (Observability + Resilience) ────

/// Circuit breaker state per agent.
#[derive(Debug, Clone, serde::Serialize)]
pub enum CircuitState {
    Closed,   // Normal — requests flow through
    Open,     // Tripped — requests blocked
    HalfOpen, // Probing — allow one request to test recovery
}

impl Default for CircuitState {
    fn default() -> Self {
        CircuitState::Closed
    }
}

/// Per-agent delegation metrics with circuit breaker.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AgentMetrics {
    pub delegations: u64,
    pub success: u64,
    pub failed: u64,
    pub total_duration_ms: u64,
    /// Consecutive failures (resets on success)
    pub consecutive_failures: u32,
    /// Circuit breaker state
    pub circuit: CircuitState,
    /// When circuit opened (epoch secs)
    #[serde(skip)]
    pub circuit_opened_at: Option<Instant>,
}

/// Circuit breaker thresholds
const CIRCUIT_FAILURE_THRESHOLD: u32 = 3;
const CIRCUIT_COOLDOWN_SECS: u64 = 60;

impl AgentMetrics {
    pub fn avg_duration_ms(&self) -> u64 {
        if self.delegations == 0 {
            0
        } else {
            self.total_duration_ms / self.delegations
        }
    }
    pub fn success_rate(&self) -> f64 {
        if self.delegations == 0 {
            0.0
        } else {
            self.success as f64 / self.delegations as f64 * 100.0
        }
    }
    /// Check if delegation should be allowed (circuit breaker logic).
    pub fn should_allow(&mut self) -> bool {
        match self.circuit {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check cooldown
                if let Some(opened) = self.circuit_opened_at {
                    if opened.elapsed() >= Duration::from_secs(CIRCUIT_COOLDOWN_SECS) {
                        self.circuit = CircuitState::HalfOpen;
                        info!("[CircuitBreaker] Half-open: allowing probe request");
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true, // Allow probe
        }
    }
    /// Record result and update circuit state.
    pub fn record(&mut self, success: bool, duration_ms: u64) {
        self.delegations += 1;
        self.total_duration_ms += duration_ms;
        if success {
            self.success += 1;
            self.consecutive_failures = 0;
            // Reset circuit on success
            if !matches!(self.circuit, CircuitState::Closed) {
                info!("[CircuitBreaker] Circuit closed (recovered)");
                self.circuit = CircuitState::Closed;
                self.circuit_opened_at = None;
            }
        } else {
            self.failed += 1;
            self.consecutive_failures += 1;
            if self.consecutive_failures >= CIRCUIT_FAILURE_THRESHOLD {
                if !matches!(self.circuit, CircuitState::Open) {
                    warn!(
                        "[CircuitBreaker] Circuit OPEN after {} consecutive failures",
                        self.consecutive_failures
                    );
                    self.circuit = CircuitState::Open;
                    self.circuit_opened_at = Some(Instant::now());
                }
            }
        }
    }
}

/// Aggregated delegation metrics for all agents.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DelegationMetrics {
    pub total: u64,
    pub success: u64,
    pub failed: u64,
    pub total_duration_ms: u64,
    pub per_agent: HashMap<String, AgentMetrics>,
    /// Last trace IDs (for distributed tracing correlation)
    #[serde(skip)]
    pub recent_traces: Vec<(String, String, String)>, // (trace_id, agent, timestamp)
}

/// Thread-safe handle to delegation metrics.
pub type MetricsHandle = Arc<RwLock<DelegationMetrics>>;

/// Create a shared metrics handle.
pub fn create_metrics() -> MetricsHandle {
    Arc::new(RwLock::new(DelegationMetrics::default()))
}

/// Generate a trace ID for distributed tracing.
pub fn new_trace_id() -> String {
    format!(
        "tr-{}",
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("0000")
    )
}

/// Record a delegation event with circuit breaker and tracing.
pub async fn record_delegation(
    handle: &MetricsHandle,
    target_agent: &str,
    success: bool,
    duration_ms: i64,
) {
    let mut m = handle.write().await;
    m.total += 1;
    let dur = duration_ms.max(0) as u64;
    m.total_duration_ms += dur;
    if success {
        m.success += 1;
    } else {
        m.failed += 1;
    }

    let agent = m.per_agent.entry(target_agent.to_string()).or_default();
    agent.record(success, dur);
}

/// Check if delegation to an agent is allowed (circuit breaker).
pub async fn is_delegation_allowed(handle: &MetricsHandle, target_agent: &str) -> bool {
    let mut m = handle.write().await;
    let agent = m.per_agent.entry(target_agent.to_string()).or_default();
    agent.should_allow()
}

/// Record a trace for distributed tracing correlation.
pub async fn record_trace(handle: &MetricsHandle, trace_id: &str, agent: &str) {
    let mut m = handle.write().await;
    let ts = chrono::Utc::now().to_rfc3339();
    m.recent_traces
        .push((trace_id.to_string(), agent.to_string(), ts));
    // Keep last 50 traces
    if m.recent_traces.len() > 50 {
        let drain = m.recent_traces.len() - 50;
        m.recent_traces.drain(..drain);
    }
}

/// Get a snapshot of current delegation metrics.
pub async fn get_delegation_metrics(handle: &MetricsHandle) -> DelegationMetrics {
    handle.read().await.clone()
}

// ── Supervisor ───────────────────────────────────────────────────────────

/// Execute external hook script on agent crash
pub async fn execute_crash_hook(role: &str, exit_code: Option<i32>) {
    let hook_script = crate::config::resolve_ilhae_data_dir()
        .join("hooks")
        .join("on_agent_crash.sh");

    if hook_script.exists() {
        let role = role.to_string();
        tokio::spawn(async move {
            let _ = tokio::process::Command::new("sh")
                .arg(hook_script)
                .env("AGENT_ROLE", role)
                .env("EXIT_CODE", exit_code.unwrap_or(-1).to_string())
                .status()
                .await;
        });
    }
}

/// The supervisor that runs as a background task.
pub struct ProcessSupervisor {
    pub processes: HashMap<String, ManagedProcess>,
    pub settings_store: Arc<SettingsStore>,
    pub spawner: Arc<dyn AgentSpawner>,
    pub health_interval: Duration,
    pub max_restart_before_backoff: u32,
    /// Processes that are being gracefully drained (PID, drain_start_time)
    pub draining_processes: Vec<(u32, Instant)>,
    /// System monitor for OOM detection
    pub sys: sysinfo::System,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Shared handle to the supervisor, allowing runtime updates from other parts of the proxy.
pub type SupervisorHandle = Arc<RwLock<ProcessSupervisor>>;

/// Smart startup cleanup: per-port health check → reuse healthy, kill unhealthy.
/// Then kill ALL orphaned agent processes by name pattern (architectural zombie prevention).
///
/// For each known A2A port, checks if a healthy server is already running:
/// - Healthy (agent-card responds) → reuse it (no cold restart needed)
/// - Port occupied but unhealthy → `fuser -k` that specific port
/// - Port free → nothing to do (supervisor/pre-spawn will handle)
///
/// After port cleanup, kills any leftover a2a-server/gemini processes from
/// previous proxy sessions that are no longer managed (orphan prevention).
pub fn startup_cleanup(kill_infra_ports: bool) {
    // All known ports: solo A2A + team, read from port_config
    let base_ports = crate::port_config::all_agent_ports();
    let infra = crate::port_config::infra_ports();
    let known_ports: Vec<u16> = if kill_infra_ports {
        base_ports.iter().chain(infra.iter()).copied().collect()
    } else {
        base_ports.to_vec()
    };

    // Parallel port cleanup — check all ports concurrently via scoped threads.
    // Each port check: TCP probe (50ms) + optional curl health (300ms) = ~350ms worst case.
    // Total: ~350ms instead of 7 × 1s = 7s sequential.
    let results: Vec<(u16, bool, bool)> = std::thread::scope(|s| {
        let infra_ref = &infra;
        let handles: Vec<_> = known_ports
            .iter()
            .map(|&port| {
                s.spawn(move || {
                    use std::net::ToSocketAddrs;
                    let addr_str = format!("localhost:{}", port);
                    let addr = match addr_str.to_socket_addrs().ok().and_then(|mut a| a.next()) {
                        Some(a) => a,
                        None => return (port, false, false),
                    };

                    let is_listening = std::net::TcpStream::connect_timeout(
                        &addr,
                        std::time::Duration::from_millis(50),
                    )
                    .is_ok();

                    if !is_listening {
                        return (port, false, false);
                    }

                    // Infrastructure ports (relay/screencast) are only considered stale
                    // when the daemon itself is starting. stdio proxy startups must not
                    // kill the long-lived relay daemon used by web review.
                    if kill_infra_ports && infra_ref.contains(&port) {
                        return (port, true, false); // daemon startup always reclaims infra ports
                    }

                    // Quick health check with reduced timeout (300ms instead of 1s)
                    let health_url = format!("http://localhost:{}/.well-known/agent.json", port);
                    let is_healthy = std::process::Command::new("curl")
                        .args(["-sf", "--max-time", "0.3", &health_url])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);

                    (port, true, is_healthy)
                })
            })
            .collect();

        handles
            .into_iter()
            .map(|h| h.join().unwrap_or((0, false, false)))
            .collect()
    });

    let mut reused = 0u16;
    let mut killed = 0u16;

    for (port, is_listening, is_healthy) in &results {
        if !is_listening {
            continue;
        }
        if *is_healthy {
            info!("[Supervisor] Port {} — healthy server found, reusing", port);
            reused += 1;
        } else {
            info!("[Supervisor] Port {} — unhealthy/stale, killing", port);
            kill_port_sync(*port);
            killed += 1;
        }
    }

    if killed > 0 {
        // Brief pause for killed ports to release
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    info!("[Supervisor] Startup: reused={}, killed={}", reused, killed);
}

/// Create a new supervisor and register solo A2A servers.
pub fn create_supervisor(settings_store: Arc<SettingsStore>) -> SupervisorHandle {
    create_supervisor_with_spawner(settings_store, Arc::new(crate::adapters::RealAgentSpawner))
}

/// Create a supervisor with a custom spawner (for testing).
pub fn create_supervisor_with_spawner(
    settings_store: Arc<SettingsStore>,
    spawner: Arc<dyn AgentSpawner>,
) -> SupervisorHandle {
    let settings = settings_store.get();

    let mut processes = HashMap::new();
    let is_team = settings.agent.team_mode;
    let primary_agent_id = infer_agent_id_from_command(&settings.agent.command);
    let is_codex = primary_agent_id == "codex" || is_ilhae_native_agent_id(&primary_agent_id);

    // Solo Gemini
    let gemini_port = {
        let ep = settings.agent.a2a_endpoint.trim();
        if ep.is_empty() {
            crate::port_config::gemini_a2a_port()
        } else {
            crate::parse_host_port(ep).1
        }
    };
    processes.insert(
        "gemini-solo".to_string(),
        ManagedProcess {
            name: "gemini-solo".to_string(),
            port: gemini_port,
            engine: "gemini".to_string(),
            card_name: None,
            pid: None,
            restart_count: 0,
            last_healthy: None,
            enabled: !is_team && !is_codex,
            workspace_path: None,
            role: None,
            is_leader: false,
            original_leader: false,
            cached_agent_card: None,
            card_last_fetched: None,
        },
    );

    // Solo Codex
    processes.insert(
        "codex-solo".to_string(),
        ManagedProcess {
            name: "codex-solo".to_string(),
            port: crate::port_config::codex_a2a_port(),
            engine: if is_codex {
                settings.agent.command.clone()
            } else {
                "codex".to_string()
            },
            card_name: None,
            pid: None,
            restart_count: 0,
            last_healthy: None,
            enabled: !is_team && is_codex,
            workspace_path: None,
            role: None,
            is_leader: false,
            original_leader: false,
            cached_agent_card: None,
            card_last_fetched: None,
        },
    );

    let supervisor = ProcessSupervisor {
        processes,
        settings_store,
        spawner,
        health_interval: Duration::from_secs(10),
        max_restart_before_backoff: 5,
        draining_processes: Vec::new(),
        sys: sysinfo::System::new_all(),
    };

    Arc::new(RwLock::new(supervisor))
}

/// Spawn the background health-check loop. Call once at startup.
/// If `shared_state` is provided, auto-restores the latest checkpoint into channel_memory
/// whenever a team agent is restarted after a crash.
pub fn spawn_supervisor_loop(
    handle: SupervisorHandle,
    shared_state: Option<std::sync::Arc<crate::SharedState>>,
) {
    tokio::spawn(async move {
        // Initial delay: let processes start up (increased to 10s for slower team agent boot)
        tokio::time::sleep(Duration::from_secs(10)).await;
        info!("[Supervisor] Health-check loop started");

        let mut cycle_count: u64 = 0;
        loop {
            let restarted_roles: Vec<String> = {
                let mut sv = handle.write().await;
                sv.check_and_restart().await
            };

            // Auto-Resume: if any team agent was restarted and we have SharedState,
            // restore the latest checkpoint into channel_memory
            if !restarted_roles.is_empty() {
                if let Some(ref state) = shared_state {
                    auto_restore_latest_checkpoint(state, &restarted_roles).await;
                }
            }

            // Periodic Agent Card discovery: every 6 cycles (~60s) when team mode is active
            cycle_count += 1;
            if cycle_count % 6 == 0 {
                let has_team = {
                    let sv = handle.read().await;
                    sv.processes.keys().any(|k| k.starts_with("team-"))
                };
                if has_team {
                    let updated = discover_agent_cards(&handle).await;
                    if updated > 0 {
                        info!(
                            "[Supervisor] Peer discovery: {} agent card(s) refreshed",
                            updated
                        );
                    }
                }
            }

            // Read interval while holding read lock briefly
            let interval = {
                let sv = handle.read().await;
                sv.health_interval
            };
            tokio::time::sleep(interval).await;
        }
    });
}

/// Register team A2A servers (called when team mode activates).
/// `workspace_map` provides the isolated workspace path per role — essential
/// for setting GEMINI_CLI_HOME / CODEX_HOME so each agent discovers its peer files.
pub async fn register_team_processes(
    handle: &SupervisorHandle,
    team_agents: &[(String, u16, String)], // (role, port, engine)
    workspace_map: &std::collections::HashMap<String, PathBuf>,
) {
    let mut sv = handle.write().await;

    // Remove old team entries
    sv.processes.retain(|k, _| !k.starts_with("team-"));

    // Disable solo processes in team mode
    for proc in sv.processes.values_mut() {
        if proc.name.ends_with("-solo") {
            proc.enabled = false;
        }
    }

    // Add team entries
    for (role, port, engine) in team_agents {
        let key = format!("team-{}", role.to_lowercase());
        let ws = workspace_map.get(&role.to_lowercase()).cloned();
        // Determine if this agent is the leader (first agent with "leader" in name,
        // or first agent if none is explicitly named leader)
        let is_leader_role =
            role.to_lowercase().contains("leader") || role.to_lowercase().contains("main");
        sv.processes.insert(
            key.clone(),
            ManagedProcess {
                name: key,
                port: *port,
                engine: engine.clone(),
                card_name: {
                    let label = crate::engine_env::resolve_engine_env(&engine)
                        .label()
                        .to_string();
                    Some(format!("{} ({})", role, label))
                },
                pid: None,
                restart_count: 0,
                last_healthy: None,
                enabled: true,
                workspace_path: ws,
                role: Some(role.clone()),
                is_leader: is_leader_role,
                original_leader: is_leader_role,
                cached_agent_card: None,
                card_last_fetched: None,
            },
        );
    }

    info!(
        "[Supervisor] Registered {} team processes",
        team_agents.len()
    );
}

/// Record a PID for a managed process identified by port.
/// Called after successfully spawning a process externally (e.g. from team_a2a or build_agent_transport).
pub async fn record_pid(handle: &SupervisorHandle, port: u16, pid: u32) {
    let mut sv = handle.write().await;
    for proc in sv.processes.values_mut() {
        if proc.port == port {
            info!(
                "[Supervisor] Recorded PID {} for {} (port {})",
                pid, proc.name, port
            );
            proc.pid = Some(pid);
            return;
        }
    }
    info!(
        "[Supervisor] No managed process on port {} to record PID {}",
        port, pid
    );
}

/// Switch back to solo mode: enable solo processes, remove team entries.
pub async fn switch_to_solo_mode(handle: &SupervisorHandle) {
    let mut sv = handle.write().await;

    // Kill all team processes by PID before removing
    for (key, proc) in sv.processes.iter() {
        if key.starts_with("team-") {
            if let Some(pid) = proc.pid {
                info!(
                    "[Supervisor] Killing team process {} (PID {})",
                    proc.name, pid
                );
                kill_pid(pid);
            }
        }
    }

    // Remove team entries
    sv.processes.retain(|k, _| !k.starts_with("team-"));

    // Re-enable solo processes
    for proc in sv.processes.values_mut() {
        if proc.name.ends_with("-solo") {
            proc.enabled = true;
            proc.restart_count = 0;
        }
    }

    info!("[Supervisor] Switched to solo mode");
}

/// Switch the active solo engine (e.g. gemini → codex) without proxy restart.
/// Kills the old engine's A2A server process and enables the new engine.
/// The supervisor health loop will auto-spawn the new engine on the next cycle.
pub async fn switch_solo_engine(handle: &SupervisorHandle, new_engine: &str) {
    let mut sv = handle.write().await;

    // Determine which solo process maps to the new engine
    let new_agent_id = infer_agent_id_from_command(new_engine);
    let new_key = if new_agent_id == "codex" || is_ilhae_native_agent_id(&new_agent_id) {
        "codex-solo"
    } else {
        "gemini-solo"
    };
    let old_key = if new_key == "codex-solo" {
        "gemini-solo"
    } else {
        "codex-solo"
    };

    // Check if engine actually changed
    let already_active = sv
        .processes
        .get(new_key)
        .map(|p| p.enabled)
        .unwrap_or(false);
    let current_engine = sv
        .processes
        .get(new_key)
        .map(|p| p.engine.clone())
        .unwrap_or_default();
    let engine_changed = current_engine != new_engine;

    if already_active && !engine_changed {
        info!(
            "[Supervisor] Solo engine '{}' already active, no switch needed",
            new_engine
        );
        return;
    }

    // If changing slot entirely (e.g. gemini -> codex), kill the old one
    if let Some(old_proc) = sv.processes.get(old_key) {
        if old_proc.enabled {
            if let Some(pid) = old_proc.pid {
                info!(
                    "[Supervisor] ⚡ Solo engine switch: killing '{}' (PID {}, port {})",
                    old_key, pid, old_proc.port
                );
                crate::process_supervisor::kill_pid(pid);
            } else {
                info!(
                    "[Supervisor] ⚡ Solo engine switch: killing port {} ({})",
                    old_proc.port, old_key
                );
                crate::process_supervisor::kill_port(old_proc.port);
            }
        }
    }

    // If changing variant WITHIN the same slot (e.g. codex -> codex-ilhae), kill the new_key's running process
    if already_active && engine_changed {
        if let Some(proc) = sv.processes.get(new_key) {
            if let Some(pid) = proc.pid {
                info!(
                    "[Supervisor] ⚡ Engine variant switch: killing PID {} for {}",
                    pid, new_key
                );
                crate::process_supervisor::kill_pid(pid);
            } else {
                crate::process_supervisor::kill_port(proc.port);
            }
        }
    }

    // Disable old, enable new
    if let Some(old_proc) = sv.processes.get_mut(old_key) {
        old_proc.enabled = false;
        old_proc.pid = None;
        old_proc.restart_count = 0;
    }
    if let Some(new_proc) = sv.processes.get_mut(new_key) {
        new_proc.enabled = true;
        new_proc.pid = None;
        new_proc.restart_count = 0;
        new_proc.last_healthy = None;
        new_proc.engine = new_engine.to_string();
    }

    info!(
        "[Supervisor] ⚡ Solo engine switched: {} → {} (will spawn on next health cycle)",
        old_key, new_key
    );
}

/// Get status of all managed processes (for admin API / debugging).
pub async fn get_status(handle: &SupervisorHandle) -> Vec<(String, u16, bool, bool, Option<u32>)> {
    let sv = handle.read().await;
    let mut results = Vec::new();
    for p in sv.processes.values() {
        let is_alive = sv.spawner.health_check(p.port).await;
        results.push((p.name.clone(), p.port, p.enabled, is_alive, p.pid));
    }
    results
}

/// Kill ALL managed processes. Call on app shutdown for clean exit.
pub async fn shutdown_all(handle: &SupervisorHandle) {
    let sv = handle.read().await;
    let mut killed = 0;
    for proc in sv.processes.values() {
        if let Some(pid) = proc.pid {
            info!(
                "[Supervisor] Shutdown: killing {} (PID {}, port {})",
                proc.name, pid, proc.port
            );
            kill_pid(pid);
            killed += 1;
        } else if proc.enabled {
            // No PID tracked, fall back to port-based kill
            info!(
                "[Supervisor] Shutdown: killing port {} ({})",
                proc.port, proc.name
            );
            kill_port(proc.port);
            killed += 1;
        }
    }
    info!(
        "[Supervisor] Shutdown complete: killed {} processes",
        killed
    );
}

// ── Dynamic Agent Card Discovery ─────────────────────────────────────────

/// Periodically fetch Agent Cards from all team endpoints and cache them.
/// Returns the count of agents whose cards changed (signals peer file refresh needed).
pub async fn discover_agent_cards(handle: &SupervisorHandle) -> usize {
    let team_endpoints: Vec<(String, String, u16)> = {
        let sv = handle.read().await;
        sv.processes
            .iter()
            .filter(|(k, p)| k.starts_with("team-") && p.enabled)
            .map(|(_, p)| {
                let role = p.role.clone().unwrap_or_default();
                let endpoint = format!("http://127.0.0.1:{}", p.port);
                (role, endpoint, p.port)
            })
            .collect()
    };

    if team_endpoints.is_empty() {
        return 0;
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return 0,
    };

    let mut updated = 0;

    for (role, endpoint, _port) in &team_endpoints {
        let card_url = format!("{}/.well-known/agent.json", endpoint);
        let card: serde_json::Value = match client
            .get(&card_url)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(res) if res.status().is_success() => match res.json().await {
                Ok(v) => v,
                Err(_) => continue,
            },
            _ => continue,
        };

        // Compare with cached card
        let changed = {
            let sv = handle.read().await;
            let key = format!("team-{}", role.to_lowercase());
            sv.processes
                .get(&key)
                .map(|p| p.cached_agent_card.as_ref() != Some(&card))
                .unwrap_or(false)
        };

        if changed {
            info!(
                "[Supervisor] Agent Card changed for '{}', caching update",
                role
            );
            let mut sv = handle.write().await;
            let key = format!("team-{}", role.to_lowercase());
            if let Some(proc) = sv.processes.get_mut(&key) {
                proc.cached_agent_card = Some(card);
                proc.card_last_fetched = Some(Instant::now());
            }
            updated += 1;
        }
    }

    updated
}

// ── Team Hot-Swap ────────────────────────────────────────────────────────────

/// Kill a process and its entire process group.
/// First tries SIGTERM on the process group (negative PID), then SIGKILL on the PID.
pub fn kill_pid(pid: u32) {
    // Kill entire process group (PGID = PID for spawned children via setsid/bash)
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &format!("-{}", pid)])
        .output();
    // Brief pause for graceful shutdown
    std::thread::sleep(std::time::Duration::from_millis(100));
    // Fallback: direct SIGKILL if group kill failed or process is stubborn
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output();
    // Also kill any orphaned children
    let _ = std::process::Command::new("pkill")
        .args(["-9", "-P", &pid.to_string()])
        .output();
}

/// Kill any process occupying the given TCP port (fallback when no PID is tracked).
pub fn kill_port(port: u16) {
    kill_port_sync(port);
}

/// Synchronous port-based kill with macOS/Linux compatibility.
/// Tries `fuser -k` first (Linux), falls back to `lsof -ti` + `kill -9` (macOS).
fn kill_port_sync(port: u16) {
    // Try fuser first (works on Linux, may not exist on macOS)
    let fuser_ok = std::process::Command::new("fuser")
        .args(["-k", &format!("{}/tcp", port)])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if fuser_ok {
        return;
    }

    // Fallback: lsof + kill (macOS compatible)
    if let Ok(output) = std::process::Command::new("lsof")
        .args(["-ti", &format!(":{}", port)])
        .output()
    {
        let pids = String::from_utf8_lossy(&output.stdout);
        for pid_str in pids.split_whitespace() {
            if let Ok(pid) = pid_str.parse::<u32>() {
                info!("[ZombieCleanup] Killing PID {} on port {}", pid, port);
                let _ = std::process::Command::new("kill")
                    .args(["-9", &pid.to_string()])
                    .output();
            }
        }
    }
}

/// Restart team A2A servers whose engine has changed.
/// Compares current supervisor entries against the new team config,
/// kills changed processes by PID, updates the registry, then lets
/// the health-check loop re-spawn with the new engine.
pub async fn restart_team_agents(
    handle: &SupervisorHandle,
    new_agents: &[(String, u16, String)], // (role, port, engine)
) {
    let mut pids_to_kill = Vec::new();
    let mut ports_to_kill = Vec::new();

    {
        let sv = handle.read().await;
        for (role, port, new_engine) in new_agents {
            let key = format!("team-{}", role.to_lowercase());
            if let Some(existing) = sv.processes.get(&key) {
                if existing.engine != *new_engine || existing.port != *port {
                    // Engine or port changed — need to kill old process
                    if let Some(pid) = existing.pid {
                        pids_to_kill.push((pid, existing.name.clone()));
                    } else {
                        ports_to_kill.push(existing.port);
                    }
                    if existing.port != *port {
                        ports_to_kill.push(*port);
                    }
                }
            } else {
                // New agent — kill target port defensively
                ports_to_kill.push(*port);
            }
        }
    }

    if pids_to_kill.is_empty() && ports_to_kill.is_empty() {
        info!("[Supervisor] No engine changes detected, skipping restart");
        return;
    }

    // Update registry FIRST — so if the supervisor health loop fires during
    // the kill/pause window, it will re-spawn with the NEW engine + card_name.
    // Re-use existing workspace_map from supervisor entries
    let ws_map: std::collections::HashMap<String, PathBuf> = {
        let sv = handle.read().await;
        sv.processes
            .iter()
            .filter_map(|(_, p)| {
                p.role.as_ref().and_then(|r| {
                    p.workspace_path
                        .as_ref()
                        .map(|ws| (r.to_lowercase(), ws.clone()))
                })
            })
            .collect()
    };
    register_team_processes(handle, new_agents, &ws_map).await;

    // Kill by PID (preferred — precise and fast)
    for (pid, name) in &pids_to_kill {
        info!(
            "[Supervisor] Killing {} (PID {}) for engine swap",
            name, pid
        );
        kill_pid(*pid);
    }

    // Kill by port (fallback when no PID was tracked)
    for port in &ports_to_kill {
        info!("[Supervisor] Killing port {} for engine swap", port);
        kill_port(*port);
    }

    // Brief pause for port release
    tokio::time::sleep(Duration::from_millis(500)).await;

    info!(
        "[Supervisor] Engine swap: {} PIDs + {} ports killed, re-spawn via health-check",
        pids_to_kill.len(),
        ports_to_kill.len()
    );
}

/// Force-restart a single managed team agent by role, even if the engine/port did not change.
/// Useful when the runtime must reload freshly generated peer files / tool registries.
pub async fn force_restart_team_role(
    handle: &SupervisorHandle,
    role: &str,
) -> anyhow::Result<bool> {
    let key = format!("team-{}", role.to_lowercase());
    let (name, port, engine, card_name, workspace_path, managed_role, pid, spawner) = {
        let sv = handle.read().await;
        let Some(proc) = sv.processes.get(&key) else {
            return Ok(false);
        };
        (
            proc.name.clone(),
            proc.port,
            proc.engine.clone(),
            proc.card_name.clone(),
            proc.workspace_path.clone(),
            proc.role.clone().unwrap_or_else(|| role.to_string()),
            proc.pid,
            sv.spawner.clone(),
        )
    };

    info!(
        "[Supervisor] Force restarting {} (role={}, port={})",
        name, managed_role, port
    );

    {
        let mut sv = handle.write().await;
        if let Some(proc) = sv.processes.get_mut(&key) {
            proc.pid = None;
            proc.restart_count = 0;
            proc.last_healthy = None;
        }
    }

    if let Some(pid) = pid {
        kill_pid(pid);
    } else {
        kill_port(port);
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    let team_env = workspace_path.map(|workspace_path| crate::TeamSpawnEnv {
        workspace_path,
        role: managed_role.clone(),
    });
    let spawned = spawner
        .spawn_agent(port, &engine, card_name.as_deref(), team_env)
        .await?;

    if let Some(pid) = spawned.pid {
        record_pid(handle, port, pid).await;
    }

    info!(
        "[Supervisor] Force restart complete: {} on port {} (pid={:?})",
        name, port, spawned.pid
    );
    Ok(true)
}

/// Ensure a specific managed process (identified by port) is healthy,
/// and if not, spawn it immediately and record its PID.
pub async fn ensure_agent_healthy(handle: &SupervisorHandle, port: u16) -> anyhow::Result<()> {
    // We need to look up the process config by port
    let (name, engine, card_name, workspace_path, role, spawner) = {
        let sv = handle.read().await;
        let Some(proc) = sv.processes.values().find(|p| p.port == port && p.enabled) else {
            anyhow::bail!("No enabled managed process found for port {}", port);
        };
        (
            proc.name.clone(),
            proc.engine.clone(),
            proc.card_name.clone(),
            proc.workspace_path.clone(),
            proc.role.clone(),
            sv.spawner.clone(),
        )
    };

    if spawner.health_check(port).await {
        return Ok(());
    }

    info!(
        "[Supervisor] ensure_agent_healthy: {} (port {}) is down, spawning...",
        name, port
    );

    // Defensive kill
    kill_port(port);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let team_env = workspace_path.map(|ws| crate::TeamSpawnEnv {
        workspace_path: ws,
        role: role.unwrap_or_default(),
    });

    let spawned = spawner
        .spawn_agent(port, &engine, card_name.as_deref(), team_env)
        .await?;

    if let Some(pid) = spawned.pid {
        record_pid(handle, port, pid).await;
    }

    // Quick wait
    tokio::time::sleep(Duration::from_millis(500)).await;
    if !spawner.health_check(port).await {
        warn!(
            "[Supervisor] ensure_agent_healthy: {} spawned but not healthy yet",
            name
        );
    } else {
        let mut sv = handle.write().await;
        if let Some(proc) = sv.processes.values_mut().find(|p| p.port == port) {
            proc.last_healthy = Some(Instant::now());
        }
    }

    Ok(())
}

// ── Internal ─────────────────────────────────────────────────────────────────

impl ProcessSupervisor {
    /// Check all enabled processes and restart any that are down.
    /// Returns a list of role names that were successfully restarted.
    /// Also handles leader failover: if the leader is down for 3+ restarts,
    /// promotes a healthy sub-agent as temporary leader.
    async fn check_and_restart(&mut self) -> Vec<String> {
        self.sys.refresh_all();

        let now = Instant::now();
        let mut i = 0;
        while i < self.draining_processes.len() {
            let (pid, drain_start) = self.draining_processes[i];
            if now.duration_since(drain_start) > Duration::from_secs(30) {
                info!(
                    "[Supervisor] Draining period expired for PID {}, killing now",
                    pid
                );
                kill_pid(pid);
                self.draining_processes.remove(i);
            } else {
                i += 1;
            }
        }

        let mut assigned_ports = std::collections::HashSet::new();
        for p in self.processes.values() {
            assigned_ports.insert(p.port);
        }

        let keys: Vec<String> = self.processes.keys().cloned().collect();
        let mut restarted_roles = Vec::new();

        for key in keys {
            let proc = match self.processes.get_mut(&key) {
                Some(p) => p,
                None => continue,
            };

            if !proc.enabled {
                continue;
            }

            let is_alive = self.spawner.health_check(proc.port).await;

            if is_alive {
                // Reset restart counter on healthy check
                if proc.restart_count > 0 {
                    info!(
                        "[Supervisor] {} recovered (port {}), resetting restart counter",
                        proc.name, proc.port
                    );
                    proc.restart_count = 0;
                }
                proc.last_healthy = Some(Instant::now());

                // OOM Detection & Proactive Migration
                if let Some(pid) = proc.pid {
                    if let Some(sys_proc) = self.sys.process(sysinfo::Pid::from_u32(pid)) {
                        let memory_usage = sys_proc.memory(); // in bytes
                        // OOM Threshold: 1.5GB
                        if memory_usage > 1_536 * 1024 * 1024 {
                            warn!(
                                "[Supervisor] ⚠️ OOM WARNING: {} (PID {}) memory usage {} bytes exceeds threshold! Initiating proactive migration...",
                                proc.name, pid, memory_usage
                            );

                            let mut new_port = 0;
                            for p in 50000..60000 {
                                if !assigned_ports.contains(&p) {
                                    if std::net::TcpListener::bind(("127.0.0.1", p)).is_ok() {
                                        new_port = p;
                                        break;
                                    }
                                }
                            }

                            if new_port > 0 {
                                info!(
                                    "[Supervisor] Spawning replacement for {} on new port {}...",
                                    proc.name, new_port
                                );
                                let team_env =
                                    proc.workspace_path.as_ref().map(|ws| crate::TeamSpawnEnv {
                                        workspace_path: ws.clone(),
                                        role: proc.role.clone().unwrap_or_default(),
                                    });

                                match self
                                    .spawner
                                    .spawn_agent(
                                        new_port,
                                        &proc.engine,
                                        proc.card_name.as_deref(),
                                        team_env,
                                    )
                                    .await
                                {
                                    Ok(agent) => {
                                        tokio::time::sleep(Duration::from_millis(1500)).await;
                                        if self.spawner.health_check(new_port).await {
                                            info!(
                                                "[Supervisor] 🔄 OOM Migration SUCCESS: {} swapped from port {} (PID {}) to port {} (PID {:?})",
                                                proc.name, proc.port, pid, new_port, agent.pid
                                            );
                                            self.draining_processes.push((pid, Instant::now()));
                                            assigned_ports.insert(new_port);
                                            proc.port = new_port;
                                            proc.pid = agent.pid;
                                        } else {
                                            warn!(
                                                "[Supervisor] New process on port {} failed health check. Migration aborted.",
                                                new_port
                                            );
                                            if let Some(new_pid) = agent.pid {
                                                kill_pid(new_pid);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!("[Supervisor] Failed to spawn OOM replacement: {}", e)
                                    }
                                }
                            }
                        }
                    }
                }

                continue;
            }

            // Check if process is still running but just hasn't opened port yet
            if let Some(pid) = proc.pid {
                let is_running = std::process::Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);

                if is_running {
                    // Process is alive but port is not bound yet. Give it more time to boot.
                    continue;
                }
            }

            // Process is down — clear stale PID
            proc.pid = None;

            // Apply backoff if needed
            if proc.restart_count >= self.max_restart_before_backoff {
                let backoff_secs =
                    30 * (2u64.pow((proc.restart_count - self.max_restart_before_backoff).min(4)));
                let last = proc.last_healthy.unwrap_or_else(Instant::now);
                if last.elapsed() < Duration::from_secs(backoff_secs) {
                    // Still in backoff period
                    continue;
                }
                warn!(
                    "[Supervisor] {} still down after {} restarts, backoff {}s elapsed — retrying",
                    proc.name, proc.restart_count, backoff_secs
                );

                // Codex Hooks Pattern: Execute external hook on severe crash/backoff
                let role_str = proc.role.clone().unwrap_or_else(|| proc.name.clone());
                tokio::spawn(async move {
                    execute_crash_hook(&role_str, None).await;
                });
            }

            // Attempt restart
            warn!(
                "[Supervisor] {} is DOWN (port {}), restarting (attempt #{})",
                proc.name,
                proc.port,
                proc.restart_count + 1
            );

            // Build team spawn env if this is a team process with a workspace
            let team_env = proc.workspace_path.as_ref().map(|ws| crate::TeamSpawnEnv {
                workspace_path: ws.clone(),
                role: proc.role.clone().unwrap_or_default(),
            });

            match self
                .spawner
                .spawn_agent(proc.port, &proc.engine, proc.card_name.as_deref(), team_env)
                .await
            {
                Ok(agent) => {
                    info!(
                        "[Supervisor] {} restarted on port {} (PID {:?})",
                        proc.name, proc.port, agent.pid
                    );
                    proc.pid = agent.pid;
                    proc.restart_count += 1;
                    // Track restarted role for Auto-Resume
                    if let Some(role) = proc.role.as_ref() {
                        restarted_roles.push(role.clone());
                    }
                    // Don't set last_healthy yet — wait for next probe to confirm
                }
                Err(e) => {
                    warn!(
                        "[Supervisor] Failed to restart {} on port {}: {}",
                        proc.name, proc.port, e
                    );
                    proc.restart_count += 1;
                }
            }
        }

        // ── Leader Failover ──────────────────────────────────────────────
        self.handle_leader_failover().await;

        restarted_roles
    }

    /// Leader failover logic:
    /// - If original leader is down for 3+ restarts → promote a healthy sub-agent
    /// - If original leader recovers → failback (restore original leader role)
    async fn handle_leader_failover(&mut self) {
        // Only applies to team processes
        let team_keys: Vec<String> = self
            .processes
            .keys()
            .filter(|k| k.starts_with("team-"))
            .cloned()
            .collect();
        if team_keys.is_empty() {
            return;
        }

        // Find the original leader
        let original_leader_key = team_keys
            .iter()
            .find(|k| {
                self.processes
                    .get(*k)
                    .map(|p| p.original_leader)
                    .unwrap_or(false)
            })
            .cloned();

        let Some(leader_key) = original_leader_key else {
            return;
        };

        let leader_down = {
            let leader = &self.processes[&leader_key];
            leader.restart_count >= 3 && !self.spawner.health_check(leader.port).await
        };

        let leader_is_healthy = {
            let leader = &self.processes[&leader_key];
            self.spawner.health_check(leader.port).await
        };

        if leader_down {
            // Check if we already promoted someone
            let already_promoted = team_keys.iter().any(|k| {
                k != &leader_key && self.processes.get(k).map(|p| p.is_leader).unwrap_or(false)
            });

            if already_promoted {
                return; // Already have a stand-in leader
            }

            // Find a healthy sub-agent to promote
            let candidate = team_keys
                .iter()
                .filter(|k| *k != &leader_key)
                .find(|k| {
                    let p = &self.processes[*k];
                    p.enabled
                        && p.last_healthy
                            .map(|t| t.elapsed() < Duration::from_secs(30))
                            .unwrap_or(false)
                })
                .cloned();

            if let Some(promoted_key) = candidate {
                let promoted_role = self.processes[&promoted_key]
                    .role
                    .clone()
                    .unwrap_or_default();
                warn!(
                    "[Supervisor] ⚠️ LEADER FAILOVER: original leader '{}' down for 3+ restarts, promoting '{}' as temporary leader",
                    leader_key, promoted_role
                );
                if let Some(proc) = self.processes.get_mut(&promoted_key) {
                    proc.is_leader = true;
                }
                // Mark original leader as no longer active leader
                if let Some(proc) = self.processes.get_mut(&leader_key) {
                    proc.is_leader = false;
                }
            } else {
                warn!(
                    "[Supervisor] Leader '{}' is down but no healthy sub-agent available for failover",
                    leader_key
                );
            }
        } else if leader_is_healthy {
            // Failback: original leader recovered, restore leadership
            let has_promoted = team_keys.iter().any(|k| {
                k != &leader_key && self.processes.get(k).map(|p| p.is_leader).unwrap_or(false)
            });

            if has_promoted {
                info!(
                    "[Supervisor] ✅ LEADER FAILBACK: original leader '{}' recovered, restoring leadership",
                    leader_key
                );
                // Demote the stand-in
                for k in &team_keys {
                    if k != &leader_key {
                        if let Some(proc) = self.processes.get_mut(k) {
                            if proc.is_leader {
                                proc.is_leader = false;
                                info!("[Supervisor] Demoted temporary leader '{}'", k);
                            }
                        }
                    }
                }
                // Restore original
                if let Some(proc) = self.processes.get_mut(&leader_key) {
                    proc.is_leader = true;
                }
            }
        }
    }

    /// Get the current active leader's endpoint info.
    /// Returns (role, port, engine) of the active leader.
    pub fn get_active_leader(&self) -> Option<(&str, u16, &str)> {
        self.processes
            .values()
            .find(|p| p.is_leader && p.enabled)
            .map(|p| {
                let role = p.role.as_deref().unwrap_or(&p.name);
                (role, p.port, p.engine.as_str())
            })
    }
}

/// Auto-Resume: Restore the latest checkpoint into channel_memory after a team agent restarts.
/// This implements the LangGraph pattern where the graph runner transparently resumes
/// from the last checkpoint without requiring agent-side intervention.
pub async fn auto_restore_latest_checkpoint(
    state: &std::sync::Arc<crate::SharedState>,
    restarted_roles: &[String],
) {
    let active_sid = state.sessions.active_session_id.read().await.clone();
    if active_sid.is_empty() {
        return;
    }

    // Find the latest checkpoint for this session
    match state
        .infra
        .brain
        .sessions()
        .list_checkpoints(&active_sid, "main", Some(1))
    {
        Ok(checkpoints) if !checkpoints.is_empty() => {
            let latest = &checkpoints[0];
            if let Ok(parsed) = serde_json::from_str::<
                std::collections::HashMap<String, serde_json::Value>,
            >(&latest.checkpoint_data)
            {
                let mut mem = state.team.channel_memory.write().await;
                *mem = parsed;
                info!(
                    "[Auto-Resume] Restored checkpoint '{}' into channel_memory after restart of: {:?}",
                    latest.version, restarted_roles
                );
            } else {
                warn!(
                    "[Auto-Resume] Failed to parse checkpoint data for version '{}'",
                    latest.version
                );
            }
        }
        Ok(_) => {
            info!(
                "[Auto-Resume] No checkpoints found for session '{}', nothing to restore",
                active_sid
            );
        }
        Err(e) => {
            warn!("[Auto-Resume] Error loading checkpoints: {}", e);
        }
    }
}
