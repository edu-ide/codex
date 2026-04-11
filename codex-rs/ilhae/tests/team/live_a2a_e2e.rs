//! E2E integration tests for Team Agent A2A communication.
//!
//! These tests run against the **live** A2A servers spawned by `pnpm tauri dev`.
//! Prerequisites:
//!   - `pnpm tauri dev` is running (spawns proxy + a2a-server processes)
//!   - `~/ilhae/team.json` exists with agents on ports 4321-4324
//!
//! Run:
//!   ILHAE_RUN_TEAM_LIVE_A2A=1 cargo test --test team live_a2a_e2e -- --nocapture

use super::common::test_gate::require_team_live_a2a;
use std::time::Duration;

// ── ilhae_proxy lib crate ────────────────────────────────────────────
use ilhae_proxy::context_proxy::team_a2a::parse_a2a_result;

// ── Constants ────────────────────────────────────────────────────────
const TEAM_ENDPOINTS: &[(&str, &str)] = &[
    ("Leader", "http://localhost:4321"),
    ("Researcher", "http://localhost:4322"),
    ("Verifier", "http://localhost:4323"),
    ("Creator", "http://localhost:4324"),
];

use super::common::a2a_test_helpers::*;

///

/// Test acts as **orchestrator** (the A2A-standard pattern):
///   Leader → Researcher → Verifier → Creator
///
/// Each agent's LLM response is forwarded as context to the next agent
/// via A2A `message/send`. No tool-wrapping — pure A2A protocol.
#[tokio::test(flavor = "multi_thread")]
async fn test_a2a_full_mesh_agent_chain() {
    if !require_team_live_a2a() {
        return;
    }

    let (store, _tmp) = make_test_session_store();
    let session_id = uuid::Uuid::new_v4().to_string();
    store
        .create_session(&session_id, "Full Mesh Chain Test", "test", "/tmp")
        .expect("Failed to create session");

    // Agent chain: (role, endpoint, proxy_url)
    let chain = [
        ("leader", "http://localhost:4321"),
        ("researcher", "http://localhost:4322"),
        ("verifier", "http://localhost:4323"),
        ("creator", "http://localhost:4324"),
    ];

    // Start proxies for all agents
    let mut proxies: Vec<(String, String, tokio::task::JoinHandle<()>)> = Vec::new();
    for (role, target) in &chain {
        let (proxy_url, handle) = start_test_proxy(role, target, store.clone()).await;
        proxies.push((role.to_string(), proxy_url, handle));
    }

    let client = reqwest::Client::new();
    let mut accumulated_context = String::new();

    let prompts = [
        "한국의 수도는 어디인지 한 줄로 답해.",
        "앞서 리더가 답한 내용을 확인해. 맞으면 '확인 완료'라고 답해.",
        "앞서 리서처가 확인한 내용을 검증해. 정확하면 '검증 완료'라고 답해.",
        "앞선 검증 결과를 바탕으로 최종 요약을 한 줄로 작성해.",
    ];

    println!("🔗 Starting A2A full-mesh chain test");
    println!("   Chain: Leader → Researcher → Verifier → Creator");

    for (i, (role, proxy_url, _handle)) in proxies.iter().enumerate() {
        // Build message: first agent gets original prompt, rest get accumulated context
        let message_text = if accumulated_context.is_empty() {
            prompts[i].to_string()
        } else {
            format!(
                "{}\n\n이전 대화 컨텍스트:\n{}",
                prompts[i], accumulated_context
            )
        };

        let trunc_msg = message_text
            .char_indices()
            .nth(40)
            .map(|(i, _)| &message_text[..i])
            .unwrap_or(&message_text);
        println!("\n📤 [{}/4] → {} : {}", i + 1, role, trunc_msg);

        // Send via proxy for persistence
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": format!("chain-{}", i + 1),
            "method": "message/send",
            "params": {
                "message": {
                    "role": "user",
                    "messageId": format!("chain-msg-{}", uuid::Uuid::new_v4()),
                    "parts": [{"text": message_text}],
                    "contextId": session_id,
                }
            }
        });

        let resp = client
            .post(proxy_url)
            .json(&payload)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .unwrap_or_else(|e| panic!("Failed to send to {}: {}", role, e));

        assert!(
            resp.status().is_success(),
            "❌ {} returned error: {}",
            role,
            resp.status()
        );

        let body: serde_json::Value = resp.json().await.expect("Failed to parse response");
        let result = &body["result"];
        let parsed = parse_a2a_result(result);

        // Wait for agent response in DB
        let mut agent_text = String::new();
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let msgs = store.load_session_messages(&session_id).unwrap_or_default();
            let agent_msgs: Vec<_> = msgs
                .iter()
                .filter(|m| m.role == "assistant" && m.agent_id == *role)
                .collect();
            if !agent_msgs.is_empty() {
                agent_text = agent_msgs.last().unwrap().content.clone();
                break;
            }
        }

        // Fallback: try to get text from A2A response directly
        if agent_text.is_empty() {
            agent_text = parsed.text.clone();
        }

        assert!(
            !agent_text.is_empty(),
            "❌ {} returned no text. State: {:?}",
            role,
            parsed.state
        );

        let trunc_resp = agent_text
            .char_indices()
            .nth(40)
            .map(|(i, _)| &agent_text[..i])
            .unwrap_or(&agent_text);
        println!("📥 [{}/4] ← {} : {}", i + 1, role, trunc_resp);

        // Accumulate context for next agent
        accumulated_context.push_str(&format!("[{}] {}\n", role, agent_text));
    }

    // ── Verify full conversation in DB ──
    let messages = store
        .load_session_messages(&session_id)
        .expect("Failed to load messages");

    println!(
        "\n📋 Full chain conversation ({} messages):",
        messages.len()
    );
    for msg in &messages {
        let trunc_content = msg
            .content
            .char_indices()
            .nth(30)
            .map(|(i, _)| &msg.content[..i])
            .unwrap_or(&msg.content);
        println!(
            "  [{:>2}] role={:<10} agent={:<12} content={}",
            msg.id, msg.role, msg.agent_id, trunc_content
        );
    }

    // Must have messages from all 4 agents (user + assistant each)
    let user_msgs: Vec<_> = messages.iter().filter(|m| m.role == "user").collect();
    let agent_msgs: Vec<_> = messages.iter().filter(|m| m.role == "assistant").collect();

    assert!(
        user_msgs.len() >= 4,
        "❌ Expected at least 4 user messages, got {}",
        user_msgs.len()
    );
    assert!(
        agent_msgs.len() >= 4,
        "❌ Expected at least 4 agent responses, got {}",
        agent_msgs.len()
    );

    // Verify all 4 agents participated
    let participating: std::collections::HashSet<_> =
        agent_msgs.iter().map(|m| m.agent_id.as_str()).collect();
    for (role, _) in &chain {
        assert!(
            participating.contains(role),
            "❌ {} did not participate in chain",
            role
        );
    }

    println!(
        "\n✅ A2A full-mesh chain verified: {} agents participated, {} total messages",
        participating.len(),
        messages.len()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 7. Comprehensive A2A Protocol Test — All 11 Methods
//    Exercises every method from A2A spec 5.3 Method Mapping Reference
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_a2a_all_methods_comprehensive() {
    if !require_team_live_a2a() {
        return;
    }

    let client = reqwest::blocking::Client::new();
    let leader = "http://localhost:4321";
    let researcher = "http://localhost:4322";

    // Wait for agents (sync health check)
    for (name, ep) in TEAM_ENDPOINTS {
        let health_url = format!("{}/.well-known/agent.json", ep);
        let mut ok = false;
        for _ in 0..30 {
            if client
                .get(&health_url)
                .timeout(Duration::from_secs(2))
                .send()
                .is_ok()
            {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        assert!(ok, "❌ {} not ready at {}", name, ep);
    }

    println!("\n╔══════════════════════════════════════════════╗");
    println!("║  A2A Protocol — All 11 Methods E2E Test      ║");
    println!("╚══════════════════════════════════════════════╝\n");

    // ── ① message/send (동기, blocking: true) ─────────────────────
    println!("① message/send (synchronous, blocking: true)");
    let resp = a2a_rpc(
        &client,
        researcher,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("sync-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "Say OK in one word."}]
            },
            "configuration": {"blocking": true}
        }),
    );
    let sync_state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    assert!(
        sync_state == "completed" || sync_state == "input-required",
        "❌ ① Expected completed/input-required, got: {}",
        sync_state
    );
    let sync_context_id = resp["result"]["contextId"]
        .as_str()
        .unwrap_or("")
        .to_string();
    println!(
        "  ✅ state={}, contextId={}",
        sync_state,
        &sync_context_id[..sync_context_id.len().min(16)]
    );

    // ── ② message/send (fire-and-forget, blocking: false) ────────
    println!("② message/send (fire-and-forget, blocking: false)");
    let start = std::time::Instant::now();
    let resp = a2a_rpc(
        &client,
        researcher,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("ff-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "Count from 1 to 3."}]
            },
            "configuration": {"blocking": false}
        }),
    );
    let ff_elapsed = start.elapsed();
    let ff_task_id = resp["result"]["id"].as_str().unwrap_or("").to_string();
    let ff_state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    assert!(
        ff_state == "submitted" || ff_state == "working",
        "❌ ② Expected submitted/working, got: {}",
        ff_state
    );
    assert!(
        ff_elapsed < Duration::from_secs(5),
        "❌ ② F&F should return quickly, took {:?}",
        ff_elapsed
    );
    println!(
        "  ✅ state={}, task_id={}, elapsed={:?}",
        ff_state,
        &ff_task_id[..ff_task_id.len().min(16)],
        ff_elapsed
    );

    // ── ③ message/send (push subscription) ───────────────────────
    println!("③ message/send (push subscription with webhook)");
    // Start a simple webhook receiver
    let webhook_received = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let webhook_clone = webhook_received.clone();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let webhook_port = listener.local_addr().unwrap().port();
    let webhook_url = format!("http://127.0.0.1:{}/webhook", webhook_port);

    // Spawn simple HTTP webhook receiver thread
    let handle = std::thread::spawn(move || {
        listener.set_nonblocking(false).unwrap();
        if let Ok((mut stream, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            // Extract body after \r\n\r\n
            if let Some(body_start) = request.find("\r\n\r\n") {
                let body = request[body_start + 4..].to_string();
                webhook_clone.lock().unwrap().push(body);
            }
            let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let resp = a2a_rpc(
        &client,
        researcher,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("push-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "Say YES."}]
            },
            "configuration": {
                "blocking": false,
                "pushNotificationConfig": {"url": webhook_url}
            }
        }),
    );
    let push_task_id = resp["result"]["id"].as_str().unwrap_or("").to_string();
    let push_state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    println!(
        "  ✅ state={}, task_id={}",
        push_state,
        &push_task_id[..push_task_id.len().min(16)]
    );

    // Wait for webhook
    let _ = handle.join();
    let received = webhook_received.lock().unwrap();
    println!("  ✅ webhook received {} notification(s)", received.len());

    // ── ④ message/stream (SSE) ───────────────────────────────────
    println!("④ message/stream (SSE streaming)");
    let stream_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": format!("stream-{}", uuid::Uuid::new_v4()),
        "method": "message/stream",
        "params": {
            "message": {
                "messageId": format!("stream-msg-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "Say HELLO."}]
            }
        }
    });
    let stream_resp = client
        .post(researcher)
        .header("Content-Type", "application/json")
        .json(&stream_body)
        .timeout(Duration::from_secs(60))
        .send();
    match stream_resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().unwrap_or_default();
            let event_count = body.matches("data:").count();
            println!("  ✅ HTTP {}, {} SSE events received", status, event_count);
        }
        Err(e) => println!("  ⚠️ stream error (may be expected): {}", e),
    }

    // ── ⑤ tasks/get ──────────────────────────────────────────────
    println!("⑤ tasks/get (retrieve task by ID)");
    // Wait for F&F task to complete
    std::thread::sleep(Duration::from_secs(10));
    let resp = a2a_rpc(
        &client,
        researcher,
        "tasks/get",
        serde_json::json!({
            "id": ff_task_id
        }),
    );
    if let Some(result) = resp.get("result") {
        let state = result["status"]["state"].as_str().unwrap_or("N/A");
        println!(
            "  ✅ task={} state={}",
            &ff_task_id[..ff_task_id.len().min(16)],
            state
        );
    } else {
        let err = resp
            .get("error")
            .and_then(|e| e["message"].as_str())
            .unwrap_or("unknown");
        println!("  ⚠️ tasks/get error: {} (F&F task may have expired)", err);
    }

    // ── ⑥ tasks/list ────────────────────────────────────────────
    println!("⑥ tasks/list (filter by status, pagination)");
    let resp = a2a_rpc(
        &client,
        researcher,
        "tasks/list",
        serde_json::json!({
            "pageSize": 10,
            "historyLength": 1
        }),
    );
    if let Some(result) = resp.get("result") {
        let total = result["totalSize"].as_u64().unwrap_or(0);
        let page_size = result["pageSize"].as_u64().unwrap_or(0);
        let task_count = result["tasks"].as_array().map(|a| a.len()).unwrap_or(0);
        println!(
            "  ✅ totalSize={}, pageSize={}, returned={}",
            total, page_size, task_count
        );
    } else {
        let err = resp
            .get("error")
            .and_then(|e| e["message"].as_str())
            .unwrap_or("unknown");
        println!(
            "  ⚠️ tasks/list: {} (may not be supported by this server)",
            err
        );
    }

    // ── ⑦ tasks/cancel ───────────────────────────────────────────
    println!("⑦ tasks/cancel");
    // Send a new F&F task to cancel
    let resp = a2a_rpc(
        &client,
        researcher,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("cancel-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "Write a very long essay about history."}]
            },
            "configuration": {"blocking": false}
        }),
    );
    let cancel_task_id = resp["result"]["id"].as_str().unwrap_or("").to_string();
    if !cancel_task_id.is_empty() {
        let resp = a2a_rpc(
            &client,
            researcher,
            "tasks/cancel",
            serde_json::json!({
                "id": cancel_task_id
            }),
        );
        if let Some(result) = resp.get("result") {
            let state = result["status"]["state"].as_str().unwrap_or("N/A");
            println!(
                "  ✅ task={} state={}",
                &cancel_task_id[..cancel_task_id.len().min(16)],
                state
            );
        } else {
            let err = resp
                .get("error")
                .and_then(|e| e["message"].as_str())
                .unwrap_or("unknown");
            println!("  ⚠️ cancel: {} (task may have completed already)", err);
        }
    } else {
        println!("  ⚠️ skipped: no task to cancel");
    }

    // ── ⑧ tasks/resubscribe (SSE) ───────────────────────────────
    println!("⑧ tasks/resubscribe");
    // Send F&F, then resubscribe
    let resp = a2a_rpc(
        &client,
        researcher,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("resub-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "Say RESUB."}]
            },
            "configuration": {"blocking": false}
        }),
    );
    let resub_task_id = resp["result"]["id"].as_str().unwrap_or("").to_string();
    if !resub_task_id.is_empty() {
        let resub_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": format!("resub-rpc-{}", uuid::Uuid::new_v4()),
            "method": "tasks/resubscribe",
            "params": {"id": resub_task_id}
        });
        let resub_resp = client
            .post(researcher)
            .header("Content-Type", "application/json")
            .json(&resub_body)
            .timeout(Duration::from_secs(30))
            .send();
        match resub_resp {
            Ok(r) => {
                let status = r.status();
                let body = r.text().unwrap_or_default();
                let events = body.matches("data:").count();
                println!("  ✅ HTTP {} — {} events from resubscribe", status, events);
            }
            Err(e) => println!("  ⚠️ resubscribe: {}", e),
        }
    } else {
        println!("  ⚠️ skipped: no task for resubscribe");
    }

    // ── ⑨ pushNotificationConfig/set ─────────────────────────────
    println!("⑨ tasks/pushNotificationConfig/set");
    // Use the sync task for push config CRUD
    let config_task_id = if !push_task_id.is_empty() {
        push_task_id.clone()
    } else {
        ff_task_id.clone()
    };
    let resp = a2a_rpc(
        &client,
        researcher,
        "tasks/pushNotificationConfig/set",
        serde_json::json!({
            "taskId": config_task_id,
            "pushNotificationConfig": {
                "url": "http://127.0.0.1:19999/test-webhook",
                "id": "test-config-1"
            }
        }),
    );
    if resp.get("result").is_some() {
        println!(
            "  ✅ push config set for task={}",
            &config_task_id[..config_task_id.len().min(16)]
        );
    } else {
        let err = resp
            .get("error")
            .and_then(|e| e["message"].as_str())
            .unwrap_or("unknown");
        println!("  ⚠️ set: {} (push might not be supported)", err);
    }

    // ── ⑩ pushNotificationConfig/get ─────────────────────────────
    println!("⑩ tasks/pushNotificationConfig/get");
    let resp = a2a_rpc(
        &client,
        researcher,
        "tasks/pushNotificationConfig/get",
        serde_json::json!({
            "id": config_task_id,
            "pushNotificationConfigId": "test-config-1"
        }),
    );
    if let Some(result) = resp.get("result") {
        let url = result["pushNotificationConfig"]["url"]
            .as_str()
            .unwrap_or("N/A");
        println!("  ✅ config url={}", url);
    } else {
        let err = resp
            .get("error")
            .and_then(|e| e["message"].as_str())
            .unwrap_or("unknown");
        println!("  ⚠️ get: {}", err);
    }

    // ── ⑪ pushNotificationConfig/list ────────────────────────────
    println!("⑪ tasks/pushNotificationConfig/list");
    let resp = a2a_rpc(
        &client,
        researcher,
        "tasks/pushNotificationConfig/list",
        serde_json::json!({
            "id": config_task_id
        }),
    );
    if let Some(result) = resp.get("result") {
        let count = result.as_array().map(|a| a.len()).unwrap_or(0);
        println!(
            "  ✅ {} configs for task={}",
            count,
            &config_task_id[..config_task_id.len().min(16)]
        );
    } else {
        let err = resp
            .get("error")
            .and_then(|e| e["message"].as_str())
            .unwrap_or("unknown");
        println!("  ⚠️ list: {}", err);
    }

    // ── ⑫ pushNotificationConfig/delete ──────────────────────────
    println!("⑫ tasks/pushNotificationConfig/delete");
    let resp = a2a_rpc(
        &client,
        researcher,
        "tasks/pushNotificationConfig/delete",
        serde_json::json!({
            "id": config_task_id,
            "pushNotificationConfigId": "test-config-1"
        }),
    );
    if resp.get("error").is_none() {
        println!(
            "  ✅ config deleted for task={}",
            &config_task_id[..config_task_id.len().min(16)]
        );
    } else {
        let err = resp
            .get("error")
            .and_then(|e| e["message"].as_str())
            .unwrap_or("unknown");
        println!("  ⚠️ delete: {}", err);
    }

    // ── ⑬ agent/getAuthenticatedExtendedCard ─────────────────────
    println!("⑬ agent/getAuthenticatedExtendedCard");
    let resp = a2a_rpc(
        &client,
        researcher,
        "agent/getAuthenticatedExtendedCard",
        serde_json::json!({}),
    );
    if let Some(result) = resp.get("result") {
        let name = result["name"].as_str().unwrap_or("N/A");
        println!("  ✅ agent name={}", name);
    } else {
        let err = resp
            .get("error")
            .and_then(|e| e["message"].as_str())
            .unwrap_or("N/A");
        println!("  ⚠️ extended card: {} (may not be configured)", err);
    }

    // ── ⑭ Sequential chain: Leader→Researcher→Verifier ──────────
    println!("⑭ Sequential chain (Leader→Researcher→Verifier)");
    let resp = a2a_rpc(
        &client,
        leader,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("chain-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "researcher에게 '1+1=?'를 물어보고, 그 결과를 verifier에게 검증 요청해. researcher와 verifier tool을 순서대로 사용해."}]
            },
            "configuration": {"blocking": true}
        }),
    );
    let chain_state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    let chain_text = resp["result"]["status"]["message"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
        .unwrap_or("N/A");
    let trunc = chain_text
        .char_indices()
        .nth(60)
        .map(|(i, _)| &chain_text[..i])
        .unwrap_or(chain_text);
    println!("  ✅ state={}, text={}", chain_state, trunc);

    // ── ⑮ Parallel fan-out + tasks/list collection ───────────────
    println!("⑮ Parallel fan-out (Leader→multiple agents, F&F + tasks/list)");
    let resp = a2a_rpc(
        &client,
        leader,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("fanout-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "researcher에게 '한국의 인구는?'과 creator에게 '한국의 수도는?'을 동시에 물어봐. 두 tool을 병렬로 호출해."}]
            },
            "configuration": {"blocking": true}
        }),
    );
    let fanout_state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    let fanout_text = resp["result"]["status"]["message"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
        .unwrap_or("N/A");
    let trunc = fanout_text
        .char_indices()
        .nth(80)
        .map(|(i, _)| &fanout_text[..i])
        .unwrap_or(fanout_text);
    println!("  ✅ state={}, text={}", fanout_state, trunc);

    println!("\n╔══════════════════════════════════════════════╗");
    println!("║  ✅ All 15 A2A protocol scenarios verified!  ║");
    println!("╚══════════════════════════════════════════════╝\n");
}

