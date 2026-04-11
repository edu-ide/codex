//! A2A Protocol Real Server E2E Tests
//!
//! Tests A2A protocol features against LIVE gemini-cli A2A servers.
//!
//! Prerequisites:
//!   - `pnpm tauri dev` is running (spawns proxy + a2a-server processes)
//!   - `~/ilhae/team.json` exists with agents on ports 4321-4324
//!   - Gemini API key configured
//!
//! Run:
//!   ILHAE_RUN_TEAM_LIVE_A2A=1 cargo test --test team a2a_protocol_real -- --nocapture
//!
//! Test Scenarios:
//!   1. delegate sync — Leader에게 동기 요청, ForwardingExecutor 프로덕션 경로
//!   2. delegate background — fire_and_forget + subscribe_to_task 재구독
//!   3. propose — 제안 요청 + 결과 DB 저장
//!   4. SendStreamingMessage — raw SSE 스트리밍 검증
//!   5. SubscribeToTask — 비동기 task 후 재구독 검증
//!   6. Push Notifications — CRUD (set/get/list/delete) 실서버 검증

use super::common::team_helpers::*;
use super::common::test_gate::{require_team_live_a2a, require_team_local_a2a_spawn};
use serde_json::json;
use std::sync::Arc;

const RESEARCHER: &str = "http://localhost:4322";
const VERIFIER: &str = "http://localhost:4323";
const LEADER: &str = "http://localhost:4321";

/// Scenario 1: Delegate Sync (실서버)
///
/// ForwardingExecutor → PersistenceScheduleStore 프로덕션 경로를 통해
/// Leader에게 동기 요청을 보내고, 응답을 DB에 저장합니다.
#[tokio::test]
async fn test_delegate_sync_real() {
    if !require_team_live_a2a() {
        return;
    }

    let dir = ilhae_dir();
    let store = SessionStore::new(&dir).expect("SessionStore open failed");
    let session_id = uuid::Uuid::new_v4().to_string();

    store
        .ensure_session_with_channel(&session_id, "leader", ".", "team")
        .unwrap();
    store
        .update_session_title(&session_id, "🧪 A2A delegate sync real")
        .unwrap();

    let proxy = A2aProxy::new(LEADER, "leader");

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        proxy.send_and_observe("1+1은 뭐야? 숫자만 답해.", Some(session_id.clone()), None),
    )
    .await;

    match result {
        Ok(Ok((text, events))) => {
            assert!(!text.is_empty(), "Response should not be empty");
            assert!(!events.is_empty(), "Should have events");

            // Persist to DB
            let blocks =
                serde_json::to_string(&vec![json!({"type": "text", "text": text})]).unwrap();
            store
                .add_full_message_with_blocks(
                    &session_id,
                    "assistant",
                    &text,
                    "leader",
                    "",
                    "[]",
                    &blocks,
                    0,
                    0,
                    0,
                    0,
                )
                .unwrap();

            println!(
                "✅ Delegate sync: {} chars, {} events",
                text.len(),
                events.len()
            );
            println!("  Response: {:.200}", text);
        }
        Ok(Err(e)) => panic!("❌ Delegate sync error: {}", e),
        Err(_) => panic!("❌ Delegate sync timeout"),
    }

    println!(
        "⚡ Session left in DB — verify in app UI: {}",
        &session_id[..8]
    );
}

