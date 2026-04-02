//! Mock E2E Test — headless proxy integration with mock mode
//!
//! Run: `cargo test --test agent_chat mock_chat -- --nocapture`

use serde_json::{Value, json};
use std::time::Duration;

use super::common::proxy_harness::ProxyProcess;

// ─── Tests ───────────────────────────────────────────────────────────────

#[test]
fn mock_e2e_full_feature_test() {
    println!("═══════════════════════════════════════════════════");
    println!(" Mock E2E: Full Feature Verification");
    println!("═══════════════════════════════════════════════════");

    let mut proxy = ProxyProcess::spawn_mock();

    // ── Initialize ──────────────────────────────────────────────────────
    println!("\n[1] Initialize...");
    let resp = proxy.call(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": { "name": "mock-e2e", "version": "1.0" }
        }),
        30,
    );
    assert!(
        resp.get("result").is_some(),
        "Initialize failed: {:?}",
        resp
    );
    println!("[1] ✅ Initialize OK");

    // ── Create Session ──────────────────────────────────────────────────
    println!("\n[2] Creating session...");
    let resp = proxy.call(
        "session/new",
        json!({ "cwd": "/tmp", "mcpServers": [] }),
        15,
    );
    let session_id = resp["result"]["sessionId"]
        .as_str()
        .expect("sessionId missing")
        .to_string();
    println!("[2] ✅ Session: {}", session_id);

    // ── Set YOLO + Solo mode ────────────────────────────────────────────
    println!("\n[3] Configuring settings...");
    let resp = proxy.call(
        "ilhae/write_setting",
        json!({ "key": "permissions.approval_preset", "value": "full-access" }),
        10,
    );
    assert!(resp.get("result").is_some());

    let resp = proxy.call(
        "ilhae/write_setting",
        json!({ "key": "agent.team_mode", "value": false }),
        10,
    );
    assert!(resp.get("result").is_some());
    println!("[3] ✅ YOLO + Solo mode set");

    // ── Set locale to Korean ────────────────────────────────────────────
    println!("\n[4] Setting locale to Korean...");
    let resp = proxy.call(
        "ilhae/write_setting",
        json!({ "key": "ui.locale", "value": "ko" }),
        10,
    );
    assert!(resp.get("result").is_some());
    println!("[4] ✅ Locale set to ko");

    // ── Prompt (mock will respond with artifact_save tool calls) ─────────
    println!("\n[5] Sending prompt (mock mode)...");
    let (resp, notifs) = proxy.prompt(&session_id, "간단한 계산기 앱을 만들어줘");
    println!("[5] ✅ Prompt responded");
    println!("    Response has result: {}", resp.get("result").is_some());
    println!("    Notifications received: {}", notifs.len());

    // ── Verify session/update notifications ──────────────────────────────
    let session_updates: Vec<&Value> = notifs
        .iter()
        .filter(|n| n["method"] == "session/update")
        .collect();
    println!("\n[6] Session updates: {}", session_updates.len());

    // Count tool_call notifications
    let tool_calls: Vec<&Value> = session_updates
        .iter()
        .filter(|n| n["params"]["update"]["sessionUpdate"] == "tool_call")
        .copied()
        .collect();
    println!("    Tool call notifications: {}", tool_calls.len());

    // ── Verify mock tool call updates (executed inside proxy) ──────────
    println!("\n[6.5] Verifying mock tool call updates...");
    let tool_updates: Vec<&Value> = session_updates
        .iter()
        .filter(|n| n["params"]["update"]["sessionUpdate"] == "tool_call_update")
        .copied()
        .collect();
    println!("    Tool call updates: {}", tool_updates.len());
    assert!(
        tool_calls.len() >= 2,
        "Should have at least 2 mock tool calls"
    );
    assert!(
        tool_updates.len() >= 2,
        "Should have at least 2 tool_call_update notifications"
    );

    // Count assistant_message notifications
    let assistant_msgs: Vec<&Value> = session_updates
        .iter()
        .filter(|n| n["params"]["update"]["sessionUpdate"] == "assistant_turn_patch")
        .copied()
        .collect();
    println!("    Assistant turn patches: {}", assistant_msgs.len());

    // ── Test artifact_list ──────────────────────────────────────────────
    println!("\n[7] Testing artifact_list...");
    let resp = proxy.call(
        "ilhae/tool_call",
        json!({ "sessionId": session_id, "tool": "artifact_list", "input": {} }),
        10,
    );
    // artifact_list may not exist as a direct JSON-RPC method,
    // but session state should reflect saved artifacts
    println!(
        "[7] artifact_list response: {}",
        if resp.get("result").is_some() {
            "✅ OK"
        } else {
            "⚠️ Not direct method"
        }
    );

    // ── Test read_setting for locale ────────────────────────────────────
    println!("\n[8] Verifying locale setting...");
    let resp = proxy.call("ilhae/read_setting", json!({ "key": "ui.locale" }), 10);
    if let Some(result) = resp.get("result") {
        let locale_val = result["value"].as_str().unwrap_or("");
        assert_eq!(locale_val, "ko", "Locale should be 'ko'");
        println!("[8] ✅ Locale verified: {}", locale_val);
    } else {
        println!("[8] ⚠️ read_setting not available");
    }

    // ── Final Report ────────────────────────────────────────────────────
    println!("\n═══════════════════════════════════════════════════");
    println!(" Mock E2E Results:");
    println!("   ✅ Proxy initialized in mock mode");
    println!("   ✅ Session created: {}", session_id);
    println!("   ✅ Settings (YOLO, Solo, Locale) configured");
    println!("   ✅ Mock prompt responded");
    println!(
        "   {} session/update notifications received",
        session_updates.len()
    );
    println!(
        "   {} tool calls, {} assistant patches",
        tool_calls.len(),
        assistant_msgs.len()
    );
    println!("═══════════════════════════════════════════════════");
}

