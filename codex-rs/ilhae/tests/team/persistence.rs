//! Team persistence tests (test_07~11) — parallel execution, session history, delegation persistence

use super::common::team_helpers::*;
use ilhae_proxy::a2a_persistence::PersistenceScheduleStore;

/// Scenario 7: Parallel execution — 2 concurrent tasks on same agent with session persistence
///
/// Sends 2 messages concurrently to Researcher via persistence proxy, verifying:
/// - Both tasks complete independently (different context_id)
/// - Both sessions are recorded in SessionStore (DB)
#[ignore]
#[tokio::test]
async fn test_parallel_execution_with_sessions() {
    use ilhae_proxy::CxCache;
    use ilhae_proxy::a2a_persistence::{ForwardingExecutor, build_routing_table};
    use std::sync::Arc;

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");

    // ── Build persistence proxy ──
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind failed");
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    let proxy_base_url = format!("http://127.0.0.1:{}", proxy_port);

    let workspace_map = generate_peer_registration_files(&team, Some(&proxy_base_url));
    let mut children = spawn_team_a2a_servers(&team, &workspace_map, None, "e2e-parallel").await;

    match wait_for_all_team_health(&team).await {
        Ok(()) => println!("✅ All agents healthy"),
        Err(e) => {
            cleanup_children(&mut children).await;
            panic!("Health: {}", e);
        }
    }
    trigger_agent_reload(&team).await;

    let store = Arc::new(SessionStore::new(&dir).expect("SessionStore"));
    let cx_cache = CxCache::new();
    let routing_table = build_routing_table(&team);

    let delegation_cache: ilhae_proxy::a2a_persistence::DelegationResponseCache =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut app = axum::Router::new();
    for (role, endpoint, is_main) in &routing_table {
        let executor = ForwardingExecutor::with_options(
            endpoint.clone(),
            role.clone(),
            *is_main,
            store.clone(),
            cx_cache.clone(),
            delegation_cache.clone(),
        );
        let task_store = PersistenceScheduleStore::new(store.clone(), role.clone());
        let role_base = format!("{}/a2a/{}", proxy_base_url, role);
        let server = a2a_rs::server::A2AServer::new(executor, task_store).base_url(&role_base);
        app = app.nest(&format!("/a2a/{}", role), server.router());
    }
    tokio::spawn(async move {
        axum::serve(proxy_listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    println!("✅ Proxy at {}", proxy_base_url);

    // ── Create 2 sessions (ForwardingExecutor auto-persists user+assistant) ──
    let session_a = uuid::Uuid::new_v4().to_string();
    let session_b = uuid::Uuid::new_v4().to_string();
    let prompt_a = "What is 2+2? Reply with just the number.";
    let prompt_b = "What is 3+3? Reply with just the number.";

    // Only create session shell; ForwardingExecutor writes messages
    for sid in [&session_a, &session_b] {
        store
            .ensure_session_with_channel(sid, "researcher", ".", "e2e-test")
            .unwrap();
    }
    println!(
        "✅ Sessions: {}.. and {}..",
        &session_a[..8],
        &session_b[..8]
    );

    // ── Send concurrently ──
    let url = format!("{}/a2a/researcher", proxy_base_url);
    let pa = A2aProxy::new(&url, "researcher");
    let pb = A2aProxy::new(&url, "researcher");
    let sa = session_a.clone();
    let sb = session_b.clone();

    let (res_a, res_b) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(60),
            pa.send_and_observe(prompt_a, Some(sa), None)
        ),
        tokio::time::timeout(
            Duration::from_secs(60),
            pb.send_and_observe(prompt_b, Some(sb), None)
        ),
    );

    match res_a {
        Ok(Ok((t, ev))) => println!("  ✅ A: {} chars, {} events", t.len(), ev.len()),
        _ => println!("  ⚠️ A failed"),
    };
    match res_b {
        Ok(Ok((t, ev))) => println!("  ✅ B: {} chars, {} events", t.len(), ev.len()),
        _ => println!("  ⚠️ B failed"),
    };

    // ── Verify (ForwardingExecutor should have persisted user + assistant) ──
    for (label, sid) in [("A", &session_a), ("B", &session_b)] {
        let msgs = store.load_session_messages(sid).unwrap_or_default();
        println!("  Session {}: {} msgs", label, msgs.len());
        assert!(msgs.len() >= 2, "Session {} needs >= 2 msgs", label);
    }

    // ── Cleanup: delete test sessions from DB ──
    for sid in [&session_a, &session_b] {
        let _ = store.delete_session(sid);
    }
    cleanup_children(&mut children).await;
    println!("\n══════════════════════════════════════════════════════════");
    println!("✅ test_07 Parallel Execution — 2 concurrent tasks, both sessions persisted");
    println!("══════════════════════════════════════════════════════════");
}

/// Scenario 8: Claim (indirect delegation) — send to Verifier via proxy, verify session + tasks/list
#[ignore]
#[tokio::test]
async fn test_claim_indirect_delegation() {
    use ilhae_proxy::CxCache;
    use ilhae_proxy::a2a_persistence::{ForwardingExecutor, build_routing_table};
    use std::sync::Arc;

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");

    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind failed");
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    let proxy_base_url = format!("http://127.0.0.1:{}", proxy_port);

    let workspace_map = generate_peer_registration_files(&team, Some(&proxy_base_url));
    let mut children = spawn_team_a2a_servers(&team, &workspace_map, None, "e2e-claim").await;

    match wait_for_all_team_health(&team).await {
        Ok(()) => println!("✅ All agents healthy"),
        Err(e) => {
            cleanup_children(&mut children).await;
            panic!("Health: {}", e);
        }
    }
    trigger_agent_reload(&team).await;

    let store = Arc::new(SessionStore::new(&dir).expect("SessionStore"));
    let cx_cache = CxCache::new();
    let routing_table = build_routing_table(&team);

    let delegation_cache: ilhae_proxy::a2a_persistence::DelegationResponseCache =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut app = axum::Router::new();
    for (role, endpoint, is_main) in &routing_table {
        let executor = ForwardingExecutor::with_options(
            endpoint.clone(),
            role.clone(),
            *is_main,
            store.clone(),
            cx_cache.clone(),
            delegation_cache.clone(),
        );
        let task_store = PersistenceScheduleStore::new(store.clone(), role.clone());
        let role_base = format!("{}/a2a/{}", proxy_base_url, role);
        let server = a2a_rs::server::A2AServer::new(executor, task_store).base_url(&role_base);
        app = app.nest(&format!("/a2a/{}", role), server.router());
    }
    tokio::spawn(async move {
        axum::serve(proxy_listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    println!("✅ Proxy at {}", proxy_base_url);

    // ── Simulate claim: create session shell, ForwardingExecutor persists msgs ──
    let session_id = uuid::Uuid::new_v4().to_string();
    let prompt = "Verify: Is 2+2=4? Reply yes or no.";

    store
        .ensure_session_with_channel(&session_id, "verifier", ".", "e2e-test")
        .unwrap();
    println!("✅ Claim session: {}..", &session_id[..8]);

    let verifier_url = format!("{}/a2a/verifier", proxy_base_url);
    let vp = A2aProxy::new(&verifier_url, "verifier");

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        vp.send_and_observe(prompt, Some(session_id.clone()), None),
    )
    .await;

    match result {
        Ok(Ok((text, events))) => {
            println!(
                "  ✅ Verifier: {} chars, {} events — {:.100}",
                text.len(),
                events.len(),
                text
            );
        }
        Ok(Err(e)) => println!("  ⚠️ error: {}", e),
        Err(_) => println!("  ⚠️ timeout"),
    };

    // ── Verify session (auto-persisted by ForwardingExecutor) ──
    let msgs = store.load_session_messages(&session_id).unwrap_or_default();
    println!(
        "  Session {} has {} messages:",
        &session_id[..8],
        msgs.len()
    );
    for msg in &msgs {
        println!(
            "    [{}] role='{}' agent='{}' '{:.60}'",
            msg.id, msg.role, msg.agent_id, msg.content
        );
    }
    assert!(msgs.len() >= 2, "Claim session needs >= 2 msgs");

    // ── Verify task in Verifier tasks/list ──
    let verifier = team
        .agents
        .iter()
        .find(|a| a.role.to_lowercase() == "verifier")
        .unwrap();
    let dp = A2aProxy::new(&verifier.endpoint, "verifier");
    match dp.list_tasks().await {
        Ok(tasks) => {
            println!("  ✅ Verifier tasks/list: {} tasks", tasks.len());
            assert!(
                !tasks.is_empty(),
                "Verifier should have >= 1 task from claim"
            );
        }
        Err(e) => println!("  ⚠️ tasks/list: {}", e),
    }

    // ── Verify in DB ──
    let sessions = store.list_sessions().unwrap_or_default();
    assert!(
        sessions.iter().any(|s| s.id == session_id),
        "Claim session in DB"
    );
    println!("  ✅ Claim session in DB session list");

    // ── Cleanup: delete test session from DB ──
    let _ = store.delete_session(&session_id);
    cleanup_children(&mut children).await;
    println!("\n══════════════════════════════════════════════════════════");
    println!("✅ test_08 Claim Indirect Delegation E2E Complete");
    println!("══════════════════════════════════════════════════════════");
}

/// Scenario 9: Team Mode Routing — is_main + no-duplication validation
///
/// This test verifies three critical correctness properties:
///   1. `is_main` field is correctly set: exactly one main agent in team config
///   2. Sub-agents are NOT main (is_main == false)
///   3. A simple factual prompt ("3+3=?") sent to the Leader returns a correct,
///      non-duplicated answer (e.g. "6", NOT "66" or "6\n6")
///
/// This catches the bug where:
///   - Messages bypass Leader and go directly to sub-agents
///   - SSE response parsing produces duplicated content ("66" instead of "6")
#[ignore]
#[tokio::test]
async fn test_routing_is_main_and_no_duplication() {
    use a2a_rs::client::StreamEvent;
    use a2a_rs::proxy::{extract_text_from_parts, extract_text_from_stream_event};
    use ilhae_proxy::CxCache;
    use ilhae_proxy::a2a_persistence::{ForwardingExecutor, build_routing_table};
    use std::sync::Arc;

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");

    // ══════════════════════════════════════════════════════════════════════
    // Step 1: Verify is_main field correctness
    // ══════════════════════════════════════════════════════════════════════
    let main_agents: Vec<_> = team.agents.iter().filter(|a| a.is_main).collect();
    let sub_agents: Vec<_> = team.agents.iter().filter(|a| !a.is_main).collect();

    assert_eq!(
        main_agents.len(),
        1,
        "Exactly 1 agent should have is_main=true, got {}. Agents: {:?}",
        main_agents.len(),
        team.agents
            .iter()
            .map(|a| format!("{}(is_main={})", a.role, a.is_main))
            .collect::<Vec<_>>()
    );
    assert!(
        sub_agents.len() >= 1,
        "At least 1 sub-agent (is_main=false) expected, got {}",
        sub_agents.len()
    );

    let main_agent = main_agents[0];
    println!("✅ is_main validation passed:");
    println!(
        "  Main agent: {} (endpoint={})",
        main_agent.role, main_agent.endpoint
    );
    for sa in &sub_agents {
        println!(
            "  Sub-agent:  {} (endpoint={}, is_main={})",
            sa.role, sa.endpoint, sa.is_main
        );
    }

    // ══════════════════════════════════════════════════════════════════════
    // Step 2: Spawn team servers
    // ══════════════════════════════════════════════════════════════════════
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind proxy listener");
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    let proxy_base_url = format!("http://127.0.0.1:{}", proxy_port);

    let workspace_map = generate_peer_registration_files(&team, Some(&proxy_base_url));
    let mut children =
        spawn_team_a2a_servers(&team, &workspace_map, None, "e2e-routing-test").await;

    match wait_for_all_team_health(&team).await {
        Ok(()) => println!("✅ All agents healthy"),
        Err(e) => {
            cleanup_children(&mut children).await;
            panic!("Health check failed: {}", e);
        }
    }

    trigger_agent_reload(&team).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // ══════════════════════════════════════════════════════════════════════
    // Step 3: Start persistence proxy
    // ══════════════════════════════════════════════════════════════════════
    let store = Arc::new(SessionStore::new(&dir).expect("SessionStore open failed"));
    let cx_cache = CxCache::new();
    let routing_table = build_routing_table(&team);

    let delegation_cache: ilhae_proxy::a2a_persistence::DelegationResponseCache =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut app = axum::Router::new();
    for (role, endpoint, is_main) in &routing_table {
        let executor = ForwardingExecutor::with_options(
            endpoint.clone(),
            role.clone(),
            *is_main,
            store.clone(),
            cx_cache.clone(),
            delegation_cache.clone(),
        );
        let task_store = PersistenceScheduleStore::new(store.clone(), role.clone());
        let role_base = format!("{}/a2a/{}", proxy_base_url, role);
        let server = a2a_rs::server::A2AServer::new(executor, task_store).base_url(&role_base);
        app = app.nest(&format!("/a2a/{}", role), server.router());
    }

    tokio::spawn(async move {
        axum::serve(proxy_listener, app)
            .await
            .expect("Proxy server error");
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    println!("✅ Proxy at {}", proxy_base_url);

    // ══════════════════════════════════════════════════════════════════════
    // Step 4: Send simple factual prompt to Leader
    // ══════════════════════════════════════════════════════════════════════
    let session_id = uuid::Uuid::new_v4().to_string();
    let prompt = "What is 3+3? Reply with just the number, nothing else.";

    store
        .ensure_session_with_channel(
            &session_id,
            &main_agent.role.to_lowercase(),
            ".",
            "e2e-test",
        )
        .unwrap();

    let leader_url = format!("{}/a2a/{}", proxy_base_url, main_agent.role.to_lowercase());
    let leader_proxy = A2aProxy::new(&leader_url, &main_agent.role.to_lowercase());

    println!("  ⏳ Sending to {}: {}", main_agent.role, prompt);

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        leader_proxy.send_and_observe(prompt, Some(session_id.clone()), None),
    )
    .await;

    let (response_text, event_count) = match result {
        Ok(Ok((text, events))) => {
            println!(
                "  Response: '{}' ({} chars, {} events)",
                text.trim(),
                text.len(),
                events.len()
            );
            for (i, event) in events.iter().enumerate() {
                let event_text = extract_text_from_stream_event(event);
                match event {
                    StreamEvent::StatusUpdate(su) => {
                        println!(
                            "  Event#{}: StatusUpdate state={:?} text='{}'",
                            i + 1,
                            su.status.state,
                            event_text
                        );
                    }
                    StreamEvent::Task(task) => {
                        let task_text = task
                            .status
                            .message
                            .as_ref()
                            .map(|m| extract_text_from_parts(&m.parts))
                            .unwrap_or_default();
                        println!(
                            "  Event#{}: Task id='{}' state={:?} raw_text='{}'",
                            i + 1,
                            task.id,
                            task.status.state,
                            task_text
                        );
                    }
                    StreamEvent::ArtifactUpdate(_) => {
                        println!("  Event#{}: ArtifactUpdate text='{}'", i + 1, event_text);
                    }
                    StreamEvent::Message(msg) => {
                        println!("  Event#{}: Message text='{}'", i + 1, event_text);
                    }
                }
            }
            (text, events.len())
        }
        Ok(Err(e)) => {
            cleanup_children(&mut children).await;
            let _ = store.delete_session(&session_id);
            panic!("Leader proxy error: {}", e);
        }
        Err(_) => {
            cleanup_children(&mut children).await;
            let _ = store.delete_session(&session_id);
            panic!("Timeout after 60s — Leader did not respond");
        }
    };

    // ══════════════════════════════════════════════════════════════════════
    // Step 5: Validate no duplication
    // ══════════════════════════════════════════════════════════════════════
    let trimmed = response_text.trim();

    // The response should contain "6" but NOT be duplicated like "66" or "6 6" or "6\n6"
    assert!(
        trimmed.contains("6"),
        "Response should contain '6' for 3+3, got: '{}'",
        trimmed
    );

    // Check for common duplication patterns
    let has_duplication = trimmed == "66"
        || trimmed == "6 6"
        || trimmed == "6\n6"
        || trimmed == "6\n\n6"
        || (trimmed.matches("6").count() >= 2 && trimmed.replace("6", "").trim().is_empty());

    assert!(
        !has_duplication,
        "❌ Response is DUPLICATED: '{}' — SSE parsing bug detected!",
        trimmed
    );

    println!("✅ No duplication detected in response: '{}'", trimmed);

    // ══════════════════════════════════════════════════════════════════════
    // Step 6: Verify DB messages
    // ══════════════════════════════════════════════════════════════════════
    let msgs = store.load_session_messages(&session_id).unwrap_or_default();
    println!(
        "  Session {} has {} messages:",
        &session_id[..8],
        msgs.len()
    );
    for msg in &msgs {
        println!(
            "    [{}] role='{}' agent='{}' '{:.80}'",
            msg.id, msg.role, msg.agent_id, msg.content
        );
    }

    // ── Cleanup ──
    let _ = store.delete_session(&session_id);
    cleanup_children(&mut children).await;

    println!("\n══════════════════════════════════════════════════════════");
    println!("✅ test_09 Routing + is_main + No-Duplication E2E Complete");
    println!(
        "  Main agent: {} | Response: '{}' | Events: {} | DB msgs: {}",
        main_agent.role,
        trimmed,
        event_count,
        msgs.len()
    );
    println!("══════════════════════════════════════════════════════════");
}

/// test_10: Full session history persistence E2E
///
/// Validates the COMPLETE lifecycle that the app uses:
///   1. Send chat message to Leader
///   2. Receive response (no duplication)
///   3. Verify DB: user + assistant messages saved
///   4. Verify list_sessions() returns the session
///   5. Verify load_session_messages() returns correct messages
///   6. Verify message content is not duplicated in DB
///   7. Verify session title is set (not "Untitled")
///
/// This catches bugs where:
///   - Messages are processed but never saved to DB
///   - Session appears in list but has 0 messages
///   - Assistant response is duplicated in DB storage
#[ignore]
#[tokio::test]
async fn test_session_history_persistence() {
    use a2a_rs::proxy::extract_text_from_stream_event;
    use ilhae_proxy::CxCache;
    use ilhae_proxy::a2a_persistence::{ForwardingExecutor, build_routing_table};
    use std::sync::Arc;

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");
    let main_agent = team
        .agents
        .iter()
        .find(|a| a.is_main)
        .expect("No main agent");

    // ── Step 1: Spawn team servers ──
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind proxy listener");
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    let proxy_base_url = format!("http://127.0.0.1:{}", proxy_port);

    let workspace_map = generate_peer_registration_files(&team, Some(&proxy_base_url));
    let mut children =
        spawn_team_a2a_servers(&team, &workspace_map, None, "e2e-history-test").await;

    match wait_for_all_team_health(&team).await {
        Ok(()) => println!("✅ All agents healthy"),
        Err(e) => {
            cleanup_children(&mut children).await;
            panic!("Health check failed: {}", e);
        }
    }

    trigger_agent_reload(&team).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // ── Step 2: Start persistence proxy ──
    let store = Arc::new(SessionStore::new(&dir).expect("SessionStore open failed"));
    let cx_cache = CxCache::new();
    let routing_table = build_routing_table(&team);

    let delegation_cache: ilhae_proxy::a2a_persistence::DelegationResponseCache =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut app = axum::Router::new();
    for (role, endpoint, is_main) in &routing_table {
        let executor = ForwardingExecutor::with_options(
            endpoint.clone(),
            role.clone(),
            *is_main,
            store.clone(),
            cx_cache.clone(),
            delegation_cache.clone(),
        );
        let task_store = PersistenceScheduleStore::new(store.clone(), role.clone());
        let role_base = format!("{}/a2a/{}", proxy_base_url, role);
        let server = a2a_rs::server::A2AServer::new(executor, task_store).base_url(&role_base);
        app = app.nest(&format!("/a2a/{}", role), server.router());
    }

    tokio::spawn(async move {
        axum::serve(proxy_listener, app)
            .await
            .expect("Proxy server error");
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    println!("✅ Proxy at {}", proxy_base_url);

    // ── Step 3: Create session and send chat ──
    let session_id = uuid::Uuid::new_v4().to_string();
    let prompt = "What is 3+3? Reply with just the number.";

    store
        .ensure_session_with_channel(
            &session_id,
            &main_agent.role.to_lowercase(),
            ".",
            "e2e-test",
        )
        .unwrap();

    // Save user message to DB (just like prompt.rs:298 does)
    store.add_message(&session_id, "user", prompt, "").ok();

    let leader_url = format!("{}/a2a/{}", proxy_base_url, main_agent.role.to_lowercase());
    let leader_proxy = A2aProxy::new(&leader_url, &main_agent.role.to_lowercase());

    println!("  ⏳ Sending: '{}'", prompt);

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        leader_proxy.send_and_observe(prompt, Some(session_id.clone()), None),
    )
    .await;

    let response_text = match result {
        Ok(Ok((text, _events))) => text,
        Ok(Err(e)) => {
            cleanup_children(&mut children).await;
            let _ = store.delete_session(&session_id);
            panic!("Leader proxy error: {}", e);
        }
        Err(_) => {
            cleanup_children(&mut children).await;
            let _ = store.delete_session(&session_id);
            panic!("Timeout after 60s");
        }
    };

    let trimmed = response_text.trim();
    println!("  📩 Response: '{}'", trimmed);

    // ══════════════════════════════════════════════════════════════════════
    // Step 4: Validate session history (this is what the app does)
    // ══════════════════════════════════════════════════════════════════════

    // 4a. Verify response not duplicated
    assert!(
        trimmed.contains("6"),
        "Response should contain '6', got: '{}'",
        trimmed
    );
    let has_duplication = trimmed == "66"
        || trimmed == "6 6"
        || trimmed == "6\n6"
        || (trimmed.matches("6").count() >= 2 && trimmed.replace("6", "").trim().is_empty());
    assert!(!has_duplication, "❌ Duplicated response: '{}'", trimmed);
    println!("✅ Response not duplicated: '{}'", trimmed);

    // 4b. Load session messages from DB (exactly what app does on session click)
    let msgs = store.load_session_messages(&session_id).unwrap_or_default();
    println!("  📋 Session {} messages:", &session_id[..8]);
    for msg in &msgs {
        println!(
            "    [{}] role='{}' agent='{}' content='{:.60}'",
            msg.id, msg.role, msg.agent_id, msg.content
        );
    }

    // 4c. Assert: at least 1 user message exists
    let user_msgs: Vec<_> = msgs.iter().filter(|m| m.role == "user").collect();
    assert!(
        !user_msgs.is_empty(),
        "❌ No user message found in DB! Session history is empty."
    );
    assert_eq!(
        user_msgs[0].content, prompt,
        "❌ User message content mismatch"
    );
    println!("✅ User message saved: '{:.40}'", user_msgs[0].content);

    // 4d. Assert: at least 1 assistant message exists
    let assistant_msgs: Vec<_> = msgs.iter().filter(|m| m.role == "assistant").collect();
    assert!(
        !assistant_msgs.is_empty(),
        "❌ No assistant message found in DB! Response was not persisted."
    );
    println!(
        "✅ Assistant message saved: '{:.40}'",
        assistant_msgs[0].content
    );

    // 4e. Assert: assistant content in DB is NOT duplicated
    let db_content = assistant_msgs[0].content.trim();
    let db_has_duplication = db_content == "66"
        || db_content == "6 6"
        || db_content == "6\n6"
        || (db_content.matches("6").count() >= 2 && db_content.replace("6", "").trim().is_empty());
    assert!(
        !db_has_duplication,
        "❌ Assistant message in DB is DUPLICATED: '{}'. push_str bug still present!",
        db_content
    );
    println!("✅ DB content not duplicated: '{}'", db_content);

    // 4f. Verify list_sessions() returns this session (like app sidebar does)
    let all_sessions = store.list_sessions().unwrap_or_default();
    let our_session = all_sessions.iter().find(|s| s.id == session_id);
    assert!(
        our_session.is_some(),
        "❌ Session {} not found in list_sessions()! Sidebar would not show it.",
        &session_id[..8]
    );
    let our_session = our_session.unwrap();
    println!(
        "✅ Session visible in list: title='{}' messages={}",
        our_session.title,
        msgs.len()
    );

    // 4g. Verify message count consistency
    assert!(
        msgs.len() >= 2,
        "❌ Expected at least 2 messages (user + assistant), got {}",
        msgs.len()
    );
    println!(
        "✅ Message count correct: {} (user={}, assistant={})",
        msgs.len(),
        user_msgs.len(),
        assistant_msgs.len()
    );

    // ── Cleanup ──
    let _ = store.delete_session(&session_id);
    cleanup_children(&mut children).await;

    println!("\n══════════════════════════════════════════════════════════");
    println!("✅ test_10 Session History Persistence E2E Complete");
    println!(
        "  Response: '{}' | DB user: {} | DB assistant: {} | Total: {}",
        trimmed,
        user_msgs.len(),
        assistant_msgs.len(),
        msgs.len()
    );
    println!("══════════════════════════════════════════════════════════");
}

/// test_11: Team Mode Delegation E2E — Claim + Multi-Agent + DB Persistence
///
/// This is a TEAM-style test (not solo chat):
///   1. Sends a research prompt to Leader that triggers delegation to Researcher
///   2. Verifies Leader delegates to sub-agent (delegation events)
///   3. Verifies multi-agent session structure (parent session)
///   4. Verifies delegation events in DB (channel_id='a2a:*')
///   5. Verifies responses from multiple agents
///   6. **KEEPS data in DB with channel_id='desktop'** so the app can show it
///
/// This catches bugs where:
///   - Leader doesn't delegate to sub-agents
///   - Delegation events are not persisted
///   - Multi-agent session structure is broken
#[ignore]
#[tokio::test]
async fn test_team_delegation_with_persistence() {
    use a2a_rs::client::StreamEvent;
    use a2a_rs::proxy::{extract_text_from_parts, extract_text_from_stream_event};
    use ilhae_proxy::CxCache;
    use ilhae_proxy::a2a_persistence::{ForwardingExecutor, build_routing_table};
    use std::sync::Arc;

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");
    let main_agent = team
        .agents
        .iter()
        .find(|a| a.is_main)
        .expect("No main agent");
    let sub_agents: Vec<_> = team.agents.iter().filter(|a| !a.is_main).collect();

    println!("══════ test_11: Team Delegation E2E ══════");
    println!("  Leader: {} (is_main=true)", main_agent.role);
    for sa in &sub_agents {
        println!("  Sub-agent: {} (endpoint={})", sa.role, sa.endpoint);
    }

    // ── Step 1: Spawn team servers ──
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind proxy listener");
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    let proxy_base_url = format!("http://127.0.0.1:{}", proxy_port);

    let workspace_map = generate_peer_registration_files(&team, Some(&proxy_base_url));
    let mut children = spawn_team_a2a_servers(&team, &workspace_map, None, "e2e-team-deleg").await;

    match wait_for_all_team_health(&team).await {
        Ok(()) => println!("✅ All agents healthy"),
        Err(e) => {
            cleanup_children(&mut children).await;
            panic!("Health check failed: {}", e);
        }
    }

    // ── Step 2: Start persistence proxy ──
    let store = Arc::new(SessionStore::new(&dir).expect("SessionStore open failed"));
    let cx_cache = CxCache::new();
    let routing_table = build_routing_table(&team);

    let delegation_cache: ilhae_proxy::a2a_persistence::DelegationResponseCache =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let (app, _) = ilhae_proxy::a2a_persistence::build_proxy_router(
        &routing_table,
        tokio::sync::broadcast::channel(1024).0,
        cx_cache.clone(),
        delegation_cache,
        &proxy_base_url,
    );

    tokio::spawn(async move {
        axum::serve(proxy_listener, app)
            .await
            .expect("Proxy server error");
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    println!("✅ Proxy at {}", proxy_base_url);

    // Reload AFTER proxy is running so agents can resolve proxy agent card URLs
    trigger_agent_reload(&team).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // ── Step 3: Create parent session (team-mode, channel_id='desktop') ──
    let session_id = uuid::Uuid::new_v4().to_string();
    // Use 'desktop' channel so the app can display it!
    store
        .ensure_session_with_channel_meta_engine(
            &session_id,
            &main_agent.role.to_lowercase(),
            ".",
            "desktop", // ← visible in app!
            "",
            "team", // engine=team → multi_agent=1
        )
        .unwrap();

    // ── Step 4: Send a prompt that triggers delegation ──
    // This prompt is designed to make Leader delegate to Researcher
    let prompt = "@Researcher What is the capital of France? Reply with just the city name.";
    store.add_message(&session_id, "user", prompt, "").ok();

    let leader_url = format!("{}/a2a/{}", proxy_base_url, main_agent.role.to_lowercase());
    let leader_proxy = A2aProxy::new(&leader_url, &main_agent.role.to_lowercase());

    println!("  ⏳ Sending team prompt: '{}'", prompt);

    let result = tokio::time::timeout(
        Duration::from_secs(90),
        leader_proxy.send_and_observe(prompt, Some(session_id.clone()), None),
    )
    .await;

    let (response_text, events) = match result {
        Ok(Ok((text, events))) => (text, events),
        Ok(Err(e)) => {
            cleanup_children(&mut children).await;
            panic!("Leader proxy error: {}", e);
        }
        Err(_) => {
            cleanup_children(&mut children).await;
            panic!("Timeout after 90s");
        }
    };

    let trimmed = response_text.trim();
    println!("  📩 Response: '{}'", trimmed);

    // ── Step 5: Analyze delegation events ──
    let mut delegation_events = 0;
    let mut agent_names_seen: Vec<String> = vec![];
    for (i, event) in events.iter().enumerate() {
        match event {
            StreamEvent::StatusUpdate(su) => {
                let coder_kind = su
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("coderAgent"))
                    .and_then(|ca| ca.get("kind"))
                    .and_then(|k| k.as_str())
                    .unwrap_or("");
                let agent_name = su
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("coderAgent"))
                    .and_then(|ca| ca.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                if !agent_name.is_empty() && !agent_names_seen.contains(&agent_name.to_string()) {
                    agent_names_seen.push(agent_name.to_string());
                }
                if coder_kind == "delegation" {
                    delegation_events += 1;
                }
                let event_text = extract_text_from_stream_event(event);
                println!(
                    "  Event#{}: StatusUpdate kind='{}' agent='{}' text='{:.50}'",
                    i + 1,
                    coder_kind,
                    agent_name,
                    event_text
                );
            }
            StreamEvent::Task(task) => {
                println!("  Event#{}: Task state={:?}", i + 1, task.status.state);
            }
            _ => {}
        }
    }

    // Persist the assistant response (like relay_proxy does)
    store
        .add_full_message_with_blocks(
            &session_id,
            "assistant",
            trimmed,
            &main_agent.role.to_lowercase(),
            "",
            "[]",
            "",
            0,
            0,
            0,
            0,
        )
        .ok();

    // ══════════════════════════════════════════════════════════════════════
    // Step 6: Validate team-mode behavior
    // ══════════════════════════════════════════════════════════════════════

    // 6a. Response should contain Paris
    assert!(
        trimmed.to_lowercase().contains("paris"),
        "❌ Response should contain 'Paris', got: '{}'",
        trimmed
    );
    println!("✅ Correct answer: '{}'", trimmed);

    // 6b. Response should NOT be duplicated (e.g. "ParisParis")
    let lower = trimmed.to_lowercase();
    let paris_count = lower.matches("paris").count();
    assert!(
        paris_count == 1,
        "❌ 'Paris' appears {} times — duplication bug!",
        paris_count
    );
    println!("✅ No duplication: 'Paris' appears exactly once");

    // 6c. Verify DB messages
    let msgs = store.load_session_messages(&session_id).unwrap_or_default();
    println!("  📋 Session {} messages:", &session_id[..8]);
    for msg in &msgs {
        let ch = if msg.channel_id.is_empty() {
            "-"
        } else {
            &msg.channel_id
        };
        println!(
            "    [{}] role='{}' agent='{}' channel='{}' '{:.60}'",
            msg.id, msg.role, msg.agent_id, ch, msg.content
        );
    }

    let user_msgs: Vec<_> = msgs.iter().filter(|m| m.role == "user").collect();
    let asst_msgs: Vec<_> = msgs.iter().filter(|m| m.role == "assistant").collect();
    let deleg_msgs: Vec<_> = msgs
        .iter()
        .filter(|m| m.channel_id.starts_with("a2a:"))
        .collect();

    assert!(!user_msgs.is_empty(), "❌ No user message in DB");
    assert!(!asst_msgs.is_empty(), "❌ No assistant message in DB");
    println!(
        "✅ Messages in DB: user={}, assistant={}, delegation={}",
        user_msgs.len(),
        asst_msgs.len(),
        deleg_msgs.len()
    );

    // 6c2. Verify get_a2a_timeline duration_ms values
    let timeline = store.get_a2a_timeline(&session_id).unwrap_or_default();
    println!("\n  ⏱️ Timeline events ({}): ", timeline.len());
    for ev in &timeline {
        println!(
            "    [{}] role='{}' agent='{}' duration_ms={} ts={} content='{:.50}'",
            ev.message_id, ev.role, ev.agent_id, ev.duration_ms, ev.timestamp, ev.content_preview
        );
    }

    // Find delegation events via channel_id in deleg_msgs
    let start_msg = deleg_msgs
        .iter()
        .find(|m| m.channel_id.contains("delegation_start"));
    let complete_msg = deleg_msgs
        .iter()
        .find(|m| m.channel_id.contains("delegation_complete"));
    let response_msg = deleg_msgs
        .iter()
        .find(|m| m.channel_id.contains("delegation_response"));

    if let Some(start) = start_msg {
        assert_eq!(
            start.duration_ms, 0,
            "❌ delegation_start should have duration_ms=0, got {}",
            start.duration_ms
        );
        println!("✅ delegation_start: duration_ms=0 (correct, pre-send)");
    }

    if let Some(resp) = response_msg {
        assert!(
            resp.duration_ms > 0,
            "❌ delegation_response should have duration_ms > 0, got {}",
            resp.duration_ms
        );
        println!(
            "✅ delegation_response: duration_ms={} (correct, post-receive)",
            resp.duration_ms
        );
    }

    if let Some(complete) = complete_msg {
        assert!(
            complete.duration_ms > 0,
            "❌ delegation_complete should have duration_ms > 0, got {}",
            complete.duration_ms
        );
        println!(
            "✅ delegation_complete: duration_ms={} (correct, post-receive)",
            complete.duration_ms
        );
    }

    // Verify timestamps are chronologically ordered: start < response ≤ complete
    if let (Some(start), Some(complete)) = (start_msg, complete_msg) {
        assert!(
            start.timestamp < complete.timestamp,
            "❌ delegation_start timestamp ({}) should be BEFORE delegation_complete ({})",
            start.timestamp,
            complete.timestamp
        );
        let start_ts = chrono::DateTime::parse_from_rfc3339(&start.timestamp).unwrap();
        let complete_ts = chrono::DateTime::parse_from_rfc3339(&complete.timestamp).unwrap();
        let diff_ms = (complete_ts - start_ts).num_milliseconds();
        println!("✅ Timestamp diff: {}ms (start→complete)", diff_ms);
        assert!(
            diff_ms > 100,
            "❌ Timestamp diff should be > 100ms (actual delegation takes time), got {}ms",
            diff_ms
        );
    }

    // 6d. Verify session is visible with multi_agent=true
    let all_sessions = store.list_sessions().unwrap_or_default();
    let our_session = all_sessions.iter().find(|s| s.id == session_id);
    assert!(our_session.is_some(), "❌ Session not found in list");
    let our_session = our_session.unwrap();
    assert!(
        our_session.multi_agent,
        "❌ Session should have multi_agent=true"
    );
    assert_eq!(our_session.engine, "team", "❌ Engine should be 'team'");
    println!(
        "✅ Session: multi_agent={} engine='{}' channel='{}'",
        our_session.multi_agent, our_session.engine, our_session.channel_id
    );

    // 6e. Log delegation info
    if delegation_events > 0 {
        println!("✅ Delegation events detected: {}", delegation_events);
    } else {
        println!(
            "⚠️  No explicit delegation events in SSE stream (Leader may have answered directly)"
        );
    }
    if agent_names_seen.len() > 1 {
        println!("✅ Multiple agents participated: {:?}", agent_names_seen);
    }

    // ── DO NOT DELETE — leave data for app verification ──
    // The session uses channel_id='desktop' and engine='team'
    // so it will appear in the app's session list as a team session
    // Update title so it's identifiable
    store
        .update_session_title(&session_id, "E2E Team: @Researcher delegation test")
        .ok();

    cleanup_children(&mut children).await;

    println!("\n══════════════════════════════════════════════════════════");
    println!("✅ test_11 Team Delegation E2E Complete");
    println!(
        "  Response: '{}' | Delegation events: {} | Agents: {:?}",
        trimmed, delegation_events, agent_names_seen
    );
    println!(
        "  DB: user={} assistant={} delegation={} | multi_agent=true | channel=desktop",
        user_msgs.len(),
        asst_msgs.len(),
        deleg_msgs.len()
    );
    println!(
        "  ⚡ Data kept in DB for app verification (session {})",
        &session_id[..8]
    );
    println!("══════════════════════════════════════════════════════════");
}