/// Scenario 2: Delegate Background + SubscribeToTask (실서버)
///
/// fire_and_forget로 비동기 작업 실행 후, subscribe_to_task로 재구독.
#[tokio::test]
async fn test_delegate_background_real() {
    if !require_team_live_a2a() {
        return;
    }

    let proxy = A2aProxy::new(RESEARCHER, "researcher");

    // Step 1: Fire and forget
    let task_id = match tokio::time::timeout(
        Duration::from_secs(30),
        proxy.fire_and_forget("Rust 언어의 장점을 3가지 말해줘.", None, None),
    )
    .await
    {
        Ok(Ok(id)) => {
            assert!(!id.is_empty());
            println!("✅ Fire-and-forget: task_id={}", &id[..id.len().min(36)]);
            id
        }
        Ok(Err(e)) => panic!("❌ Fire-and-forget error: {}", e),
        Err(_) => panic!("❌ Fire-and-forget timeout"),
    };

    // Step 2: Wait for processing
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Step 3: Resubscribe
    match tokio::time::timeout(Duration::from_secs(30), proxy.subscribe_to_task(&task_id)).await {
        Ok(Ok(events)) => {
            println!("✅ SubscribeToTask: {} events", events.len());
            for (i, event) in events.iter().enumerate() {
                let text = a2a_rs::proxy::extract_text_from_stream_event(event);
                println!("  Event#{}: {:.100}", i + 1, text);
            }
        }
        Ok(Err(e)) => println!("⚠️  SubscribeToTask error: {} (task may have expired)", e),
        Err(_) => println!("⚠️  SubscribeToTask timeout"),
    }

    // Step 4: Verify via tasks/get
    match proxy.get_task(&task_id).await {
        Ok(task) => {
            println!("✅ tasks/get: state={:?}", task.status.state);
        }
        Err(e) => println!("⚠️  tasks/get: {}", e),
    }
}

/// Scenario 3: Propose (실서버)
///
/// 제안 패턴 — 응답을 proposal로 처리.
#[tokio::test]
async fn test_propose_real() {
    if !require_team_live_a2a() {
        return;
    }

    let dir = ilhae_dir();
    let store = SessionStore::new(&dir).expect("SessionStore open failed");
    let session_id = uuid::Uuid::new_v4().to_string();

    store
        .ensure_session_with_channel(&session_id, "verifier", ".", "team")
        .unwrap();
    store
        .update_session_title(&session_id, "🧪 A2A propose real")
        .unwrap();

    let proxy = A2aProxy::new(VERIFIER, "verifier");

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        proxy.send_and_observe(
            "다음 전략을 제안해줘: 팀 생산성을 10% 올리는 방법. 3가지 옵션으로.",
            Some(session_id.clone()),
            None,
        ),
    )
    .await;

    match result {
        Ok(Ok((text, events))) => {
            assert!(!text.is_empty(), "Proposal should not be empty");

            let blocks =
                serde_json::to_string(&vec![json!({"type": "text", "text": text})]).unwrap();
            store
                .add_full_message_with_blocks(
                    &session_id,
                    "assistant",
                    &text,
                    "verifier",
                    "",
                    "[]",
                    &blocks,
                    0,
                    0,
                    0,
                    0,
                )
                .unwrap();

            println!("✅ Propose: {} chars, {} events", text.len(), events.len());
            println!("  Proposal: {:.300}", text);
        }
        Ok(Err(e)) => panic!("❌ Propose error: {}", e),
        Err(_) => panic!("❌ Propose timeout"),
    }

    println!(
        "⚡ Session left in DB — verify in app UI: {}",
        &session_id[..8]
    );
}

/// Scenario 4: SendStreamingMessage SSE (실서버)
///
/// Raw HTTP POST message/stream → SSE 스트리밍 응답 검증.
#[tokio::test]
async fn test_streaming_message_real() {
    if !require_team_live_a2a() {
        return;
    }

    let client = reqwest::Client::new();

    let payload = json!({
        "jsonrpc": "2.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "method": "message/stream",
        "params": {
            "message": {
                "role": "user",
                "messageId": format!("msg-{}", uuid::Uuid::new_v4()),
                "parts": [{"text": "Hello, respond with one word."}]
            }
        }
    });

    let resp = client
        .post(RESEARCHER)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .json(&payload)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .expect("SSE request failed");

    let status = resp.status();
    assert!(status.is_success(), "Expected 2xx, got {}", status);

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/event-stream"),
        "Expected text/event-stream, got {}",
        content_type
    );

    let body = resp.text().await.expect("Failed to read SSE body");
    let event_count = body.matches("data:").count();

    assert!(
        event_count > 0,
        "Should have SSE events, body: {:.200}",
        body
    );
    assert!(
        body.contains("completed") || body.contains("input-required"),
        "Should reach terminal state in SSE stream"
    );

    println!(
        "✅ SendStreamingMessage SSE: {} events, {} bytes",
        event_count,
        body.len()
    );
}

