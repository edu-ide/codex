// commands

use crate::SharedState;
use crate::relay_server::RelayEvent;
use crate::{
    AssistantBuffer, RELAY_DESKTOP_READY_TIMEOUT_MS, RelayAttachmentPayload, broadcast_event,
    infer_agent_id_from_command, relay_wait_timeout_from_payload, save_mobile_attachments_to_cwd,
    send_new_session_with_bootstrap,
};
use agent_client_protocol_schema::{ContentBlock, PromptRequest, TextContent};
use sacp::{Agent, Client};
use serde_json::json;
use tracing::info;
use uuid::Uuid;
pub async fn handle_chat_message(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let payload = &cmd.payload;
    let text = payload
        .get("text")
        .and_then(|v| v.as_str())
        .map(|v| v.trim())
        .unwrap_or("");
    let attachments = payload
        .get("attachments")
        .and_then(|v| serde_json::from_value::<Vec<RelayAttachmentPayload>>(v.clone()).ok())
        .unwrap_or_default();
    if text.is_empty() && attachments.is_empty() {
        maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some("text or attachments is required".to_string()),
        );
        return;
    }

    let settings_snapshot = ctx.infra.settings_store.get();
    let current_agent_id = infer_agent_id_from_command(&settings_snapshot.agent.command);

    let requested_session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string());
    let db_session_id = requested_session_id.unwrap_or_else(|| Uuid::new_v4().to_string());

    // Update active session ID for MCP tools (e.g. artifact_save)
    *ctx.sessions.active_session_id.write().await = db_session_id.clone();

    match ctx.infra.brain.session_get_raw(&db_session_id) {
        Ok(None) => {
            let _ =
                ctx.infra
                    .brain
                    .session_create(&db_session_id, "Untitled", &current_agent_id, "/");
        }
        Ok(Some(_)) => {}
        Err(e) => {
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some(e.to_string()),
            );
            return;
        }
    }

    let mut session_cwd = "/".to_string();
    if let Ok(Some(info)) = ctx.infra.brain.session_get_raw(&db_session_id) {
        session_cwd = info.cwd.clone();
        if info.title == "Untitled" {
            let inferred_title: String = text
                .lines()
                .next()
                .unwrap_or("Untitled")
                .chars()
                .take(40)
                .collect();
            let _ = ctx
                .infra
                .brain
                .session_update_title(&db_session_id, &inferred_title);
        }
    }

    let attachment_paths =
        match save_mobile_attachments_to_cwd(&db_session_id, &session_cwd, &attachments) {
            Ok(paths) => paths,
            Err(e) => {
                maybe_respond(cmd.request_id.as_deref(), serde_json::Value::Null, Some(e));
                return;
            }
        };
    let outbound_text = if attachment_paths.is_empty() {
        text.to_string()
    } else if text.is_empty() {
        attachment_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        format!(
            "{}\n\n{}",
            text,
            attachment_paths
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    if let Err(e) =
        ctx.infra
            .brain
            .session_add_message_simple(&db_session_id, "user", &outbound_text, "")
    {
        maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some(e.to_string()),
        );
        return;
    }

    {
        let next_turn_seq = {
            let seq_map = &ctx.sessions.turn_seq;
            let next = seq_map.get(&db_session_id).unwrap_or(0).saturating_add(1);
            seq_map.insert(db_session_id.clone(), next);
            next
        };
        let lock = &ctx.sessions.assistant_buffers;
        lock.insert(
            db_session_id.clone(),
            AssistantBuffer {
                session_id: db_session_id.clone(),
                agent_id: current_agent_id.clone(),
                turn_seq: next_turn_seq,
                patch_seq: 0,
                content: String::new(),
                thinking: String::new(),
                tool_calls: Vec::new(),
                content_blocks: Vec::new(),
                input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
                duration_ms: 0,
                start_time: Some(std::time::Instant::now()),
                db_message_id: None,
                model_id: String::new(),
            },
        );
    }

    let _wait_timeout = relay_wait_timeout_from_payload(payload, RELAY_DESKTOP_READY_TIMEOUT_MS);
    let maybe_cx = ctx
        .infra
        .relay_conductor_cx
        .wait_for(std::time::Duration::from_millis(500))
        .await;

    // Codex agents are exposed through the local codex-a2a HTTP server, not an ACP endpoint.
    // Route relay chat directly to A2A for Codex so CLI/daemon mode doesn't stall waiting for
    // `/acp/stream` on a server that only speaks A2A HTTP.
    if current_agent_id == "codex" {
        info!("[Standalone] Codex engine selected — using direct A2A HTTP path");
        standalone_chat_via_a2a(
            ctx,
            cmd,
            client_id,
            &db_session_id,
            &current_agent_id,
            &outbound_text,
            maybe_respond,
        )
        .await;
        return;
    }

    // ── Standalone A2A HTTP fallback (daemon mode without Desktop) ──────
    if maybe_cx.is_none() {
        info!("[Standalone] No conductor connection — using A2A HTTP fallback");
        standalone_chat_via_a2a(
            ctx,
            cmd,
            client_id,
            &db_session_id,
            &current_agent_id,
            &outbound_text,
            maybe_respond,
        )
        .await;
        return;
    }
    let cx = maybe_cx.unwrap();

    let acp_session_id = {
        let existing = {
            let map = &ctx.sessions.id_map;
            map.get(&db_session_id)
        };
        if let Some(mapped) = existing {
            mapped
        } else {
            let created = send_new_session_with_bootstrap(&cx, "/").await;
            match created {
                Ok(new_session_response) => {
                    let mapped = new_session_response.session_id.0.to_string();
                    ctx.sessions
                        .id_map
                        .insert(db_session_id.clone(), mapped.clone());
                    ctx.sessions
                        .reverse_map
                        .insert(mapped.clone(), db_session_id.clone());
                    mapped
                }
                Err(e) => {
                    maybe_respond(
                        cmd.request_id.as_deref(),
                        serde_json::Value::Null,
                        Some(e.to_string()),
                    );
                    return;
                }
            }
        }
    };

    // Broadcast user message event to all relay clients + conductor
    let timestamp = chrono::Utc::now().to_rfc3339();
    broadcast_event(
        &ctx.infra.relay_tx,
        RelayEvent::UserMessage {
            session_id: db_session_id.clone(),
            text: outbound_text.clone(),
            channel_id: "telegram".to_string(),
            timestamp: timestamp.clone(),
        },
    );
    if let Ok(notif) = sacp::UntypedMessage::new(
        crate::types::NOTIF_RELAY_USER_MESSAGE,
        json!({
            "session_id": db_session_id,
            "text": outbound_text,
            "channel_id": "telegram",
            "timestamp": timestamp,
        }),
    ) {
        let _ = cx.send_notification_to(Client, notif);
    }

    let request_id = cmd.request_id.clone();
    let response_state = ctx.infra.relay_state.clone();
    let response_client_id = client_id;
    let save_brain = ctx.infra.brain.clone();
    let save_buffers = ctx.sessions.assistant_buffers.clone();
    let save_session_id = db_session_id.clone();
    let save_agent_id = current_agent_id.clone();
    let prompt_req = PromptRequest::new(
        acp_session_id.clone(),
        vec![ContentBlock::Text(TextContent::new(outbound_text))],
    );

    let sent = cx.send_request_to(Agent, prompt_req).on_receiving_result(async move |result| {
                let lock = &save_buffers;
                let buf_opt = lock.get(&save_session_id);
                if buf_opt.is_some() {
                    lock.invalidate(&save_session_id);
                }
                let (mut response_text, thinking_text) = if let Some(buf) = buf_opt {
                    info!("[TG-Debug] Buffer for session {}: content_len={}, thinking_len={}, tools={}",
                        save_session_id, buf.content.len(), buf.thinking.len(), buf.tool_calls.len());
                    info!("[TG-Debug] content first 200 chars: {:?}", buf.content.chars().take(200).collect::<String>());
                    if !buf.thinking.is_empty() {
                        info!("[TG-Debug] thinking first 200 chars: {:?}", buf.thinking.chars().take(200).collect::<String>());
                    }
                    if !buf.content.is_empty() || !buf.tool_calls.is_empty() {
                        let final_tool_calls = if buf.tool_calls.is_empty() {
                            String::new()
                        } else {
                            serde_json::to_string(&buf.tool_calls).unwrap_or_default()
                        };
                        let final_content_blocks = if buf.content_blocks.is_empty() {
                            String::new()
                        } else {
                            serde_json::to_string(&buf.content_blocks).unwrap_or_default()
                        };
                        let _ = save_brain.session_add_message_with_blocks(
                            &save_session_id,
                            "assistant",
                            &buf.content,
                            &save_agent_id,
                            &buf.thinking,
                            &final_tool_calls,
                            &final_content_blocks,
                            buf.input_tokens,
                            buf.output_tokens,
                            buf.total_tokens,
                            buf.duration_ms,
                        );
                    }
                    let thinking_text = buf.thinking;
                    let response_text = buf.content;
                    (response_text, thinking_text)
                } else {
                    (String::new(), String::new())
                };

                // ── A2AAgent bridge fallback ──
                // When using A2AAgent transport (team mode), streaming notifications
                // are not emitted — the response text is in the ACP result's _meta.a2a_text.
                if response_text.is_empty() {
                    if let Ok(ref prompt_response) = result {
                        if let Some(meta) = &prompt_response.meta {
                            if let Some(a2a_text) = meta.get("a2a_text").and_then(|v| v.as_str()) {
                                info!("[A2A-Fallback] Using _meta.a2a_text ({} chars)", a2a_text.len());
                                response_text = a2a_text.to_string();
                                // Persist the A2A response to session history
                                let _ = save_brain.session_add_message_simple(
                                    &save_session_id, "assistant", &response_text, &save_agent_id,
                                );
                            }
                        }
                    }
                }

                if let Some(rid) = request_id.clone() {
                    let (result_value, error_value) = match result {
                        Ok(_) => (json!({ "ok": true, "session_id": save_session_id, "text": response_text, "thinking": thinking_text }), None),
                        Err(e) => (serde_json::Value::Null, Some(e.to_string())),
                    };
                    let event = RelayEvent::CommandResponse {
                        request_id: rid,
                        result: result_value,
                        error: error_value,
                    };
                    if let Ok(json) = serde_json::to_string(&event) {
                        response_state.send_to_client(response_client_id.into(), &json).await;
                    }
                }

                Ok(())
            });

    if let Err(e) = sent {
        maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some(e.to_string()),
        );
    }
}

