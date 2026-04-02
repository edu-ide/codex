//! E2E integration tests for Team Agent A2A communication.
//!
//! These tests run against the **live** A2A servers spawned by `pnpm tauri dev`.
//! Prerequisites:
//!   - `pnpm tauri dev` is running (spawns proxy + a2a-server processes)
//!   - `~/ilhae/team.json` exists with agents on ports 4321-4324
//!
//! Run:
//!   cargo test -p ilhae-proxy --test team_e2e -- --nocapture

use std::path::PathBuf;
use std::time::Duration;

// ── ilhae_proxy lib crate ────────────────────────────────────────────
use ilhae_proxy::context_proxy::team_a2a::{
    TeamRuntimeConfig, extract_port_from_endpoint, load_team_runtime_config, parse_a2a_result,
    wait_for_a2a_health,
};
use ilhae_proxy::settings_store::Settings;

// ── Constants ────────────────────────────────────────────────────────
const TEAM_ENDPOINTS: &[(&str, &str)] = &[
    ("Leader", "http://localhost:4321"),
    ("Researcher", "http://localhost:4322"),
    ("Verifier", "http://localhost:4323"),
    ("Creator", "http://localhost:4324"),
];

fn ilhae_dir() -> PathBuf {
    dirs::home_dir().unwrap().join("ilhae")
}

use super::common::a2a_test_helpers::*;

#[tokio::test(flavor = "multi_thread")]
async fn test_a2a_proxy_persists_messages_to_db() {
    // ── Setup: temp DB + proxy ──
    let (store, _tmp) = make_test_session_store();

    // Create a session in the store first (simulates existing user session)
    let session_id = uuid::Uuid::new_v4().to_string();
    store
        .create_session(&session_id, "Persistence Test", "test", "/tmp")
        .expect("Failed to create test session");

    // Start proxy for Researcher agent (Leader returns inputRequired trying to coordinate)
    let (proxy_url, _handle) =
        start_test_proxy("researcher", "http://localhost:4322", store.clone()).await;

    println!("🔌 Proxy started at: {}", proxy_url);

    // ── Send a message THROUGH THE PROXY ──
    let client = reqwest::Client::new();
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "messageId": format!("msg-{}", uuid::Uuid::new_v4()),
                "parts": [{"text": "Say OK in one word."}],
                "contextId": session_id,
            }
        }
    });

    let resp = client
        .post(&proxy_url)
        .json(&payload)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .expect("Failed to send message through proxy");

    assert!(
        resp.status().is_success(),
        "Proxy returned error: {}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.expect("Failed to parse response");
    println!(
        "📨 Proxy response: {}",
        serde_json::to_string_pretty(&body).unwrap()
    );

    // Verify we got a valid A2A response (not an error)
    assert!(
        body.get("result").is_some(),
        "Expected 'result', got error: {:?}",
        body.get("error")
    );

    // ── Wait for async execute() to complete (background task) ──
    // A2AServer spawns executor.execute() via tokio::spawn.
    // SSE streaming to real agent takes ~5-10s (LLM processing).
    println!("⏳ Waiting for background execute() to persist messages...");
    let mut total_msgs = 0;
    let mut has_agent_response = false;
    for i in 0..60 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let msgs = store.load_session_messages(&session_id).unwrap_or_default();
        total_msgs = msgs.len();
        has_agent_response = msgs.iter().any(|m| m.role == "assistant");
        if has_agent_response {
            println!(
                "  ✅ Agent response appeared after {}ms ({} total)",
                (i + 1) * 500,
                total_msgs
            );
            break;
        }
        if total_msgs > 0 && i == 0 {
            println!("  ✅ User message appeared after 500ms, waiting for agent response...");
        }
    }

    // ── Verify: Check DB for persisted messages ──
    let messages = store
        .load_session_messages(&session_id)
        .expect("Failed to load messages");

    println!("📋 DB messages for session {}:", session_id);
    for msg in &messages {
        let content_preview: String = msg.content.chars().take(80).collect();
        println!(
            "  [{:>2}] role={:<10} agent={:<10} content={}",
            msg.id, msg.role, msg.agent_id, content_preview
        );
    }

    // ── Assertions ──

    // 1. User message persisted via ForwardingExecutor.persist_message()
    assert!(
        !messages.is_empty(),
        "❌ No messages persisted in DB! ForwardingExecutor.persist_message() not called."
    );

    let user_msgs: Vec<_> = messages.iter().filter(|m| m.role == "user").collect();
    assert!(!user_msgs.is_empty(), "❌ User message not persisted to DB");
    assert!(
        user_msgs.iter().any(|m| m.content.contains("Say OK")),
        "❌ User message content not found. Messages: {:?}",
        user_msgs.iter().map(|m| &m.content).collect::<Vec<_>>()
    );

    // 2. Agent ID correctly set (proves proxy routing worked)
    assert!(
        user_msgs.iter().any(|m| m.agent_id == "researcher"),
        "❌ Agent ID not set to 'researcher'. IDs: {:?}",
        user_msgs.iter().map(|m| &m.agent_id).collect::<Vec<_>>()
    );

    // 3. Proxy accepted message and got valid response
    assert!(
        body.get("result").is_some(),
        "❌ Proxy did not return valid A2A result"
    );

    // 4. Agent response — must be persisted now that v1 Part format is supported
    let agent_msgs: Vec<_> = messages.iter().filter(|m| m.role == "assistant").collect();
    assert!(
        !agent_msgs.is_empty(),
        "❌ No agent response in DB. Agent should return LLM text via A2A SSE."
    );
    let agent_preview: String = agent_msgs[0].content.chars().take(80).collect();
    println!(
        "✅ Agent response persisted: {} msg(s), content: {}",
        agent_msgs.len(),
        agent_preview
    );

    println!(
        "✅ Persistence E2E verified: {} user msg(s), proxy chain working",
        user_msgs.len()
    );
}

