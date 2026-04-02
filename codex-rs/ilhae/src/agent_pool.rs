//! AgentPool — Per-agent ACP monitor/compatibility pool.
//!
//! This pool is **not** the canonical southbound transport for team mode.
//! The canonical direct-target path now uses A2A from `context_proxy::prompt`.
//!
//! AgentPool remains useful for:
//! - subscribing to per-agent ACP/session updates for monitoring
//! - compatibility paths where a subagent intentionally exposes a stable ACP endpoint
//! - legacy/fallback experiments while the team stack converges on a single model
//!
//! ```text
//! ┌──────────┐     ┌───────────┐     ┌──────────────────────┐
//! │  Proxy   │────▶│ AgentPool │────▶│ Agent1 :4321 (ACP)   │
//! │ (compat) │     │           │────▶│ Agent2 :4322 (ACP)   │
//! │          │     │           │────▶│ Agent3 :4323 (ACP)   │
//! └──────────┘     └───────────┘     └──────────────────────┘
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc;
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::context_proxy::team_a2a::TeamRuntimeConfig;

// ─── Types ──────────────────────────────────────────────────────────────

/// A single agent's ACP connection state.
#[derive(Debug)]
pub struct AgentConnection {
    pub role: String,
    pub endpoint: String,
    /// Channel TX: send JSON-RPC messages TO the agent.
    pub tx: mpsc::UnboundedSender<Result<sacp::jsonrpcmsg::Message, sacp::Error>>,
    /// Channel RX: receive JSON-RPC messages FROM the agent (stored in Arc for cloning).
    pub rx: Arc<
        tokio::sync::Mutex<mpsc::UnboundedReceiver<Result<sacp::jsonrpcmsg::Message, sacp::Error>>>,
    >,
    /// ACP session ID obtained from session/new response.
    pub session_id: RwLock<Option<String>>,
    /// Background task handle for the HTTP POST + SSE bridge.
    pub _bridge_handle: tokio::task::JoinHandle<()>,
}

/// Pool of per-agent ACP connections used for monitoring/compatibility.
pub struct AgentPool {
    pub agents: RwLock<HashMap<String, Arc<AgentConnection>>>,
}

// ─── AgentPool Implementation ───────────────────────────────────────────

impl AgentPool {
    /// Create an empty pool.
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    /// Initialize ACP monitor connections to all agents in the team config.
    ///
    /// Use this when the proxy wants to observe per-agent `/acp/stream` notifications,
    /// or when intentionally running an ACP-direct compatibility path.
    /// This is not the canonical team delegation path.
    pub async fn init_from_team_config(&self, team: &TeamRuntimeConfig) {
        let mut pool = self.agents.write().await;
        for agent in &team.agents {
            let role_key = agent.role.to_lowercase();
            if pool.contains_key(&role_key) {
                info!("[AgentPool] {} already connected, skipping", role_key);
                continue;
            }

            let endpoint = agent.endpoint.trim_end_matches('/').to_string();
            let acp_endpoint = format!("{}/acp", endpoint);
            info!("[AgentPool] Connecting to {} at {}", role_key, acp_endpoint);

            match Self::create_agent_connection(&role_key, &acp_endpoint).await {
                Ok(conn) => {
                    pool.insert(role_key.clone(), Arc::new(conn));
                    info!("[AgentPool] {} connected successfully", role_key);
                }
                Err(e) => {
                    error!("[AgentPool] Failed to connect to {}: {}", role_key, e);
                }
            }
        }
    }

    /// Create a single agent connection with HTTP POST + SSE bridge.
    async fn create_agent_connection(
        role: &str,
        acp_endpoint: &str,
    ) -> Result<AgentConnection, String> {
        // Create two unbounded channels:
        // - outbound_tx/outbound_rx: messages FROM proxy TO agent (HTTP POST)
        // - inbound_tx/inbound_rx: messages FROM agent TO proxy (SSE)
        let (outbound_tx, outbound_rx) = mpsc::unbounded();
        let (inbound_tx, inbound_rx) = mpsc::unbounded();

        let endpoint = acp_endpoint.to_string();
        let role_owned = role.to_string();

        // Spawn the HTTP POST + SSE bridge (similar to AcpHttpAgent::into_channel_and_future)
        let bridge_handle = tokio::spawn(async move {
            Self::run_bridge(&role_owned, &endpoint, outbound_rx, inbound_tx).await;
        });

        Ok(AgentConnection {
            role: role.to_string(),
            endpoint: acp_endpoint.to_string(),
            tx: outbound_tx,
            rx: Arc::new(tokio::sync::Mutex::new(inbound_rx)),
            session_id: RwLock::new(None),
            _bridge_handle: bridge_handle,
        })
    }