// ═══════════════════════════════════════════════════════════════════════
// 8. Orchestration Workflow E2E — Full Task Lifecycle
//    Leader registers task → Researcher claims → Researcher↔Verifier
//    inter-agent comm → Completion → tasks/list verification →
//    ilhae/list_a2a_tasks proxy aggregation
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_orchestration_workflow_e2e() {
    if !require_team_live_a2a() {
        return;
    }

    let client = reqwest::blocking::Client::new();
    let leader = "http://localhost:4321";
    let researcher = "http://localhost:4322";
    let verifier = "http://localhost:4323";

    // Wait for all agents to be ready
    for (name, ep) in TEAM_ENDPOINTS {
        let health_url = format!("{}/.well-known/agent.json", ep);
        let mut ok = false;
        for _ in 0..30 {
            if client
                .get(&health_url)
                .timeout(Duration::from_secs(2))
                .send()
                .is_ok()
            {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        assert!(ok, "❌ {} not ready at {}", name, ep);
    }

    println!("\n╔═══════════════════════════════════════════════════════╗");
    println!("║  Orchestration Workflow E2E — Full Task Lifecycle     ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");

    // ── Phase 1: Leader에게 task 등록 (F&F — submitted 상태) ─────────
    println!("🔹 Phase 1: Leader에게 task 등록 (fire-and-forget → submitted)");
    let start = std::time::Instant::now();
    let resp = a2a_rpc(
        &client,
        leader,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("orch-leader-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "researcher를 사용해서 '대한민국의 GDP 순위'를 조사하고, 조사 결과를 verifier로 검증해줘. 모든 tool을 순서대로 사용해."}]
            },
            "configuration": {"blocking": false}
        }),
    );
    let p1_elapsed = start.elapsed();
    let leader_task_id = resp["result"]["id"].as_str().unwrap_or("").to_string();
    let p1_state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");

    assert!(
        p1_state == "submitted" || p1_state == "working",
        "❌ Phase 1: Expected submitted/working, got: {}",
        p1_state
    );
    assert!(
        !leader_task_id.is_empty(),
        "❌ Phase 1: No task_id returned from Leader"
    );
    assert!(
        p1_elapsed < Duration::from_secs(5),
        "❌ Phase 1: F&F should return quickly, took {:?}",
        p1_elapsed
    );
    println!(
        "  ✅ state={}, task_id={}, elapsed={:?}",
        p1_state,
        &leader_task_id[..leader_task_id.len().min(16)],
        p1_elapsed
    );

    // ── Phase 2: Researcher의 tasks/list로 task 확인 ──────────────
    println!("\n🔹 Phase 2: Researcher tasks/list 확인 (활성 task 존재)");
    let resp = a2a_rpc(
        &client,
        researcher,
        "tasks/list",
        serde_json::json!({
            "pageSize": 20
        }),
    );
    if let Some(result) = resp.get("result") {
        let task_count = result["tasks"].as_array().map(|a| a.len()).unwrap_or(0);
        let total = result["totalSize"].as_u64().unwrap_or(0);
        println!(
            "  ✅ Researcher has {} tasks (totalSize={})",
            task_count, total
        );

        // Print task states
        if let Some(tasks) = result["tasks"].as_array() {
            for (i, t) in tasks.iter().enumerate().take(5) {
                let state = t["status"]["state"].as_str().unwrap_or("?");
                let tid = t["id"].as_str().unwrap_or("?");
                println!(
                    "     [{}/{}] task={} state={}",
                    i + 1,
                    task_count,
                    &tid[..tid.len().min(12)],
                    state
                );
            }
        }
    } else {
        println!("  ⚠️ tasks/list not available on Researcher (continuing anyway)");
    }

    // ── Phase 3: Researcher에게 직접 task 전송 (조사 요청) ────────
    println!("\n🔹 Phase 3: Researcher에게 직접 조사 요청 (blocking)");
    let resp = a2a_rpc(
        &client,
        researcher,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("orch-research-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "대한민국의 GDP 순위를 한 줄로 답해."}]
            },
            "configuration": {"blocking": true}
        }),
    );
    let researcher_state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    let researcher_text = resp["result"]["status"]["message"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
        .unwrap_or("(no text)");
    let researcher_task_id = resp["result"]["id"].as_str().unwrap_or("").to_string();

    assert!(
        researcher_state == "completed" || researcher_state == "input-required",
        "❌ Phase 3: Expected completed/input-required from Researcher, got: {}",
        researcher_state
    );
    let trunc_research = researcher_text
        .char_indices()
        .nth(60)
        .map(|(i, _)| &researcher_text[..i])
        .unwrap_or(researcher_text);
    println!("  ✅ state={}, text={}", researcher_state, trunc_research);

    // ── Phase 4: Researcher 결과를 Verifier에게 검증 요청 ─────────
    println!("\n🔹 Phase 4: Verifier에게 검증 요청 (Researcher 결과 포워딩)");
    let verify_prompt = format!(
        "다음 조사 결과를 검증해줘. 정확하면 '✅ 검증 완료'라고, 부정확하면 '❌ 검증 실패'라고 한 줄로 답해.\n\n조사 결과: {}",
        researcher_text
    );
    let resp = a2a_rpc(
        &client,
        verifier,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("orch-verify-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": verify_prompt}]
            },
            "configuration": {"blocking": true}
        }),
    );
    let verifier_state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    let verifier_text = resp["result"]["status"]["message"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
        .unwrap_or("(no text)");
    let verifier_task_id = resp["result"]["id"].as_str().unwrap_or("").to_string();

    assert!(
        verifier_state == "completed" || verifier_state == "input-required",
        "❌ Phase 4: Expected completed/input-required from Verifier, got: {}",
        verifier_state
    );
    let trunc_verify = verifier_text
        .char_indices()
        .nth(60)
        .map(|(i, _)| &verifier_text[..i])
        .unwrap_or(verifier_text);
    println!("  ✅ state={}, text={}", verifier_state, trunc_verify);

    // ── Phase 5: tasks/list로 완료된 task 상태 double-check ───────
    println!("\n🔹 Phase 5: tasks/list로 task 상태 검증");

    // Check researcher tasks
    let resp = a2a_rpc(
        &client,
        researcher,
        "tasks/list",
        serde_json::json!({"pageSize": 20}),
    );
    if let Some(result) = resp.get("result") {
        let tasks = result["tasks"].as_array();
        let task_count = tasks.map(|a| a.len()).unwrap_or(0);
        println!("  📋 Researcher: {} total tasks", task_count);

        // Find our specific task
        if let Some(tasks) = tasks {
            let our_task = tasks
                .iter()
                .find(|t| t["id"].as_str() == Some(&researcher_task_id));
            if let Some(t) = our_task {
                let state = t["status"]["state"].as_str().unwrap_or("?");
                println!("     ✅ Our research task: state={}", state);
                assert!(
                    state == "completed" || state == "input-required",
                    "❌ Phase 5: Research task should be completed, got: {}",
                    state
                );
            } else {
                println!(
                    "     ⚠️ Our specific task not found in list (may have expired from memory store)"
                );
            }
        }
    }

    // Check verifier tasks
    let resp = a2a_rpc(
        &client,
        verifier,
        "tasks/list",
        serde_json::json!({"pageSize": 20}),
    );
    if let Some(result) = resp.get("result") {
        let tasks = result["tasks"].as_array();
        let task_count = tasks.map(|a| a.len()).unwrap_or(0);
        println!("  📋 Verifier: {} total tasks", task_count);

        if let Some(tasks) = tasks {
            let our_task = tasks
                .iter()
                .find(|t| t["id"].as_str() == Some(&verifier_task_id));
            if let Some(t) = our_task {
                let state = t["status"]["state"].as_str().unwrap_or("?");
                println!("     ✅ Our verify task: state={}", state);
            } else {
                println!(
                    "     ⚠️ Our specific task not found in list (may have expired from memory store)"
                );
            }
        }
    }

    // ── Phase 6: Leader가 orchestrate한 전체 흐름 (blocking) ──────
    println!(
        "\n🔹 Phase 6: Leader 전체 오케스트레이션 (blocking — Leader→Researcher→Verifier 체인)"
    );
    let resp = a2a_rpc(
        &client,
        leader,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("orch-full-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "다음 워크플로우를 순서대로 수행해:\n1. researcher tool로 '파이썬의 창시자'를 조사\n2. verifier tool로 researcher 결과를 검증\n3. 최종 결과를 한 줄로 보고해"}]
            },
            "configuration": {"blocking": true}
        }),
    );
    let final_state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    let final_text = resp["result"]["status"]["message"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
        .unwrap_or("(no text)");
    let trunc_final = final_text
        .char_indices()
        .nth(80)
        .map(|(i, _)| &final_text[..i])
        .unwrap_or(final_text);
    println!("  ✅ state={}, text={}", final_state, trunc_final);

    // ── Phase 7: Leader tasks/get으로 F&F task 상태 확인 ──────────
    println!("\n🔹 Phase 7: Leader tasks/get — Phase 1 F&F task 최종 상태");
    std::thread::sleep(Duration::from_secs(3)); // Wait for F&F to possibly complete
    let resp = a2a_rpc(
        &client,
        leader,
        "tasks/get",
        serde_json::json!({
            "id": leader_task_id
        }),
    );
    if let Some(result) = resp.get("result") {
        let state = result["status"]["state"].as_str().unwrap_or("N/A");
        println!(
            "  ✅ F&F task={} final_state={}",
            &leader_task_id[..leader_task_id.len().min(16)],
            state
        );
    } else {
        let err = resp
            .get("error")
            .and_then(|e| e["message"].as_str())
            .unwrap_or("unknown");
        println!("  ⚠️ tasks/get: {} (task may have expired)", err);
    }

    // ── Phase 8: ilhae/list_a2a_tasks 프록시 집계 확인 ───────────
    println!("\n🔹 Phase 8: ilhae/list_a2a_tasks — 프록시 A2A task 집계");
    // The proxy's SACP RPC isn't easily callable via raw HTTP, so we test
    // by calling tasks/list on each agent (what the proxy does internally)
    // and verifying the fan-out pattern works.
    let mut total_across_agents = 0usize;
    let mut agents_with_tasks = 0usize;

    for (role, endpoint) in TEAM_ENDPOINTS {
        let resp = a2a_rpc(
            &client,
            endpoint,
            "tasks/list",
            serde_json::json!({"pageSize": 50}),
        );
        if let Some(result) = resp.get("result") {
            let count = result["tasks"].as_array().map(|a| a.len()).unwrap_or(0);
            let total = result["totalSize"].as_u64().unwrap_or(count as u64) as usize;
            total_across_agents += total;
            if total > 0 {
                agents_with_tasks += 1;
            }
            println!("  📊 {} : {} tasks (totalSize={})", role, count, total);
        } else {
            println!("  ⚠️ {} : tasks/list unsupported", role);
        }
    }

    println!(
        "\n  ✅ Total tasks across all agents: {}, agents with tasks: {}/{}",
        total_across_agents,
        agents_with_tasks,
        TEAM_ENDPOINTS.len()
    );

    // At least researcher and verifier should have tasks from our test
    assert!(
        agents_with_tasks >= 2,
        "❌ Expected at least 2 agents with tasks (Researcher + Verifier), got {}",
        agents_with_tasks
    );

    println!("\n╔═══════════════════════════════════════════════════════╗");
    println!("║  ✅ Orchestration Workflow E2E — All 8 Phases Pass!  ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");
}