/// Standalone A2A HTTP fallback for daemon mode (no Desktop/Conductor).
///
/// Resolves the agent's A2A endpoint from settings and sends the prompt
/// via HTTP POST to `tasks/send` (A2A JSON-RPC). Collects the response
/// from the SSE stream and sends it back via relay `command_response`.
async fn standalone_chat_via_a2a(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    db_session_id: &str,
    agent_id: &str,
    outbound_text: &str,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let settings = ctx.infra.settings_store.get();
    let endpoint = {
        let ep = settings.agent.a2a_endpoint.trim();
        if !ep.is_empty() {
            ep.to_string()
        } else {
            // In team mode, route to the leader agent instead of solo
            let team_backend = crate::config::normalize_team_backend(&settings.agent.team_backend);
            let use_remote_team = settings.agent.team_mode
                && crate::config::team_backend_uses_remote_transport(&team_backend);
            let team_leader_port = if use_remote_team {
                let sv = ctx.team.supervisor.read().await;
                sv.processes
                    .iter()
                    .find(|(_, p)| p.is_leader && p.last_healthy.is_some())
                    .map(|(_, p)| p.port)
            } else {
                None
            };

            if let Some(leader_port) = team_leader_port {
                format!("http://127.0.0.1:{}", leader_port)
            } else {
                let agent_id = infer_agent_id_from_command(&settings.agent.command);
                let resolved_engine = crate::engine_env::resolve_engine_env(&agent_id);
                format!("http://127.0.0.1:{}", resolved_engine.default_port())
            }
        }
    };

    info!(
        "[Standalone] A2A endpoint: {} | session: {} | agent: {}",
        endpoint, db_session_id, agent_id
    );

    // Build A2A message/send JSON-RPC request (Google A2A spec)
    let message_id = uuid::Uuid::new_v4().to_string();
    let rpc_id = uuid::Uuid::new_v4().to_string();
    let a2a_request = json!({
        "jsonrpc": "2.0",
        "id": rpc_id,
        "method": "message/send",
        "params": {
            "message": {
                "messageId": message_id,
                "role": "user",
                "parts": [
                    { "kind": "text", "text": outbound_text }
                ]
            },
            "configuration": {
                "acceptedOutputModes": ["text"]
            }
        }
    });

    // Send HTTP POST with SSE streaming
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some(format!("HTTP 클라이언트 생성 실패: {}", e)),
            );
            return;
        }
    };

    let response = match client
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .json(&a2a_request)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some(format!("A2A 요청 실패 ({}): {}", endpoint, e)),
            );
            return;
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "(no body)".to_string());
        maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some(format!("A2A HTTP {} ({}): {}", status, endpoint, body)),
        );
        return;
    }

    // Collect response — parse SSE or plain JSON
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let mut response_text = String::new();

    if content_type.contains("text/event-stream") {
        // SSE streaming: collect text parts from each event
        let mut stream = response.bytes_stream();
        let mut buf = String::new();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.push_str(&String::from_utf8_lossy(&bytes));
                    // Process complete SSE events
                    while let Some(pos) = buf.find("\n\n") {
                        let event_block = buf[..pos].to_string();
                        buf = buf[pos + 2..].to_string();

                        for line in event_block.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                if let Ok(ev) = serde_json::from_str::<serde_json::Value>(data) {
                                    // Extract text from A2A event
                                    let text = crate::a2a_client::extract_text_from_raw_event(&ev);
                                    if !text.is_empty() {
                                        response_text = text;
                                        // Broadcast streaming update to relay clients
                                        broadcast_event(
                                            &ctx.infra.relay_tx,
                                            RelayEvent::SessionNotification {
                                                session_id: db_session_id.to_string(),
                                                update: json!({
                                                    "textDelta": response_text,
                                                }),
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    info!("[Standalone] SSE stream error: {}", e);
                    break;
                }
            }
        }
    } else {
        // Plain JSON response
        match response.text().await {
            Ok(body) => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body) {
                    // Extract text from A2A result — try multiple paths
                    // Path 1: /result/status/message/parts (A2A spec)
                    if let Some(parts) = val.pointer("/result/status/message/parts") {
                        extract_text_from_parts(parts, &mut response_text);
                    }
                    // Path 2: /result/history (Gemini A2A format)
                    if response_text.is_empty() {
                        if let Some(history) = val.pointer("/result/history") {
                            if let Some(arr) = history.as_array() {
                                for msg in arr.iter().rev() {
                                    if msg.get("role").and_then(|v| v.as_str()) == Some("agent") {
                                        if let Some(parts) = msg.get("parts") {
                                            extract_text_from_parts(parts, &mut response_text);
                                        }
                                        if !response_text.is_empty() {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // Path 3: /result/artifacts
                    if response_text.is_empty() {
                        if let Some(artifacts) = val.pointer("/result/artifacts") {
                            if let Some(arr) = artifacts.as_array() {
                                for artifact in arr {
                                    if let Some(parts) = artifact.get("parts") {
                                        extract_text_from_parts(parts, &mut response_text);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                response_text = format!("(응답 읽기 실패: {})", e);
            }
        }
    }

    // Save assistant message to session store
    if !response_text.is_empty() {
        let _ = ctx.infra.brain.session_add_message_simple(
            db_session_id,
            "assistant",
            &response_text,
            agent_id,
        );
    }

    // Send result back via relay
    maybe_respond(
        cmd.request_id.as_deref(),
        json!({
            "ok": true,
            "session_id": db_session_id,
            "text": response_text,
            "thinking": "",
            "standalone": true,
        }),
        None,
    );
}

/// Extract text from A2A parts array.
///
/// Handles multiple part kinds:
/// - `kind: "text"` → `.text` field
/// - `kind: "data"` → `.data.description` or `.data.subject`  
/// - Legacy `type: "text"` → `.text` field
fn extract_text_from_parts(parts: &serde_json::Value, out: &mut String) {
    let Some(arr) = parts.as_array() else { return };
    for part in arr {
        // kind: "text" → text field
        if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
            if !t.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
        // kind: "data" → data.description or data.subject
        if let Some(data) = part.get("data") {
            if let Some(desc) = data.get("description").and_then(|v| v.as_str()) {
                if !desc.is_empty() {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(desc);
                }
            } else if let Some(subj) = data.get("subject").and_then(|v| v.as_str()) {
                if !subj.is_empty() {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(subj);
                }
            }
        }
    }
}