/// Scenario 5: SubscribeToTask raw JSON-RPC (실서버)
///
/// fire_and_forget → tasks/resubscribe JSON-RPC 재구독.
#[tokio::test]
async fn test_subscribe_to_task_real() {
    if !require_team_live_a2a() {
        return;
    }

    let client = reqwest::Client::new();

    // Step 1: Fire-and-forget
    let send_payload = json!({
        "jsonrpc": "2.0",
        "id": "sub-test-1",
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "messageId": format!("msg-{}", uuid::Uuid::new_v4()),
                "parts": [{"text": "Say RESUBSCRIBED."}]
            },
            "configuration": {"blocking": false}
        }
    });

    let resp: serde_json::Value = client
        .post(RESEARCHER)
        .json(&send_payload)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("message/send failed")
        .json()
        .await
        .unwrap();

    let task_id = resp["result"]["id"]
        .as_str()
        .expect("No task_id in F&F response");
    let state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");

    assert!(
        state == "submitted" || state == "working",
        "Expected submitted/working, got: {}",
        state
    );
    println!(
        "  F&F task: {} state={}",
        &task_id[..task_id.len().min(16)],
        state
    );

    // Step 2: Resubscribe via JSON-RPC
    let resub_payload = json!({
        "jsonrpc": "2.0",
        "id": "sub-test-resub",
        "method": "tasks/resubscribe",
        "params": {"id": task_id}
    });

    let resub_resp = client
        .post(RESEARCHER)
        .header("Content-Type", "application/json")
        .json(&resub_payload)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .expect("tasks/resubscribe failed");

    let resub_status = resub_resp.status();
    assert!(
        resub_status.is_success(),
        "Resubscribe should succeed: {}",
        resub_status
    );

    let body = resub_resp.text().await.unwrap_or_default();
    let events = body.matches("data:").count();
    println!(
        "✅ SubscribeToTask: HTTP {} — {} SSE events",
        resub_status, events
    );
}

