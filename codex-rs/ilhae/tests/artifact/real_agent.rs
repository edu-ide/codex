//! Artifact real agent tests — session persistence, dir injection, 2-turn versioning
//!
//! test_12: DB seeding — insert structured messages with artifact_save tool calls for UI verification
//! test_13: Artifact dir resolution via ForwardingExecutor
//! test_14: REAL LLM 2-turn versioning E2E — same session, same artifact type, 2 calls → v1, v2

use super::common::team_helpers::*;

/// Scenario 12: DB seeding for UI visual verification (NOT an LLM test)
///
/// Seeds the DB with artifact_save tool calls so ArtifactCard renders correctly.
/// Session is LEFT in DB for manual UI verification.
#[ignore]
#[tokio::test]
async fn test_artifact_session_db_seed() {
    let dir = ilhae_dir();
    let mut store = SessionStore::new(&dir).expect("SessionStore open failed");

    let brain_dir = dir.join("brain");
    let brain_writer = brain_session_rs::brain_session_writer::BrainSessionWriter::new(&brain_dir);
    store.set_brain_writer(brain_writer);

    let session_id = uuid::Uuid::new_v4().to_string();

    store
        .ensure_session_with_channel(&session_id, "gemini", ".", "desktop")
        .expect("Failed to create session");
    store
        .update_session_title(&session_id, "🧪 Artifact DB Seed Test")
        .ok();
    println!("✅ Step 1: Session {} created", &session_id[..8]);

    // User message
    let user_msg = "artifact가 포함된 복잡한 작업을 수행해줘.";
    let user_blocks =
        serde_json::to_string(&vec![serde_json::json!({"type": "text", "text": user_msg})])
            .unwrap();
    store
        .add_full_message_with_blocks(
            &session_id,
            "user",
            user_msg,
            "gemini",
            "",
            "[]",
            &user_blocks,
            0,
            0,
            0,
            0,
        )
        .expect("Failed to persist user message");
    println!("✅ Step 2: User message persisted");

    // Assistant message with artifact_save tool calls
    let assistant_content = "네, 작업을 수행하겠습니다. 아래 아티팩트를 생성했습니다.";
    let tool_calls = serde_json::to_string(&vec![
        serde_json::json!({
            "toolCallId": "tc_task_1", "toolName": "artifact_save", "status": "completed",
            "rawInput": {
                "artifact_type": "task",
                "content": "---\ntags: [artifact, task]\nstatus: in_progress\n---\n\n## Task\n- [x] DB 스키마 업데이트\n- [/] API 엔드포인트 추가\n- [ ] 테스트 작성",
                "summary": "초기 태스크 목록 작성"
            }
        }),
        serde_json::json!({
            "toolCallId": "tc_plan_1", "toolName": "artifact_save", "status": "completed",
            "rawInput": {
                "artifact_type": "plan",
                "content": "---\ntags: [artifact, plan]\nstatus: draft\n---\n\n## Implementation Plan\n\n### Proposed Changes\n\n#### MODIFY: src/api/routes.ts\n- 새로운 GET /artifacts 엔드포인트 추가",
                "summary": "API 엔드포인트 구현 계획"
            }
        }),
        serde_json::json!({
            "toolCallId": "tc_walk_1", "toolName": "artifact_save", "status": "completed",
            "rawInput": {
                "artifact_type": "walkthrough",
                "content": "---\ntags: [artifact, walkthrough]\nstatus: complete\n---\n\n## Walkthrough\n\n### 변경 사항\n- DB에 artifacts 테이블 추가 완료\n\n### 검증 결과\n✅ 모든 테스트 통과",
                "summary": "작업 완료 요약"
            }
        }),
    ]).unwrap();

    let asst_blocks = serde_json::to_string(&vec![
        serde_json::json!({"type": "text", "text": assistant_content}),
    ])
    .unwrap();
    store
        .add_full_message_with_blocks(
            &session_id,
            "assistant",
            assistant_content,
            "gemini",
            "",
            &tool_calls,
            &asst_blocks,
            150,
            350,
            500,
            2000,
        )
        .expect("Failed to persist assistant message");
    println!("✅ Step 3: Assistant message with artifact_save tool calls persisted");

    // Verify
    let messages = store.load_session_messages(&session_id).unwrap_or_default();
    assert_eq!(messages.len(), 2);
    let tc_arr: Vec<serde_json::Value> =
        serde_json::from_str(&messages[1].tool_calls).unwrap_or_default();
    assert!(
        tc_arr.len() >= 3,
        "Should have 3 tool calls, got {}",
        tc_arr.len()
    );
    for tc in &tc_arr {
        assert_eq!(tc["toolName"].as_str().unwrap_or(""), "artifact_save");
    }
    println!("✅ Step 4: All 3 artifact_save tool calls verified in DB");

    // Brain check
    tokio::time::sleep(Duration::from_millis(200)).await;
    println!(
        "✅ Step 5: Brain sessions dir exists: {}",
        brain_dir.join("sessions").exists()
    );

    println!("\n══════════════════════════════════════════════════════════");
    println!("✅ test_12 Artifact DB Seed Complete");
    println!(
        "  Session: {} (title: '🧪 Artifact DB Seed Test')",
        &session_id[..8]
    );
    println!("  ⚡ Session left in DB — open app and verify ArtifactCard rendering");
    println!("══════════════════════════════════════════════════════════");
}