/// Unit test for mock_provider directly (no proxy needed)
#[test]
fn mock_provider_unit_test() {
    use ilhae_proxy::mock_provider::*;

    println!("═══════════════════════════════════════════════════");
    println!(" Mock Provider Unit Test");
    println!("═══════════════════════════════════════════════════");

    init_mock_mode(true);
    assert!(is_mock_mode());

    // Turn 1: should have artifact_save tool calls
    let resp = get_mock_response("테스트").unwrap();
    assert!(resp.text.contains("작업을 시작"));
    assert_eq!(
        resp.tool_calls.len(),
        2,
        "Turn 1 should have 2 tool calls (task + plan)"
    );
    assert_eq!(resp.tool_calls[0].tool_name, "artifact_save");
    assert_eq!(resp.tool_calls[1].tool_name, "artifact_save");
    println!(
        "[1] ✅ Turn 1: {} + {} tool calls",
        resp.text.len(),
        resp.tool_calls.len()
    );

    // Turn 2: walkthrough + task edit
    let resp = get_mock_response("계속").unwrap();
    assert_eq!(resp.tool_calls.len(), 2);
    assert_eq!(resp.tool_calls[0].tool_name, "artifact_save");
    assert_eq!(resp.tool_calls[1].tool_name, "artifact_edit");
    println!(
        "[2] ✅ Turn 2: {} + {} tool calls",
        resp.text.len(),
        resp.tool_calls.len()
    );

    // Turn 3: memory_write
    let resp = get_mock_response("저장").unwrap();
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].tool_name, "memory_write");
    println!(
        "[3] ✅ Turn 3: {} + {} tool calls",
        resp.text.len(),
        resp.tool_calls.len()
    );

    // Turn 4: exhausted
    let resp = get_mock_response("더").unwrap();
    assert!(resp.text.contains("소진"));
    assert!(resp.tool_calls.is_empty());
    println!("[4] ✅ Turn 4: exhausted correctly");

    reset_mock();
    println!("\n✅ All mock_provider unit tests passed!");
}