    /// HTTP POST outbound + SSE inbound bridge for a single agent.
    async fn run_bridge(
        role: &str,
        endpoint: &str,
        mut outbound_rx: mpsc::UnboundedReceiver<Result<sacp::jsonrpcmsg::Message, sacp::Error>>,
        inbound_tx: mpsc::UnboundedSender<Result<sacp::jsonrpcmsg::Message, sacp::Error>>,
    ) {
        let http_client = reqwest::Client::new();

        // Start SSE listener in background
        let sse_endpoint = format!("{}/stream", endpoint);
        let sse_tx = inbound_tx.clone();
        let sse_role = role.to_string();
        let sse_handle = tokio::spawn(async move {
            Self::listen_sse(&sse_role, &sse_endpoint, &sse_tx).await;
        });

        debug!("[AgentPool:{}] Bridge started at {}", role, endpoint);

        // Forward outgoing messages as HTTP POST
        while let Some(msg_result) = outbound_rx.next().await {
            let msg = match msg_result {
                Ok(msg) => msg,
                Err(e) => {
                    warn!("[AgentPool:{}] Channel error: {}", role, e);
                    continue;
                }
            };

            let json = match serde_json::to_value(&msg) {
                Ok(j) => j,
                Err(e) => {
                    warn!("[AgentPool:{}] Serialize error: {}", role, e);
                    continue;
                }
            };

            debug!("[AgentPool:{}] → POST {}", role, endpoint);

            let mut retries = 0;
            let max_retries = 5;
            let mut backoff_ms = 500u64;

            loop {
                match http_client.post(endpoint).json(&json).send().await {
                    Ok(r) if r.status().is_success() => break,
                    Ok(r) => {
                        warn!("[AgentPool:{}] POST error: {}", role, r.status());
                        break;
                    }
                    Err(e) => {
                        if retries >= max_retries {
                            error!(
                                "[AgentPool:{}] POST failed after {} retries: {}",
                                role, max_retries, e
                            );
                            break;
                        }
                        warn!("[AgentPool:{}] POST retry in {}ms: {}", role, backoff_ms, e);
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                        retries += 1;
                        backoff_ms = (backoff_ms * 2).min(5000);
                    }
                }
            }
        }

        sse_handle.abort();
        debug!("[AgentPool:{}] Bridge shut down", role);
    }

