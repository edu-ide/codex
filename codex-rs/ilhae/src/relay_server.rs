//! Relay WebSocket server for mobile client communication.
//! Broadcasts session events and handles commands from mobile clients.
//! Supports request-response pattern via `request_id` for task CRUD.
//!
//! Also serves ACP-over-WS bridge on path `/acp` for Desktop daemon integration.
//! Path `/acp` → raw SACP NDJSON (Desktop's AcpWebSocketTransport)
//! All other paths → RelayCommand JSON protocol (CLI, Telegram, mobile)

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use crate::session_store::SessionStore;
use brain_rs::schedule::{ScheduleChangeEvent, ScheduleStore};

/// Events broadcast from proxy to mobile clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RelayEvent {
    #[serde(rename = "session_notification")]
    SessionNotification {
        session_id: String,
        update: serde_json::Value,
    },
    #[serde(rename = "browser_activity")]
    BrowserActivity {
        session_id: String,
        tool_name: String,
    },
    #[serde(rename = "settings_changed")]
    SettingsChanged {
        key: String,
        value: serde_json::Value,
    },
    #[serde(rename = "browser_status")]
    BrowserStatus {
        running: bool,
        pid: Option<u32>,
        port: Option<u16>,
        browser_type: String,
    },
    #[serde(rename = "schedules_changed")]
    TasksChanged { event: ScheduleChangeEvent },
    /// Response to a RelayCommand that included a request_id.
    #[serde(rename = "command_response")]
    CommandResponse {
        request_id: String,
        result: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// UI notification from agent via ui_notify tool.
    #[serde(rename = "ui_notification")]
    UiNotification { message: String, level: String },
    /// Permission request forwarded to Telegram for user approval.
    #[serde(rename = "permission_request")]
    PermissionRequest {
        /// Unique ID for correlating the response back to the proxy.
        permission_id: String,
        session_id: String,
        tool_title: String,
        tool_kind: String,
        /// Human-readable summary of what the tool wants to do.
        description: String,
        /// Available options: [{id, title}]
        options: Vec<serde_json::Value>,
    },
    /// User message sent from a relay channel (e.g. Telegram) to an agent session.
    #[serde(rename = "user_message")]
    UserMessage {
        session_id: String,
        text: String,
        channel_id: String,
        timestamp: String,
    },
}

/// Commands received from mobile clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayCommand {
    pub action: String,
    pub payload: serde_json::Value,
    /// Optional request ID for request-response pattern.
    #[serde(default)]
    pub request_id: Option<String>,
}

/// Internal: command + originating client id for targeted response.
#[derive(Debug)]
pub struct RelayCommandWithClient {
    pub cmd: RelayCommand,
    pub client_id: u64,
}

const RELAY_EVENT_QUEUE_CAPACITY: usize = 8192;
const RELAY_CLIENT_QUEUE_CAPACITY: usize = 512;
const RELAY_CLIENT_MAX_DROPPED_EVENTS: u32 = 64;

pub struct RelayClient {
    pub tx: mpsc::Sender<Arc<str>>,
    pub dropped_events: std::sync::atomic::AtomicU32,
}

/// Shared state for the relay server.
#[allow(dead_code)]
pub struct RelayState {
    pub store: Arc<SessionStore>,
    pub schedule_store: Arc<ScheduleStore>,
    pub command_tx: mpsc::Sender<RelayCommandWithClient>,
    /// Connected WebSocket clients: id → sender
    pub clients: RwLock<HashMap<u64, Arc<RelayClient>>>,
    pub next_id: std::sync::atomic::AtomicU64,
    /// Optional Telegram event channel (for forwarding events to the Telegram bot)
    pub telegram_tx: RwLock<Option<mpsc::Sender<String>>>,
    /// Handle to the Telegram bot task (for abort on reload)
    pub telegram_handle: RwLock<Option<tokio::task::JoinHandle<()>>>,
    /// ACP-over-WS bridge (set in daemon mode for Desktop integration)
    pub acp_bridge: RwLock<Option<Arc<crate::acp_ws_server::AcpWsBridge>>>,
}