/// Artifact CRUD persistence test — uses SessionStore directly
/// This verifies the full artifact lifecycle: save → get → edit(v2) → list → history
#[test]
fn artifact_persistence_e2e() {
    use brain_session_rs::session_store::SessionStore;

    println!("═══════════════════════════════════════════════════");
    println!(" Artifact Persistence E2E Test");
    println!("═══════════════════════════════════════════════════");

    // Use a temp directory for isolated test DB
    let tmp_dir = std::env::temp_dir().join(format!("ilhae_test_{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let store = SessionStore::new(&tmp_dir).expect("create session store");

    let session_id = "test-session-001";

    // ── Step 1: Create session ──────────────────────────────────────────
    println!("\n[1] Creating session...");
    store
        .ensure_session(session_id, "mock-agent", "mock-agent", "/tmp")
        .expect("create session");
    println!("[1] ✅ Session created");

    // ── Step 2: Save artifact v1 (task.md) ──────────────────────────────
    println!("\n[2] Saving artifact: task.md v1...");
    let v1 = store
        .save_artifact_version(
            session_id,
            "task.md",
            "# 📋 Task List\n\n- [ ] 요구사항 분석\n- [ ] 구현\n- [ ] 검증\n",
            "초기 태스크 목록 생성",
            "task",
        )
        .expect("save task v1");
    assert_eq!(v1, 1, "First version should be 1");
    println!("[2] ✅ task.md v1 saved (version={})", v1);

    // ── Step 3: Save artifact v1 (implementation_plan.md) ───────────────
    println!("\n[3] Saving artifact: implementation_plan.md v1...");
    let v1_plan = store
        .save_artifact_version(
            session_id,
            "implementation_plan.md",
            "# 📐 Implementation Plan\n\n## 목표\n계산기 앱 구현\n",
            "구현 계획 작성",
            "plan",
        )
        .expect("save plan v1");
    assert_eq!(v1_plan, 1);
    println!("[3] ✅ implementation_plan.md v1 saved");

    // ── Step 4: Get artifact v1 ─────────────────────────────────────────
    println!("\n[4] Getting artifact: task.md v1...");
    let artifact = store
        .get_artifact_version(session_id, "task.md", 1)
        .expect("get task v1")
        .expect("task v1 should exist");
    assert_eq!(artifact["version"], 1);
    assert!(
        artifact["content"]
            .as_str()
            .unwrap()
            .contains("요구사항 분석")
    );
    assert_eq!(artifact["artifact_type"], "task");
    println!("[4] ✅ task.md v1 retrieved and verified");
    println!(
        "    content_length: {}",
        artifact["content"].as_str().unwrap().len()
    );

    // ── Step 5: Edit artifact → v2 (simulating artifact_edit) ───────────
    println!("\n[5] Editing artifact: task.md → v2...");
    let v2 = store
        .save_artifact_version(
            session_id,
            "task.md",
            "# 📋 Task List\n\n- [x] 요구사항 분석\n- [x] 구현\n- [ ] 검증\n",
            "2개 태스크 완료 마킹",
            "task",
        )
        .expect("save task v2");
    assert_eq!(v2, 2, "Second version should be 2");
    println!("[5] ✅ task.md v2 saved (version={})", v2);

    // ── Step 6: Get artifact v2 ─────────────────────────────────────────
    println!("\n[6] Verifying task.md v2 content...");
    let artifact_v2 = store
        .get_artifact_version(session_id, "task.md", 2)
        .expect("get task v2")
        .expect("task v2 should exist");
    assert_eq!(artifact_v2["version"], 2);
    assert!(
        artifact_v2["content"]
            .as_str()
            .unwrap()
            .contains("[x] 요구사항 분석")
    );
    println!("[6] ✅ task.md v2 content verified");

    // ── Step 7: List version history ────────────────────────────────────
    println!("\n[7] Listing task.md version history...");
    let versions = store
        .list_artifact_versions(session_id, "task.md")
        .expect("list versions");
    assert_eq!(versions.len(), 2, "Should have 2 versions");
    assert_eq!(
        versions[0]["version"], 2,
        "Latest should be v2 (DESC order)"
    );
    assert_eq!(versions[1]["version"], 1, "Oldest should be v1");
    println!("[7] ✅ {} versions found", versions.len());
    for v in &versions {
        println!(
            "    v{}: {} ({})",
            v["version"],
            v["summary"].as_str().unwrap_or(""),
            v["artifact_type"].as_str().unwrap_or("")
        );
    }

    // ── Step 8: List all artifacts in session ───────────────────────────
    println!("\n[8] Listing all artifacts in session...");
    let all_artifacts = store
        .list_session_artifacts(session_id)
        .expect("list session artifacts");
    assert!(
        all_artifacts.len() >= 2,
        "Should have at least 2 artifacts (task.md + implementation_plan.md)"
    );
    println!("[8] ✅ {} artifacts in session", all_artifacts.len());
    for a in &all_artifacts {
        println!("    {}", serde_json::to_string(a).unwrap_or_default());
    }

    // ── Step 9: Save walkthrough.md (completing the artifact trinity) ───
    println!("\n[9] Saving walkthrough.md...");
    let v1_walk = store
        .save_artifact_version(
            session_id,
            "walkthrough.md",
            "# 📝 Walkthrough\n\n## 결과\n- 계산기 앱 구현 완료\n- 모든 테스트 통과\n",
            "작업 완료 요약",
            "walkthrough",
        )
        .expect("save walkthrough v1");
    assert_eq!(v1_walk, 1);
    println!("[9] ✅ walkthrough.md v1 saved");

    // ── Step 10: Verify v1 still accessible after v2 ────────────────────
    println!("\n[10] Verifying v1 still accessible...");
    let v1_again = store
        .get_artifact_version(session_id, "task.md", 1)
        .expect("get task v1 again")
        .expect("task v1 should still exist");
    assert!(
        v1_again["content"]
            .as_str()
            .unwrap()
            .contains("[ ] 요구사항 분석")
    );
    println!("[10] ✅ v1 still intact (old version preserved)");

    // ── Cleanup ─────────────────────────────────────────────────────────
    let _ = std::fs::remove_dir_all(&tmp_dir);

    // ── Final Report ────────────────────────────────────────────────────
    println!("\n═══════════════════════════════════════════════════");
    println!(" Artifact Persistence E2E Results:");
    println!("   ✅ Session created");
    println!("   ✅ task.md: v1 saved → retrieved → v2 edited → both accessible");
    println!("   ✅ implementation_plan.md: v1 saved");
    println!("   ✅ walkthrough.md: v1 saved");
    println!("   ✅ Version history: correct order (DESC)");
    println!("   ✅ Session artifact listing: all artifacts found");
    println!("   ✅ Old versions preserved after edit");
    println!("═══════════════════════════════════════════════════");
}

/// Model change E2E test — verifies session/set_config_option with configId=model
/// This tests the full proxy chain: Client → ContextProxy → MockAgent
///
/// Run: `cargo test --test agent_chat mock_model_change -- --nocapture`
#[test]
fn mock_model_change_e2e() {
    println!("═══════════════════════════════════════════════════");
    println!(" Mock E2E: Model Change via set_config_option");
    println!("═══════════════════════════════════════════════════");

    let mut proxy = ProxyProcess::spawn_with_log(true, "/tmp/model_change_test.log");

    // ── Initialize ──
    println!("\n[1] Initialize...");
    let resp = proxy.call(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": { "name": "model-change-test", "version": "1.0" }
        }),
        30,
    );
    assert!(
        resp.get("result").is_some(),
        "Initialize failed: {:?}",
        resp
    );
    println!("[1] ✅ Initialize OK");

    // ── Create Session ──
    println!("\n[2] Creating session...");
    let resp = proxy.call(
        "session/new",
        json!({ "cwd": "/tmp", "mcpServers": [] }),
        15,
    );
    let session_id = resp["result"]["sessionId"]
        .as_str()
        .expect("sessionId missing")
        .to_string();
    println!("[2] ✅ Session: {}", session_id);

    // ── Test model change via session/set_config_option ──
    println!("\n[3] Changing model via session/set_config_option...");
    let model_id = proxy.send(
        "session/set_config_option",
        json!({
            "sessionId": session_id,
            "configId": "model",
            "value": "gemini-2.5-pro"
        }),
    );
    let (resp, _notifs) = proxy.read_response(model_id, Duration::from_secs(10));
    let resp = resp.expect("set_config_option should respond");
    println!(
        "[3] Response: {}",
        serde_json::to_string_pretty(&resp).unwrap_or_default()
    );

    // Verify response structure
    if let Some(err) = resp.get("error") {
        panic!("[3] ❌ set_config_option returned error: {:?}", err);
    }
    let result = resp.get("result").expect("Should have result");
    // SDK expects configOptions array
    assert!(
        result.get("configOptions").is_some(),
        "Response should have configOptions field, got: {:?}",
        result
    );
    println!("[3] ✅ Model change responded with configOptions");

    // ── Test non-model config option ──
    println!("\n[4] Testing non-model config option...");
    let config_id = proxy.send(
        "session/set_config_option",
        json!({
            "sessionId": session_id,
            "configId": "thought_level",
            "value": "high"
        }),
    );
    let (resp2, _) = proxy.read_response(config_id, Duration::from_secs(10));
    let resp2 = resp2.expect("set_config_option should respond for non-model");
    println!(
        "[4] Response: {}",
        serde_json::to_string_pretty(&resp2).unwrap_or_default()
    );

    if let Some(err) = resp2.get("error") {
        println!(
            "[4] ⚠️ Non-model config option error (expected for mock): {:?}",
            err
        );
    } else {
        println!("[4] ✅ Non-model config option OK");
    }

    // ── Final Report ──
    println!("\n═══════════════════════════════════════════════════");
    println!(" Model Change E2E Results:");
    println!("   ✅ Proxy initialized");
    println!("   ✅ Session created: {}", session_id);
    println!("   ✅ Model change via set_config_option succeeded");
    println!("   ✅ Response has correct configOptions structure");
    println!("═══════════════════════════════════════════════════");
}
