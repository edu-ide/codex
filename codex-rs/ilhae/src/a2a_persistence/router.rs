use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::{debug, info, warn};

use super::forwarding_executor::DelegationResponseCache;
use crate::CxCache;

/// Shared routing map: role → (endpoint, is_main). Updatable at runtime.
pub type RoutingMap = Arc<tokio::sync::RwLock<HashMap<String, (String, bool)>>>;

// ═════════════════════════════════════════════════════════════════════
// Server startup
// ═════════════════════════════════════════════════════════════════════

/// Build routing table from team runtime config.
/// Build routing table from team config: `[(role, endpoint, is_main)]`.
pub fn build_routing_table(
    team: &crate::context_proxy::TeamRuntimeConfig,
) -> Vec<(String, String, bool)> {
    team.agents
        .iter()
        .map(|a| {
            (
                a.role.to_lowercase(),
                a.endpoint.trim_end_matches('/').to_string(),
                a.is_main,
            )
        })
        .collect()
}

/// Build a `RoutingMap` from a flat routing table slice.
pub fn build_routing_map(routing_table: &[(String, String, bool)]) -> RoutingMap {
    let map: HashMap<String, (String, bool)> = routing_table
        .iter()
        .map(|(role, endpoint, is_main)| {
            (
                role.clone(),
                (endpoint.trim_end_matches('/').to_string(), *is_main),
            )
        })
        .collect();
    Arc::new(tokio::sync::RwLock::new(map))
}

/// Update an existing routing map in-place from a new team config.
pub async fn update_routing_map(
    routing_map: &RoutingMap,
    team: &crate::context_proxy::TeamRuntimeConfig,
) {
    let new_map: HashMap<String, (String, bool)> = team
        .agents
        .iter()
        .map(|a| {
            (
                a.role.to_lowercase(),
                (a.endpoint.trim_end_matches('/').to_string(), a.is_main),
            )
        })
        .collect();
    info!("[A2aProxy] Updating routing map: {} agents", new_map.len());
    for (role, (endpoint, is_main)) in &new_map {
        info!(
            "[A2aProxy]   /a2a/{} → {} (main={})",
            role, endpoint, is_main
        );
    }
    *routing_map.write().await = new_map;
}

/// Build the A2A proxy router — transparent reverse proxy, NO nest().
///
/// Single fallback handler parses `/a2a/{role}[/rest]` from the raw URI path,
/// looks up the target endpoint, and forwards the request via reqwest.
/// Agent card responses get `url` overridden to point back through the proxy.
/// Sub-agent responses are tapped to persist delegation events.
pub fn build_proxy_router(
    routing_table: &[(String, String, bool)],
    event_tx: tokio::sync::broadcast::Sender<crate::a2a_persistence::events::DelegationEvent>,
    _cx_cache: CxCache,
    _delegation_cache: DelegationResponseCache,
    base_url: &str,
) -> (axum::Router, RoutingMap) {
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .expect("Failed to build reqwest client");

    let routing_map = build_routing_map(routing_table);
    let base_url = base_url.trim_end_matches('/').to_string();

    {
        let rt = tokio::runtime::Handle::try_current();
        if let Ok(handle) = rt {
            let map = routing_map.clone();
            handle.spawn(async move {
                let map = map.read().await;
                for (role, (endpoint, is_main)) in map.iter() {
                    info!(
                        "[A2aProxy] Reverse proxy: /a2a/{} → {} (main={})",
                        role, endpoint, is_main
                    );
                }
            });
        }
    }

    // Shared state: per-context leader session_id map (fixes race condition with concurrent sessions)
    let leader_session_map: Arc<tokio::sync::RwLock<HashMap<String, String>>> =
        Arc::new(tokio::sync::RwLock::new(HashMap::new()));

    let handler = {
        let map = routing_map.clone();
        let base = base_url;
        let client = http_client;
        let event_tx = event_tx;
        let leader_sid = leader_session_map;

        move |req: axum::extract::Request| {
            let map = map.clone();
            let base = base.clone();
            let client = client.clone();
            let event_tx = event_tx.clone();
            let leader_sid = leader_sid.clone();

            async move {
                let map_guard = map.read().await;
                reverse_proxy_handler(req, &map_guard, &base, &client, &event_tx, &leader_sid).await
            }
        }
    };

    (axum::Router::new().fallback(handler), routing_map)
}