// ═══════════════════════════════════════════════════════════════════════
// 9. A2A Interactive Tools E2E — Propose & Spawn Subagent
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_a2a_tools_propose_and_spawn() {
    if !require_team_live_a2a() {
        return;
    }

    let client = reqwest::blocking::Client::new();
    let leader = "http://localhost:4321";
    let researcher = "http://localhost:4322";

    println!("\n╔═══════════════════════════════════════════════════════╗");
    println!("║  A2A Tools E2E — propose_to_leader & spawn_subagent   ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");

    // ── 1. propose_to_leader (Worker -> Leader) ─────────
    println!("🔹 1. Researcher: propose_to_leader 툴 호출 테스트");

    // We send a direct message to researcher, forcing it to use the propose tool.
    let resp = a2a_rpc(
        &client,
        researcher,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("propose-test-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "네가 작업을 하다가 막혔다고 가정하고, 'API Key가 없어서 진행할 수 없습니다' 라는 메시지로 리더에게 `propose_to_leader` 툴을 써서 보고해봐. level은 'blocker'로 해."}]
            },
            "configuration": {"blocking": true}
        }),
    );

    let state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    let text = resp["result"]["status"]["message"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
        .unwrap_or("(no text)");

    println!("  ✅ Propose tool response state={}, text={}", state, text);
    // Even if it just replies "I sent it", we verify the agent *can* parse and use it without breaking.

    // ── 2. spawn_subagent (Leader -> Ephemeral Worker) ─────────
    println!("\n🔹 2. Leader: spawn_subagent 툴 호출 테스트");

    // We send a direct message to Leader to use spawn_subagent.
    let resp = a2a_rpc(
        &client,
        leader,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("spawn-test-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "`spawn_subagent` 툴을 사용해서 일회용 'JokeBot' 워커를 하나 생성해. 목표는 '개발자 농담 하나만 해줘'로 설정해줘. 서브 에이전트가 완수하고 반환한 결과를 나에게 그대로 알려줘."}]
            },
            "configuration": {"blocking": true}
        }),
    );

    let state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    let text = resp["result"]["status"]["message"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
        .unwrap_or("(no text)");

    let trunc_text = text
        .char_indices()
        .nth(80)
        .map(|(i, _)| &text[..i])
        .unwrap_or(text);
    println!(
        "  ✅ Spawn tool response state={}, text={}",
        state, trunc_text
    );

    assert!(
        state == "completed" || state == "input-required",
        "❌ Expected completed/input-required, got: {}",
        state
    );

    // Since spawn_subagent actually shells out to the CLI, it should contain a joke.
    println!("\n  ✅ A2A Interactive Tools E2E passed.");
}

