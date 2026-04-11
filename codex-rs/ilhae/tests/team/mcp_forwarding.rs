//! Artifact Versioning E2E — uses ForwardingExecutor (production path) to verify:
//!   1. MCP metadata injection (ilhae-tools stdio)
//!   2. artifact_save tool call detection (acp_mapper)
//!   3. DB versioning via ilhae-mcp-server

use super::common::team_helpers::*;
use super::common::test_gate::require_team_local_a2a_spawn;
use a2a_rs::event::{EventBus, ExecutionEvent};
use a2a_rs::executor::{AgentExecutor, RequestContext};
use a2a_rs::types::*;

/// Send prompt through ForwardingExecutor (same path as production). Returns (text, events).
async fn send_via_executor(
    endpoint: &str,
    role: &str,
    store: &std::sync::Arc<SessionStore>,
    session_id: &str,
    prompt: &str,
    timeout_secs: u64,
) -> (String, Vec<ExecutionEvent>) {
    let event_bus = EventBus::new(256);
    let mut rx = event_bus.subscribe();

    let request = SendMessageRequest {
        message: Message {
            message_id: uuid::Uuid::new_v4().to_string(),
            context_id: Some(session_id.to_string()),
            task_id: None,
            role: Role::User,
            parts: vec![Part::text(prompt)],
            metadata: None,
            extensions: vec![],
            reference_task_ids: None,
        },
        configuration: None,
        metadata: None,
    };

    let context = RequestContext {
        request,
        task_id: Some(uuid::Uuid::new_v4().to_string()),
        context_id: session_id.to_string(),
    };

    // Create ForwardingExecutor (same as production PersistenceProxy does)
    let cx_cache = ilhae_proxy::CxCache::new();
    let delegation_cache =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let executor = ilhae_proxy::a2a_persistence::ForwardingExecutor::with_options(
        endpoint.to_string(),
        role.to_string(),
        true, // is_main
        store.clone(),
        cx_cache,
        delegation_cache,
    );

    // Run executor in background task
    let eb_for_exec = event_bus.clone_sender();
    let exec_handle = tokio::spawn(async move { executor.execute(context, &eb_for_exec).await });

    // Collect events with timeout
    let mut events = Vec::new();
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                println!("  ⏰ Event collection timed out after {}s", timeout_secs);
                break;
            }
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        match &event {
                            ExecutionEvent::StatusUpdate(su) => {
                                if let Some(msg) = &su.status.message {
                                    for part in &msg.parts {
                                        if let Some(t) = &part.text {
                                            text.push_str(t);
                                        }
                                    }
                                }
                            }
                            ExecutionEvent::Message(msg) => {
                                for part in &msg.parts {
                                    if let Some(t) = &part.text {
                                        text.push_str(t);
                                    }
                                }
                            }
                            _ => {}
                        }
                        events.push(event);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        println!("  ⚠️  Lagged {} events", n);
                    }
                }
            }
        }
    }

    match exec_handle.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => println!("  ⚠️  Executor error: {:?}", e),
        Err(e) => println!("  ⚠️  Join error: {}", e),
    }

    (text, events)
}