    /// SSE listener for a single agent — reconnects on failure.
    async fn listen_sse(
        role: &str,
        sse_url: &str,
        tx: &mpsc::UnboundedSender<Result<sacp::jsonrpcmsg::Message, sacp::Error>>,
    ) {
        let mut backoff_ms: u64 = 100;
        const MAX_BACKOFF_MS: u64 = 2000;

        loop {
            debug!("[AgentPool:{}] SSE connecting to {}", role, sse_url);

            let resp = match reqwest::get(sse_url).await {
                Ok(r) if r.status().is_success() => r,
                Ok(r) => {
                    if r.status() == reqwest::StatusCode::NOT_FOUND {
                        info!(
                            "[AgentPool:{}] SSE endpoint not available at {} (404); disabling monitor stream",
                            role, sse_url
                        );
                        return;
                    }
                    warn!(
                        "[AgentPool:{}] SSE connect failed ({}), retry in {}ms",
                        role,
                        r.status(),
                        backoff_ms
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms + 200).min(MAX_BACKOFF_MS);
                    continue;
                }
                Err(e) => {
                    warn!(
                        "[AgentPool:{}] SSE connect error ({}), retry in {}ms",
                        role, e, backoff_ms
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms + 200).min(MAX_BACKOFF_MS);
                    continue;
                }
            };

            backoff_ms = 100;
            debug!("[AgentPool:{}] SSE connected", role);

            let mut buffer = String::new();
            let mut resp = resp;

            loop {
                match resp.chunk().await {
                    Ok(Some(chunk)) => {
                        buffer.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(end) = buffer.find("\n\n") {
                            let event_block = buffer[..end].to_string();
                            buffer = buffer[end + 2..].to_string();
                            for line in event_block.lines() {
                                if let Some(json_str) = line.strip_prefix("data: ") {
                                    let json_str = json_str.trim();
                                    if json_str.is_empty() {
                                        continue;
                                    }
                                    match serde_json::from_str::<sacp::jsonrpcmsg::Message>(
                                        json_str,
                                    ) {
                                        Ok(msg) => {
                                            if tx.unbounded_send(Ok(msg)).is_err() {
                                                return; // Channel closed
                                            }
                                        }
                                        Err(e) => {
                                            debug!(
                                                "[AgentPool:{}] SSE parse error: {} — {}",
                                                role, e, json_str
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        warn!("[AgentPool:{}] SSE stream ended, reconnecting", role);
                        break;
                    }
                    Err(e) => {
                        warn!("[AgentPool:{}] SSE read error: {}, reconnecting", role, e);
                        break;
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms = (backoff_ms + 200).min(MAX_BACKOFF_MS);
        }
    }

    // ─── ACP Protocol Methods ───────────────────────────────────────────

    /// Send a raw JSON-RPC message to an agent and wait for a response.
    pub async fn send_rpc(&self, role: &str, request: Value) -> Result<Value, String> {
        let pool = self.agents.read().await;
        let conn = pool
            .get(role)
            .ok_or_else(|| format!("Agent '{}' not in pool", role))?
            .clone();
        drop(pool);

        let request_id = request
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                request
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .map(|n| n.to_string())
            })
            .ok_or_else(|| "Request missing 'id' field".to_string())?;

        // Serialize and send
        let msg: sacp::jsonrpcmsg::Message = serde_json::from_value(request.clone())
            .map_err(|e| format!("Invalid JSON-RPC message: {}", e))?;

        conn.tx
            .unbounded_send(Ok(msg))
            .map_err(|e| format!("Failed to send to {}: {}", role, e))?;

        // Wait for matching response
        let mut rx = conn.rx.lock().await;
        let timeout = Duration::from_secs(30);
        let start = std::time::Instant::now();

        loop {
            match tokio::time::timeout(Duration::from_millis(100), rx.next()).await {
                Ok(Some(Ok(response_msg))) => {
                    let response_val = serde_json::to_value(&response_msg)
                        .map_err(|e| format!("Serialize response: {}", e))?;
                    let resp_id = response_val
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            response_val
                                .get("id")
                                .and_then(|v| v.as_u64())
                                .map(|n| n.to_string())
                        });

                    if resp_id.as_deref() == Some(&request_id) {
                        return Ok(response_val);
                    }
                    // Not our response — this could be a notification, log it
                    debug!(
                        "[AgentPool:{}] Non-matching message (id={:?})",
                        role, resp_id
                    );
                }
                Ok(Some(Err(e))) => {
                    warn!("[AgentPool:{}] Channel error: {}", role, e);
                }
                Ok(None) => {
                    return Err(format!("Agent '{}' channel closed", role));
                }
                Err(_) => {
                    // Timeout on this iteration — check overall timeout
                }
            }

            if start.elapsed() >= timeout {
                return Err(format!("RPC timeout waiting for response from '{}'", role));
            }
        }
    }

    /// Initialize ACP on a specific agent: send `initialize` request.
    pub async fn initialize_agent(&self, role: &str) -> Result<Value, String> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": uuid::Uuid::new_v4().to_string(),
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "clientInfo": {
                    "name": "ilhae-proxy-pool",
                    "version": "0.1.0"
                },
                "clientCapabilities": {}
            }
        });
        info!("[AgentPool] Initializing ACP for '{}'", role);
        self.send_rpc(role, request).await
    }

    /// Create a new ACP session on a specific agent.
    ///
    /// Compatibility helper only. Prefer A2A southbound for canonical team paths.
    pub async fn create_session(&self, role: &str, cwd: &str) -> Result<String, String> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": uuid::Uuid::new_v4().to_string(),
            "method": "session/new",
            "params": {
                "cwd": cwd,
                "mcpServers": []
            }
        });
        info!("[AgentPool] Creating session for '{}' (cwd={})", role, cwd);
        let response = self.send_rpc(role, request).await?;

        let session_id = response
            .get("result")
            .and_then(|r| r.get("sessionId"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("No sessionId in session/new response for '{}'", role))?
            .to_string();

        // Store the session ID
        let pool = self.agents.read().await;
        if let Some(conn) = pool.get(role) {
            let mut sid = conn.session_id.write().await;
            *sid = Some(session_id.clone());
        }

        info!("[AgentPool] Session created for '{}': {}", role, session_id);
        Ok(session_id)
    }

    /// Send a prompt to a specific agent via ACP `session/prompt`.
    ///
    /// Compatibility helper only. Prefer A2A southbound for canonical team paths.
    pub async fn send_prompt(&self, role: &str, prompt_text: &str) -> Result<Value, String> {
        let pool = self.agents.read().await;
        let conn = pool
            .get(role)
            .ok_or_else(|| format!("Agent '{}' not in pool", role))?
            .clone();
        drop(pool);

        let session_id = {
            let sid = conn.session_id.read().await;
            sid.clone()
                .ok_or_else(|| format!("No session for '{}'. Call create_session first.", role))?
        };

        let request = json!({
            "jsonrpc": "2.0",
            "id": uuid::Uuid::new_v4().to_string(),
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [
                    { "type": "text", "text": prompt_text }
                ]
            }
        });

        info!(
            "[AgentPool] Sending prompt to '{}' (session={}, len={})",
            role,
            session_id,
            prompt_text.len()
        );
        self.send_rpc(role, request).await
    }

    /// Subscribe to all session/update notifications from a specific agent.
    ///
    /// This is the main remaining canonical responsibility of `AgentPool`:
    /// monitoring/subscribing to agent ACP update streams for proxy-side observation.
    pub async fn subscribe_updates(
        &self,
        role: &str,
    ) -> Result<mpsc::UnboundedReceiver<Value>, String> {
        let pool = self.agents.read().await;
        let conn = pool
            .get(role)
            .ok_or_else(|| format!("Agent '{}' not in pool", role))?
            .clone();
        drop(pool);

        let (notify_tx, notify_rx) = mpsc::unbounded();
        let rx = conn.rx.clone();
        let role_owned = role.to_string();

        tokio::spawn(async move {
            loop {
                let msg = {
                    let mut guard = rx.lock().await;
                    guard.next().await
                };

                match msg {
                    Some(Ok(msg)) => {
                        let val = match serde_json::to_value(&msg) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        // Check if this is a notification (no "id" field or method is agent_client_protocol_schema::CLIENT_METHOD_NAMES.session_update)
                        let method = val.get("method").and_then(|v| v.as_str()).unwrap_or("");
                        if method.starts_with("session/")
                            || method.starts_with("ilhae/")
                            || val.get("id").is_none()
                        {
                            if notify_tx.unbounded_send(val).is_err() {
                                break; // Receiver dropped
                            }
                        }
                    }
                    Some(Err(e)) => {
                        warn!("[AgentPool:{}] Update stream error: {}", role_owned, e);
                    }
                    None => {
                        info!("[AgentPool:{}] Update stream ended", role_owned);
                        break;
                    }
                }
            }
        });

        Ok(notify_rx)
    }

    /// Get a list of connected agent roles.
    pub async fn connected_roles(&self) -> Vec<String> {
        self.agents.read().await.keys().cloned().collect()
    }

    /// Check if a specific agent is connected.
    pub async fn has_agent(&self, role: &str) -> bool {
        self.agents.read().await.contains_key(role)
    }

    /// Get the session ID for a specific agent, if one has been created.
    pub async fn get_session_id(&self, role: &str) -> Option<String> {
        let pool = self.agents.read().await;
        if let Some(conn) = pool.get(role) {
            conn.session_id.read().await.clone()
        } else {
            None
        }
    }
}

impl std::fmt::Debug for AgentPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPool")
            .field("agents", &"<locked>")
            .finish()
    }
}
