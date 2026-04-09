//! RelayProxy — Initialize handling and Session notification relay.
//!
//! Handles:
//! - Client → Agent: Initialize (cache conductor cx for relay commands)
//! - Agent → Client: Session notifications (save chunks to buffer + relay broadcast)

use std::sync::Arc;

use agent_client_protocol_schema::{
    ContentBlock, InitializeRequest, InitializeResponse, SessionNotification, SessionUpdate,
};
use sacp::{Agent, Client, Conductor, ConnectTo, ConnectionTo, Proxy, Responder, UntypedMessage};
use serde_json::json;
use tracing::{debug, info, warn};

use crate::detect_browser_tool_in_update;
use crate::relay_server::{RelayEvent, broadcast_event};
use crate::team_timeline::{
    agent_response_event, delegation_completed_event, delegation_started_event, persist_events,
};
use crate::turn_accumulator::{TurnAccumulator, extract_tool_call_id};

// ─── RelayProxy state ─────────────────────────────────────────────────

pub struct RelayProxy {
    pub state: Arc<crate::SharedState>,
}

fn team_tool_name(value: &serde_json::Value) -> String {
    let raw = value
        .get("name")
        .or_else(|| value.get("toolName"))
        .or_else(|| value.get("tool_name"))
        .or_else(|| value.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let raw = raw
        .replace("(ilhae-tools mcp server)", "")
        .trim()
        .to_string();
    let raw = raw
        .strip_prefix("mcp_ilhae-tools_")
        .unwrap_or(&raw)
        .to_string();
    let raw = raw
        .strip_prefix("ilhae-tools__")
        .unwrap_or(&raw)
        .to_string();
    raw
}

fn team_tool_input<'a>(value: &'a serde_json::Value) -> Option<&'a serde_json::Value> {
    value
        .get("rawInput")
        .or_else(|| value.get("raw_input"))
        .or_else(|| value.get("input"))
}

fn team_tool_output_value(value: &serde_json::Value) -> Option<serde_json::Value> {
    let raw = value
        .get("rawOutput")
        .or_else(|| value.get("raw_output"))
        .or_else(|| value.get("output"))
        .or_else(|| value.get("responseText"))?;
    if let Some(s) = raw.as_str() {
        serde_json::from_str::<serde_json::Value>(s)
            .ok()
            .or_else(|| Some(json!(s)))
    } else {
        Some(raw.clone())
    }
}

