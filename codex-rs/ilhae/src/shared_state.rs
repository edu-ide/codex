use moka::sync::Cache;
use sacp::{ConnectionTo, role::Role};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicU64};
use tokio::sync::RwLock;

use crate::AssistantBuffer;
use crate::CxCache;
use crate::agent_pool;
use crate::approval_manager;
use crate::browser_manager::BrowserManager;
use crate::context_proxy;
use crate::mcp_manager::McpManager;
use crate::notification_store;
use crate::process_supervisor;
use crate::relay_server::{RelayEvent, RelayState};
use crate::settings_store::SettingsStore;
use brain_rs::BrainService;

/// Signal from pre-spawn to build_agent_transport: leader has been spawned.
pub static LEADER_READY: tokio::sync::Notify = tokio::sync::Notify::const_new();

// ── Sub-State: Per-session tracking maps ─────────────────────────────────

/// Per-session state and lifecycle tracking.
#[derive(Clone)]
pub struct SessionState {
    /// Per-session: last instructions version that was injected.
    pub instructions_ver: Arc<Cache<String, u64>>,
    /// Per-session: latest cancel_version observed for this session.
    pub cancel_ver: Arc<Cache<String, u64>>,
    /// Per-session monotonic assistant turn sequence (incremented on each PromptRequest).
    pub turn_seq: Arc<Cache<String, u64>>,
    /// ACP session ID → internal session ID mapping.
    pub id_map: Arc<Cache<String, String>>,
    /// Internal session ID → ACP session ID (reverse lookup).
    pub reverse_map: Arc<Cache<String, String>>,
    /// Per-session desired delegation mode hint ("sync" | "background" | "subscribe").
    pub delegation_mode: Arc<Cache<String, String>>,
    /// Per-session MCP servers injected via session/new (used for A2A delegation).
    pub mcp_servers: Arc<Cache<String, Vec<agent_client_protocol_schema::McpServer>>>,
    /// Per-session assistant scratch buffers.
    pub assistant_buffers: Arc<Cache<String, AssistantBuffer>>,
    /// Global per-process counter for session-local instructions version updates.
    pub instructions_version: Arc<AtomicU64>,
    /// Global per-process counter for cancel notifications.
    pub cancel_version: Arc<AtomicU64>,
    /// Client session_id → internal session_id mapping for quick context lookups.
    pub pending_history: Arc<Cache<String, String>>,
    /// Host connection key → internal session_id mapping for tool calls.
    pub connection_sessions: Arc<Cache<String, String>>,
    /// Currently active session ID (set when chat starts, used by MCP tools like artifact_save).
    pub active_session_id: Arc<RwLock<String>>, // active_session_id remains RwLock as it's a single value
    /// Per-session autonomous execution state (running / approval break / done).
    pub autonomous_sessions:
        Arc<Cache<String, context_proxy::autonomy::state::AutonomousSessionState>>,
}

impl SessionState {
    pub fn new() -> Self {
        fn build_cache<K, V>() -> Arc<Cache<K, V>>
        where
            K: std::hash::Hash + Eq + Send + Sync + 'static,
            V: Clone + Send + Sync + 'static,
        {
            Arc::new(
                Cache::builder()
                    .time_to_idle(std::time::Duration::from_secs(3600))
                    .build(),
            )
        }
        Self {
            instructions_ver: build_cache(),
            cancel_ver: build_cache(),
            turn_seq: build_cache(),
            id_map: build_cache(),
            reverse_map: build_cache(),
            delegation_mode: build_cache(),
            mcp_servers: build_cache(),
            assistant_buffers: build_cache(),
            instructions_version: Arc::new(AtomicU64::new(1)),
            cancel_version: Arc::new(AtomicU64::new(0)),
            pending_history: build_cache(),
            connection_sessions: build_cache(),
            active_session_id: Arc::new(RwLock::new(String::new())),
            autonomous_sessions: build_cache(),
        }
    }
}

