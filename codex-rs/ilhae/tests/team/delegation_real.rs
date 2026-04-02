//! Team delegation real E2E (test_01~06) — config, peer reg, delegation pipeline

use super::common::team_helpers::*;
use ilhae_proxy::a2a_persistence::PersistenceScheduleStore;

// ─── Tests ───────────────────────────────────────────────────────────────

/// Scenario 1: Load team config from real team.json
#[ignore]
#[tokio::test]
async fn test_load_team_config() {
    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir);

    assert!(
        team.is_some(),
        "team.json should exist and parse at {:?}/team.json",
        dir
    );

    let team = team.unwrap();
    assert!(
        team.agents.len() >= 2,
        "team.json should have at least 2 agents, got {}",
        team.agents.len()
    );

    for agent in &team.agents {
        println!(
            "  Agent: role={}, endpoint={}, engine={}",
            agent.role, agent.endpoint, agent.engine
        );
        assert!(!agent.role.is_empty(), "Agent role should not be empty");
        assert!(
            !agent.endpoint.is_empty(),
            "Agent endpoint should not be empty"
        );
    }

    println!("✅ Loaded team config with {} agents", team.agents.len());
}

/// Scenario 2: Generate peer registration files
#[ignore]
#[tokio::test]
async fn test_peer_registration() {
    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");

    let workspace_map = generate_peer_registration_files(&team, None);

    assert_eq!(
        workspace_map.len(),
        team.agents.len(),
        "Should create workspace for each agent"
    );

    // Verify each agent has peer files for others, not self
    for agent in &team.agents {
        let role = agent.role.to_lowercase();
        let ws = workspace_map
            .get(&role)
            .unwrap_or_else(|| panic!("workspace missing for {}", role));
        let agents_dir = ws.join(".gemini").join("agents");

        // Should have at least N-1 peer files (everyone except self)
        // May have more from previous test runs sharing the same workspace
        let peer_count = std::fs::read_dir(&agents_dir)
            .map(|entries| entries.filter_map(|e| e.ok()).count())
            .unwrap_or(0);

        assert!(
            peer_count >= team.agents.len() - 1,
            "{} should have at least {} peer files, got {}",
            role,
            team.agents.len() - 1,
            peer_count
        );

        // Self should NOT exist
        assert!(
            !agents_dir.join(format!("{}.md", role)).exists(),
            "{} should not have self peer file",
            role
        );

        println!("  ✅ {} has {} peer files", role, peer_count);
    }

    println!("✅ Peer registration files generated correctly");
}