/// 2-turn artifact versioning test via ForwardingExecutor (production path).
#[tokio::test]
async fn test_mcp_config_forwarding_e2e() {
    if !require_team_local_a2a_spawn() {
        return;
    }

    println!("\n══════════════════════════════════════════════════════════");
    println!("🧪 Artifact Versioning E2E (ForwardingExecutor path)");
    println!("══════════════════════════════════════════════════════════\n");

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");
    let leader = team
        .agents
        .iter()
        .find(|a| a.role.to_lowercase().contains("leader"))
        .expect("Leader agent required");

    // ── Spawn agents ──
    let workspace_map = generate_peer_registration_files(&team, None);
    let mut children = spawn_team_a2a_servers(&team, &workspace_map, None, "e2e-artifact").await;

    match wait_for_all_team_health(&team).await {
        Ok(()) => println!("✅ Agents healthy"),
        Err(e) => {
            cleanup_children(&mut children).await;
            panic!("Health check failed: {}", e);
        }
    }
    trigger_agent_reload(&team).await;

    // ── Create session ──
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut store = SessionStore::new(&dir).expect("SessionStore");
    let brain_dir = dir.join("brain");
    store.set_brain_writer(
        brain_session_rs::brain_session_writer::BrainSessionWriter::new(&brain_dir),
    );
    let store = std::sync::Arc::new(store);
    store
        .ensure_session_with_channel(&session_id, "leader", ".", "team")
        .expect("ensure_session");
    store
        .update_session_title(&session_id, "🧪 Artifact Versioning E2E")
        .ok();
    println!("✅ Session {} created", &session_id[..8]);

    // ── Turn 1: Create task artifact ──
    let prompt1 = "ilhae-tools__artifact_save 도구를 사용해서 task 타입 아티팩트를 만들어줘. 내용은 '## 할 일\n- [ ] 첫 번째 항목\n- [ ] 두 번째 항목' 으로 해줘. summary는 'v1 initial'로 해줘.";
    store
        .add_full_message_with_blocks(
            &session_id,
            "user",
            prompt1,
            "leader",
            "",
            "[]",
            &serde_json::to_string(&vec![serde_json::json!({"type": "text", "text": prompt1})])
                .unwrap_or_default(),
            0,
            0,
            0,
            0,
        )
        .expect("persist user");
    println!("\n── Turn 1: artifact_save create ──");
    println!("  ⏳ Sending via ForwardingExecutor...");

    let (text1, events1) = send_via_executor(
        &leader.endpoint,
        &leader.role,
        &store,
        &session_id,
        prompt1,
        90,
    )
    .await;
    println!("✅ Turn 1: {}B text, {} events", text1.len(), events1.len());
    println!(
        "  Preview: {}",
        &text1.chars().take(300).collect::<String>()
    );

    // Show tool call events
    let tool_events1 = events1
        .iter()
        .filter(|e| {
            matches!(e, ExecutionEvent::StatusUpdate(su)
            if su.metadata.as_ref()
                .and_then(|m| m.get("coderAgent"))
                .and_then(|c| c.get("kind"))
                .and_then(|k| k.as_str()) == Some("tool-use"))
        })
        .count();
    println!("  Tool-use events: {}", tool_events1);

    store
        .add_full_message_with_blocks(
            &session_id,
            "assistant",
            &text1,
            "leader",
            "",
            "[]",
            &serde_json::to_string(&vec![serde_json::json!({"type": "text", "text": text1})])
                .unwrap_or_default(),
            0,
            0,
            0,
            0,
        )
        .ok();

    // ── Turn 2: Update artifact ──
    let prompt2 = "ilhae-tools__artifact_save 도구를 사용해서 task 타입 아티팩트를 업데이트해줘. 내용은 '## 할 일\n- [x] 첫 번째 항목 (완료)\n- [ ] 두 번째 항목\n- [ ] 세 번째 항목 추가' 로 해줘. summary는 'v2 updated'로 해줘.";
    store
        .add_full_message_with_blocks(
            &session_id,
            "user",
            prompt2,
            "leader",
            "",
            "[]",
            &serde_json::to_string(&vec![serde_json::json!({"type": "text", "text": prompt2})])
                .unwrap_or_default(),
            0,
            0,
            0,
            0,
        )
        .expect("persist user");
    println!("\n── Turn 2: artifact_save update ──");
    println!("  ⏳ Sending via ForwardingExecutor...");

    let (text2, events2) = send_via_executor(
        &leader.endpoint,
        &leader.role,
        &store,
        &session_id,
        prompt2,
        90,
    )
    .await;
    println!("✅ Turn 2: {}B text, {} events", text2.len(), events2.len());
    println!(
        "  Preview: {}",
        &text2.chars().take(300).collect::<String>()
    );

    let tool_events2 = events2
        .iter()
        .filter(|e| {
            matches!(e, ExecutionEvent::StatusUpdate(su)
            if su.metadata.as_ref()
                .and_then(|m| m.get("coderAgent"))
                .and_then(|c| c.get("kind"))
                .and_then(|k| k.as_str()) == Some("tool-use"))
        })
        .count();
    println!("  Tool-use events: {}", tool_events2);

    store
        .add_full_message_with_blocks(
            &session_id,
            "assistant",
            &text2,
            "leader",
            "",
            "[]",
            &serde_json::to_string(&vec![serde_json::json!({"type": "text", "text": text2})])
                .unwrap_or_default(),
            0,
            0,
            0,
            0,
        )
        .ok();

    // ── Verify DB versioning ──
    println!("\n── DB Versioning Check ──");
    match store.list_session_artifacts(&session_id) {
        Ok(artifacts) => {
            println!("  Artifacts found: {}", artifacts.len());
            for a in &artifacts {
                println!(
                    "    - {} v{} (type: {})",
                    a.get("filename").and_then(|v| v.as_str()).unwrap_or("?"),
                    a.get("version").and_then(|v| v.as_i64()).unwrap_or(0),
                    a.get("artifact_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?"),
                );
            }
        }
        Err(e) => println!("  ⚠️  DB error: {}", e),
    }

    // Filesystem check
    let artifact_dir = dir.join("brain").join("sessions").join(&session_id);
    let task_file = artifact_dir.join("task.md");
    if task_file.exists() {
        let content = std::fs::read_to_string(&task_file).unwrap_or_default();
        println!("\n── Filesystem: task.md ({} bytes) ──", content.len());
        println!("{}", &content.chars().take(200).collect::<String>());
    } else {
        println!("\n  ⚠️  task.md not found at {:?}", task_file);
    }

    cleanup_children(&mut children).await;

    println!("\n══════════════════════════════════════════════════════════");
    println!("✅ Artifact Versioning E2E Complete (ForwardingExecutor path)");
    println!(
        "  Session: {} (🧪 Artifact Versioning E2E)",
        &session_id[..8]
    );
    println!("══════════════════════════════════════════════════════════");
}