/// Reverse proxy handler — no nest(), no path stripping.
///
/// Parses `/a2a/{role}[/rest]` from the full request URI, looks up the target
/// endpoint, and forwards the entire request. For agent card responses, overrides
/// the `url` field. For sub-agent responses, taps to persist delegation events.
async fn reverse_proxy_handler(
    req: axum::extract::Request,
    routing_map: &HashMap<String, (String, bool)>,
    base_url: &str,
    client: &reqwest::Client,
    event_tx: &tokio::sync::broadcast::Sender<crate::a2a_persistence::events::DelegationEvent>,
    leader_session_map: &tokio::sync::RwLock<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let method = req.method().clone();
    let uri = req.uri().clone();
    let full_path = uri.path();
    let headers = req.headers().clone();

    // Parse: /a2a/{role}[/rest]
    let after_a2a = match full_path.strip_prefix("/a2a/") {
        Some(rest) => rest,
        None => {
            warn!("[A2aProxy] Path does not start with /a2a/: {}", full_path);
            return (StatusCode::NOT_FOUND, "Not an A2A proxy path").into_response();
        }
    };

    let (role, rest_path) = match after_a2a.find('/') {
        Some(idx) => (&after_a2a[..idx], &after_a2a[idx..]), // "/rest/path..."
        None => (after_a2a, "/"),                            // just role, no trailing
    };

    let (endpoint, is_main) = match routing_map.get(role) {
        Some((ep, main)) => (ep.as_str(), *main),
        None => {
            warn!("[A2aProxy] Unknown agent role: {}", role);
            return (StatusCode::NOT_FOUND, format!("Unknown agent: {}", role)).into_response();
        }
    };

    let proxy_base = format!("{}/a2a/{}", base_url, role);

    // Target URL = endpoint + rest_path (the part after /a2a/{role})
    let target_url = format!("{}{}", endpoint, rest_path);
    debug!(
        "[A2aProxy:{}] {} {} → {}",
        role, method, full_path, target_url
    );

    // Agent card request?
    let is_agent_card = rest_path.contains(".well-known/agent.json")
        || rest_path.contains(".well-known/agent.json");

    // Read request body
    let body_bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            warn!("[A2aProxy:{}] Failed to read request body: {}", role, e);
            return (StatusCode::BAD_REQUEST, format!("Bad request: {}", e)).into_response();
        }
    };

    // Build outgoing reqwest request
    let mut out_req = client.request(
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::POST),
        &target_url,
    );

    // Forward headers (skip hop-by-hop)
    for (name, value) in headers.iter() {
        let n = name.as_str().to_lowercase();
        if matches!(
            n.as_str(),
            "host" | "connection" | "transfer-encoding" | "te" | "trailer"
        ) {
            continue;
        }
        if let Ok(v) = reqwest::header::HeaderValue::from_bytes(value.as_bytes()) {
            out_req = out_req.header(name.as_str(), v);
        }
    }

    let mut final_body_bytes = body_bytes.to_vec();

    if !body_bytes.is_empty() {
        if let Ok(mut req_json) = serde_json::from_slice::<Value>(&body_bytes) {
            // SubAgent Isolation Check
            let subagent_val_opt = headers
                .get("x-ilhae-subagent")
                .and_then(|v| v.to_str().ok().map(|s| s.to_string()))
                .or_else(|| {
                    req_json
                        .pointer("/params/message/metadata/x-ilhae-subagent")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                });
            let is_subagent_spawn = subagent_val_opt.is_some();

            // 1. Leader Session Tracker.
            let mut captured_ctx_id = String::new();
            if let Some(ctx_id) = req_json
                .pointer("/params/message/contextId")
                .or_else(|| req_json.pointer("/params/contextId"))
                .and_then(|v| v.as_str())
            {
                captured_ctx_id = ctx_id.to_string();
                if is_main {
                    leader_session_map
                        .write()
                        .await
                        .insert(ctx_id.to_string(), ctx_id.to_string());
                    debug!("[A2aProxy] Leader session_id captured: {}", ctx_id);
                }
            }

            // 2. SubAgent Spawn Isolation handling
            if !is_main && is_subagent_spawn && !captured_ctx_id.is_empty() {
                let subagent_purpose =
                    subagent_val_opt.unwrap_or_else(|| "specialized worker".to_string());
                // FORCE complete isolation from the main context.
                let isolated_session_id =
                    format!("subagent_{}_{}", subagent_purpose, uuid::Uuid::new_v4());
                info!(
                    "[A2aProxy:{}] SubAgent spawn detected, isolating: {} → {}",
                    role, captured_ctx_id, isolated_session_id
                );

                // Override the Context ID so the target agent starts completely fresh!
                if let Some(obj) = req_json
                    .pointer_mut("/params/message")
                    .and_then(|v| v.as_object_mut())
                {
                    obj.insert(
                        "contextId".to_string(),
                        Value::String(isolated_session_id.clone()),
                    );

                    // --- DYNAMIC PROMPT INJECTION ---
                    // Prepend a strict system instruction to the agent's parts array.
                    if let Some(parts) = obj.get_mut("parts").and_then(|p| p.as_array_mut()) {
                        let sys_prompt = format!(
                            "[[SYSTEM INSTRUCTION: You are a one-off SUBAGENT spawned specifically for the '{}' role. Focus ONLY on this purpose and ignore irrelevant chat history.]]\n\n",
                            subagent_purpose
                        );
                        let prompt_json = serde_json::json!({
                            "type": "text",
                            "text": sys_prompt
                        });
                        parts.insert(0, prompt_json);
                        debug!(
                            "[A2aProxy:{}] Injected prompt for '{}'",
                            role, subagent_purpose
                        );
                    }
                }
                if let Some(obj) = req_json
                    .pointer_mut("/params")
                    .and_then(|v| v.as_object_mut())
                {
                    if obj.contains_key("contextId") {
                        obj.insert(
                            "contextId".to_string(),
                            Value::String(isolated_session_id.clone()),
                        );
                    }
                }

                // Track start (We map the subagent spawn back to the leader's session)
                let leader_role = routing_map
                    .iter()
                    .find(|(_, (_, m))| *m)
                    .map(|(r, _)| r.as_str())
                    .unwrap_or("leader");
                let request_text = req_json
                    .pointer("/params/message/parts")
                    .and_then(|v| v.as_array())
                    .map(|parts| extract_text_from_json_parts(parts))
                    .unwrap_or_default();

                let _ = event_tx.send(crate::a2a_persistence::events::DelegationEvent::Started {
                    leader_session_id: captured_ctx_id.clone(),
                    target_role: role.to_string(),
                    leader_role: leader_role.to_string(),
                    mode: "async_subagent".to_string(),
                    request_text,
                    channel_id: "a2a:delegation_start".to_string(),
                });

                if let Ok(new_body_bytes) = serde_json::to_vec(&req_json) {
                    final_body_bytes = new_body_bytes;
                }
            } else if !is_main {
                // ── Pre-send: persist delegation_start for NORMAL sub-agent requests ──
                let method = req_json
                    .get("method")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if method == "message/send" || method == "message/stream" {
                    let map_guard = leader_session_map.read().await;
                    let leader_sid = map_guard
                        .get(&captured_ctx_id)
                        .cloned()
                        .or_else(|| map_guard.values().last().cloned())
                        .unwrap_or_default();
                    drop(map_guard);
                    let leader_role = routing_map
                        .iter()
                        .find(|(_, (_, m))| *m)
                        .map(|(r, _)| r.as_str())
                        .unwrap_or("leader");
                    let request_text = req_json
                        .pointer("/params/message/parts")
                        .and_then(|v| v.as_array())
                        .map(|parts| extract_text_from_json_parts(parts))
                        .unwrap_or_default();
                    if !leader_sid.is_empty() {
                        let _ = event_tx.send(
                            crate::a2a_persistence::events::DelegationEvent::Started {
                                leader_session_id: leader_sid.clone(),
                                target_role: role.to_string(),
                                leader_role: leader_role.to_string(),
                                mode: "sync".to_string(),
                                request_text,
                                channel_id: "a2a:delegation_start".to_string(),
                            },
                        );
                        debug!("[A2aProxy:{}] delegation_start persisted (pre-send)", role);
                    }
                }
            }
        }
    }

    drop(out_req); // not used — we build fresh requests per attempt

    let start_time = Instant::now();

    // Send with retry (max 2 retries for transient failures)
    let resp = {
        let mut last_err = String::new();
        let mut result = None;
        for attempt in 0..3u32 {
            if attempt > 0 {
                let backoff = Duration::from_millis(500 * 2u64.pow(attempt - 1));
                warn!("[A2aProxy:{}] Retry {} after {:?}", role, attempt, backoff);
                tokio::time::sleep(backoff).await;
            }
            let mut req_builder = client.request(
                reqwest::Method::from_bytes(method.as_str().as_bytes())
                    .unwrap_or(reqwest::Method::POST),
                &target_url,
            );
            for (name, value) in headers.iter() {
                let n = name.as_str().to_lowercase();
                if matches!(
                    n.as_str(),
                    "host" | "connection" | "transfer-encoding" | "te" | "trailer"
                ) {
                    continue;
                }
                if let Ok(v) = reqwest::header::HeaderValue::from_bytes(value.as_bytes()) {
                    req_builder = req_builder.header(name.as_str(), v);
                }
            }
            if !final_body_bytes.is_empty() {
                req_builder = req_builder.body(final_body_bytes.clone());
            }
            match req_builder.send().await {
                Ok(r) if r.status().is_server_error() => {
                    last_err = format!("HTTP {}", r.status());
                    continue;
                }
                Ok(r) => {
                    result = Some(r);
                    break;
                }
                Err(e) if e.is_connect() || e.is_timeout() => {
                    last_err = e.to_string();
                    continue;
                }
                Err(e) => {
                    warn!("[A2aProxy:{}] Forward error: {}", role, e);
                    return (StatusCode::BAD_GATEWAY, format!("Upstream error: {}", e))
                        .into_response();
                }
            }
        }
        match result {
            Some(r) => r,
            None => {
                warn!("[A2aProxy:{}] All retries exhausted: {}", role, last_err);
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("Upstream error after retries: {}", last_err),
                )
                    .into_response();
            }
        }
    };

    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let content_type = resp_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // ── Agent card: override url to point through proxy ──
    if is_agent_card && status.is_success() {
        let card_bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => {
                warn!("[A2aProxy:{}] Failed to read agent card: {}", role, e);
                return (StatusCode::BAD_GATEWAY, "Failed to read agent card").into_response();
            }
        };
        if let Ok(mut card) = serde_json::from_slice::<Value>(&card_bytes) {
            let proxy_url = format!("{}/", proxy_base.trim_end_matches('/'));
            card["url"] = Value::String(proxy_url.clone());
            // Disable extended card — proxy handles all routing
            card["supportsAuthenticatedExtendedCard"] = Value::Bool(false);
            if let Some(ifaces) = card
                .get_mut("supportedInterfaces")
                .and_then(|v| v.as_array_mut())
            {
                for iface in ifaces {
                    iface["url"] = Value::String(proxy_url.clone());
                }
            }
            debug!("[A2aProxy:{}] Agent card url → {}", role, proxy_url);
            let body = serde_json::to_vec(&card).unwrap_or_default();
            return axum::response::Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
        }
        // JSON parse fail → return as-is
        return axum::response::Response::builder()
            .status(status.as_u16())
            .header("content-type", &content_type)
            .body(Body::from(card_bytes.to_vec()))
            .unwrap();
    }

    // ── SSE stream: forward as streaming body + persist delegation start ──
    if content_type.contains("text/event-stream") {
        // Record that a streaming delegation started (completion tracked via push notifications)
        if !is_main {
            let leader_sid = {
                let map = leader_session_map.read().await;
                map.values().last().cloned().unwrap_or_default()
            };
            if !leader_sid.is_empty() {
                let leader_role = routing_map
                    .iter()
                    .find(|(_, (_, m))| *m)
                    .map(|(r, _)| r.as_str())
                    .unwrap_or("leader");
                let _ = event_tx.send(crate::a2a_persistence::events::DelegationEvent::Started {
                    leader_session_id: leader_sid.clone(),
                    target_role: role.to_string(),
                    leader_role: leader_role.to_string(),
                    mode: "stream".to_string(),
                    request_text: "(SSE streaming)".to_string(),
                    channel_id: "a2a:delegation_stream_start".to_string(),
                });
                debug!(
                    "[A2aProxy:{}] SSE delegation_start persisted for session={}",
                    role, leader_sid
                );
            }
        }
        let stream = resp.bytes_stream();
        let body = Body::from_stream(stream);
        return axum::response::Response::builder()
            .status(status.as_u16())
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(body)
            .unwrap();
    }

    // ── Read full response body ──
    let resp_bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            warn!("[A2aProxy:{}] Failed to read response: {}", role, e);
            return (StatusCode::BAD_GATEWAY, "Failed to read upstream response").into_response();
        }
    };

    // ── Post-receive: persist delegation_response + delegation_complete ──
    if !is_main && status.is_success() && content_type.contains("application/json") {
        let duration_ms = start_time.elapsed().as_millis() as i64;
        let leader_sid = {
            let map = leader_session_map.read().await;
            map.values().last().cloned().unwrap_or_default()
        };
        let leader_role = routing_map
            .iter()
            .find(|(_, (_, m))| *m)
            .map(|(r, _)| r.as_str())
            .unwrap_or("leader");
        tap_delegation_result(
            event_tx,
            role,
            &resp_bytes,
            duration_ms,
            &leader_sid,
            leader_role,
        );
    }

    // ── All other responses: forward as-is ──
    let mut builder = axum::response::Response::builder().status(status.as_u16());
    for (name, value) in resp_headers.iter() {
        let n = name.as_str().to_lowercase();
        if matches!(n.as_str(), "transfer-encoding" | "connection") {
            continue;
        }
        builder = builder.header(name.as_str(), value);
    }
    builder.body(Body::from(resp_bytes.to_vec())).unwrap()
}