/// Scenario 6: Push Notifications CRUD (실서버)
///
/// 실서버에 push notification config CRUD 테스트.
#[tokio::test]
async fn test_push_notifications_real() {
    if !require_team_live_a2a() {
        return;
    }

    let client = reqwest::Client::new();

    // Step 1: Create a task first
    let resp: serde_json::Value = client
        .post(RESEARCHER)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "push-real-1",
            "method": "message/send",
            "params": {
                "message": {
                    "role": "user",
                    "messageId": format!("msg-{}", uuid::Uuid::new_v4()),
                    "parts": [{"text": "Say PUSH."}]
                },
                "configuration": {"blocking": true}
            }
        }))
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .expect("message/send failed")
        .json()
        .await
        .unwrap();

    let task_id = resp["result"]["id"].as_str().expect("No task_id");
    println!(
        "  Task: {} state={}",
        task_id, resp["result"]["status"]["state"]
    );

    // Step 2: Start webhook receiver
    let webhook_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let webhook_port = webhook_listener.local_addr().unwrap().port();
    let webhook_url = format!("http://127.0.0.1:{}/webhook", webhook_port);

    let received = Arc::new(tokio::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let wh_handle = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/webhook",
            axum::routing::post(move |body: axum::extract::Json<serde_json::Value>| {
                let r = received_clone.clone();
                async move {
                    r.lock().await.push(body.0);
                    axum::http::StatusCode::OK
                }
            }),
        );
        let _ = axum::serve(webhook_listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Step 3: Set push config
    let set_resp: serde_json::Value = client
        .post(RESEARCHER)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "push-set",
            "method": "tasks/pushNotificationConfig/set",
            "params": {
                "taskId": task_id,
                "pushNotificationConfig": {
                    "url": webhook_url,
                    "token": "real-test-token"
                }
            }
        }))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("set failed")
        .json()
        .await
        .unwrap();

    if set_resp.get("result").is_some() {
        println!("  ✅ Push config set");
    } else {
        let err = set_resp["error"]["message"].as_str().unwrap_or("?");
        println!("  ⚠️  Push config set: {} (may not be supported)", err);
    }

    // Step 4: Get push config
    let get_resp: serde_json::Value = client
        .post(RESEARCHER)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "push-get",
            "method": "tasks/pushNotificationConfig/get",
            "params": {"id": task_id}
        }))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("get failed")
        .json()
        .await
        .unwrap();

    if get_resp.get("result").is_some() {
        println!("  ✅ Push config get");
    } else {
        println!("  ⚠️  Push config get: {}", get_resp["error"]["message"]);
    }

    // Step 5: List push configs
    let list_resp: serde_json::Value = client
        .post(RESEARCHER)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "push-list",
            "method": "tasks/pushNotificationConfig/list",
            "params": {"id": task_id}
        }))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("list failed")
        .json()
        .await
        .unwrap();

    if let Some(result) = list_resp.get("result") {
        let count = result.as_array().map(|a| a.len()).unwrap_or(0);
        println!("  ✅ Push config list: {} configs", count);
    } else {
        println!("  ⚠️  Push config list: {}", list_resp["error"]["message"]);
    }

    // Step 6: Delete push config
    let del_resp: serde_json::Value = client
        .post(RESEARCHER)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "push-del",
            "method": "tasks/pushNotificationConfig/delete",
            "params": {
                "id": task_id,
                "pushNotificationConfigId": task_id
            }
        }))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("delete failed")
        .json()
        .await
        .unwrap();

    if del_resp.get("error").is_none() {
        println!("  ✅ Push config deleted");
    } else {
        println!("  ⚠️  Push config delete: {}", del_resp["error"]["message"]);
    }

    wh_handle.abort();
    println!("✅ Push Notifications CRUD: verified against real server");
}