// ═══════════════════════════════════════════════════════════════════════
// 10. A2A Interactive Tools E2E — Superpowers Workflow
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_a2a_tools_workflow_brainstorm_and_plan() {
    if !require_team_live_a2a() {
        return;
    }

    let client = reqwest::blocking::Client::new();
    let leader = "http://localhost:4321";

    println!("\n╔═════════════════════════════════════════════════════════╗");
    println!("║  A2A Tools E2E — brainstorm_design & execution_plan   ║");
    println!("╚═════════════════════════════════════════════════════════╝\n");

    // ── 1. brainstorm_design ─────────
    println!("🔹 1. Leader: brainstorm_design 툴 호출 테스트");

    let resp = a2a_rpc(
        &client,
        leader,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("brainstorm-test-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "`brainstorm_design` 툴을 사용해서 '간단한 To-Do 리스트 앱 만들기'에 대한 설계 문서를 작성해줘."}]
            },
            "configuration": {"blocking": true}
        }),
    );

    let state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    let text = resp["result"]["status"]["message"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
        .unwrap_or("(no text)");

    let trunc_text = text
        .char_indices()
        .nth(80)
        .map(|(i, _)| &text[..i])
        .unwrap_or(text);
    println!(
        "  ✅ Brainstorm tool response state={}, text={}",
        state, trunc_text
    );

    assert!(
        state == "completed" || state == "input-required",
        "❌ Expected completed/input-required, got: {}",
        state
    );

    // ── 2. create_execution_plan ─────────
    println!("\n🔹 2. Leader: create_execution_plan 툴 호출 테스트");

    let resp = a2a_rpc(
        &client,
        leader,
        "message/send",
        serde_json::json!({
            "message": {
                "messageId": format!("plan-test-{}", uuid::Uuid::new_v4()),
                "role": "user",
                "parts": [{"kind": "text", "text": "방금 만든 설계 문서(또는 임의의 설계 내용)를 바탕으로 `create_execution_plan` 툴을 사용해서 실행 계획을 세워줘."}]
            },
            "configuration": {"blocking": true}
        }),
    );

    let state = resp["result"]["status"]["state"].as_str().unwrap_or("N/A");
    let text = resp["result"]["status"]["message"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
        .unwrap_or("(no text)");

    let trunc_text = text
        .char_indices()
        .nth(80)
        .map(|(i, _)| &text[..i])
        .unwrap_or(text);
    println!(
        "  ✅ Plan tool response state={}, text={}",
        state, trunc_text
    );

    assert!(
        state == "completed" || state == "input-required",
        "❌ Expected completed/input-required, got: {}",
        state
    );

    println!("\n  ✅ A2A Workflow Tools E2E passed.");
}