pub fn connection_key<Counterpart: Role>(cx: &ConnectionTo<Counterpart>) -> String {
    format!("{cx:?}")
}

// ── Sub-State: Team orchestration ────────────────────────────────────────

/// Groups team/supervisor concerns (process management, agent pool, metrics).
#[derive(Clone)]
pub struct TeamState {
    /// Process supervisor for agent lifecycle management.
    pub supervisor: process_supervisor::SupervisorHandle,
    /// Per-agent ACP connection pool for hybrid group chat (Method C).
    pub agent_pool: Arc<agent_pool::AgentPool>,
    /// A2A proxy routing map (team mode only). Updated on team config changes.
    pub a2a_routing_map: Option<crate::a2a_persistence::RoutingMap>,
    /// Centralized delegation metrics for observability.
    pub delegation_metrics: crate::process_supervisor::MetricsHandle,
    /// Team communication channel (broadcast, progress, handoff).
    pub comms: TeamCommsChannel,
    /// Session-scoped channel memory (LangGraph-like checkpointed context for team flow).
    pub channel_memory: Arc<RwLock<HashMap<String, HashMap<String, serde_json::Value>>>>,
    /// Delegation event bus channel transmitter for persistence decoupling.
    pub event_tx: tokio::sync::broadcast::Sender<crate::a2a_persistence::events::DelegationEvent>,
    /// Agent process spawner (production uses RealAgentSpawner).
    pub agent_spawner: Arc<dyn crate::ports::AgentSpawner>,
}

// ── Sub-State: Infra/persistence/service handles ────────────────────────

#[derive(Clone)]
pub struct InfraContext {
    /// Unified brain service layer (memory, session, knowledge, artifacts, schedules).
    pub brain: Arc<BrainService>,
    /// Settings store source of truth used by multiple built-ins and proxies.
    pub settings_store: Arc<SettingsStore>,
    /// Browser lifecycle + tool runtime wrapper.
    pub browser_mgr: Arc<BrowserManager>,
    /// MCP manager + connected server catalog.
    pub mcp_mgr: Arc<McpManager>,
    /// Notification persistence handle.
    pub notification_store: Arc<notification_store::NotificationStore>,
    /// Relay state to fanout status/events.
    pub relay_state: Arc<RelayState>,
    pub relay_tx: tokio::sync::mpsc::Sender<RelayEvent>,
    /// Engine-side cache used by context/transport glue.
    pub relay_conductor_cx: CxCache,
    /// Cross-channel approval coordinator.
    pub approval_manager: Arc<approval_manager::ApprovalManager>,
    /// Data directory path for ilhae (used by relay commands for context files).
    pub ilhae_dir: PathBuf,
    /// ACP terminal lifecycle manager.
    pub terminal_manager: Arc<context_proxy::terminal_handlers::TerminalManager>,
    /// Cached configOptions from the last ACP session/new response.
    pub cached_config_options: Arc<RwLock<Vec<serde_json::Value>>>,
    /// Shared task pool for unassigned schedules (claim-based delegation).
    pub shared_task_pool: Arc<RwLock<Vec<crate::types::SharedTaskDto>>>,
    /// Triggers agent refresh for pre-spawn/team updates.
    pub agent_refresh_tx: tokio::sync::mpsc::UnboundedSender<()>,
}

// ── SharedState ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SharedState {
    // ── Grouped sub-states ──
    /// Session context (session map tracking + session/lifecycle counters).
    pub sessions: SessionState,
    /// Team orchestration (supervisor, agent pool, routing, metrics, comms).
    pub team: TeamState,
    /// Infra/persistence/service context (brain, settings, browser, mcp, relay, approvals).
    pub infra: InfraContext,
}

impl SharedState {
    /// Convenience accessor used during gradual migration.
    pub fn session_state(&self) -> &SessionState {
        &self.sessions
    }

    /// Convenience accessor used during gradual migration.
    pub fn team_state(&self) -> &TeamState {
        &self.team
    }