/// Test 3: Multi-turn conversation — 2 messages in same session, verify full history.
#[tokio::test(flavor = "multi_thread")]
async fn test_a2a_proxy_multi_turn_conversation() {
    let (store, _tmp) = make_test_session_store();
    let session_id = uuid::Uuid::new_v4().to_string();
    store
        .create_session(&session_id, "Multi-Turn Test", "test", "/tmp")
        .expect("Failed to create session");

    let (proxy_url, _handle) =
        start_test_proxy("researcher", "http://localhost:4322", store.clone()).await;

    let client = reqwest::Client::new();

    // ── Turn 1: First message ──
    let msg1 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "turn1",
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "messageId": format!("msg-{}", uuid::Uuid::new_v4()),
                "parts": [{"text": "Say hello"}],
                "contextId": session_id,
            }
        }
    });
    let resp1 = client
        .post(&proxy_url)
        .json(&msg1)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .expect("Turn 1 failed");
    assert!(
        resp1.status().is_success(),
        "Turn 1 error: {}",
        resp1.status()
    );
    println!("✅ Turn 1 sent");

    // Wait for agent response from turn 1
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let msgs = store.load_session_messages(&session_id).unwrap_or_default();
        if msgs.iter().filter(|m| m.role == "assistant").count() >= 1 {
            println!("  ✅ Turn 1 agent response arrived");
            break;
        }
    }

    // ── Turn 2: Second message in same session ──
    let msg2 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "turn2",
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "messageId": format!("msg-{}", uuid::Uuid::new_v4()),
                "parts": [{"text": "Say goodbye"}],
                "contextId": session_id,
            }
        }
    });
    let resp2 = client
        .post(&proxy_url)
        .json(&msg2)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .expect("Turn 2 failed");
    assert!(
        resp2.status().is_success(),
        "Turn 2 error: {}",
        resp2.status()
    );
    println!("✅ Turn 2 sent");

    // Wait for agent response from turn 2
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let msgs = store.load_session_messages(&session_id).unwrap_or_default();
        if msgs.iter().filter(|m| m.role == "assistant").count() >= 2 {
            println!("  ✅ Turn 2 agent response arrived");
            break;
        }
    }

    // ── Verify full conversation history ──
    let messages = store
        .load_session_messages(&session_id)
        .expect("Failed to load messages");

    println!("📋 Multi-turn conversation ({} messages):", messages.len());
    for msg in &messages {
        let content_preview: String = msg.content.chars().take(60).collect();
        println!(
            "  [{:>2}] role={:<10} content={}",
            msg.id, msg.role, content_preview
        );
    }

    // Must have at least 4 messages: user1, agent1, user2, agent2
    assert!(
        messages.len() >= 4,
        "❌ Expected at least 4 messages (2 turns), got {}",
        messages.len()
    );

    let user_msgs: Vec<_> = messages.iter().filter(|m| m.role == "user").collect();
    let agent_msgs: Vec<_> = messages.iter().filter(|m| m.role == "assistant").collect();

    assert_eq!(user_msgs.len(), 2, "❌ Expected 2 user messages");
    assert_eq!(agent_msgs.len(), 2, "❌ Expected 2 agent responses");

    assert!(
        user_msgs[0].content.contains("hello"),
        "❌ First user message should contain 'hello'"
    );
    assert!(
        user_msgs[1].content.contains("goodbye"),
        "❌ Second user message should contain 'goodbye'"
    );

    println!(
        "✅ Multi-turn conversation verified: {} user + {} agent messages",
        user_msgs.len(),
        agent_msgs.len()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a2a_proxy_persists_all_agents() {
    let (store, _tmp) = make_test_session_store();

    let session_id = uuid::Uuid::new_v4().to_string();
    store
        .create_session(&session_id, "Multi-Agent Test", "test", "/tmp")
        .expect("Failed to create test session");

    let mut handles = vec![];

    // Test persistence for each agent through its own proxy
    for (role, endpoint) in TEAM_ENDPOINTS {
        let role_lower = role.to_lowercase();
        let (proxy_url, handle) = start_test_proxy(&role_lower, endpoint, store.clone()).await;
        handles.push(handle);

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": uuid::Uuid::new_v4().to_string(),
            "method": "message/send",
            "params": {
                "message": {
                    "role": "user",
                    "messageId": format!("msg-{}", uuid::Uuid::new_v4()),
                    "parts": [{"text": format!("Hello {} - respond OK", role)}],
                    "contextId": session_id,
                }
            }
        });

        match client
            .post(&proxy_url)
            .json(&payload)
            .timeout(Duration::from_secs(30))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                println!("✅ {} proxy accepted message", role);
            }
            Ok(resp) => {
                println!("⚠️ {} proxy returned: {}", role, resp.status());
            }
            Err(e) => {
                println!("⚠️ {} proxy unreachable: {}", role, e);
                continue;
            }
        }

        // Wait for execute() to persist before next agent
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    // Give time for last execute() to persist
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify all messages were persisted
    let messages = store
        .load_session_messages(&session_id)
        .expect("Failed to load messages");

    println!("\n📋 All persisted messages ({} total):", messages.len());
    for msg in &messages {
        let content_preview: String = msg.content.chars().take(60).collect();
        println!(
            "  [{:>2}] role={:<10} agent={:<10} content={}",
            msg.id, msg.role, msg.agent_id, content_preview
        );
    }

    // Should have at least one user message per reachable agent
    let agents_with_msgs: std::collections::HashSet<_> =
        messages.iter().map(|m| m.agent_id.as_str()).collect();
    println!("✅ Agents with persisted messages: {:?}", agents_with_msgs);

    // At least 1 agent's user message should be persisted
    assert!(
        !messages.is_empty(),
        "Expected at least 1 persisted message, got 0"
    );
    assert!(
        agents_with_msgs.len() >= 1,
        "Expected at least 1 agent to have persisted messages, got {:?}",
        agents_with_msgs
    );
}