fn team_tool_response_text(value: &serde_json::Value) -> String {
    let Some(output) = team_tool_output_value(value) else {
        return String::new();
    };
    output
        .get("response")
        .or_else(|| output.get("result"))
        .or_else(|| output.get("message"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            output
                .as_str()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| output.to_string())
}

fn team_tool_target_role(
    value: &serde_json::Value,
    team_cfg: &crate::context_proxy::TeamRuntimeConfig,
) -> Option<String> {
    if let Some(role) = team_tool_input(value)
        .and_then(|v| v.get("role").or_else(|| v.get("agent")))
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return Some(role.to_string());
    }

    if let Some(role) = team_tool_output_value(value)
        .and_then(|v| {
            v.get("role")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
    {
        return Some(role);
    }

    let query = team_tool_input(value)
        .and_then(|v| v.get("query"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if query.starts_with('@') {
        let after = &query[1..];
        let mentioned = after.split_whitespace().next().unwrap_or("");
        if let Some(agent) = team_cfg.agents.iter().find(|a| {
            let r = a.role.to_ascii_lowercase();
            !a.is_main && (r == mentioned || r.starts_with(mentioned))
        }) {
            return Some(agent.role.clone());
        }
    }

    team_cfg
        .agents
        .iter()
        .find(|a| !a.is_main)
        .map(|a| a.role.clone())
}

fn is_team_tool(name: &str) -> bool {
    matches!(
        name,
        "delegate"
            | "delegate_background"
            | "subscribe_task"
            | "task_status"
            | "task_result"
            | "team_list"
            | "propose"
            | "propose_to_leader"
    )
}

fn maybe_emit_team_tool_events(
    store: &Arc<crate::session_store::SessionStore>,
    cx: &ConnectionTo<Conductor>,
    session_id: &str,
    val: &serde_json::Value,
    previous_status: Option<&str>,
) {
    let tool_status = val.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let tool_name = team_tool_name(val);
    if !is_team_tool(&tool_name) {
        return;
    }

    if previous_status == Some(tool_status)
        && matches!(
            tool_status,
            "completed" | "success" | "in_progress" | "pending"
        )
    {
        return;
    }

    let ilhae_dir = dirs::home_dir()
        .map(|h| h.join(crate::helpers::ILHAE_DIR_NAME))
        .unwrap_or_default();
    let Some(team_cfg) = crate::context_proxy::load_team_runtime_config(&ilhae_dir) else {
        return;
    };

    let actual_role = team_tool_target_role(val, &team_cfg).unwrap_or_else(|| "Agent".to_string());
    let raw_input = team_tool_input(val);
    let delegation_mode = if tool_name == "delegate_background" {
        "background"
    } else if tool_name == "subscribe_task" {
        "subscribe"
    } else {
        raw_input
            .and_then(|v| v.get("mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("sync")
    };
    let delegation_query = raw_input
        .and_then(|v| {
            v.get("query")
                .or_else(|| v.get("message"))
                .or_else(|| v.get("proposal"))
        })
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_call_id = extract_tool_call_id(val).unwrap_or_default();

    if matches!(tool_status, "in_progress" | "pending") {
        info!(
            "[DelegationEvent] START: {} → {} (mode={})",
            session_id, actual_role, delegation_mode
        );
        store.write_delegation_event(
            session_id,
            &actual_role,
            delegation_mode,
            if tool_call_id.is_empty() {
                None
            } else {
                Some(&tool_call_id)
            },
            if delegation_query.is_empty() {
                None
            } else {
                Some(delegation_query)
            },
            None,
        );
        let delegation_start_msg = format!(
            "🛰️ Delegating to {} (mode: {})",
            actual_role, delegation_mode
        );
        let a2a_event = json!({
            "session_id": session_id,
            "source_role": "Leader",
            "assigned_role": actual_role,
            "event_type": "delegation_start",
            "message": delegation_start_msg,
            "event_line": format!("🛰️ Leader → {} [{}]", actual_role, delegation_mode),
            "mode": delegation_mode,
            "task_id": if tool_call_id.is_empty() { serde_json::Value::Null } else { json!(tool_call_id) },
            "request": delegation_query,
        });
        if let Ok(notif) = UntypedMessage::new(crate::types::NOTIF_A2A_EVENT, a2a_event) {
            let _ = cx.send_notification_to(Client, notif);
        }
        persist_events(
            store,
            session_id,
            [delegation_started_event(
                &actual_role,
                delegation_mode,
                delegation_query,
                if tool_call_id.is_empty() {
                    None
                } else {
                    Some(tool_call_id.as_str())
                },
                val.get("toolCallId").or_else(|| val.get("tool_call_id")),
            )],
        );
    }

    if matches!(tool_status, "completed" | "success") {
        let response_text = team_tool_response_text(val);
        let result_preview: String = response_text.chars().take(200).collect();
        store.write_delegation_event(
            session_id,
            &actual_role,
            delegation_mode,
            if tool_call_id.is_empty() {
                None
            } else {
                Some(&tool_call_id)
            },
            if delegation_query.is_empty() {
                None
            } else {
                Some(delegation_query)
            },
            if result_preview.is_empty() {
                None
            } else {
                Some(&result_preview)
            },
        );

        if !response_text.is_empty() {
            info!(
                "[DelegationBubble] Emitting bubble for {} ({}B)",
                actual_role,
                response_text.len()
            );
            let turn_id = if tool_call_id.is_empty() {
                format!("delegation-{}", uuid::Uuid::new_v4())
            } else {
                format!("delegation-{tool_call_id}")
            };
            let item_id = format!("{turn_id}:{actual_role}");
            if let Ok(notif) = UntypedMessage::new(
                crate::types::NOTIF_APP_SESSION_EVENT,
                crate::types::IlhaeAppSessionEventNotification {
                    engine: actual_role.to_string(),
                    event: crate::types::IlhaeAppSessionEventDto::MessageDelta {
                        thread_id: session_id.to_string(),
                        turn_id: turn_id.clone(),
                        item_id,
                        channel: "assistant".to_string(),
                        delta: response_text.clone(),
                    },
                },
            ) {
                let _ = cx.send_notification_to(Client, notif);
            }
            if let Ok(notif) = UntypedMessage::new(
                crate::types::NOTIF_APP_SESSION_EVENT,
                crate::types::IlhaeAppSessionEventNotification {
                    engine: actual_role.to_string(),
                    event: crate::types::IlhaeAppSessionEventDto::TurnCompleted {
                        thread_id: session_id.to_string(),
                        turn_id,
                        status: "completed".to_string(),
                    },
                },
            ) {
                let _ = cx.send_notification_to(Client, notif);
            }
            persist_events(
                store,
                session_id,
                [agent_response_event(
                    &actual_role,
                    &response_text,
                    "[]",
                    delegation_mode,
                    if tool_call_id.is_empty() {
                        None
                    } else {
                        Some(tool_call_id.as_str())
                    },
                )],
            );
        }

        let delegation_complete_msg = format!("✅ {} completed delegation", actual_role);
        let a2a_event = json!({
            "session_id": session_id,
            "source_role": actual_role,
            "assigned_role": "Leader",
            "event_type": "delegation_complete",
            "message": delegation_complete_msg,
            "event_line": format!("✅ {} → Leader [completed]", actual_role),
            "mode": delegation_mode,
            "task_id": if tool_call_id.is_empty() { serde_json::Value::Null } else { json!(tool_call_id) },
            "response": response_text,
        });
        if let Ok(notif) = UntypedMessage::new(crate::types::NOTIF_A2A_EVENT, a2a_event) {
            let _ = cx.send_notification_to(Client, notif);
        }
        persist_events(
            store,
            session_id,
            [delegation_completed_event(
                &actual_role,
                if tool_call_id.is_empty() {
                    None
                } else {
                    Some(tool_call_id.as_str())
                },
                "completed",
                &response_text,
                delegation_mode,
            )],
        );
    }
}

impl ConnectTo<Conductor> for RelayProxy {
    async fn connect_to(self, conductor: impl ConnectTo<Proxy>) -> Result<(), sacp::Error> {
        let s = self.state;
        let cx_cache_for_connect = s.infra.relay_conductor_cx.clone();

        Proxy.builder()
            .name("relay-proxy")
            // ═══ Client → Agent: Initialize (cache conductor cx for relay commands) ═══
            .on_receive_request_from(Client, {
                let cx_cache = s.infra.relay_conductor_cx.clone();
                async move |req: InitializeRequest, responder: Responder<InitializeResponse>, cx: ConnectionTo<Conductor>| {
                    cx_cache.try_add(cx.clone()).await;
                    responder.respond(InitializeResponse::new(req.protocol_version).agent_capabilities(agent_client_protocol_schema::AgentCapabilities::new()))
                }
            }, sacp::on_receive_request!())
            // ═══ Agent → Client: Session notifications (save + relay) ═══
            .on_receive_notification_from(Agent, {
                let buffers = s.sessions.assistant_buffers.clone();
                let session_turn_seq = s.sessions.turn_seq.clone();
                let relay_tx = s.infra.relay_tx.clone();
                let browser = s.infra.browser_mgr.clone();
                let store = s.infra.brain.sessions().clone();
                let _session_id_map = s.sessions.id_map.clone();
                let reverse_session_map = s.sessions.reverse_map.clone();
                let cx_cache = s.infra.relay_conductor_cx.clone();
                move |mut notif: SessionNotification, cx: ConnectionTo<Conductor>| {
                    let cx_cache = cx_cache.clone();
                    let buffers = buffers.clone();
                    let session_turn_seq = session_turn_seq.clone();
                    let relay_tx = relay_tx.clone();
                    let browser = browser.clone();
                    let store = store.clone();
                    let reverse_session_map = reverse_session_map.clone();
                    
                    async move {
                        cx_cache.try_add(cx.clone()).await;
                        let acp_session_id = notif.session_id.0.to_string();

                        // Reverse-map ACP session ID → DB session ID for cross-agent continuity (O(1))
                        let session_id = {
                            let rev = reverse_session_map;
                            rev.get(&acp_session_id)
                                .map(|v| v)
                                .unwrap_or_else(|| acp_session_id.clone())
                        };

                        // If remapped, rewrite the notification's session ID to the DB session ID
                        if session_id != acp_session_id {
                            info!("Cross-agent: remapping notification session {} → {}", acp_session_id, session_id);
                            match serde_json::from_value(json!(session_id)) {
                                Ok(sid) => notif.session_id = sid,
                                Err(e) => warn!("Failed to remap session ID: {}", e),
                            }
                        }
                        let current_turn_seq = {
                            let seq_map = session_turn_seq;
                            seq_map.get(&session_id).unwrap_or(0)
                        };

                        // ── Accumulate chunks into buffer via TurnAccumulator methods ──
                    let should_patch = match &notif.update {
                        SessionUpdate::AgentMessageChunk(chunk) => {
                            if let ContentBlock::Text(t) = &chunk.content {
                                let lock = buffers.clone();
                                let mut buffer = lock.get(&session_id).unwrap_or_else(|| {
                                    TurnAccumulator::new(session_id.clone(), String::new(), current_turn_seq)
                                });
                                if buffer.content.is_empty() {
                                    info!("[TG-Debug] AgentMessageChunk START for session {}: {:?}", session_id, t.text.chars().take(80).collect::<String>());
                                }
                                buffer.append_text(&t.text);
                                buffer.advance_patch();
                                lock.insert(session_id.clone(), buffer);
                                true
                            } else { false }
                        },
                        SessionUpdate::AgentThoughtChunk(chunk) => {
                            if let ContentBlock::Text(t) = &chunk.content {
                                let lock = buffers.clone();
                                let mut buffer = lock.get(&session_id).unwrap_or_else(|| {
                                    TurnAccumulator::new(session_id.clone(), String::new(), current_turn_seq)
                                });
                                if buffer.thinking.is_empty() {
                                    info!("[TG-Debug] AgentThoughtChunk START for session {}: {:?}", session_id, t.text.chars().take(80).collect::<String>());
                                }
                                buffer.append_thinking(&t.text);
                                buffer.advance_patch();
                                lock.insert(session_id.clone(), buffer);
                                true
                            } else { false }
                        },
                        SessionUpdate::ToolCall(tc) => {
                            info!("[ToolCall Debug] title={:?} kind={:?} status={:?} rawInput={:?}",
                                tc.title, tc.kind, tc.status,
                                tc.raw_input.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default())
                            );
                            match serde_json::to_value(&tc) {
                                Ok(val) => {
                                    // Debug: log all JSON keys and title/name values
                                    if let Some(obj) = val.as_object() {
                                        let keys: Vec<&String> = obj.keys().collect();
                                        info!("[ToolCall JSON] keys={:?} title={:?} name={:?} toolCallId={:?}",
                                            keys,
                                            obj.get("title").and_then(|v| v.as_str()),
                                            obj.get("name").and_then(|v| v.as_str()),
                                            obj.get("toolCallId").and_then(|v| v.as_str()),
                                        );
                                    }
                                    let tool_call_id = extract_tool_call_id(&val);
                                    let lock = buffers.clone();
                                    let mut buffer = lock.get(&session_id).unwrap_or_else(|| {
                                        TurnAccumulator::new(session_id.clone(), String::new(), current_turn_seq)
                                    });
                                    buffer.push_tool_call(val.clone(), tool_call_id);
                                    buffer.advance_patch();
                                    lock.insert(session_id.clone(), buffer);

                                    maybe_emit_team_tool_events(&store, &cx, &session_id, &val, None);

                                    true
                                }
                                Err(_) => false,
                            }
                        },
                        SessionUpdate::ToolCallUpdate(tc_update) => {
                            match serde_json::to_value(tc_update) {
                                Ok(val) => {
                                    let tool_call_id = extract_tool_call_id(&val);
                                    let lock = buffers.clone();
                                    let mut buffer = lock.get(&session_id).unwrap_or_else(|| {
                                        TurnAccumulator::new(session_id.clone(), String::new(), current_turn_seq)
                                    });
                                    let previous_status = tool_call_id
                                        .as_ref()
                                        .and_then(|uid| {
                                            buffer
                                                .tool_calls
                                                .iter()
                                                .find(|tc| tc.get("toolCallId").and_then(|v| v.as_str()) == Some(uid.as_str()))
                                                .and_then(|tc| tc.get("status").and_then(|v| v.as_str()).map(|s| s.to_string()))
                                        });
                                    if let Some(ref uid) = tool_call_id {
                                        debug!("[ToolCallAccum] session {} MergeToolCallUpdate id={} total_tc={}", session_id, uid, buffer.tool_calls.len() + 1);
                                    }
                                    buffer.merge_tool_call_update(val.clone(), tool_call_id.clone());
                                    let merged_val = tool_call_id
                                        .as_ref()
                                        .and_then(|uid| {
                                            buffer
                                                .tool_calls
                                                .iter()
                                                .find(|tc| tc.get("toolCallId").and_then(|v| v.as_str()) == Some(uid.as_str()))
                                                .cloned()
                                        })
                                        .unwrap_or(val);
                                    buffer.advance_patch();
                                    lock.insert(session_id.clone(), buffer);
                                    
                                    maybe_emit_team_tool_events(&store, &cx, &session_id, &merged_val, previous_status.as_deref());
                                    true
                                }
                                Err(_) => false,
                            }
                        },
                        _ => false,
                    };

                    // Broadcast the unified turn state to all clients (Desktop UI, Mobile)
                    if should_patch {
                        let lock = buffers;
                        if let Some(buffer) = lock.get(&session_id) {
                            let patch_notif = UntypedMessage::new(crate::types::NOTIF_ASSISTANT_TURN_PATCH, buffer.to_patch()).unwrap();
                            let _ = cx.send_notification_to(Client, patch_notif);
                        }
                    }

                    // Detect browser tool usage — notify frontend for UI updates
                    if let Some(tool_name) = detect_browser_tool_in_update(&notif.update) {
                        info!("[BrowserDetect] Browser tool: {} in session {}", tool_name, session_id);
                        let browser_notif = UntypedMessage::new(crate::types::NOTIF_BROWSER_ACTIVITY, json!({
                            "sessionId": session_id,
                            "toolName": tool_name,
                            "browserStatus": serde_json::to_value(browser.get_status()).unwrap_or_default()
                        })).unwrap();
                        let _ = cx.send_notification_to(Client, browser_notif);
                        broadcast_event(&relay_tx, RelayEvent::BrowserActivity { session_id: session_id.clone(), tool_name });
                    }

                    // Serialize for relay broadcast
                    let update_val = serde_json::to_value(&notif.update).unwrap_or(json!({}));

                    // Sync agent-reported CWD to DB
                    if update_val.pointer("/sessionUpdate").and_then(|v| v.as_str()) == Some("session_info_update") {
                        if let Some(cwd) = update_val.pointer("/cwd").and_then(|v| v.as_str()) {
                            info!("[CWD Sync] Updating session {} CWD to: {}", session_id, cwd);
                            let _ = store.update_session_cwd(&session_id, cwd);
                        }
                    }

                    broadcast_event(&relay_tx, RelayEvent::SessionNotification { session_id, update: update_val });

                    // Forward the original notification to the client
                    cx.send_notification_to(Client, notif)?;
                    Ok(())
                    }
                }
            }, sacp::on_receive_notification!())
            .connect_with(conductor, async move |cx: ConnectionTo<Conductor>| {
                // Initial connection callback — always fires exactly once per client connection.
                cx_cache_for_connect.try_add(cx).await;
                std::future::pending::<Result<(), sacp::Error>>().await
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use crate::AssistantContentBlock;
    use crate::turn_accumulator::{
        append_text_block, append_thinking_block, append_tool_call_block, merge_stream_chunk,
    };

    #[test]
    fn merge_stream_chunk_handles_delta_chunks() {
        let mut s = String::new();
        merge_stream_chunk(&mut s, "안녕");
        merge_stream_chunk(&mut s, "하세요");
        assert_eq!(s, "안녕하세요");
    }

    #[test]
    fn merge_stream_chunk_handles_snapshot_chunks() {
        let mut s = String::new();
        merge_stream_chunk(&mut s, "안");
        merge_stream_chunk(&mut s, "안녕");
        merge_stream_chunk(&mut s, "안녕하세요");
        assert_eq!(s, "안녕하세요");
    }

    #[test]
    fn merge_stream_chunk_ignores_duplicate_replay() {
        let mut s = String::new();
        merge_stream_chunk(&mut s, "hello world");
        merge_stream_chunk(&mut s, "hello world");
        merge_stream_chunk(&mut s, "hello world");
        assert_eq!(s, "hello world");
    }

    #[test]
    fn merge_stream_chunk_appends_only_non_overlapping_tail() {
        let mut s = String::from("Hello wor");
        merge_stream_chunk(&mut s, "world");
        assert_eq!(s, "Hello world");
    }

    #[test]
    fn content_blocks_preserve_arrival_order() {
        let mut blocks = Vec::new();
        append_thinking_block(&mut blocks, "t1");
        append_tool_call_block(&mut blocks, "tool-a".to_string());
        append_text_block(&mut blocks, "answer");

        assert!(matches!(
            blocks.first(),
            Some(AssistantContentBlock::Thinking { .. })
        ));
        assert!(matches!(
            blocks.get(1),
            Some(AssistantContentBlock::ToolCalls { .. })
        ));
        assert!(matches!(
            blocks.get(2),
            Some(AssistantContentBlock::Text { .. })
        ));
    }

    #[test]
    fn content_blocks_merge_consecutive_same_type() {
        let mut blocks = Vec::new();
        append_text_block(&mut blocks, "Hel");
        append_text_block(&mut blocks, "Hello");
        append_text_block(&mut blocks, "Hello world");

        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            AssistantContentBlock::Text { text } => assert_eq!(text, "Hello world"),
            _ => panic!("expected text block"),
        }
    }
}
