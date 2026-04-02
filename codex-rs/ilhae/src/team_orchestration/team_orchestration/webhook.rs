use serde_json::json;
use tracing::{info, warn};
use std::time::Duration;
use sacp::{UntypedMessage, ConnectionTo, Conductor, Client};
use uuid::Uuid;
use crate::team_orchestration::runner::*;

/// Tracks async schedules dispatched with `subscribe: true` (legacy orchestration path).
#[derive(Clone)]
pub struct SubscribeTracker {
    pub pending: std::sync::Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    pub completed_tx: tokio::sync::broadcast::Sender<String>,
    pub total_registered: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl SubscribeTracker {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel::<String>(64);
        Self {
            pending: std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
            completed_tx: tx,
            total_registered: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }
}

pub fn normalize_a2a_state(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace('_', "-")
}

pub fn is_terminal_a2a_state(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "canceled" | "cancelled")
}

pub fn is_input_required_a2a_state(state: &str) -> bool {
    matches!(state, "input-required" | "inputrequired")
}

pub fn extract_text_from_a2a_part(part: &serde_json::Value) -> Option<String> {
    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let data = part.get("data")?;
    if let Some(text) = data.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let obj = data.as_object()?;
    for key in ["text", "description", "content", "summary", "message"] {
        if let Some(text) = obj.get(key).and_then(|v| v.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub fn collect_text_from_a2a_parts(parts: &[serde_json::Value], delimiter: &str) -> String {
    parts
        .iter()
        .filter_map(extract_text_from_a2a_part)
        .collect::<Vec<_>>()
        .join(delimiter)
}

pub fn parse_a2a_message_text(result: &serde_json::Value) -> String {
    let parts_text = result
        .get("parts")
        .and_then(|v| v.as_array())
        .map(|parts| collect_text_from_a2a_parts(parts, ""))
        .unwrap_or_default();
    if !parts_text.trim().is_empty() {
        return parts_text.trim().to_string();
    }

    let status_text = result
        .get("status")
        .and_then(|v| v.get("message"))
        .and_then(|v| v.get("parts"))
        .and_then(|v| v.as_array())
        .map(|parts| collect_text_from_a2a_parts(parts, ""))
        .unwrap_or_default();
    if !status_text.trim().is_empty() {
        return status_text.trim().to_string();
    }

    let history_text = result
        .get("history")
        .and_then(|v| v.as_array())
        .map(|history| {
            history
                .iter()
                .filter(|msg| msg.get("role").and_then(|v| v.as_str()) == Some("agent"))
                .flat_map(|msg| {
                    msg.get("parts")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|part| extract_text_from_a2a_part(&part))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if !history_text.trim().is_empty() {
        return history_text.trim().to_string();
    }

    let artifact_text = result
        .get("artifacts")
        .and_then(|v| v.as_array())
        .map(|artifacts| {
            artifacts
                .iter()
                .flat_map(|artifact| {
                    artifact
                        .get("parts")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|part| extract_text_from_a2a_part(&part))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    artifact_text.trim().to_string()
}

pub fn parse_a2a_result(result: &serde_json::Value) -> A2AResponseParsed {
    let state = result
        .get("status")
        .and_then(|v| v.get("state"))
        .and_then(|v| v.as_str())
        .map(normalize_a2a_state)
        .or_else(|| {
            result
                .get("state")
                .and_then(|v| v.as_str())
                .map(normalize_a2a_state)
        });

    let schedule_id = result
        .get("taskId")
        .and_then(|v| v.as_str())
        .or_else(|| result.get("schedule_id").and_then(|v| v.as_str()))
        .or_else(|| result.get("id").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    let context_id = result
        .get("contextId")
        .and_then(|v| v.as_str())
        .or_else(|| result.get("context_id").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    A2AResponseParsed {
        text: parse_a2a_message_text(result),
        state,
        schedule_id,
        context_id,
    }
}

#[cfg(test)]
pub fn parse_sse_jsonrpc_events(body: &str) -> Result<Vec<serde_json::Value>, String> {
    let normalized = body.replace("\r\n", "\n");
    let mut events = Vec::new();
    for chunk in normalized.split("\n\n") {
        if let Some(parsed) = parse_sse_jsonrpc_event_chunk(chunk)? {
            events.push(parsed);
        }
    }
    Ok(events)
}

pub fn parse_sse_jsonrpc_event_chunk(chunk: &str) -> Result<Option<serde_json::Value>, String> {
    let trimmed = chunk.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let payload = trimmed
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if payload.is_empty() || payload.trim() == "[DONE]" {
        return Ok(None);
    }
    let parsed: serde_json::Value = serde_json::from_str(payload.trim())
        .map_err(|e| format!("invalid SSE payload JSON: {}", e))?;
    Ok(Some(parsed))
}

pub async fn dispatch_a2a_send_once(
    client: &reqwest::Client,
    endpoint: &str,
    msg_obj: &serde_json::Map<String, serde_json::Value>,
    push_notification_url: Option<&str>,
) -> Result<Vec<A2AResponseParsed>, String> {
    let mut config = json!({
        "acceptedOutputModes": ["text"],
        "blocking": push_notification_url.is_none()
    });
    if let Some(url) = push_notification_url {
        config["pushNotificationConfig"] = json!({ "url": url });
    }
    let payload = json!({
        "jsonrpc": "2.0",
        "id": Uuid::new_v4().to_string(),
        "method": "message/send",
        "params": {
            "message": msg_obj,
            "configuration": config
        }
    });

    let response = tokio::time::timeout(
        Duration::from_secs(TEAM_A2A_TIMEOUT_SECS),
        client.post(endpoint).json(&payload).send(),
    )
    .await
    .map_err(|_| format!("timeout after {}s", TEAM_A2A_TIMEOUT_SECS))?
    .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("http {}", response.status()));
    }
    let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    if let Some(err) = body.get("error") {
        return Err(format!("rpc error: {}", err));
    }
    let result = body
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(vec![parse_a2a_result(&result)])
}

pub async fn dispatch_a2a_stream_once(
    client: &reqwest::Client,
    endpoint: &str,
    msg_obj: &serde_json::Map<String, serde_json::Value>,
    sse_tx: Option<&tokio::sync::broadcast::Sender<serde_json::Value>>,
    push_notification_url: Option<&str>,
) -> Result<Vec<A2AResponseParsed>, String> {
    let mut config = json!({
        "acceptedOutputModes": ["text"],
        "blocking": push_notification_url.is_none()
    });
    if let Some(url) = push_notification_url {
        config["pushNotificationConfig"] = json!({ "url": url });
    }
    let payload = json!({
        "jsonrpc": "2.0",
        "id": Uuid::new_v4().to_string(),
        "method": "message/stream",
        "params": {
            "message": msg_obj,
            "configuration": config
        }
    });

    let response = tokio::time::timeout(
        Duration::from_secs(TEAM_A2A_TIMEOUT_SECS),
        client.post(endpoint).json(&payload).send(),
    )
    .await
    .map_err(|_| format!("timeout after {}s", TEAM_A2A_TIMEOUT_SECS))?
    .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("http {}", response.status()));
    }

    let mut events = Vec::new();
    let mut pending = String::new();
    let read_deadline = std::time::Instant::now() + Duration::from_secs(TEAM_A2A_TIMEOUT_SECS);
    let mut response = response;
    let mut parsed_events = Vec::new();
    loop {
        let remaining = read_deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!("stream read timeout after {}s", TEAM_A2A_TIMEOUT_SECS));
        }

        let chunk_opt = tokio::time::timeout(remaining, response.chunk())
            .await
            .map_err(|_| format!("stream read timeout after {}s", TEAM_A2A_TIMEOUT_SECS))?
            .map_err(|e| e.to_string())?;

        let Some(chunk) = chunk_opt else { break };
        let piece = std::str::from_utf8(&chunk).map_err(|e| e.to_string())?;
        pending.push_str(&piece.replace("\r\n", "\n"));

        while let Some(split_idx) = pending.find("\n\n") {
            let raw_event = pending[..split_idx].to_string();
            pending.drain(..split_idx + 2);
            if let Some(event) = parse_sse_jsonrpc_event_chunk(&raw_event)? {
                events.push(event.clone());
                // Transparent relay: broadcast raw A2A SSE event
                if let Some(tx) = sse_tx {
                    let _ = tx.send(event.clone());
                }
                if let Some(err) = event.get("error") {
                    return Err(format!("rpc error: {}", err));
                }
                let result = event
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let parsed = parse_a2a_result(&result);
                let is_final = result
                    .get("final")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let should_stop = parsed
                    .state
                    .as_deref()
                    .map(|s| is_terminal_a2a_state(s) || is_input_required_a2a_state(s))
                    .unwrap_or(false);
                parsed_events.push(parsed);
                if is_final || should_stop {
                    return Ok(parsed_events);
                }
            }
        }
    }

    if parsed_events.is_empty() {
        if let Some(event) = parse_sse_jsonrpc_event_chunk(&pending)? {
            events.push(event);
        }
        if events.is_empty() && !pending.trim().is_empty() {
            let parsed: serde_json::Value = serde_json::from_str(pending.trim())
                .map_err(|e| format!("stream response is neither SSE nor JSON-RPC: {}", e))?;
            events.push(parsed);
        }
        for event in events {
            if let Some(err) = event.get("error") {
                return Err(format!("rpc error: {}", err));
            }
            let result = event
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            parsed_events.push(parse_a2a_result(&result));
        }
    }
    Ok(parsed_events)
}





/// Spawn a tiny axum HTTP server to receive tool-call events from A2A agents.
/// Each agent POSTs to /events when it schedules a tool call, enabling full
/// P2P observability (even Verifier→Researcher calls become visible).
/// Returns (webhook_url, abort_handle).
pub async fn spawn_team_event_webhook(
    cx: ConnectionTo<Conductor>,
    session_id: String,
    store: std::sync::Arc<crate::session_store::SessionStore>,
) -> (String, tokio::task::JoinHandle<()>, tokio::sync::broadcast::Sender<serde_json::Value>, SubscribeTracker) {
    use axum::{routing::{get, post}, extract::State as AxState, Json, Router};
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures_util::stream::StreamExt;

    let (sse_tx, _) = tokio::sync::broadcast::channel::<serde_json::Value>(256);
    let tracker = SubscribeTracker::new();

    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(l) => l,
        Err(e) => {
            warn!("[TeamWebhook] Failed to bind webhook listener: {}", e);
            return ("http://127.0.0.1:33499/events".to_string(), tokio::spawn(async {}), sse_tx, tracker);
        }
    };
    let local_addr = listener.local_addr().ok();
    let bind_port = local_addr.map(|addr| addr.port()).unwrap_or(33499);
    let url = format!("http://127.0.0.1:{}/events", bind_port);
    info!("[TeamWebhook] Listening at {}", url);
    
    // Track assigned DB message IDs per agent for order preservation
    let patch_message_ids: std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, i64>>> = 
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    
    let sse_tx_shared = std::sync::Arc::new(sse_tx.clone());

    // SSE stream handler
    async fn sse_handler(
        AxState(tx): AxState<std::sync::Arc<tokio::sync::broadcast::Sender<serde_json::Value>>>,
    ) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
        let rx = tx.subscribe();
        let stream = tokio_stream::wrappers::BroadcastStream::new(rx)
            .filter_map(|result| {
                futures_util::future::ready(match result {
                    Ok(event) => Some(Ok(
                        Event::default()
                            .json_data(event)
                            .unwrap_or_else(|_| Event::default().data("{}")),
                    )),
                    Err(_) => None,
                })
            });
        Sse::new(stream).keep_alive(KeepAlive::default())
    }

    let handle = tokio::spawn(async move {
        let cx_for_patch = cx.clone();
        let session_id_for_patch = session_id.clone();
        let store_for_patch = store.clone();
        let patch_ids_for_patch = patch_message_ids.clone();
        let cx_for_callback = cx.clone();
        let store_for_callback = store.clone();
        let store_for_events = store.clone();
        let cx_for_events = cx.clone();
        let session_id_for_events = session_id.clone();
        let session_id_for_callback = session_id.clone();

        let app = Router::new()
            .route(
                "/a2a_callback",
                post(move |axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>, Json(body): Json<serde_json::Value>| {
                    let cx = cx_for_callback.clone();
                    let db = store_for_callback.clone();
                    let session_id_str = session_id_for_callback.clone();
                    async move {
                        let _child_context_id = body.get("contextId").and_then(|v| v.as_str()).unwrap_or(&session_id_str).to_string();
                        let caller = query.get("caller").cloned();
                        let callee = query.get("callee").cloned();
                        let caller_endpoint = query.get("caller_endpoint").cloned();

                        // A2A spec Section 4.3.3: StreamResponse is oneof { task, statusUpdate, artifactUpdate, message }
                        let (inner, schedule_id, state) = if let Some(task_val) = body.get("task") {
                            let tid = task_val.get("id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                            let st = task_val.pointer("/status/state").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            (task_val.clone(), tid, st)
                        } else if let Some(su) = body.get("statusUpdate") {
                            let tid = su.get("taskId").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                            let st = su.pointer("/status/state").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            (su.clone(), tid, st)
                        } else if let Some(au) = body.get("artifactUpdate") {
                            let tid = au.get("taskId").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                            (au.clone(), tid, "working".to_string())
                        } else if let Some(msg) = body.get("message") {
                            let tid = msg
                                .get("taskId")
                                .or_else(|| body.get("taskId"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let st = body
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("completed")
                                .to_string();
                            (msg.clone(), tid, st)
                        } else {
                            // Legacy fallback: body IS the task itself
                            let tid = body.get("id").and_then(|v| v.as_str())
                                .or_else(|| body.get("taskId").and_then(|v| v.as_str()))
                                .unwrap_or("unknown").to_string();
                            let st = body.pointer("/status/state").and_then(|v| v.as_str())
                                .or_else(|| body.get("status").and_then(|v| v.as_str()))
                                .unwrap_or("").to_string();
                            (body.clone(), tid, st)
                        };
                        let schedule_id = schedule_id.as_str();
                        let state = state.as_str();
                        info!("[A2A_Webhook] Received callback for Task ID {}. State: {}. Caller: {:?}, Callee: {:?}", schedule_id, state, caller, callee);
                        
                        let mut result_text = String::new();
                        // 1. Try to extract text from status.message.parts (works for both task and statusUpdate)
                        if let Some(parts) = inner
                            .pointer("/status/message/parts")
                            .or_else(|| inner.get("parts"))
                            && let Some(arr) = parts.as_array() {
                                for p in arr {
                                    if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                                        result_text.push_str(t);
                                    }
                                }
                            }
                        
                        // 2. If empty, try artifacts (task-level or artifactUpdate)
                        if result_text.is_empty() {
                            let artifacts = inner.get("artifacts")
                                .or_else(|| inner.get("artifact").map(|a| a));
                            if let Some(arts) = artifacts {
                                let art_list: Vec<&serde_json::Value> = if let Some(arr) = arts.as_array() {
                                    arr.iter().collect()
                                } else {
                                    vec![arts]
                                };
                                for a in art_list {
                                    if let Some(parts) = a.get("parts").and_then(|v| v.as_array()) {
                                        for p in parts {
                                            if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                                                result_text.push_str(t);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // 3. Still empty? Query the SQLite DB directly for the latest message by this agent
                        if result_text.is_empty() && (state == "completed" || state == "success") {
                            if let Some(target) = callee.as_deref() {
                                if let Ok(Some(content)) = db.get_latest_agent_content(&session_id_str, target) {
                                    result_text = content;
                                }
                            } else if let Ok(schedules) = db.list_schedules(&session_id_str)
                                && let Some(t) = schedules.iter().find(|t| t.get("id").and_then(|v| v.as_str()) == Some(schedule_id))
                                    && let Some(desc) = t.get("description").and_then(|v| v.as_str()) {
                                        // "leader → researcher" -> extract "researcher"
                                        if let Some(target) = desc.split("→").last().map(|s| s.trim())
                                            && let Ok(Some(content)) = db.get_latest_agent_content(&session_id_str, target) {
                                                result_text = content;
                                            }
                                    }
                        }

                        // Persist task update to DB
                        let _ = db.update_task_status(schedule_id, state, if result_text.is_empty() { None } else { Some(&result_text) });

                        if state == "completed" || state == "failed" {
                            info!("[A2A_Webhook] Task {} terminal state: {}. Notifying frontend.", schedule_id, state);
                            if let Ok(notif) = UntypedMessage::new(
                                crate::types::NOTIF_BACKGROUND_TASK_COMPLETED,
                                json!({
                                    "taskId": schedule_id,
                                    "status": state,
                                    "result": result_text
                                })
                            ) {
                                let _ = cx.send_notification_to(Client, notif);
                            }

                            // WAKE-UP Caller logic
                            if let Some(caller_role) = caller {
                                info!("[A2A_Webhook] System wake-up triggered for caller: {}", caller_role);
                                
                                let callee_role = callee.as_deref().unwrap_or("Agent");
                                // Inject a system message so the caller knows the result, showing up nicely in UI
                                let msg_content = format!("백그라운드 위임 작업이 완료되었습니다.\n- 담당자: {}\n- 작업상태: {}\n- 결과:\n{}", callee_role, state, result_text);
                                let _ = db.add_full_message(
                                    &session_id_str,
                                    "system",
                                    &msg_content,
                                    &caller_role.to_lowercase(),
                                    "",
                                    "",
                                    0, 0, 0, 0
                                );

                                if let Ok(notif) = UntypedMessage::new(
                                    crate::types::NOTIF_APP_SESSION_EVENT,
                                    json!({
                                        "engine": caller_role.to_lowercase(),
                                        "event": {
                                            "event": "message_delta",
                                            "thread_id": session_id_str,
                                            "turn_id": format!("a2a-completion-{}", schedule_id),
                                            "item_id": format!("a2a-completion-{}:{}", schedule_id, caller_role.to_lowercase()),
                                            "channel": "assistant",
                                            "delta": msg_content,
                                        }
                                    }),
                                ) {
                                    let _ = cx.send_notification_to(Client, notif);
                                }
                                if let Ok(notif) = UntypedMessage::new(
                                    crate::types::NOTIF_APP_SESSION_EVENT,
                                    json!({
                                        "engine": caller_role.to_lowercase(),
                                        "event": {
                                            "event": "turn_completed",
                                            "thread_id": session_id_str,
                                            "turn_id": format!("a2a-completion-{}", schedule_id),
                                            "status": "completed",
                                        }
                                    }),
                                ) {
                                    let _ = cx.send_notification_to(Client, notif);
                                }

                                // Trigger the caller via stateless HTTP
                                // Wait, to trigger the caller we need its endpoint.
                                // The caller's endpoint is something like `http://127.0.0.1:4321` (if leader)
                                // We can deduce the port from the role, but that's hardcoded.
                                // Better yet, the caller can just be polling, or we can send a stream patch.
                                // For now, let's just insert the system message. The user or the UI can resume the turn.
                                // A true wake-up would require making an HTTP POST to `caller_endpoint/v1/message:stream`
                                // with an empty message.
                                
                                // Use caller_endpoint from query param instead of hardcoded ports
                                let target_url = if let Some(ref ep) = caller_endpoint {
                                    format!("{}/", ep.trim_end_matches('/'))
                                } else {
                                    warn!("[A2A_Webhook] No caller_endpoint query param, cannot wake up caller {}", caller_role);
                                    return axum::http::StatusCode::OK;
                                };
                                
                                let wake_up_text = format!("A background task has just completed.\nTask ID: {}\nStatus: {}\nResult: {}\n\nPlease provide a final response or summary to the user.", schedule_id, state, result_text);
                                let payload = json!({
                                    "jsonrpc": "2.0",
                                    "id": uuid::Uuid::new_v4().to_string(),
                                    "method": "message/send",
                                    "params": {
                                        "message": {
                                            "kind": "message",
                                            "messageId": uuid::Uuid::new_v4().to_string(),
                                            "contextId": session_id_str,
                                            "role": "user",
                                            "parts": [{ "kind": "text", "text": wake_up_text }]
                                        }
                                    }
                                });

                                tokio::spawn(async move {
                                    let client = reqwest::Client::new();
                                    let res = client.post(&target_url)
                                        .header("Content-Type", "application/json")
                                        .json(&payload)
                                        .send()
                                        .await;
                                    match res {
                                        Ok(r) => {
                                            let status = r.status();
                                            let text = r.text().await.unwrap_or_default();
                                            if !status.is_success() {
                                                tracing::error!("[A2A_Webhook] Wake-up failed: {} - {}", status, text);
                                            } else {
                                                tracing::info!("[A2A_Webhook] Wake-up successful, response: {}", text);
                                            }
                                        }
                                        Err(e) => tracing::error!("[A2A_Webhook] Wake-up request error: {}", e),
                                    }
                                });
                            }
                        }

                        axum::http::StatusCode::OK
                    }
                })
            )
            .route(
                "/events",
                post(move |Json(body): Json<serde_json::Value>| {
                    let cx = cx_for_events.clone();
                    let db = store_for_events.clone();
                    let session_id = session_id_for_events.clone();
                    async move {
                        let sid = session_id.clone();
                        let from = body.get("from").and_then(|v| v.as_str()).unwrap_or("?");
                        let to = body.get("to").and_then(|v| v.as_str()).unwrap_or("?");
                        let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                        let schedule_id = body.get("taskId").and_then(|v| v.as_str()).unwrap_or("");
                        
                        let msg = format!("{} → {} ({})", from, to, status);
                        info!("[TeamWebhook] {}", msg);

                        // If it's a new task, register it in the DB
                        if !schedule_id.is_empty() {
                            if status == "scheduled" {
                                let _ = db.upsert_task(schedule_id, &sid, from, &format!("{} → {}", from, to));
                            } else {
                                let _ = db.update_task_status(schedule_id, status, None);
                            }
                            
                            // Real-time task update push for UI pulse/indictators
                            if let Ok(notif) = UntypedMessage::new(
                                crate::types::NOTIF_TASK_UPDATED,
                                json!({
                                    "sessionId": sid,
                                    "taskId": schedule_id,
                                    "agentId": from,
                                    "target": to,
                                    "status": status
                                })
                            ) {
                                let _ = cx.send_notification_to(Client, notif);
                            }
                        }
                        let to = body.get("to").and_then(|v| v.as_str()).unwrap_or("?");
                        let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                        let _result = body.get("result").and_then(|v| v.as_str()).unwrap_or("");
                        let msg = format!("{} → {} ({})", from, to, status);
                        info!("[TeamWebhook] {}", msg);
                        // Only show A2A events for agent↔agent communication,
                        // not tool calls (google_web_search, read_file, etc.)
                        // with scheduled/success system messages. The SwarmPulseBoard handles it.

                        // We no longer save tool_call and tool_result as separate DB rows here.
                        // Full detailed tool calls (with rawInput and rawOutput) are now saved
                        // inside the main 'assistant' message's tool_calls array via /stream_patch.

                        // NOTE: Do NOT send assistant_turn_patch from /events.
                        // The /stream_patch endpoint sends proper patches with toolCallId,
                        // rawInput, rawOutput, and contentBlocks. Sending from /events
                        // creates spurious empty message bubbles (no toolCallId) that
                        // get overwritten when /stream_patch arrives.

                        axum::http::StatusCode::OK
                    }
                }),
            )
            .route(
                "/stream_patch",
                post(move |Json(mut body): Json<serde_json::Value>| {
                    let cx = cx_for_patch.clone();
                    let db = store_for_patch.clone();
                    let patch_ids = patch_ids_for_patch.clone();
                    let session_id = session_id_for_patch.clone();
                    async move {
                        let context_id = body.get("contextId").and_then(|v| v.as_str()).unwrap_or(&session_id).to_string();
                        let _schedule_id = body.get("taskId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let mut agent_id_val = body.get("agentId").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                        
                        // If this patch comes from a child context, append its short ID to the agentId
                        // so multiple parallel agents of the same role don't overwrite each other's bubbles.
                        let id_to_use = &context_id;
                        if id_to_use != &session_id && !id_to_use.is_empty() {
                            let short_id = id_to_use.split('-').next().unwrap_or(id_to_use);
                            agent_id_val = format!("{}_{}", agent_id_val, short_id);
                            if let Some(obj) = body.as_object_mut() {
                                obj.insert("agentId".to_string(), json!(agent_id_val));
                            }
                        }

                        // ALWAYS save to the parent session so the UI can load it.
                        let sid = session_id.clone();
                        if let Some(obj) = body.as_object_mut() {
                            obj.insert("sessionId".to_string(), json!(sid));
                        }

                        let agent_id = body.get("agentId").and_then(|v| v.as_str()).unwrap_or("");
                        let patch_seq = body.get("patchSeq").and_then(|v| v.as_u64()).unwrap_or(1);
                        let is_final = body.get("final").and_then(|v| v.as_bool()).unwrap_or(false);
                        let _model_id = body.get("modelId").and_then(|v| v.as_str()).unwrap_or("");

                        if !agent_id.is_empty() {
                            let key = format!("{}::{}", sid, agent_id);
                            
                            // Insert empty message at start of turn to preserve chronological order
                            if patch_seq == 0 {
                                if let Ok(new_id) = db.add_full_message_with_blocks_returning_id(
                                    &sid, "assistant", "", agent_id, "", "", "", 0, 0, 0, 0
                                ) {
                                    patch_ids.lock().await.insert(key.clone(), new_id);
                                }
                                
                                // Dynamically ensure a real sub-session exists for this agent_id
                                let parent_cwd = db
                                    .get_session(&sid)
                                    .ok()
                                    .and_then(|s| s.map(|v| v.cwd))
                                    .filter(|cwd| !cwd.trim().is_empty())
                                    .unwrap_or_else(|| "/".to_string());
                                let _ = db.ensure_team_sub_session(&sid, agent_id, agent_id, &parent_cwd);
                            }

                            // Only persist content to DB on turn-complete (final=true)
                            if is_final {
                                let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");
                                let thinking = body.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                                let tool_calls = body.get("toolCalls")
                                    .map(|v| serde_json::to_string(v).unwrap_or_default())
                                    .unwrap_or_default();
                                let content_blocks = body.get("contentBlocks")
                                    .map(|v| serde_json::to_string(v).unwrap_or_default())
                                    .unwrap_or_default();
                                let duration = body.get("durationMs").and_then(|v| v.as_i64()).unwrap_or(0);
                                
                                let mut id_map = patch_ids.lock().await;
                                if let Some(msg_id) = id_map.remove(&key) {
                                    let _ = db.update_full_message_with_blocks_by_id(
                                        msg_id, content, thinking, &tool_calls, &content_blocks, 0, 0, 0, duration
                                    );
                                    info!("[StreamPatch] {} UPDATE final (id:{}, c:{}B t:{}B cb:{}B d:{}ms)",
                                        agent_id, msg_id, content.len(), thinking.len(), content_blocks.len(), duration);
                                } else {
                                    // Fallback if patchSeq == 0 was missed
                                    let _ = db.add_full_message_with_blocks(
                                        &sid, "assistant", content, agent_id,
                                        thinking, &tool_calls, &content_blocks,
                                        0, 0, 0, duration,
                                    );
                                    info!("[StreamPatch] {} INSERT final (c:{}B t:{}B cb:{}B d:{}ms)",
                                        agent_id, content.len(), thinking.len(), content_blocks.len(), duration);
                                }
                            }
                        }

                        let patch_session_id = body.get("sessionId").and_then(|v| v.as_str()).unwrap_or_default();
                        let patch_agent_id = body.get("agentId").and_then(|v| v.as_str()).unwrap_or_default();
                        let patch_content = body.get("content").and_then(|v| v.as_str()).unwrap_or_default();
                        let patch_seq = body.get("patchSeq").and_then(|v| v.as_u64()).unwrap_or(0);
                        let patch_final = body.get("final").and_then(|v| v.as_bool()).unwrap_or(false);
                        let patch_turn_id = format!("webhook-stream-{}-{}", patch_agent_id, patch_seq);
                        if !patch_session_id.is_empty() {
                            try_notify(&shared, crate::types::NOTIF_APP_SESSION_EVENT, json!({
                                "engine": patch_agent_id,
                                "event": {
                                    "event": "message_delta",
                                    "thread_id": patch_session_id,
                                    "turn_id": patch_turn_id,
                                    "item_id": format!("{}:{}", patch_turn_id, patch_agent_id),
                                    "channel": "assistant",
                                    "delta": patch_content,
                                }
                            })).await;
                            if patch_final {
                                try_notify(&shared, crate::types::NOTIF_APP_SESSION_EVENT, json!({
                                    "engine": patch_agent_id,
                                    "event": {
                                        "event": "turn_completed",
                                        "thread_id": patch_session_id,
                                        "turn_id": patch_turn_id,
                                        "status": "completed",
                                    }
                                })).await;
                            }
                        }
                        axum::http::StatusCode::OK
                    }
                })
            )
            .route("/stream", get(sse_handler))
            .with_state(sse_tx_shared);
        if let Err(e) = axum::serve(listener, app).await {
            warn!("[TeamWebhook] server error: {}", e);
        }
    });

    (url, handle, sse_tx, tracker)
}

/// Phase 1: Pre-bind a TCP port for the daemon webhook.
///
/// Called during TeamPreSpawn (before SharedState exists) to get a URL
/// that can be passed to `spawn_team_a2a_servers` via `TEAM_EVENT_WEBHOOK`.
///
/// Returns `(webhook_url, listener)`. Pass the listener to [`start_daemon_webhook`].
pub async fn prebind_webhook_port() -> Option<(String, tokio::net::TcpListener)> {
    match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => {
            let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
            let url = format!("http://127.0.0.1:{}/events", port);
            info!("[DaemonWebhook] Pre-bound port {} → {}", port, url);
            Some((url, listener))
        }
        Err(e) => {
            warn!("[DaemonWebhook] Failed to pre-bind port: {}", e);
            None
        }
    }
}

/// Phase 2: Start the daemon webhook server on a pre-bound listener.
///
/// Called after SharedState is constructed. Uses `relay_conductor_cx` for
/// lazy UI notifications and extracts session_id from request params.
pub fn start_daemon_webhook(
    listener: tokio::net::TcpListener,
    shared: std::sync::Arc<crate::SharedState>,
) -> tokio::task::JoinHandle<()> {
    use axum::{routing::post, Json, Router};

    let patch_message_ids: std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, i64>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));

    tokio::spawn({
        let shared = shared.clone();
        let patch_ids = patch_message_ids.clone();
        async move {
            async fn try_get_cx(shared: &crate::SharedState) -> Option<ConnectionTo<Conductor>> {
                shared.infra.relay_conductor_cx.latest().await
            }

            async fn try_notify(shared: &crate::SharedState, method: &str, params: serde_json::Value) {
                if let Some(cx) = try_get_cx(shared).await {
                    if let Ok(notif) = UntypedMessage::new(method, params) {
                        let _ = cx.send_notification_to(Client, notif);
                    }
                }
            }

            let shared_for_callback = shared.clone();
            let shared_for_events = shared.clone();
            let shared_for_patch = shared.clone();
            let patch_ids_for_patch = patch_ids.clone();

            let app = Router::new()
                .route(
                    "/a2a_callback",
                    post(move |axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>, Json(body): Json<serde_json::Value>| {
                        let shared = shared_for_callback.clone();
                        async move {
                            let session_id_str = body.get("contextId").and_then(|v| v.as_str())
                                .or_else(|| query.get("contextId").map(|s| s.as_str()))
                                .unwrap_or("unknown").to_string();
                            let caller = query.get("caller").cloned();
                            let callee = query.get("callee").cloned();
                            let caller_endpoint = query.get("caller_endpoint").cloned();

                            let (inner, schedule_id, state) = if let Some(task_val) = body.get("task") {
                                let tid = task_val.get("id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                                let st = task_val.pointer("/status/state").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                (task_val.clone(), tid, st)
                            } else if let Some(su) = body.get("statusUpdate") {
                                let tid = su.get("taskId").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                                let st = su.pointer("/status/state").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                (su.clone(), tid, st)
                            } else if let Some(au) = body.get("artifactUpdate") {
                                let tid = au.get("taskId").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                                (au.clone(), tid, "working".to_string())
                            } else if let Some(msg) = body.get("message") {
                                let tid = msg.get("taskId").or_else(|| body.get("taskId")).and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                                let st = body.get("status").and_then(|v| v.as_str()).unwrap_or("completed").to_string();
                                (msg.clone(), tid, st)
                            } else {
                                let tid = body.get("id").and_then(|v| v.as_str())
                                    .or_else(|| body.get("taskId").and_then(|v| v.as_str()))
                                    .unwrap_or("unknown").to_string();
                                let st = body.pointer("/status/state").and_then(|v| v.as_str())
                                    .or_else(|| body.get("status").and_then(|v| v.as_str()))
                                    .unwrap_or("").to_string();
                                (body.clone(), tid, st)
                            };
                            let schedule_id = schedule_id.as_str();
                            let state = state.as_str();
                            info!("[DaemonWebhook] a2a_callback: task={}, state={}, caller={:?}", schedule_id, state, caller);

                            let mut result_text = String::new();
                            if let Some(parts) = inner.pointer("/status/message/parts").or_else(|| inner.get("parts"))
                                && let Some(arr) = parts.as_array() {
                                    for p in arr {
                                        if let Some(t) = p.get("text").and_then(|v| v.as_str()) { result_text.push_str(t); }
                                    }
                                }
                            if result_text.is_empty() {
                                if let Some(arts) = inner.get("artifacts").or_else(|| inner.get("artifact")) {
                                    let art_list: Vec<&serde_json::Value> = if let Some(arr) = arts.as_array() { arr.iter().collect() } else { vec![arts] };
                                    for a in art_list {
                                        if let Some(parts) = a.get("parts").and_then(|v| v.as_array()) {
                                            for p in parts {
                                                if let Some(t) = p.get("text").and_then(|v| v.as_str()) { result_text.push_str(t); }
                                            }
                                        }
                                    }
                                }
                            }

                            let db = &shared.infra.brain.sessions();
                            let _ = db.update_task_status(schedule_id, state, if result_text.is_empty() { None } else { Some(&result_text) });

                            if state == "completed" || state == "failed" {
                                info!("[DaemonWebhook] Task {} terminal: {}", schedule_id, state);
                                try_notify(&shared, crate::types::NOTIF_BACKGROUND_TASK_COMPLETED, json!({
                                    "taskId": schedule_id, "status": state, "result": result_text
                                })).await;

                                if let Some(caller_role) = caller {
                                    let callee_role = callee.as_deref().unwrap_or("Agent");
                                    let msg_content = format!("백그라운드 위임 작업이 완료되었습니다.\n- 담당자: {}\n- 작업상태: {}\n- 결과:\n{}", callee_role, state, result_text);
                                    let _ = db.add_full_message(&session_id_str, "system", &msg_content, &caller_role.to_lowercase(), "", "", 0, 0, 0, 0);
                                    try_notify(&shared, crate::types::NOTIF_APP_SESSION_EVENT, json!({
                                        "engine": caller_role.to_lowercase(),
                                        "event": {
                                            "event": "message_delta",
                                            "thread_id": session_id_str,
                                            "turn_id": format!("daemon-a2a-completion-{}", schedule_id),
                                            "item_id": format!("daemon-a2a-completion-{}:{}", schedule_id, caller_role.to_lowercase()),
                                            "channel": "assistant",
                                            "delta": msg_content.clone(),
                                        }
                                    })).await;
                                    try_notify(&shared, crate::types::NOTIF_APP_SESSION_EVENT, json!({
                                        "engine": caller_role.to_lowercase(),
                                        "event": {
                                            "event": "turn_completed",
                                            "thread_id": session_id_str,
                                            "turn_id": format!("daemon-a2a-completion-{}", schedule_id),
                                            "status": "completed",
                                        }
                                    })).await;

                                    if let Some(ref ep) = caller_endpoint {
                                        let target_url = format!("{}/", ep.trim_end_matches('/'));
                                        let wake_text = format!("A background task completed.\nTask ID: {}\nStatus: {}\nResult: {}", schedule_id, state, result_text);
                                        let payload = json!({
                                            "jsonrpc": "2.0", "id": Uuid::new_v4().to_string(),
                                            "method": "message/send",
                                            "params": { "message": {
                                                "kind": "message", "messageId": Uuid::new_v4().to_string(),
                                                "contextId": session_id_str, "role": "user",
                                                "parts": [{ "kind": "text", "text": wake_text }]
                                            }}
                                        });
                                        tokio::spawn(async move {
                                            let client = reqwest::Client::new();
                                            match client.post(&target_url).json(&payload).send().await {
                                                Ok(r) if r.status().is_success() => info!("[DaemonWebhook] Wake-up sent to {}", target_url),
                                                Ok(r) => warn!("[DaemonWebhook] Wake-up failed: {}", r.status()),
                                                Err(e) => warn!("[DaemonWebhook] Wake-up error: {}", e),
                                            }
                                        });
                                    }
                                }
                            }
                            axum::http::StatusCode::OK
                        }
                    })
                )
                .route(
                    "/events",
                    post(move |Json(body): Json<serde_json::Value>| {
                        let shared = shared_for_events.clone();
                        async move {
                            let sid = body.get("contextId").and_then(|v| v.as_str())
                                .or_else(|| body.get("sessionId").and_then(|v| v.as_str()))
                                .unwrap_or("unknown").to_string();
                            let from = body.get("from").and_then(|v| v.as_str()).unwrap_or("?");
                            let to = body.get("to").and_then(|v| v.as_str()).unwrap_or("?");
                            let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                            let schedule_id = body.get("taskId").and_then(|v| v.as_str()).unwrap_or("");
                            info!("[DaemonWebhook] event: {} → {} ({})", from, to, status);

                            if !schedule_id.is_empty() {
                                let db = &shared.infra.brain.sessions();
                                if status == "scheduled" {
                                    let _ = db.upsert_task(schedule_id, &sid, from, &format!("{} → {}", from, to));
                                } else {
                                    let _ = db.update_task_status(schedule_id, status, None);
                                }
                                try_notify(&shared, crate::types::NOTIF_TASK_UPDATED, json!({
                                    "sessionId": sid, "taskId": schedule_id,
                                    "agentId": from, "target": to, "status": status
                                })).await;
                            }
                            axum::http::StatusCode::OK
                        }
                    })
                )
                .route(
                    "/stream_patch",
                    post(move |Json(mut body): Json<serde_json::Value>| {
                        let shared = shared_for_patch.clone();
                        let patch_ids = patch_ids_for_patch.clone();
                        async move {
                            let session_id = body.get("contextId").and_then(|v| v.as_str())
                                .or_else(|| body.get("sessionId").and_then(|v| v.as_str()))
                                .unwrap_or("unknown").to_string();
                            let agent_id = body.get("agentId").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                            let patch_seq = body.get("patchSeq").and_then(|v| v.as_u64()).unwrap_or(1);
                            let is_final = body.get("final").and_then(|v| v.as_bool()).unwrap_or(false);

                            if let Some(obj) = body.as_object_mut() {
                                obj.insert("sessionId".to_string(), json!(session_id));
                            }

                            let db = &shared.infra.brain.sessions();
                            if !agent_id.is_empty() {
                                let key = format!("{}::{}", session_id, agent_id);

                                if patch_seq == 0 {
                                    if let Ok(new_id) = db.add_full_message_with_blocks_returning_id(
                                        &session_id, "assistant", "", &agent_id, "", "", "", 0, 0, 0, 0
                                    ) {
                                        patch_ids.lock().await.insert(key.clone(), new_id);
                                    }
                                    let parent_cwd = db.get_session(&session_id).ok()
                                        .and_then(|s| s.map(|v| v.cwd))
                                        .filter(|cwd| !cwd.trim().is_empty())
                                        .unwrap_or_else(|| "/".to_string());
                                    let _ = db.ensure_team_sub_session(&session_id, &agent_id, &agent_id, &parent_cwd);
                                }

                                if is_final {
                                    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");
                                    let thinking = body.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                                    let tool_calls = body.get("toolCalls").map(|v| serde_json::to_string(v).unwrap_or_default()).unwrap_or_default();
                                    let content_blocks = body.get("contentBlocks").map(|v| serde_json::to_string(v).unwrap_or_default()).unwrap_or_default();
                                    let duration = body.get("durationMs").and_then(|v| v.as_i64()).unwrap_or(0);

                                    let mut id_map = patch_ids.lock().await;
                                    if let Some(msg_id) = id_map.remove(&key) {
                                        let _ = db.update_full_message_with_blocks_by_id(msg_id, content, thinking, &tool_calls, &content_blocks, 0, 0, 0, duration);
                                        info!("[DaemonWebhook] stream_patch UPDATE {} (id:{}, {}B)", agent_id, msg_id, content.len());
                                    } else {
                                        let _ = db.add_full_message_with_blocks(&session_id, "assistant", content, &agent_id, thinking, &tool_calls, &content_blocks, 0, 0, 0, duration);
                                        info!("[DaemonWebhook] stream_patch INSERT {} ({}B)", agent_id, content.len());
                                    }
                                }
                            }

                            let patch_session_id = body.get("sessionId").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                            let patch_agent_id = body.get("agentId").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                            let patch_content = body.get("content").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                            let patch_seq = body.get("patchSeq").and_then(|v| v.as_u64()).unwrap_or(0);
                            let patch_final = body.get("final").and_then(|v| v.as_bool()).unwrap_or(false);
                            if !patch_session_id.is_empty() {
                                let patch_turn_id = format!("daemon-webhook-stream-{}-{}", patch_agent_id, patch_seq);
                                try_notify(&shared, crate::types::NOTIF_APP_SESSION_EVENT, json!({
                                    "engine": patch_agent_id,
                                    "event": {
                                        "event": "message_delta",
                                        "thread_id": patch_session_id,
                                        "turn_id": patch_turn_id,
                                        "item_id": format!("{}:{}", patch_turn_id, patch_agent_id),
                                        "channel": "assistant",
                                        "delta": patch_content,
                                    }
                                })).await;
                                if patch_final {
                                    try_notify(&shared, crate::types::NOTIF_APP_SESSION_EVENT, json!({
                                        "engine": patch_agent_id,
                                        "event": {
                                            "event": "turn_completed",
                                            "thread_id": patch_session_id,
                                            "turn_id": patch_turn_id,
                                            "status": "completed",
                                        }
                                    })).await;
                                }
                            }
                            axum::http::StatusCode::OK
                        }
                    })
                );

            if let Err(e) = axum::serve(listener, app).await {
                warn!("[DaemonWebhook] server error: {}", e);
            }
        }
    })
}