impl RelayState {
    pub fn new(
        store: Arc<SessionStore>,
        schedule_store: Arc<ScheduleStore>,
        command_tx: mpsc::Sender<RelayCommandWithClient>,
    ) -> (Arc<Self>, mpsc::Sender<RelayEvent>) {
        let (relay_tx, relay_rx) = mpsc::channel(RELAY_EVENT_QUEUE_CAPACITY);
        let state = Arc::new(Self {
            store,
            schedule_store,
            command_tx,
            clients: RwLock::new(HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
            telegram_tx: RwLock::new(None),
            telegram_handle: RwLock::new(None),
            acp_bridge: RwLock::new(None),
        });

        // Spawn broadcast task: relay_rx → all connected clients + telegram
        let state_clone = state.clone();
        tokio::spawn(async move {
            let mut rx = relay_rx;
            while let Some(event) = rx.recv().await {
                if let Ok(json) = serde_json::to_string(&event) {
                    state_clone.broadcast(&json).await;
                    // Also forward to Telegram if connected
                    if let Some(tx) = state_clone.telegram_tx.read().await.as_ref() {
                        let _ = tx.try_send(json.clone());
                    }
                }
            }
        });

        (state, relay_tx)
    }

    /// Register the Telegram event forwarder channel.
    pub async fn set_telegram_tx(&self, tx: mpsc::Sender<String>) {
        *self.telegram_tx.write().await = Some(tx);
    }

    /// Store the Telegram bot task handle for later abort.
    pub async fn set_telegram_handle(&self, handle: tokio::task::JoinHandle<()>) {
        *self.telegram_handle.write().await = Some(handle);
    }

    /// Stop the Telegram bot: abort the task and clear the event channel.
    pub async fn clear_telegram(&self) {
        if let Some(handle) = self.telegram_handle.write().await.take() {
            handle.abort();
        }
        *self.telegram_tx.write().await = None;
    }

    /// Set the ACP-over-WS bridge (daemon mode). Enables `/acp` routing.
    pub async fn set_acp_bridge(&self, bridge: Arc<crate::acp_ws_server::AcpWsBridge>) {
        *self.acp_bridge.write().await = Some(bridge);
    }

    /// Returns true when at least one relay websocket client is connected.
    pub async fn has_connected_clients(&self) -> bool {
        !self.clients.read().await.is_empty()
    }

    /// Returns true when Telegram bridge channel is active.
    pub async fn has_telegram_bridge(&self) -> bool {
        self.telegram_tx.read().await.is_some()
    }

    /// Send a message to a specific client by id.
    pub async fn send_to_client(&self, client_id: u64, msg: &str) {
        // Route to Telegram if it's the Telegram synthetic client_id
        if client_id == u64::MAX - 1 {
            if let Some(tx) = self.telegram_tx.read().await.as_ref() {
                let _ = tx.try_send(msg.to_string());
            }
            return;
        }
        let client = self.clients.read().await.get(&client_id).cloned();
        let Some(client) = client else { return };

        let payload: Arc<str> = Arc::<str>::from(msg);
        match client.tx.try_send(payload) {
            Ok(_) => {
                client
                    .dropped_events
                    .store(0, std::sync::atomic::Ordering::Relaxed);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                let dropped = client
                    .dropped_events
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                if dropped >= RELAY_CLIENT_MAX_DROPPED_EVENTS {
                    warn!(
                        "[Relay] Evicting slow client {} (dropped {} targeted events)",
                        client_id, dropped
                    );
                    self.remove_client(client_id).await;
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.remove_client(client_id).await;
            }
        }
    }

    async fn add_client(&self, tx: mpsc::Sender<Arc<str>>) -> u64 {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.clients.write().await.insert(
            id,
            Arc::new(RelayClient {
                tx,
                dropped_events: std::sync::atomic::AtomicU32::new(0),
            }),
        );
        id
    }

    async fn remove_client(&self, id: u64) {
        self.clients.write().await.remove(&id);
    }

    async fn broadcast(&self, msg: &str) {
        let clients: Vec<(u64, Arc<RelayClient>)> = self
            .clients
            .read()
            .await
            .iter()
            .map(|(id, c)| (*id, Arc::clone(c)))
            .collect();
        if clients.is_empty() {
            return;
        }

        let payload: Arc<str> = Arc::<str>::from(msg);
        let mut stale_clients = HashSet::new();

        for (id, client) in clients {
            match client.tx.try_send(Arc::clone(&payload)) {
                Ok(_) => {
                    client
                        .dropped_events
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    let dropped = client
                        .dropped_events
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                        + 1;
                    if dropped >= RELAY_CLIENT_MAX_DROPPED_EVENTS {
                        warn!(
                            "[Relay] Evicting slow client {} (dropped {} events)",
                            id, dropped
                        );
                        stale_clients.insert(id);
                    }
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    debug!("[Relay] Client {} disconnected during broadcast", id);
                    stale_clients.insert(id);
                }
            }
        }

        if !stale_clients.is_empty() {
            let mut guard = self.clients.write().await;
            for id in stale_clients {
                guard.remove(&id);
            }
        }
    }
}

/// Broadcast an event through the unbounded channel (consumed by the broadcast task).
pub fn broadcast_event(tx: &mpsc::Sender<RelayEvent>, event: RelayEvent) {
    match tx.try_send(event) {
        Ok(_) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            warn!("[Relay] Dropping event: relay queue is full");
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            warn!("[Relay] Dropping event: relay queue is closed");
        }
    }
}

/// Start the unified WebSocket server on the given port.
///
/// Routing:
/// - Path `/acp` → ACP SACP NDJSON bridge (Desktop's `AcpWebSocketTransport`)
/// - All other paths → RelayCommand JSON protocol (CLI, Telegram, mobile)
pub async fn start_relay_server(state: Arc<RelayState>, port: u16) {
    let addr = format!("0.0.0.0:{}", port);
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => {
            info!("[Relay] Unified WS server listening on ws://{}", addr);
            l
        }
        Err(e) => {
            error!("[Relay] Failed to bind to {}: {}", addr, e);
            return;
        }
    };