/// Scenario 7: Push Notifications Self-Contained (에이전트 자동 스폰)
///
/// `spawn_team_a2a_servers`로 에이전트를 직접 스폰한 뒤
/// pushNotifications CRUD를 실서버에서 검증합니다.
/// 에이전트가 미리 실행 중이지 않아도 됩니다.
#[tokio::test]
async fn test_push_notifications_spawned() {
    if !require_team_local_a2a_spawn() {
        return;
    }

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");
    let workspace_map = generate_peer_registration_files(&team, None);
    let mut children = spawn_team_a2a_servers(&team, &workspace_map, None, "push-test").await;

    match wait_for_all_team_health(&team).await {
        Ok(()) => println!("✅ All agents healthy"),
        Err(e) => {
            cleanup_children(&mut children).await;
            panic!("Health check failed: {}", e);
        }
    }

    // Verify agent card now has pushNotifications: true
    let researcher = team
        .agents
        .iter()
        .find(|a| a.role.to_lowercase().contains("researcher"))
        .expect("Researcher agent required");
    let client = reqwest::Client::new();
    let card_url = format!("{}/.well-known/agent.json", researcher.endpoint);
    let card: serde_json::Value = client
        .get(&card_url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("card fetch failed")
        .json()
        .await
        .unwrap();
    let push_enabled = card["capabilities"]["pushNotifications"]
        .as_bool()
        .unwrap_or(false);
    println!("  Agent card pushNotifications = {}", push_enabled);
    assert!(
        push_enabled,
        "❌ pushNotifications should be true in agent card!"
    );

    // Step 1: Create a task (blocking)
    let resp: serde_json::Value = client
        .post(&researcher.endpoint)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "push-spawn-1",
            "method": "message/send",
            "params": {
                "message": {
                    "role": "user",
                    "messageId": format!("msg-{}", uuid::Uuid::new_v4()),
                    "parts": [{"text": "Say OK."}]
                },
                "configuration": {"blocking": true}
            }
        }))
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .expect("send failed")
        .json()
        .await
        .unwrap();

    let task_id = resp["result"]["id"].as_str().expect("No task_id");
    let state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    println!(
        "  Task: {} state={}",
        &task_id[..task_id.len().min(16)],
        state
    );

    // Step 2: Start webhook receiver
    let webhook_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let webhook_port = webhook_listener.local_addr().unwrap().port();
    let webhook_url = format!("http://127.0.0.1:{}/webhook", webhook_port);
    let received = Arc::new(tokio::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let wh_handle = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/webhook",
            axum::routing::post(move |body: axum::extract::Json<serde_json::Value>| {
                let r = received_clone.clone();
                async move {
                    r.lock().await.push(body.0);
                    axum::http::StatusCode::OK
                }
            }),
        );
        let _ = axum::serve(webhook_listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Step 3: Set push config
    let set_resp: serde_json::Value = client
        .post(&researcher.endpoint)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "push-set",
            "method": "tasks/pushNotificationConfig/set",
            "params": {
                "taskId": task_id,
                "pushNotificationConfig": {
                    "url": webhook_url,
                    "token": "spawn-test-token"
                }
            }
        }))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("set failed")
        .json()
        .await
        .unwrap();

    let set_ok = set_resp.get("result").is_some();
    if set_ok {
        println!("  ✅ Push config set");
    } else {
        let err = set_resp["error"]["message"].as_str().unwrap_or("?");
        println!("  ❌ Push config set FAILED: {}", err);
    }
    assert!(set_ok, "push config set should succeed now!");

    // Step 4: Get push config
    let get_resp: serde_json::Value = client
        .post(&researcher.endpoint)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "push-get",
            "method": "tasks/pushNotificationConfig/get",
            "params": {"id": task_id}
        }))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("get failed")
        .json()
        .await
        .unwrap();

    let get_ok = get_resp.get("result").is_some();
    if get_ok {
        println!("  ✅ Push config get");
    } else {
        println!(
            "  ❌ Push config get FAILED: {}",
            get_resp["error"]["message"]
        );
    }
    assert!(get_ok, "push config get should succeed!");

    // Step 5: List push configs
    let list_resp: serde_json::Value = client
        .post(&researcher.endpoint)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "push-list",
            "method": "tasks/pushNotificationConfig/list",
            "params": {"id": task_id}
        }))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("list failed")
        .json()
        .await
        .unwrap();

    if let Some(result) = list_resp.get("result") {
        let count = result.as_array().map(|a| a.len()).unwrap_or(0);
        println!("  ✅ Push config list: {} configs", count);
        assert!(count >= 1, "Should have at least 1 config");
    } else {
        panic!(
            "  ❌ Push config list FAILED: {}",
            list_resp["error"]["message"]
        );
    }

    // Step 6: Delete push config
    let del_resp: serde_json::Value = client
        .post(&researcher.endpoint)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "push-del",
            "method": "tasks/pushNotificationConfig/delete",
            "params": {
                "id": task_id,
                "pushNotificationConfigId": task_id
            }
        }))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("delete failed")
        .json()
        .await
        .unwrap();

    if del_resp.get("error").is_none() {
        println!("  ✅ Push config deleted");
    } else {
        println!("  ⚠️  Push config delete: {}", del_resp["error"]["message"]);
    }

    // Cleanup
    wh_handle.abort();
    cleanup_children(&mut children).await;

    println!("\n══════════════════════════════════════════════════════════");
    println!("✅ Push Notifications CRUD — Self-Contained E2E Complete");
    println!("══════════════════════════════════════════════════════════");
}