    /// Convenience accessor used during gradual migration.
    pub fn infra_context(&self) -> &InfraContext {
        &self.infra
    }
}

// ── Team Communication Channel ───────────────────────────────────────────

/// Structured handoff payload for agent-to-agent work transfer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HandoffPayload {
    /// Source agent role
    pub from: String,
    /// Target agent role
    pub to: String,
    /// Summary of work done so far
    pub summary: String,
    /// Key findings / data to hand off (structured)
    #[serde(default)]
    pub findings: Vec<String>,
    /// References (URLs, file paths, session IDs)
    #[serde(default)]
    pub references: Vec<String>,
    /// Specific instructions for the receiving agent
    pub instructions: String,
    /// Priority: "low" | "normal" | "high" | "urgent"
    #[serde(default = "default_priority")]
    pub priority: String,
}

fn default_priority() -> String {
    "normal".to_string()
}

/// Team communication event (broadcast to all listeners).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TeamEvent {
    /// Broadcast message from leader to all agents
    Broadcast { from: String, message: String },
    /// Progress report from sub-agent to leader
    Progress {
        from: String,
        percent: u8,
        status: String,
        details: Option<String>,
    },
    /// Structured handoff between agents
    Handoff(HandoffPayload),
}

/// Team-wide communication channel.
#[derive(Clone)]
pub struct TeamCommsChannel {
    tx: Arc<tokio::sync::broadcast::Sender<TeamEvent>>,
    /// Persisted event log (last N events)
    log: Arc<RwLock<Vec<(String, TeamEvent)>>>,
}

impl TeamCommsChannel {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(64);
        Self {
            tx: Arc::new(tx),
            log: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Publish an event to all listeners.
    pub async fn publish(&self, event: TeamEvent) {
        let ts = chrono::Utc::now().to_rfc3339();
        let mut log = self.log.write().await;
        log.push((ts, event.clone()));
        // Keep last 100 events
        if log.len() > 100 {
            let drain = log.len() - 100;
            log.drain(..drain);
        }
        let _ = self.tx.send(event);
    }

    /// Subscribe to events.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<TeamEvent> {
        self.tx.subscribe()
    }

    /// Get recent event log.
    pub async fn recent_events(&self, limit: usize) -> Vec<(String, TeamEvent)> {
        let log = self.log.read().await;
        log.iter().rev().take(limit).cloned().collect()
    }
}

// ── Delegation Event Bus (inspired by CrewAI crewai_event_bus) ───────────

/// Delegation lifecycle event.
#[derive(Debug, Clone, serde::Serialize)]
pub enum DelegationEvent {
    /// Delegation started — emitted before A2A call
    Started {
        trace_id: String,
        from_agent: String,
        to_agent: String,
        endpoint: String,
        task_description: String,
        context_id: Option<String>,
    },
    /// Delegation completed successfully
    Completed {
        trace_id: String,
        from_agent: String,
        to_agent: String,
        duration_ms: u64,
        result_preview: String,
    },
    /// Delegation failed
    Failed {
        trace_id: String,
        from_agent: String,
        to_agent: String,
        error: String,
        duration_ms: u64,
        /// Whether circuit breaker tripped
        circuit_opened: bool,
    },
    /// Content-type negotiation result
    ContentNegotiated {
        agent: String,
        client_modes: Vec<String>,
        server_modes: Vec<String>,
        compatible: bool,
    },
}

/// Event bus for delegation lifecycle (fan-out to all listeners).
#[derive(Clone)]
pub struct DelegationEventBus {
    tx: Arc<tokio::sync::broadcast::Sender<DelegationEvent>>,
}

impl DelegationEventBus {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(128);
        Self { tx: Arc::new(tx) }
    }

    /// Emit a delegation event.
    pub fn emit(&self, event: DelegationEvent) {
        let _ = self.tx.send(event);
    }

    /// Subscribe to delegation events.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<DelegationEvent> {
        self.tx.subscribe()
    }
}

