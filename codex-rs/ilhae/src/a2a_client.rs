//! A2A Observer — ilhae-specific observer that uses `a2a_rs::A2aProxy`
//! to forward team agent events to the Desktop UI.
//!
//! Generic A2A proxy logic lives in `a2a_rs::proxy`.
//! This module only contains ilhae-specific Desktop UI forwarding.

use futures::StreamExt;
use sacp::{Client, Conductor, ConnectionTo, UntypedMessage};
use tracing::info;
use uuid::Uuid;

// Re-export for convenience
pub use a2a_rs::proxy::A2aProxy as A2aAgent;

// ─── Team Agent Observer ─────────────────────────────────────────────────

/// Spawn an A2A observer for a team agent using the AgentPool native stream.
///
/// Converts a raw JSON-RPC JSON stream (from `AgentPool`) into
/// Desktop UI events (`ilhae/assistant_turn_patch`).
pub fn spawn_a2a_observer(
    agent_role: &str,
    session_id: &str,
    cx: &ConnectionTo<Conductor>,
    mut receiver: futures::channel::mpsc::UnboundedReceiver<serde_json::Value>,
) -> tokio::task::JoinHandle<()> {
    let sid = session_id.to_string();
    let role = agent_role.to_lowercase();
    let cx = cx.clone();

    tokio::spawn(async move {
        info!(
            "[A2A-Observer] {} started processing native ACP stream",
            role
        );

        let mut accumulated_text = String::new();
        let mut current_turn_id = format!("a2a-observer-{}", Uuid::new_v4());

        while let Some(event) = receiver.next().await {
            // event is a JSON-RPC Message from AgentPool (e.g. session/update or methodless stream items)
            let text = extract_text_from_raw_event(&event);

            let state = event
                .pointer("/params/state")
                .or_else(|| event.pointer("/status/state"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let is_final = matches!(state, "completed" | "failed" | "canceled" | "rejected");

            if !text.is_empty() {
                // Replace, not append: SSE sends full snapshots
                accumulated_text = text;
            }

            if !accumulated_text.is_empty() || is_final {
                if let Ok(notif) = UntypedMessage::new(
                    crate::types::NOTIF_APP_SESSION_EVENT,
                    crate::types::IlhaeAppSessionEventNotification {
                        engine: role.clone(),
                        event: crate::types::IlhaeAppSessionEventDto::MessageDelta {
                            thread_id: sid.clone(),
                            turn_id: current_turn_id.clone(),
                            item_id: format!("{}:{}", current_turn_id, role),
                            channel: "assistant".to_string(),
                            delta: accumulated_text.clone(),
                        },
                    },
                ) {
                    let _ = cx.send_notification_to(Client, notif);
                }
                if is_final {
                    if let Ok(notif) = UntypedMessage::new(
                        crate::types::NOTIF_APP_SESSION_EVENT,
                        crate::types::IlhaeAppSessionEventNotification {
                            engine: role.clone(),
                            event: crate::types::IlhaeAppSessionEventDto::TurnCompleted {
                                thread_id: sid.clone(),
                                turn_id: current_turn_id.clone(),
                                status: "completed".to_string(),
                            },
                        },
                    ) {
                        let _ = cx.send_notification_to(Client, notif);
                    }
                }
            }

            if is_final {
                info!(
                    "[A2A-Observer] {} task terminal: {} ({}B)",
                    role,
                    state,
                    accumulated_text.len()
                );
                accumulated_text.clear();
                current_turn_id = format!("a2a-observer-{}", Uuid::new_v4());
            }
        }

        info!("[A2A-Observer] {} stream channel closed", role);
    })
}

// ─── Helpers ─────────────────────────────────────────────────────────────

/// Extract text from a raw JSON event (handles JSON-RPC and legacy formats)
pub fn extract_text_from_raw_event(event: &serde_json::Value) -> String {
    let mut texts = Vec::new();

    // JSON-RPC ACP Notification format: method = agent_client_protocol_schema::CLIENT_METHOD_NAMES.session_update
    if let Some(params) = event.get("params") {
        if let Some(content) = params.get("content").and_then(|v| v.as_array()) {
            for block in content {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    texts.push(text.to_string());
                }
            }
        }
    }

    // A2A task status message parts (Legacy)
    if let Some(parts) = event.pointer("/status/message/parts") {
        if let Some(arr) = parts.as_array() {
            for part in arr {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    texts.push(text.to_string());
                }
            }
        }
    }

    // A2A artifact parts (Legacy)
    if let Some(artifact) = event.get("artifact") {
        if let Some(parts) = artifact.get("parts").and_then(|v| v.as_array()) {
            for part in parts {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    texts.push(text.to_string());
                }
            }
        }
    }

    // Direct content (Legacy backward compat)
    if let Some(text) = event.pointer("/content/text").and_then(|v| v.as_str()) {
        texts.push(text.to_string());
    }

    texts.join("")
}