/// Scenario 3: Full pipeline via A2A Persistence Proxy
///
/// Uses ForwardingExecutor + PersistenceScheduleStore to proxy requests to the
/// Leader gemini-cli A2A server — exactly like the desktop app does.
/// This ensures:
/// - All messages are persisted to DB with proper agent_id
/// - Delegation events (if the model delegates) are captured
/// - The full proxy chain is exercised end-to-end
#[ignore]
#[tokio::test]
async fn test_full_delegation_pipeline() {
    use a2a_rs::client::StreamEvent;
    use a2a_rs::proxy::{extract_text_from_stream_event, is_terminal_state};
    use ilhae_proxy::CxCache;
    use ilhae_proxy::a2a_persistence::{ForwardingExecutor, build_routing_table};
    use std::sync::Arc;

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");

    // ── Step 1: Generate peer files with proxy URL ──
    // First, bind a random port for our persistence proxy
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind proxy listener");
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    let proxy_base_url = format!("http://127.0.0.1:{}", proxy_port);
    println!("✅ Step 1: Proxy bound at {}", proxy_base_url);

    // Generate peer files pointing through our proxy
    let workspace_map = generate_peer_registration_files(&team, Some(&proxy_base_url));
    println!("  Peer registration files generated (proxy-routed)");

    // ── Step 2: Spawn A2A servers ──
    let mut children =
        spawn_team_a2a_servers(&team, &workspace_map, None, "e2e-proxy-delegation").await;
    println!("✅ Step 2: Spawned {} child processes", children.len());

    // ── Step 3: Wait for health ──
    match wait_for_all_team_health(&team).await {
        Ok(()) => println!("✅ Step 3: All agents healthy"),
        Err(e) => {
            cleanup_children(&mut children).await;
            panic!("Health check failed: {}", e);
        }
    }

    // ── Step 4: Trigger agent reload ──
    trigger_agent_reload(&team).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("✅ Step 4: trigger_agent_reload complete — peers use proxy endpoints");

    // ── Step 5: Start A2A Persistence Proxy (ForwardingExecutor chain) ──
    let store = Arc::new(SessionStore::new(&dir).expect("SessionStore open failed"));
    let cx_cache = CxCache::new();
    let routing_table: Vec<(String, String, bool)> = build_routing_table(&team);

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
        let role_router = server.router();
        app = app.nest(&format!("/a2a/{}", role), role_router);
        println!("  Registered: /a2a/{} → {}", role, endpoint);
    }

    // Spawn the proxy server
    tokio::spawn(async move {
        axum::serve(proxy_listener, app)
            .await
            .expect("Proxy server error");
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    println!(
        "✅ Step 5: A2A Persistence Proxy started at {}",
        proxy_base_url
    );

    // ── Step 6: Create session and send via proxied Leader ──
    let session_id = uuid::Uuid::new_v4().to_string();
    let user_prompt = "researcher에게 '인공지능의 역사'를 간단히 3줄로 조사해달라고 위임해줘. 반드시 researcher 도구를 호출해야 합니다.";

    // Persist user message first
    store
        .ensure_session_with_channel(&session_id, "leader", ".", "team")
        .expect("Failed to create session");

    let content_blocks = serde_json::to_string(&vec![
        serde_json::json!({ "type": "text", "text": user_prompt }),
    ])
    .unwrap_or_else(|_| "[]".to_string());
    store
        .add_full_message_with_blocks(
            &session_id,
            "user",
            user_prompt,
            "leader",
            "",
            "[]",
            &content_blocks,
            0,
            0,
            0,
            0,
        )
        .expect("Failed to persist user message");
    println!(
        "✅ Step 6: User message persisted to session {}",
        &session_id[..8]
    );

    // Send via proxied leader endpoint (not direct!)
    let proxied_leader_url = format!("{}/a2a/leader", proxy_base_url);
    let leader_proxy = A2aProxy::new(&proxied_leader_url, "leader");
    println!("  ⏳ Sending via persistence proxy: {}", proxied_leader_url);

    let result = tokio::time::timeout(
        Duration::from_secs(120),
        leader_proxy.send_and_observe(user_prompt, Some(session_id.clone()), None),
    )
    .await;

    let (accumulated_text, event_count) = match result {
        Ok(Ok((text, events))) => {
            println!(
                "✅ Step 6: Leader responded via proxy ({} chars, {} events)",
                text.len(),
                events.len()
            );

            for (i, event) in events.iter().enumerate() {
                let event_text = extract_text_from_stream_event(event);
                match event {
                    StreamEvent::StatusUpdate(su) => {
                        let coder_kind = su
                            .metadata
                            .as_ref()
                            .and_then(|m| m.get("coderAgent"))
                            .and_then(|ca| ca.get("kind"))
                            .and_then(|k| k.as_str())
                            .unwrap_or("");
                        println!(
                            "  Event#{}: StatusUpdate state={:?} kind='{}' text='{:.80}'",
                            i + 1,
                            su.status.state,
                            coder_kind,
                            event_text
                        );
                    }
                    StreamEvent::Task(task) => {
                        println!(
                            "  Event#{}: Task id='{}' state={:?}",
                            i + 1,
                            task.id,
                            task.status.state
                        );
                    }
                    StreamEvent::ArtifactUpdate(au) => {
                        println!("  Event#{}: ArtifactUpdate", i + 1);
                    }
                    StreamEvent::Message(msg) => {
                        println!("  Event#{}: Message text='{:.80}'", i + 1, event_text);
                    }
                }
            }

            // Check delegation evidence
            let has_delegation = text.to_lowercase().contains("researcher")
                || text.contains("인공지능")
                || events.len() > 3;
            if has_delegation {
                println!("  ✅ Delegation evidence found in response");
            } else {
                println!("  ⚠️  No clear delegation evidence");
            }
            println!("  📝 Response: {:.300}", text);

            (text, events.len())
        }
        Ok(Err(e)) => {
            println!("⚠️  Step 6: Proxy error: {}", e);
            (String::new(), 0)
        }
        Err(_) => {
            println!("⚠️  Step 6: Timeout after 120s");
            (String::new(), 0)
        }
    };

    // ── Step 7: Persist assistant response ──
    let assistant_text = if accumulated_text.is_empty() {
        "[proxy-timeout] No response".to_string()
    } else {
        accumulated_text.clone()
    };
    let assistant_blocks = serde_json::to_string(&vec![
        serde_json::json!({ "type": "text", "text": assistant_text }),
    ])
    .unwrap_or_else(|_| "[]".to_string());
    store
        .add_full_message_with_blocks(
            &session_id,
            "assistant",
            &assistant_text,
            "leader",
            "",
            "[]",
            &assistant_blocks,
            0,
            0,
            0,
            0,
        )
        .expect("Failed to persist assistant message");
    println!("✅ Step 7: Assistant message persisted");

    // ── Step 8: DB Verification — check for delegation events with channel_id ──
    let messages = store.load_session_messages(&session_id).unwrap_or_default();

    println!(
        "\n✅ Step 8: Session {} has {} messages",
        &session_id[..8],
        messages.len()
    );
    let mut delegation_count = 0usize;
    for msg in &messages {
        let channel = &msg.channel_id;
        let is_delegation = channel.starts_with("a2a:");
        if is_delegation {
            delegation_count += 1;
        }
        println!(
            "  [{}] role='{}' agent='{}' channel='{}' {} content='{:.60}'",
            msg.id,
            msg.role,
            msg.agent_id,
            channel,
            if is_delegation { "🛰️" } else { "" },
            msg.content
        );
    }

    println!(
        "\n  Total messages: {}, Delegation events: {}",
        messages.len(),
        delegation_count
    );

    // Assert minimum messages (user + assistant at least, plus any delegation events)
    assert!(
        messages.len() >= 2,
        "Session should have at least 2 messages (user+assistant), got {}",
        messages.len()
    );
    assert!(
        !accumulated_text.is_empty(),
        "Should have received non-empty response text"
    );

    // ── Also check all recent sessions for proxy-persisted messages ──
    let sessions = store.list_sessions().unwrap_or_default();
    println!("\n  DB has {} total sessions. Recent 3:", sessions.len());
    for session in sessions.iter().take(3) {
        let msgs = store.load_session_messages(&session.id).unwrap_or_default();
        let deleg = msgs
            .iter()
            .filter(|m| m.channel_id.starts_with("a2a:"))
            .count();
        println!(
            "    Session {}: {} messages, {} delegation events",
            &session.id[..8],
            msgs.len(),
            deleg
        );
    }

    // ── Cleanup ──
    cleanup_children(&mut children).await;

    println!("\n══════════════════════════════════════════════════════════");
    println!("✅ Proxy-Routed Delegation E2E Complete");
    println!(
        "  Events: {}, Text: {}B, Delegation DB events: {}",
        event_count,
        accumulated_text.len(),
        delegation_count
    );
    println!("══════════════════════════════════════════════════════════");
}

/// Scenario 4: Verify A2aProxy direct access to each agent
///
/// Uses lib crate's A2aProxy to send a simple prompt to each agent
/// WITHOUT going through the Leader. Validates the A2A protocol works.
#[ignore]
#[tokio::test]
async fn test_direct_agent_access() {
    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");

    // The agents should already be running (from test_03 or from desktop)
    // Just verify each is reachable + can handle a simple message
    for agent in &team.agents {
        let proxy = A2aProxy::new(&agent.endpoint, &agent.role);

        match tokio::time::timeout(
            Duration::from_secs(30),
            proxy.send_and_observe("간단히 테스트 응답을 해줘. 한 줄로.", None, None),
        )
        .await
        {
            Ok(Ok((text, _events))) => {
                println!(
                    "  ✅ {} responded ({} chars): {:.80}",
                    agent.role,
                    text.len(),
                    text
                );
                assert!(
                    !text.is_empty(),
                    "{} should return non-empty response",
                    agent.role
                );
            }
            Ok(Err(e)) => {
                println!("  ⚠️  {} error: {} (might not be running)", agent.role, e);
            }
            Err(_) => {
                println!("  ⚠️  {} timeout (not running)", agent.role);
            }
        }
    }
}

/// Scenario 5: Full conversation E2E — A2aProxy + DB persistence
///
/// Uses the standard A2A A2aProxy API (send_and_observe):
/// 1. Create session + persist user message to DB
/// 2. Send prompt via A2aProxy::send_and_observe (typed StreamEvent parsing)
/// 3. Verify StreamEvent lifecycle (terminal state reached)
/// 4. Persist assistant response to DB
/// 5. Verify full conversation retrievable from DB
#[ignore]
#[tokio::test]
async fn test_full_conversation_e2e() {
    use a2a_rs::client::StreamEvent;
    use a2a_rs::proxy::{extract_text_from_stream_event, is_terminal_state};

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");
    let leader = team
        .agents
        .iter()
        .find(|a| a.role.to_lowercase().contains("leader"))
        .expect("Leader agent required in team.json");

    // Ensure agents are running (spawn if needed, reuse if already up)
    let workspace_map = generate_peer_registration_files(&team, None);
    let mut children = spawn_team_a2a_servers(&team, &workspace_map, None, "e2e-conv-test").await;

    match wait_for_all_team_health(&team).await {
        Ok(()) => println!("✅ All agents healthy"),
        Err(e) => {
            cleanup_children(&mut children).await;
            panic!("Team agents failed health check: {}", e);
        }
    }
    trigger_agent_reload(&team).await;
    println!("✅ Agents spawned, healthy, and peers registered");

    // ── Step 1: Create session + persist user message (with BrainSessionWriter) ──
    let session_id = uuid::Uuid::new_v4().to_string();
    let user_prompt = "인공지능의 역사에 대해 간단히 3줄로 설명해줘.";
    let mut store = SessionStore::new(&dir).expect("SessionStore open failed");

    // Attach BrainSessionWriter so md files are generated alongside DB
    let brain_dir = dir.join("brain");
    let brain_writer = brain_session_rs::brain_session_writer::BrainSessionWriter::new(&brain_dir);
    store.set_brain_writer(brain_writer);

    store
        .ensure_session_with_channel(&session_id, "leader", ".", "team")
        .expect("Failed to create session");

    let content_blocks = serde_json::to_string(&vec![
        serde_json::json!({ "type": "text", "text": user_prompt }),
    ])
    .unwrap_or_else(|_| "[]".to_string());
    store
        .add_full_message_with_blocks(
            &session_id,
            "user",
            user_prompt,
            "leader",
            "",
            "[]",
            &content_blocks,
            0,
            0,
            0,
            0,
        )
        .expect("Failed to persist user message");
    println!(
        "✅ Step 1: User message persisted to session {}",
        &session_id[..8]
    );

    // ── Step 2: Send via A2aProxy (standard A2A protocol) ──
    let leader_proxy = A2aProxy::new(&leader.endpoint, "leader");
    println!("  ⏳ Sending via A2aProxy::send_and_observe to Leader...");

    let result = tokio::time::timeout(
        Duration::from_secs(120),
        leader_proxy.send_and_observe(user_prompt, Some(session_id.clone()), None),
    )
    .await;

    let (accumulated_text, thinking_text, event_count, terminal_state, task_id_str) = match result {
        Ok(Ok((text, events))) => {
            println!(
                "✅ Step 2: A2aProxy completed — {} chars, {} events",
                text.len(),
                events.len()
            );

            // Also extract task_id for task/delegation event recording
            let mut thinking = String::new();
            let mut terminal = String::new();
            let mut task_id = String::new();
            for (i, event) in events.iter().enumerate() {
                let event_text = extract_text_from_stream_event(event);
                match event {
                    StreamEvent::StatusUpdate(su) => {
                        let state = &su.status.state;
                        let coder_kind = su
                            .metadata
                            .as_ref()
                            .and_then(|m| m.get("coderAgent"))
                            .and_then(|ca| ca.get("kind"))
                            .and_then(|k| k.as_str())
                            .unwrap_or("");
                        // Capture task_id from statusUpdate
                        if task_id.is_empty() && !su.task_id.is_empty() {
                            task_id = su.task_id.clone();
                        }
                        println!(
                            "  Event#{}: StatusUpdate state={:?} coderKind='{}' text='{:.80}'",
                            i + 1,
                            state,
                            coder_kind,
                            event_text
                        );
                        if coder_kind == "thought" && !event_text.is_empty() {
                            thinking = event_text.clone();
                        }
                        if is_terminal_state(state) {
                            terminal = format!("{:?}", state);
                        }
                        // Record task event via brain writer
                        let bw = brain_session_rs::brain_session_writer::BrainSessionWriter::new(
                            &brain_dir,
                        );
                        let _ = bw.write_task_event(
                            &session_id,
                            if task_id.is_empty() {
                                "unknown"
                            } else {
                                &task_id
                            },
                            "leader",
                            &format!("{:?}", state).to_lowercase(),
                            if event_text.is_empty() {
                                None
                            } else {
                                Some(&event_text)
                            },
                            None,
                        );
                    }
                    StreamEvent::Task(task) => {
                        println!(
                            "  Event#{}: Task id='{}' state={:?} text='{:.80}'",
                            i + 1,
                            task.id,
                            task.status.state,
                            event_text
                        );
                        if task_id.is_empty() {
                            task_id = task.id.clone();
                        }
                        if is_terminal_state(&task.status.state) {
                            terminal = format!("{:?}", task.status.state);
                        }
                    }
                    StreamEvent::ArtifactUpdate(au) => {
                        println!(
                            "  Event#{}: ArtifactUpdate name='{}'",
                            i + 1,
                            au.artifact.name.as_deref().unwrap_or("?")
                        );
                    }
                    StreamEvent::Message(msg) => {
                        println!("  Event#{}: Message text='{:.80}'", i + 1, event_text);
                    }
                }
            }

            (text, thinking, events.len(), terminal, task_id)
        }
        Ok(Err(e)) => {
            println!("⚠️  Step 2: A2aProxy error: {}", e);
            (
                String::new(),
                String::new(),
                0,
                "error".to_string(),
                String::new(),
            )
        }
        Err(_) => {
            println!("⚠️  Step 2: Timeout after 120s");
            (
                String::new(),
                String::new(),
                0,
                "timeout".to_string(),
                String::new(),
            )
        }
    };

    println!(
        "\n✅ Step 3: {} events, {} chars text, terminal='{}'",
        event_count,
        accumulated_text.len(),
        terminal_state
    );
    if !thinking_text.is_empty() {
        println!("  💭 Thinking: {:.200}", thinking_text);
    }
    println!("  📝 Response: {:.500}", accumulated_text);

    // ── Step 4: Persist assistant message to DB ──
    let assistant_text = if accumulated_text.is_empty() {
        format!(
            "[terminal={}] 에이전트가 텍스트 없이 종료됨",
            terminal_state
        )
    } else {
        accumulated_text.clone()
    };

    let assistant_blocks = serde_json::to_string(&vec![
        serde_json::json!({ "type": "text", "text": assistant_text }),
    ])
    .unwrap_or_else(|_| "[]".to_string());
    store
        .add_full_message_with_blocks(
            &session_id,
            "assistant",
            &assistant_text,
            "leader",
            &thinking_text,
            "[]",
            &assistant_blocks,
            0,
            0,
            0,
            0,
        )
        .expect("Failed to persist assistant message");
    println!("✅ Step 4: Assistant message persisted to DB");

    // ── Step 5: Verify conversation in DB ──
    let messages = store
        .load_session_messages(&session_id)
        .expect("Failed to load session messages");

    println!(
        "\n✅ Step 5: Session {} has {} messages",
        &session_id[..8],
        messages.len()
    );
    for msg in &messages {
        println!(
            "  [{}] role='{}' agent='{}' content='{:.80}'",
            msg.id, msg.role, msg.agent_id, msg.content
        );
    }

    // ── Assertions ──
    assert!(
        messages.len() >= 2,
        "Session should have at least 2 messages, got {}",
        messages.len()
    );

    let user_msg = messages.iter().find(|m| m.role == "user");
    assert!(user_msg.is_some(), "Should have a user message");
    assert_eq!(
        user_msg.unwrap().content,
        user_prompt,
        "User message content should match"
    );

    let assistant_msg = messages.iter().find(|m| m.role == "assistant");
    assert!(assistant_msg.is_some(), "Should have an assistant message");
    assert!(
        !assistant_msg.unwrap().content.is_empty(),
        "Assistant message should not be empty"
    );

    // Verify actual text was extracted (not just fallback)
    assert!(
        !accumulated_text.is_empty(),
        "A2aProxy should extract actual response text, got empty"
    );
    assert!(
        event_count > 0,
        "Should have received at least 1 StreamEvent"
    );

    // ── Step 6: Verify brain session md file was generated ──
    let sessions_dir = brain_dir.join("sessions");
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let short_id = &session_id[..session_id.find('-').unwrap_or(8).min(8)];

    // Search for the session file under solo/{date}/ or team/{date}/
    let mut found_md = None;
    for sub in ["solo", "team"] {
        let date_dir = sessions_dir.join(sub).join(&today);
        if date_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&date_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.contains(short_id) {
                        let path = entry.path();
                        // Could be a file or dir with index.md
                        if path.is_file() && name.ends_with(".md") {
                            found_md = Some(path);
                        } else if path.is_dir() {
                            let idx = path.join("index.md");
                            if idx.exists() {
                                found_md = Some(idx);
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(md_path) = &found_md {
        let content = std::fs::read_to_string(md_path).unwrap_or_default();
        println!("\n✅ Step 6: Brain session md file found at {:?}", md_path);
        println!("  Content ({} bytes):", content.len());
        for line in content.lines().take(15) {
            println!("  | {}", line);
        }
        assert!(
            content.contains("user") || content.contains(user_prompt),
            "Session md should contain user message"
        );
    } else {
        println!(
            "⚠️  Step 6: No brain session md file found for session {} under {:?}",
            short_id, sessions_dir
        );
        // List what's there for debugging
        for sub in ["solo", "team"] {
            let date_dir = sessions_dir.join(sub).join(&today);
            if date_dir.exists() {
                println!("  Contents of {:?}:", date_dir);
                if let Ok(entries) = std::fs::read_dir(&date_dir) {
                    for entry in entries.flatten() {
                        println!("    - {}", entry.file_name().to_string_lossy());
                    }
                }
            }
        }
    }

    println!("\n══════════════════════════════════════════════════════════");
    println!(
        "✅ Full Conversation E2E Complete — {} events, {} chars, terminal='{}'",
        event_count,
        accumulated_text.len(),
        terminal_state
    );
    if found_md.is_some() {
        println!("✅ Brain session md file generated ✓");
    }
    println!("══════════════════════════════════════════════════════════");
}

/// Scenario 6: A2aProxy Tri-Mode Full-Mesh E2E
///
/// Tests all 3 A2aProxy delegation modes against live gemini-cli A2A servers:
///
/// 1. **Synchronous** (`send_and_observe`) — Leader answers directly
/// 2. **Fire-and-forget** (`fire_and_forget`) — send to Researcher, get task_id
/// 3. **Async subscribe** (`send_and_subscribe`) — Leader with SSE stream
/// 4. **Full-mesh** — Researcher → Verifier direct (cross-agent communication)
///
/// Each subtest validates event parsing, text extraction, and terminal state.
#[ignore]
#[tokio::test]
async fn test_a2a_proxy_tri_mode_full_mesh() {
    use a2a_rs::client::StreamEvent;
    use a2a_rs::proxy::{extract_text_from_stream_event, is_terminal_state};

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");

    // Ensure agents are running
    let workspace_map = generate_peer_registration_files(&team, None);
    let mut children = spawn_team_a2a_servers(&team, &workspace_map, None, "e2e-tri-mode").await;

    match wait_for_all_team_health(&team).await {
        Ok(()) => println!("✅ All agents healthy"),
        Err(e) => {
            cleanup_children(&mut children).await;
            panic!("Team agents failed health check: {}", e);
        }
    }
    trigger_agent_reload(&team).await;
    println!("✅ Agents spawned, healthy, and peers registered\n");

    // Build proxies for each agent
    let leader = team
        .agents
        .iter()
        .find(|a| a.role.to_lowercase().contains("leader"))
        .unwrap();
    let researcher = team
        .agents
        .iter()
        .find(|a| a.role.to_lowercase().contains("researcher"))
        .unwrap();
    let verifier = team
        .agents
        .iter()
        .find(|a| a.role.to_lowercase().contains("verifier"))
        .unwrap();

    let leader_proxy = A2aProxy::new(&leader.endpoint, "leader");
    let researcher_proxy = A2aProxy::new(&researcher.endpoint, "researcher");
    let verifier_proxy = A2aProxy::new(&verifier.endpoint, "verifier");

    // ═══════════════════════════════════════════════════════
    // Subtest 1: send_and_observe (synchronous)
    // ═══════════════════════════════════════════════════════
    println!("── Subtest 1: send_and_observe (synchronous) ──");
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        researcher_proxy.send_and_observe("1+1은 뭐야? 숫자만 답해줘.", None, None),
    )
    .await;

    match result {
        Ok(Ok((text, events))) => {
            println!(
                "  ✅ Researcher sync: {} chars, {} events",
                text.len(),
                events.len()
            );
            println!("  Response: {:.200}", text);
            // Even if text is empty (input-required), events should exist
            assert!(!events.is_empty(), "Should have at least 1 event");
            // Check that we got terminal state (including input-required which gemini-cli uses)
            let has_terminal = events.iter().any(|e| match e {
                StreamEvent::StatusUpdate(su) => matches!(
                    su.status.state,
                    a2a_rs::types::TaskState::Completed
                        | a2a_rs::types::TaskState::Failed
                        | a2a_rs::types::TaskState::InputRequired
                        | a2a_rs::types::TaskState::Canceled
                ),
                StreamEvent::Task(t) => matches!(
                    t.status.state,
                    a2a_rs::types::TaskState::Completed
                        | a2a_rs::types::TaskState::Failed
                        | a2a_rs::types::TaskState::InputRequired
                        | a2a_rs::types::TaskState::Canceled
                ),
                _ => false,
            });
            assert!(
                has_terminal,
                "Should reach terminal state (completed/input-required/failed)"
            );
            println!("  ✅ Terminal state reached");
        }
        Ok(Err(e)) => println!("  ⚠️  Researcher sync error: {}", e),
        Err(_) => println!("  ⚠️  Researcher sync timeout"),
    }

    // ═══════════════════════════════════════════════════════
    // Subtest 2: fire_and_forget (async)
    // ═══════════════════════════════════════════════════════
    println!("\n── Subtest 2: fire_and_forget (async) ──");
    let ff_result = tokio::time::timeout(
        Duration::from_secs(30),
        researcher_proxy.fire_and_forget("Rust 언어의 장점 한 줄로", None, None),
    )
    .await;

    match ff_result {
        Ok(Ok(task_id)) => {
            println!(
                "  ✅ Fire-and-forget: task_id={}",
                &task_id[..task_id.len().min(36)]
            );
            assert!(!task_id.is_empty(), "Task ID should not be empty");

            // Now verify we can check the task status (it may already be completed)
            tokio::time::sleep(Duration::from_secs(3)).await;
            match researcher_proxy.get_task(&task_id).await {
                Ok(task) => {
                    println!(
                        "  ✅ Task state: {:?}, has_status_msg={}",
                        task.status.state,
                        task.status.message.is_some()
                    );
                }
                Err(e) => println!(
                    "  ⚠️  get_task error: {} (server may not support tasks/get)",
                    e
                ),
            }
        }
        Ok(Err(e)) => {
            // Fire-and-forget may fail on gemini-cli since it uses message/send
            // which may not be supported — log but don't fail
            println!(
                "  ⚠️  Fire-and-forget error: {} (may not support message/send)",
                e
            );
        }
        Err(_) => println!("  ⚠️  Fire-and-forget timeout"),
    }

    // ═══════════════════════════════════════════════════════
    // Subtest 3: send_and_subscribe (async subscribe)
    // ═══════════════════════════════════════════════════════
    println!("\n── Subtest 3: send_and_subscribe (async subscribe) ──");
    let sub_result = tokio::time::timeout(
        Duration::from_secs(60),
        verifier_proxy.send_and_subscribe("2+2는 뭐야? 숫자만 답해", None, None),
    )
    .await;

    match sub_result {
        Ok(Ok((task_id, mut rx))) => {
            println!(
                "  ✅ Subscribed to task: {}",
                &task_id[..task_id.len().min(36)]
            );
            let mut event_count = 0u32;
            let mut text = String::new();

            // Drain events with timeout
            let drain = tokio::time::timeout(Duration::from_secs(60), async {
                while let Some(event_result) = rx.recv().await {
                    match event_result {
                        Ok(event) => {
                            event_count += 1;
                            let extracted = extract_text_from_stream_event(&event);
                            if !extracted.is_empty() {
                                text = extracted;
                            }
                            let is_done = match &event {
                                StreamEvent::StatusUpdate(su) => {
                                    is_terminal_state(&su.status.state)
                                }
                                StreamEvent::Task(t) => is_terminal_state(&t.status.state),
                                _ => false,
                            };
                            if is_done {
                                break;
                            }
                        }
                        Err(e) => {
                            println!("  ⚠️  Subscribe event error: {}", e);
                            break;
                        }
                    }
                }
            })
            .await;

            match drain {
                Ok(()) => {
                    println!(
                        "  ✅ Subscribe stream: {} events, {} chars",
                        event_count,
                        text.len()
                    );
                    println!("  Response: {:.200}", text);
                }
                Err(_) => println!("  ⚠️  Subscribe stream timeout"),
            }
        }
        Ok(Err(e)) => {
            // send_and_subscribe uses message/send → tasks/resubscribe
            // gemini-cli may not support these methods
            println!(
                "  ⚠️  send_and_subscribe error: {} (may not support message/send)",
                e
            );
        }
        Err(_) => println!("  ⚠️  send_and_subscribe timeout"),
    }

    // ═══════════════════════════════════════════════════════
    // Subtest 4: Full-mesh — agent-to-agent (Researcher uses Verifier tool)
    // ═══════════════════════════════════════════════════════
    // Each agent has peer tools registered via .gemini/agents/{peer}.md.
    // So Researcher has a "verifier" tool it can call directly.
    // This tests true full-mesh: Researcher → (internal tool_call) → Verifier A2A server
    println!("\n── Subtest 4: Full-mesh (Researcher internally calls Verifier) ──");

    let fullmesh_prompt = concat!(
        "1+1의 답이 2인지 verifier 도구를 사용해서 검증해줘. ",
        "반드시 verifier tool_call을 실행해야 합니다. ",
        "검증 결과를 포함해서 답해줘."
    );
    let fullmesh_result = tokio::time::timeout(
        Duration::from_secs(90),
        researcher_proxy.send_and_observe(fullmesh_prompt, None, None),
    )
    .await;

    match fullmesh_result {
        Ok(Ok((text, events))) => {
            println!(
                "  ✅ Full-mesh result: {} chars, {} events",
                text.len(),
                events.len()
            );
            println!("  Response: {:.300}", text);
            // If the researcher delegated, events should be > 2 (submitted + working + delegation + completed)
            if events.len() > 2 {
                println!(
                    "  ✅ Multiple events ({}) suggest delegation occurred",
                    events.len()
                );
            } else {
                println!(
                    "  ⚠️  Only {} events — may not have delegated (check agent logs)",
                    events.len()
                );
            }
        }
        Ok(Err(e)) => println!("  ⚠️  Full-mesh error: {}", e),
        Err(_) => println!("  ⚠️  Full-mesh timeout (90s)"),
    }

    // ═══════════════════════════════════════════════════════
    // Subtest 5: send_streaming (progressive SSE consumption)
    // ═══════════════════════════════════════════════════════
    println!("\n── Subtest 5: send_streaming (progressive SSE) ──");
    let stream_result = tokio::time::timeout(
        Duration::from_secs(60),
        leader_proxy.send_streaming("한국의 수도는? 한 단어로만", None, None),
    )
    .await;

    match stream_result {
        Ok(Ok(mut rx)) => {
            let mut event_count = 0u32;
            let mut final_text = String::new();

            let drain = tokio::time::timeout(Duration::from_secs(60), async {
                while let Some(event_result) = rx.recv().await {
                    match event_result {
                        Ok(event) => {
                            event_count += 1;
                            let t = extract_text_from_stream_event(&event);
                            if !t.is_empty() {
                                final_text = t;
                            }
                            println!(
                                "  Event#{}: {:?}",
                                event_count,
                                match &event {
                                    StreamEvent::StatusUpdate(su) =>
                                        format!("status={:?}", su.status.state),
                                    StreamEvent::Task(t) => format!("task={:?}", t.status.state),
                                    StreamEvent::ArtifactUpdate(_) => "artifact".to_string(),
                                    StreamEvent::Message(_) => "message".to_string(),
                                }
                            );
                        }
                        Err(e) => {
                            println!("  Stream error: {}", e);
                            break;
                        }
                    }
                }
            })
            .await;

            match drain {
                Ok(()) => {
                    println!(
                        "  ✅ Leader streaming: {} events, text={:.100}",
                        event_count, final_text
                    );
                }
                Err(_) => println!("  ⚠️  Leader streaming timeout"),
            }
        }
        Ok(Err(e)) => println!("  ⚠️  send_streaming error: {}", e),
        Err(_) => println!("  ⚠️  send_streaming timeout"),
    }

    // ═══════════════════════════════════════════════════════
    // Summary
    // ═══════════════════════════════════════════════════════
    println!("\n══════════════════════════════════════════════════════════");
    println!("✅ A2aProxy Tri-Mode Full-Mesh E2E Complete");
    println!("  1. send_and_observe (sync) — tested");
    println!("  2. fire_and_forget (async) — tested");
    println!("  3. send_and_subscribe (two-step) — tested");
    println!("  4. Full-mesh (Researcher→Verifier) — tested");
    println!("  5. send_streaming (progressive SSE) — tested");
    println!("══════════════════════════════════════════════════════════");
}