    while let Ok((stream, peer_addr)) = listener.accept().await {
        let state = state.clone();
        tokio::spawn(async move {
            // Use accept_hdr_async to inspect the HTTP request path before upgrade
            let detected_path;
            let path_cell = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            let path_writer = path_cell.clone();

            let ws = match tokio_tungstenite::accept_hdr_async(
                stream,
                |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                 response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    let path = req.uri().path().to_string();
                    *path_writer.lock().unwrap() = path;
                    Ok(response)
                },
            )
            .await
            {
                Ok(ws) => {
                    detected_path = path_cell.lock().unwrap().clone();
                    ws
                }
                Err(e) => {
                    error!(
                        "[Relay] WebSocket handshake failed from {}: {}",
                        peer_addr, e
                    );
                    return;
                }
            };

            // Route: /acp → ACP bridge, everything else → relay protocol
            if detected_path == "/acp" || detected_path.starts_with("/acp/") {
                let bridge = state.acp_bridge.read().await.clone();
                if let Some(ref bridge) = bridge {
                    info!(
                        "[AcpWS] Desktop client connected from {} (via /acp)",
                        peer_addr
                    );
                    handle_acp_client(ws, bridge, peer_addr).await;
                    return;
                } else {
                    warn!(
                        "[AcpWS] /acp requested but no ACP bridge available (not in daemon mode)"
                    );
                }
            }

            // ── Relay protocol ───────────────────────────────────────────
            info!(
                "[Relay] Client connected from {} (path: {})",
                peer_addr, detected_path
            );
            let (mut ws_tx, mut ws_rx) = ws.split();

            // Channel for outgoing messages to this client
            let (client_tx, mut client_rx) = mpsc::channel::<Arc<str>>(RELAY_CLIENT_QUEUE_CAPACITY);
            let client_id = state.add_client(client_tx).await;

            // Task: forward client_rx → WebSocket
            let send_task = tokio::spawn(async move {
                while let Some(msg) = client_rx.recv().await {
                    if ws_tx
                        .send(Message::Text(msg.to_string().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });

            // Read incoming messages from WebSocket
            while let Some(msg_result) = ws_rx.next().await {
                match msg_result {
                    Ok(Message::Text(text)) => {
                        let text_str: &str = &text;
                        debug!("[Relay] Received from {}: {}", peer_addr, text_str);
                        if let Ok(cmd) = serde_json::from_str::<RelayCommand>(text_str) {
                            let _ = state
                                .command_tx
                                .send(RelayCommandWithClient { cmd, client_id })
                                .await;
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Err(e) => {
                        debug!("[Relay] Error from {}: {}", peer_addr, e);
                        break;
                    }
                    _ => {} // Ignore ping/pong/binary
                }
            }

            info!("[Relay] Client disconnected: {}", peer_addr);
            state.remove_client(client_id).await;
            send_task.abort();
        });
    }
}

/// Handle an ACP WS client (Desktop): bridge SACP NDJSON via AcpWsBridge channels.
async fn handle_acp_client(
    ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    bridge: &crate::acp_ws_server::AcpWsBridge,
    peer_addr: std::net::SocketAddr,
) {
    let (mut ws_tx, mut ws_rx) = ws.split();
    let mut from_rx = bridge.from_conductor.subscribe();
    let to_tx = bridge.to_conductor.clone();

    // WS → conductor
    let ws_to_conductor = tokio::spawn(async move {
        while let Some(msg_result) = ws_rx.next().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    if to_tx.send(text.to_string()).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    debug!("[AcpWS] WS read error from {}: {}", peer_addr, e);
                    break;
                }
                _ => {}
            }
        }
    });

    // conductor → WS
    let conductor_to_ws = tokio::spawn(async move {
        loop {
            match from_rx.recv().await {
                Ok(line) => {
                    if ws_tx.send(Message::Text(line.into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("[AcpWS] Client {} lagged by {} messages", peer_addr, n);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    tokio::pin!(ws_to_conductor);
    tokio::pin!(conductor_to_ws);
    tokio::select! {
        _ = &mut ws_to_conductor => { conductor_to_ws.abort(); }
        _ = &mut conductor_to_ws => { ws_to_conductor.abort(); }
    }

    info!("[AcpWS] Desktop disconnected from {}", peer_addr);
}