// ── Content-Type Negotiation (inspired by CrewAI content_type.py) ────────

/// Check if two MIME type lists have any compatible types.
/// Supports wildcards: `image/*` matches `image/png`.
pub fn content_types_compatible(client_modes: &[String], server_modes: &[String]) -> Vec<String> {
    let mut compatible = Vec::new();
    for cm in client_modes {
        for sm in server_modes {
            if mime_compatible(cm, sm) {
                if sm.contains('*') && !cm.contains('*') {
                    if !compatible.contains(cm) {
                        compatible.push(cm.clone());
                    }
                } else {
                    if !compatible.contains(sm) {
                        compatible.push(sm.clone());
                    }
                }
                break;
            }
        }
    }
    compatible
}

fn mime_compatible(a: &str, b: &str) -> bool {
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    if a == b {
        return true;
    }
    let ap: Vec<&str> = a.split('/').collect();
    let bp: Vec<&str> = b.split('/').collect();
    if ap.len() == 2 && bp.len() == 2 {
        let type_ok = ap[0] == bp[0] || ap[0] == "*" || bp[0] == "*";
        let sub_ok = ap[1] == bp[1] || ap[1] == "*" || bp[1] == "*";
        return type_ok && sub_ok;
    }
    false
}

// ── Delegation Middleware (inspired by CrewAI A2AExtension protocol) ─────

/// Middleware hook for delegation pre/post-processing.
/// Implement this trait to add custom logic around delegations.
#[async_trait::async_trait]
pub trait DelegationMiddleware: Send + Sync {
    /// Called before delegation — can modify the message or reject it.
    async fn pre_delegate(
        &self,
        target: &str,
        message: &str,
        metadata: &mut serde_json::Value,
    ) -> Result<(), String> {
        let _ = (target, message, metadata);
        Ok(())
    }

    /// Called after successful delegation — can transform the response.
    async fn post_delegate(
        &self,
        target: &str,
        response: String,
        metadata: &serde_json::Value,
    ) -> String {
        let _ = (target, metadata);
        response
    }

    /// Called on delegation failure — can decide retry or fallback.
    async fn on_failure(&self, target: &str, error: &str, attempt: u32) -> MiddlewareAction {
        let _ = (target, error, attempt);
        MiddlewareAction::Propagate
    }
}

/// Action to take after middleware processes a failure.
#[derive(Debug, Clone)]
pub enum MiddlewareAction {
    /// Propagate the error as-is
    Propagate,
    /// Retry the delegation
    Retry,
    /// Use a fallback response
    Fallback(String),
}

/// Registry of delegation middleware (executed in order).
pub struct MiddlewareRegistry {
    middlewares: Vec<Box<dyn DelegationMiddleware>>,
}

impl MiddlewareRegistry {
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    pub fn register(&mut self, mw: Box<dyn DelegationMiddleware>) {
        self.middlewares.push(mw);
    }

    pub async fn run_pre(
        &self,
        target: &str,
        message: &str,
        metadata: &mut serde_json::Value,
    ) -> Result<(), String> {
        for mw in &self.middlewares {
            mw.pre_delegate(target, message, metadata).await?;
        }
        Ok(())
    }

    pub async fn run_post(
        &self,
        target: &str,
        response: String,
        metadata: &serde_json::Value,
    ) -> String {
        let mut resp = response;
        for mw in &self.middlewares {
            resp = mw.post_delegate(target, resp, metadata).await;
        }
        resp
    }

    pub async fn run_on_failure(
        &self,
        target: &str,
        error: &str,
        attempt: u32,
    ) -> MiddlewareAction {
        for mw in &self.middlewares {
            let action = mw.on_failure(target, error, attempt).await;
            if !matches!(action, MiddlewareAction::Propagate) {
                return action;
            }
        }
        MiddlewareAction::Propagate
    }
}