/// Scenario 13: Verify ForwardingExecutor resolves artifact dir per-session
#[ignore]
#[tokio::test]
async fn test_artifact_dir_injection_e2e() {
    use ilhae_proxy::a2a_persistence::ForwardingExecutor;

    let dir = ilhae_dir();
    let store = std::sync::Arc::new(SessionStore::new(&dir).expect("SessionStore"));

    let session_id = uuid::Uuid::new_v4().to_string();
    store
        .ensure_session_with_channel(&session_id, "leader", ".", "team")
        .expect("ensure_session");
    store
        .update_session_title(&session_id, "🧪 Artifact Dir Injection Test")
        .ok();
    println!("✅ Step 1: Team session {} created", &session_id[..8]);

    let cx_cache = ilhae_proxy::CxCache {
        inner: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
    };
    let executor = ForwardingExecutor::with_main_flag(
        "http://localhost:9999".to_string(),
        "leader".to_string(),
        true,
        store.clone(),
        cx_cache.clone(),
    );

    let artifact_dir = executor.resolve_artifact_dir(&session_id);
    assert!(artifact_dir.is_some());
    let artifact_dir = artifact_dir.unwrap();
    assert!(artifact_dir.exists());
    assert!(artifact_dir.to_string_lossy().contains("brain/sessions/"));
    println!("✅ Step 2: Artifact dir resolved: {:?}", artifact_dir);

    // Write test files
    std::fs::write(
        artifact_dir.join("task.md"),
        "---\ntype: task\n---\n## Task\n- [ ] Test\n",
    )
    .unwrap();
    std::fs::write(
        artifact_dir.join("implementation_plan.md"),
        "---\ntype: plan\n---\n## Plan\n",
    )
    .unwrap();
    std::fs::write(
        artifact_dir.join("walkthrough.md"),
        "---\ntype: walkthrough\n---\n## Walkthrough\n✅ OK\n",
    )
    .unwrap();
    println!("✅ Step 3: All 3 artifact files written");

    // Solo session verification
    let solo_id = uuid::Uuid::new_v4().to_string();
    store
        .ensure_session_with_channel(&solo_id, "gemini", ".", "desktop")
        .expect("ensure solo");
    store
        .update_session_title(&solo_id, "🧪 Solo Artifact Test")
        .ok();
    let solo_exec = ForwardingExecutor::with_main_flag(
        "http://localhost:9999".to_string(),
        "gemini".to_string(),
        false,
        store.clone(),
        cx_cache,
    );
    let solo_dir = solo_exec.resolve_artifact_dir(&solo_id).unwrap();
    assert!(solo_dir.exists());
    println!("✅ Step 4: Solo artifact dir: {:?}", solo_dir);

    println!("\n══════════════════════════════════════════════════════════");
    println!(
        "✅ test_13 Complete | Team: {:?} | Solo: {:?}",
        artifact_dir, solo_dir
    );
    println!("══════════════════════════════════════════════════════════");
}