// ═════════════════════════════════════════════════════════════════════
// Delegation tap — extract and persist delegation data from JSON-RPC
// ═════════════════════════════════════════════════════════════════════

/// Extract text from JSON parts array: `[{"kind":"text","text":"..."},...]`
fn extract_text_from_json_parts(parts: &[Value]) -> String {
    parts
        .iter()
        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("")
}

/// Persist delegation response and completion after receiving sub-agent response.
///
/// delegation_start is written pre-send (in reverse_proxy_handler).
/// This function writes delegation_response + delegation_complete post-receive.
fn tap_delegation_result(
    event_tx: &tokio::sync::broadcast::Sender<crate::a2a_persistence::events::DelegationEvent>,
    role: &str,
    resp_body: &[u8],
    duration_ms: i64,
    leader_session_id: &str,
    leader_role: &str,
) {
    if leader_session_id.is_empty() {
        debug!(
            "[A2aProxy:{}] No leader session_id for delegation result, skipping",
            role
        );
        return;
    }

    // Parse response JSON-RPC
    let resp_json: Value = match serde_json::from_slice(resp_body) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Extract response text from result.status.message.parts or result.artifacts
    let response_text = resp_json
        .pointer("/result/status/message/parts")
        .and_then(|v| v.as_array())
        .map(|parts| extract_text_from_json_parts(parts))
        .or_else(|| {
            resp_json
                .pointer("/result/artifacts")
                .and_then(|v| v.as_array())
                .and_then(|artifacts| artifacts.first())
                .and_then(|a| a.get("parts"))
                .and_then(|v| v.as_array())
                .map(|parts| extract_text_from_json_parts(parts))
        })
        .unwrap_or_default();

    if response_text.is_empty() {
        debug!("[A2aProxy:{}] Empty response text from delegation", role);
        return;
    }

    // Extract A2A task metadata
    let schedule_id = resp_json
        .pointer("/result/id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let task_state = resp_json
        .pointer("/result/status/state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    info!(
        "[A2aProxy:{}] Delegation result: session={}, task={}, state={}, {}ms",
        role, leader_session_id, schedule_id, task_state, duration_ms
    );

    // ── delegation_response (sub-agent's answer + task metadata) ──
    let artifacts = resp_json
        .pointer("/result/artifacts")
        .cloned()
        .unwrap_or(serde_json::json!([]));
    let history = resp_json
        .pointer("/result/history")
        .cloned()
        .unwrap_or(serde_json::json!([]));

    let _ = event_tx.send(
        crate::a2a_persistence::events::DelegationEvent::ResultTapped {
            leader_session_id: leader_session_id.to_string(),
            target_role: role.to_string(),
            leader_role: leader_role.to_string(),
            response_text,
            schedule_id: schedule_id.to_string(),
            task_state: task_state.to_string(),
            duration_ms,
            artifacts,
            history,
        },
    );

    info!(
        "[A2aProxy:{}] Delegation persisted: session={}, task={}, state={}, {}ms",
        role, leader_session_id, schedule_id, task_state, duration_ms
    );
}
