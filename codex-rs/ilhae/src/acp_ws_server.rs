//! ACP-over-WebSocket server for daemon mode.
//!
//! Exposes the loopback conductor's duplex transport over a WebSocket on port 18791.
//! Desktop (Tauri) connects here with `AcpWebSocketTransport` and speaks raw SACP NDJSON.
//!
//! Architecture: Channel-based bridge
//! ┌──────┐  mpsc   ┌───────────┐  duplex  ┌───────────┐
//! │  WS  │ ──────→ │ Bridge    │ ──────→  │ Conductor │
//! │client│ ←────── │ (bg task) │ ←──────  │ (SACP)    │
//! └──────┘ broadcast└───────────┘          └───────────┘
//!
//! The duplex halves stay permanently in the background task.
//! WS clients connect/disconnect freely via channels — no ownership gymnastics.
//! Multiple WS observers supported (broadcast); writes serialized via mpsc.

use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

/// Default ACP WS port.
pub const ACP_WS_PORT: u16 = 18791;

/// Channel-based bridge between WS clients and the loopback conductor.
///
/// The duplex halves live permanently in a background task.
/// WS clients send messages via `to_conductor` and receive via `from_conductor`.
pub struct AcpWsBridge {
    /// Send NDJSON lines TO the conductor (WS client → conductor).
    pub to_conductor: mpsc::Sender<String>,
    /// Receive NDJSON lines FROM the conductor (conductor → WS client).
    /// Uses broadcast so multiple observers can subscribe.
    pub from_conductor: broadcast::Sender<String>,
}

impl AcpWsBridge {
    /// Create a new bridge and spawn the background duplex pump task.
    ///
    /// The `client_write` and `client_read` halves are consumed by the background task
    /// and will live until the task is cancelled or the duplex is closed.
    pub fn spawn(
        client_write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
        client_read: tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    ) -> Arc<Self> {
        let (to_conductor_tx, to_conductor_rx) = mpsc::channel::<String>(1024);
        let (from_conductor_tx, _) = broadcast::channel::<String>(4096);

        let bridge = Arc::new(Self {
            to_conductor: to_conductor_tx,
            from_conductor: from_conductor_tx.clone(),
        });

        // Background task: pump messages between channels and duplex
        tokio::spawn(Self::duplex_pump(
            client_write,
            client_read,
            to_conductor_rx,
            from_conductor_tx,
        ));

        bridge
    }

    /// Background pump: channels ↔ duplex (runs forever until duplex closes).
    async fn duplex_pump(
        mut client_write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
        mut client_read: tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        mut to_conductor_rx: mpsc::Receiver<String>,
        from_conductor_tx: broadcast::Sender<String>,
    ) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

        // Two concurrent loops: read from duplex + write to duplex
        let read_task = tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                match client_read.read_line(&mut line).await {
                    Ok(0) => {
                        info!("[AcpWS Bridge] duplex EOF — conductor closed");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim_end().to_string();
                        if !trimmed.is_empty() {
                            // Broadcast to all WS subscribers (ignore if no receivers)
                            let _ = from_conductor_tx.send(trimmed);
                        }
                    }
                    Err(e) => {
                        warn!("[AcpWS Bridge] duplex read error: {}", e);
                        break;
                    }
                }
            }
        });

        let write_task = tokio::spawn(async move {
            while let Some(msg) = to_conductor_rx.recv().await {
                let line = if msg.ends_with('\n') {
                    msg
                } else {
                    format!("{}\n", msg)
                };
                if let Err(e) = client_write.write_all(line.as_bytes()).await {
                    warn!("[AcpWS Bridge] duplex write error: {}", e);
                    break;
                }
                let _ = client_write.flush().await;
            }
        });

        // Wait for either to finish (then the other will naturally stop)
        tokio::select! {
            _ = read_task => debug!("[AcpWS Bridge] read pump stopped"),
            _ = write_task => debug!("[AcpWS Bridge] write pump stopped"),
        }
    }
}

/// Start the ACP-over-WS server.
///
/// Each WS client subscribes to `from_conductor` and sends via `to_conductor`.
/// No ownership transfer — clients can connect/disconnect freely.
pub async fn start_acp_ws_server(bridge: Arc<AcpWsBridge>, port: u16) {
    let addr = format!("127.0.0.1:{}", port);
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => {
            info!("[AcpWS] SACP-over-WS server listening on ws://{}", addr);
            l
        }
        Err(e) => {
            warn!("[AcpWS] Failed to bind to {}: {}", addr, e);
            return;
        }
    };

    while let Ok((stream, peer_addr)) = listener.accept().await {
        let bridge = bridge.clone();
        tokio::spawn(async move {
            let ws = match tokio_tungstenite::accept_async(stream).await {
                Ok(ws) => ws,
                Err(e) => {
                    warn!("[AcpWS] Handshake failed from {}: {}", peer_addr, e);
                    return;
                }
            };

            info!("[AcpWS] Client connected from {}", peer_addr);
            let (mut ws_tx, mut ws_rx) = ws.split();

            // Subscribe to conductor output
            let mut from_rx = bridge.from_conductor.subscribe();
            let to_tx = bridge.to_conductor.clone();

            // Task 1: WS → conductor (client sends SACP messages)
            let ws_to_conductor = tokio::spawn(async move {
                while let Some(msg_result) = ws_rx.next().await {
                    match msg_result {
                        Ok(Message::Text(text)) => {
                            if to_tx.send(text.to_string()).await.is_err() {
                                break; // bridge closed
                            }
                        }
                        Ok(Message::Close(_)) => break,
                        Err(e) => {
                            debug!("[AcpWS] WS read error from {}: {}", peer_addr, e);
                            break;
                        }
                        _ => {} // Ignore ping/pong/binary
                    }
                }
            });

            // Task 2: conductor → WS (broadcast responses to client)
            let conductor_to_ws = tokio::spawn(async move {
                loop {
                    match from_rx.recv().await {
                        Ok(line) => {
                            if ws_tx.send(Message::Text(line.into())).await.is_err() {
                                break; // WS closed
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("[AcpWS] Client {} lagged by {} messages", peer_addr, n);
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });

            // Wait for either direction to finish, then clean up the other
            tokio::pin!(ws_to_conductor);
            tokio::pin!(conductor_to_ws);
            tokio::select! {
                _ = &mut ws_to_conductor => {
                    conductor_to_ws.abort();
                }
                _ = &mut conductor_to_ws => {
                    ws_to_conductor.abort();
                }
            }

            info!("[AcpWS] Client disconnected from {}", peer_addr);
        });
    }
}