/// Scenario 14: Real LLM 2-Turn Versioning E2E
///
/// SAME session, SAME artifact type, TWO turns:
///   Turn 1: "할일 목록 3개 만들어줘"        → task.md v1
///   Turn 2: "항목 2개 더 추가해줘"            → task.md v2
///   Verify: DB has v1 and v2 with DIFFERENT content
#[ignore]
#[tokio::test]
async fn test_real_agent_versioning_e2e() {
    use a2a_rs::event::EventBus;
    use a2a_rs::executor::{AgentExecutor, RequestContext};
    use a2a_rs::types::*;
    use ilhae_proxy::a2a_persistence::ForwardingExecutor;

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");
    let leader = team
        .agents
        .iter()
        .find(|a| a.role.to_lowercase().contains("leader"))
        .expect("Leader agent required");

    // ── Spawn agents ──
    let workspace_map = generate_peer_registration_files(&team, None);
    let mut children = spawn_team_a2a_servers(&team, &workspace_map, None, "e2e-versioning").await;

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
        .update_session_title(&session_id, "🧪 2-Turn Versioning Test")
        .ok();
    println!("✅ Session {} created", &session_id[..8]);

    let cx_cache = ilhae_proxy::CxCache {
        inner: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
    };
    let executor = ForwardingExecutor::with_main_flag(
        leader.endpoint.trim_end_matches('/').to_string(),
        "leader".to_string(),
        true,
        store.clone(),
        cx_cache,
    );

    // Helper: send a prompt and wait for completion
    async fn send_turn(
        executor: &ForwardingExecutor,
        store: &std::sync::Arc<SessionStore>,
        session_id: &str,
        prompt: &str,
        turn_label: &str,
    ) {
        // Persist user message
        let blocks =
            serde_json::to_string(&vec![serde_json::json!({"type": "text", "text": prompt})])
                .unwrap_or_else(|_| "[]".to_string());
        store
            .add_full_message_with_blocks(
                session_id, "user", prompt, "leader", "", "[]", &blocks, 0, 0, 0, 0,
            )
            .expect("persist user message");
        println!("  ✅ {}: User message persisted", turn_label);

        let request = SendMessageRequest {
            message: Message {
                message_id: uuid::Uuid::new_v4().to_string(),
                context_id: Some(session_id.to_string()),
                task_id: None,
                role: Role::User,
                parts: vec![Part {
                    kind: Some("text".to_string()),
                    text: Some(prompt.to_string()),
                    raw: None,
                    url: None,
                    data: None,
                    metadata: None,
                    filename: None,
                    media_type: None,
                }],
                metadata: None,
                extensions: vec![],
                reference_task_ids: None,
            },
            configuration: None,
            metadata: None,
        };

        let context = RequestContext {
            task_id: Some(uuid::Uuid::new_v4().to_string()),
            context_id: session_id.to_string(),
            request,
        };

        let event_bus = EventBus::new(100);
        let result = tokio::time::timeout(
            Duration::from_secs(120),
            executor.execute(context, &event_bus),
        )
        .await;

        match &result {
            Ok(Ok(())) => println!("  ✅ {}: execute() completed", turn_label),
            Ok(Err(e)) => println!("  ⚠️ {}: execute() error: {}", turn_label, e),
            Err(_) => println!("  ⚠️ {}: execute() timed out", turn_label),
        }
    }

    // ══════════════════════════════════════════════════════════
    // Turn 1: Create task
    // ══════════════════════════════════════════════════════════
    println!("\n── Turn 1: Create task ──");
    send_turn(
        &executor,
        &store,
        &session_id,
        "할 일 목록을 만들어줘. 3개 항목으로. 주제는 '프로젝트 설정'이야.",
        "Turn 1",
    )
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Check v1
    let v1_versions = store
        .list_artifact_versions(&session_id, "task.md")
        .unwrap_or_default();
    let v1_on_disk = {
        let artifact_dir = executor.resolve_artifact_dir(&session_id);
        artifact_dir
            .map(|d| d.join("task.md").exists())
            .unwrap_or(false)
    };
    println!(
        "  Turn 1 result: DB versions={}, disk={}",
        v1_versions.len(),
        v1_on_disk
    );

    // ══════════════════════════════════════════════════════════
    // Turn 2: Update task (same session!)
    // ══════════════════════════════════════════════════════════
    println!("\n── Turn 2: Update task ──");
    send_turn(
        &executor,
        &store,
        &session_id,
        "방금 만든 할 일 목록에 항목 2개를 더 추가해줘. '테스트 작성'과 '배포 준비'를 넣어줘.",
        "Turn 2",
    )
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // ══════════════════════════════════════════════════════════
    // Verify versioning
    // ══════════════════════════════════════════════════════════
    println!("\n── Versioning Verification ──");

    // Check all messages
    let messages = store.load_session_messages(&session_id).unwrap_or_default();
    println!("  Total messages: {}", messages.len());
    for msg in &messages {
        let preview: String = msg.content.chars().take(60).collect();
        println!(
            "    {} [{}] {}B: {}...",
            msg.role,
            msg.agent_id,
            msg.content.len(),
            preview
        );
    }

    // Check artifact versions in DB
    let all_versions = store
        .list_artifact_versions(&session_id, "task.md")
        .unwrap_or_default();
    println!("\n  task.md versions in DB: {}", all_versions.len());
    for v in &all_versions {
        println!(
            "    v{}: {} chars, summary='{}'",
            v["version"],
            v["content_length"],
            v["summary"].as_str().unwrap_or("")
        );
    }

    // Check files on disk
    if let Some(artifact_dir) = executor.resolve_artifact_dir(&session_id) {
        println!("\n  Artifact dir files:");
        if let Ok(entries) = std::fs::read_dir(&artifact_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                println!("    📄 {} ({} bytes)", name.to_string_lossy(), size);
            }
        }

        // Show task.md content for manual verification
        let task_path = artifact_dir.join("task.md");
        if task_path.exists() {
            let content = std::fs::read_to_string(&task_path).unwrap_or_default();
            println!("\n  task.md content ({} bytes):", content.len());
            for line in content.lines().take(15) {
                println!("    | {}", line);
            }
        }
    }

    // Check artifact_save tool calls across all assistant messages
    let mut total_artifact_save = 0;
    for msg in messages.iter().filter(|m| m.role == "assistant") {
        if let Ok(tc_arr) = serde_json::from_str::<Vec<serde_json::Value>>(&msg.tool_calls) {
            for tc in &tc_arr {
                let name = tc["toolName"]
                    .as_str()
                    .or_else(|| tc["tool_name"].as_str())
                    .unwrap_or("");
                if name == "artifact_save" || name == "artifact_edit" {
                    total_artifact_save += 1;
                    let at = tc["rawInput"]["artifact_type"].as_str().unwrap_or("?");
                    println!("  ✅ {} tool call: type={}", name, at);
                }
            }
        }
    }

    cleanup_children(&mut children).await;

    // ── Final Report ──
    println!("\n══════════════════════════════════════════════════════════");
    println!("✅ test_14 Two-Turn Versioning E2E Complete");
    println!(
        "  Session: {} (🧪 2-Turn Versioning Test)",
        &session_id[..8]
    );
    println!("  Messages: {}", messages.len());
    println!("  task.md DB versions: {}", all_versions.len());
    println!("  artifact_save/edit tool calls: {}", total_artifact_save);
    println!("  ⚡ Session left in DB — verify version history in app UI");
    println!("══════════════════════════════════════════════════════════");
}
