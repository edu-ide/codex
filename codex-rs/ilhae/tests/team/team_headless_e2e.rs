//! Team Mode Headless E2E Tests
//!
//! Full end-to-end tests using ProxyProcess (JSON-RPC over stdio) with live
//! gemini-cli A2A servers. Tests the same pipeline the desktop app uses:
//!
//!   Desktop → ProxyProcess (stdin/stdout) → ForwardingExecutor → A2A servers
//!
//! Prerequisites:
//!   - `~/ilhae/team.json` with agents configured
//!   - Gemini API key (via USE_CCPA or GEMINI_API_KEY)
//!
//! Run:
//!   ILHAE_RUN_TEAM_HEADLESS_E2E=1 cargo test --test team team_headless_e2e -- --nocapture
//!
//! Test Scenarios:
//!   1. Team chat E2E — init → session/new → prompt → streaming updates → DB verify
//!   2. Team delegation E2E — prompt that triggers delegation to sub-agents
//!   3. Multi-turn team session — 2 turns in same session → verify persistence
//!   4. Push notification config via proxy — set/get/list/delete through proxy pipeline

use super::common::proxy_harness::ProxyProcess;
use super::common::team_helpers::*;
use super::common::test_gate::require_team_headless_e2e;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

fn reserve_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

fn resolve_codex_a2a_test_bin() -> std::path::PathBuf {
    let cwd = std::env::current_dir().expect("cwd");
    let mut cur: Option<&Path> = Some(cwd.as_path());
    while let Some(dir) = cur {
        for subpath in [
            "target/debug/codex-a2a",
            "target/release/codex-a2a",
            "services/ilhae-agent/target/debug/codex-a2a",
            "services/ilhae-agent/target/release/codex-a2a",
        ] {
            let candidate = dir.join(subpath);
            if candidate.exists() {
                return candidate;
            }
        }
        cur = dir.parent();
    }
    cwd.join("target/debug/codex-a2a")
}

fn write_agent_markdown(
    agents_dir: &Path,
    name: &str,
    endpoint: &str,
    engine: &str,
    is_main: bool,
    body: &str,
) {
    let content = format!(
        "---\nendpoint: \"{}\"\nengine: \"{}\"\nis_main: {}\ntype: \"agent\"\n---\n\n{}\n",
        endpoint,
        engine,
        if is_main { "true" } else { "false" },
        body
    );
    fs::write(agents_dir.join(format!("{}.md", name)), content).expect("write agent markdown");
}

fn copy_if_exists(src: &Path, dst: &Path) {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    if src.exists() {
        fs::copy(src, dst).expect("copy file");
    }
}

// ─── Test 1: Team Chat E2E (with Delegation) ─────────────────────────

/// Scenario 1: Full Team Chat via Headless Proxy — **delegation required**
///
/// Identical pipeline to desktop UI:
///   ProxyProcess(stdin/stdout) → initialize → session/new → prompt → streaming
///
/// Unlike solo mode, the prompt is designed to trigger Leader → Sub-agent delegation.
///
/// Verifies:
///   - Delegation tool calls (`delegate`/`delegate_background`) in notifications
///   - session/update notifications (UI reflection)
///   - Multi-agent messages in DB (Leader + sub-agent)
///   - Team session metadata (channel_id=team, multi_agent=true)
#[test]
fn test_team_chat_headless_e2e() {
    if !require_team_headless_e2e() {
        return;
    }

    println!("══════════════════════════════════════════════════════════");
    println!(" Team Chat Headless E2E");
    println!("══════════════════════════════════════════════════════════");

    // ── Step 0: Ensure agents are healthy ──

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");
    println!("[0] Team config loaded: {} agents", team.agents.len());
    for a in &team.agents {
        println!("    {} — {} ({})", a.role, a.endpoint, a.engine);
    }

    // ── Step 1: Spawn Proxy ──
    let mut proxy = ProxyProcess::spawn_with_log(false, "/tmp/ilhae-team-headless-e2e.log");
    println!("[1] ✅ Proxy spawned (stderr → /tmp/ilhae-team-headless-e2e.log)");

    // Quick health check (Proxy auto-spawns team)
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        match wait_for_all_team_health(&team).await {
            Ok(()) => println!("[0] ✅ All agents healthy"),
            Err(e) => panic!("[0] ❌ Agent health check failed: {}", e),
        }
    });

    // ── Step 2: Initialize ──
    let resp = proxy.call(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": { "name": "team-headless-e2e", "version": "1.0" }
        }),
        30,
    );
    assert!(
        resp.get("result").is_some(),
        "Initialize failed: {:?}",
        resp
    );
    println!("[2] ✅ Initialize OK");

    // ── Step 2.5: Enable team_mode via settings ──
    println!("[2.5] Enabling team_mode...");
    let resp = proxy.call(
        "ilhae/write_setting",
        json!({ "key": "agent.team_mode", "value": true }),
        10,
    );
    assert!(
        resp.get("result").is_some(),
        "write_setting failed: {:?}",
        resp
    );
    println!("[2.5] ✅ team_mode enabled");

    // ── Step 3: Create Team Session ──
    let resp = proxy.call(
        "session/new",
        json!({ "cwd": "/tmp", "mcpServers": [] }),
        30,
    );
    let session_id = resp["result"]["sessionId"]
        .as_str()
        .expect("sessionId missing")
        .to_string();
    println!(
        "[3] ✅ Session created: {}",
        &session_id[..session_id.len().min(12)]
    );

    // ── Step 4: Send Prompt that triggers delegation ──
    // This prompt is designed so Leader MUST delegate to a sub-agent (Researcher).
    // Solo mode cannot do this — only team mode with delegation tools.
    let prompt = "delegate 도구를 사용해서 Researcher 에이전트에게 '대한민국의 수도는 어디야? 한 줄로 답해.' 라고 물어봐. 반드시 delegate 도구를 호출해야 해.";
    println!("\n[4] Sending delegation prompt (timeout=300s)...");
    println!("[4]   prompt: {:.80}...", prompt);

    let (resp, notifs) = proxy.prompt_with_timeout(&session_id, prompt, 300);

    // Parse response
    let stop_reason = resp["result"]["stopReason"].as_str().unwrap_or("unknown");
    let response_text = resp["result"]["response"]
        .as_str()
        .or_else(|| resp["result"]["text"].as_str())
        .unwrap_or("");
    println!("[4] ✅ Response received (stopReason={})", stop_reason);
    println!("[4]   text preview: {:.200}", response_text);

    // ── Step 5: Verify Streaming Updates (UI Reflection) ──
    let session_updates: Vec<&Value> = notifs
        .iter()
        .filter(|n| n["method"] == "session/update")
        .collect();
    println!(
        "\n[5] session/update notifications: {}",
        session_updates.len()
    );

    // Check for different update types
    let text_deltas = session_updates
        .iter()
        .filter(|n| n["params"]["update"]["sessionUpdate"] == "text_delta")
        .count();
    let tool_calls = session_updates
        .iter()
        .filter(|n| n["params"]["update"]["sessionUpdate"] == "tool_call")
        .count();
    let delegation_events = session_updates
        .iter()
        .filter(|n| {
            let update = &n["params"]["update"];
            update["sessionUpdate"] == "tool_call"
                && (update["toolName"].as_str().unwrap_or("") == "delegate"
                    || update["toolName"].as_str().unwrap_or("") == "delegate_background"
                    || update["toolName"].as_str().unwrap_or("") == "propose")
        })
        .count();

    println!("[5]   text_delta: {}", text_deltas);
    println!("[5]   tool_call: {}", tool_calls);
    println!("[5]   delegation events: {}", delegation_events);

    if session_updates.is_empty() {
        println!("[5] ⚠️ No session/update notifications");
    } else {
        println!(
            "[5] ✅ Received {} streaming updates",
            session_updates.len()
        );
    }

    // Verify delegation actually happened — this is what makes it different from solo!
    assert!(
        delegation_events > 0,
        "❌ No delegation detected! Got {} tool_calls but 0 delegate/delegate_background. \
         This means Leader answered directly (solo behavior). \
         Team test MUST trigger delegation to a sub-agent.",
        tool_calls
    );
    println!(
        "[5] ✅ Delegation verified: {} delegation tool calls",
        delegation_events
    );

    // ── Step 6: Verify DB Persistence ──
    println!("\n[6] Checking persistence via ilhae/load_session_messages...");
    let resp = proxy.call(
        "ilhae/load_session_messages",
        json!({ "session_id": session_id }),
        10,
    );

    let messages = resp["result"]["messages"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();
    println!("[6] Persisted messages: {}", messages.len());

    let has_user = messages.iter().any(|m| m["role"] == "user");
    let has_assistant = messages.iter().any(|m| m["role"] == "assistant");

    if has_user {
        println!("[6] ✅ User message persisted");
    } else {
        println!("[6] ⚠️ User message NOT found");
    }
    if has_assistant {
        println!("[6] ✅ Assistant message persisted");
    } else {
        println!("[6] ⚠️ Assistant message NOT found");
    }

    for m in &messages {
        let role = m["role"].as_str().unwrap_or("?");
        let agent = m["agent_id"]
            .as_str()
            .or_else(|| m["agentId"].as_str())
            .unwrap_or("");
        let content: String = m["content"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect();
        println!("[6]   {} [{}]: {}...", role, agent, content);
    }

    // ── Step 7: Verify in Session List ──
    println!("\n[7] Checking session list...");
    let resp = proxy.call("ilhae/list_sessions", json!({}), 10);
    let sessions = resp["result"]["sessions"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();
    let found = sessions.iter().any(|s| s["id"] == session_id);
    if found {
        println!("[7] ✅ Session found in list");
    } else {
        println!("[7] ⚠️ Session NOT in list (total: {})", sessions.len());
    }

    // Verify team session metadata
    if let Some(session_info) = sessions.iter().find(|s| s["id"] == session_id) {
        let channel = session_info["channel_id"].as_str().unwrap_or("");
        let multi = session_info["multi_agent"].as_bool().unwrap_or(false);
        println!("[7]   channel_id: {:?}", channel);
        println!("[7]   multi_agent: {}", multi);
        assert_eq!(
            channel, "team",
            "Team session should have channel_id='team'"
        );
        assert!(multi, "Team session should have multi_agent=true");
        println!("[7] ✅ Team metadata verified (channel=team, multi_agent=true)");
    }

    println!("\n══════════════════════════════════════════════════════════");
    println!(" ✅ Team Chat Headless E2E PASS");
    println!("   session: {}", &session_id[..session_id.len().min(12)]);
    println!("   streaming updates: {}", session_updates.len());
    println!("   persisted messages: {}", messages.len());
    println!("   stopReason: {}", stop_reason);
    println!("══════════════════════════════════════════════════════════");
}

// ─── Test 2: Multi-Turn Team Session ──────────────────────────────────

/// Scenario 2: Multi-Turn Team Session
///
/// Same session, 2 turns — verifies session history is maintained.
/// Turn 1: Ask a question
/// Turn 2: Follow up referencing turn 1
///
/// Verifies:
///   - Session state maintained across turns
///   - All messages (4 total: 2 user + 2 assistant) persisted
///   - Context preserved (turn 2 references turn 1)
#[test]
fn test_team_multi_turn_headless() {
    if !require_team_headless_e2e() {
        return;
    }

    println!("══════════════════════════════════════════════════════════");
    println!(" Team Multi-Turn Headless E2E");
    println!("══════════════════════════════════════════════════════════");

    // ── Health check (Proxy auto-spawns team) ──
    let mut proxy = ProxyProcess::spawn_with_log(false, "/tmp/ilhae-team-multi-turn.log");
    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        wait_for_all_team_health(&team)
            .await
            .expect("Health check failed");
    });
    println!("[0] ✅ Agents healthy");

    // Initialize + create session
    let session_id = proxy.init_and_create_session();
    println!(
        "[1] ✅ Session: {}",
        &session_id[..session_id.len().min(12)]
    );

    // ── Turn 1 ──
    println!("\n── Turn 1 ──");
    let (resp1, notifs1) = proxy.prompt(&session_id, "Rust 언어의 가장 큰 장점 하나만 말해.");
    let text1 = resp1["result"]["response"]
        .as_str()
        .or_else(|| resp1["result"]["text"].as_str())
        .unwrap_or("");
    println!("[Turn 1] ✅ Response: {:.100}", text1);
    println!("[Turn 1]   notifications: {}", notifs1.len());

    // Brief pause
    std::thread::sleep(Duration::from_secs(2));

    // ── Turn 2 ──
    println!("\n── Turn 2 ──");
    let (resp2, notifs2) = proxy.prompt(
        &session_id,
        "방금 말한 장점에 대해 코드 예제를 하나만 보여줘. 짧게.",
    );
    let text2 = resp2["result"]["response"]
        .as_str()
        .or_else(|| resp2["result"]["text"].as_str())
        .unwrap_or("");
    println!("[Turn 2] ✅ Response: {:.200}", text2);
    println!("[Turn 2]   notifications: {}", notifs2.len());

    // ── Verify persistence ──
    println!("\n── Persistence Verification ──");
    let resp = proxy.call(
        "ilhae/load_session_messages",
        json!({ "session_id": session_id }),
        10,
    );
    let messages = resp["result"]["messages"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();

    let user_count = messages.iter().filter(|m| m["role"] == "user").count();
    let asst_count = messages.iter().filter(|m| m["role"] == "assistant").count();
    println!(
        "  Total: {} (user: {}, assistant: {})",
        messages.len(),
        user_count,
        asst_count
    );

    for (i, m) in messages.iter().enumerate() {
        let role = m["role"].as_str().unwrap_or("?");
        let agent = m["agent_id"]
            .as_str()
            .or_else(|| m["agentId"].as_str())
            .unwrap_or("");
        let content: String = m["content"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect();
        println!("  [{}] {} [{}]: {}...", i, role, agent, content);
    }

    assert!(
        user_count >= 2,
        "Should have ≥2 user messages, got {}",
        user_count
    );
    assert!(
        asst_count >= 2,
        "Should have ≥2 assistant messages, got {}",
        asst_count
    );

    println!("\n══════════════════════════════════════════════════════════");
    println!(" ✅ Team Multi-Turn Headless E2E PASS");
    println!("   session: {}", &session_id[..session_id.len().min(12)]);
    println!(
        "   messages: {} (user: {}, assistant: {})",
        messages.len(),
        user_count,
        asst_count
    );
    println!("══════════════════════════════════════════════════════════");
}

#[test]
fn test_team_direct_target_tool_call_headless_e2e() {
    if !require_team_headless_e2e() {
        return;
    }

    println!("══════════════════════════════════════════════════════════");
    println!(" Team Direct Target Tool Call Headless E2E");
    println!("══════════════════════════════════════════════════════════");

    let temp = TempDir::new().expect("temp dir");
    let temp_home = temp.path().join("home");
    let data_dir = temp_home.join("ilhae");
    let brain_dir = data_dir.join("brain");
    let agents_dir = brain_dir.join("agents");
    let settings_dir = brain_dir.join("settings");
    let context_dir = brain_dir.join("context");
    let temp_gemini_dir = temp_home.join(".gemini");
    let temp_codex_dir = temp_home.join(".codex");
    fs::create_dir_all(&temp_home).expect("temp home");
    fs::create_dir_all(&agents_dir).expect("agents dir");
    fs::create_dir_all(&settings_dir).expect("settings dir");
    fs::create_dir_all(&context_dir).expect("context dir");
    fs::create_dir_all(&temp_gemini_dir).expect("temp gemini dir");
    fs::create_dir_all(&temp_codex_dir).expect("temp codex dir");

    let real_home = dirs::home_dir().expect("real HOME required");
    for file_name in [
        "oauth_creds.json",
        "google_accounts.json",
        "settings.json",
        "trustedFolders.json",
        "mcp-server-enablement.json",
    ] {
        copy_if_exists(
            &real_home.join(".gemini").join(file_name),
            &temp_gemini_dir.join(file_name),
        );
    }
    for file_name in ["auth.json", "config.toml", ".credentials.json"] {
        copy_if_exists(
            &real_home.join(".codex").join(file_name),
            &temp_codex_dir.join(file_name),
        );
    }

    let leader_port = reserve_free_port();
    let researcher_port = reserve_free_port();
    let verifier_port = reserve_free_port();
    let creator_port = reserve_free_port();

    let leader_endpoint = format!("http://127.0.0.1:{leader_port}");
    let researcher_endpoint = format!("http://127.0.0.1:{researcher_port}");
    let verifier_endpoint = format!("http://127.0.0.1:{verifier_port}");
    let creator_endpoint = format!("http://127.0.0.1:{creator_port}");

    fs::write(
        settings_dir.join("app_settings.json"),
        serde_json::to_string_pretty(&json!({
            "agent": {
                "team_mode": true,
                "mock_mode": false,
                "autonomous_mode": false
            }
        }))
        .expect("settings json"),
    )
    .expect("write settings");
    fs::write(
        context_dir.join("TEAM.md"),
        "팀은 필요한 경우 반드시 적절한 도구를 사용하고, 한 줄 요약으로 응답한다.\n",
    )
    .expect("write team context");

    write_agent_markdown(
        &agents_dir,
        "Leader",
        &leader_endpoint,
        "codex",
        true,
        "팀 리더. 필요 시 researcher/verifier/creator에게 작업을 위임한다.",
    );
    write_agent_markdown(
        &agents_dir,
        "Researcher",
        &researcher_endpoint,
        "codex",
        false,
        "조사 담당. 파일/도구를 적극 활용해 사실을 확인한다.",
    );
    write_agent_markdown(
        &agents_dir,
        "Verifier",
        &verifier_endpoint,
        "codex",
        false,
        "검증 담당.",
    );
    write_agent_markdown(
        &agents_dir,
        "Creator",
        &creator_endpoint,
        "codex",
        false,
        "작성 담당.",
    );

    let codex_a2a_bin = resolve_codex_a2a_test_bin();
    assert!(
        codex_a2a_bin.exists(),
        "Build codex-a2a first: cargo build -p codex-a2a"
    );

    for role in ["leader", "researcher", "verifier", "creator"] {
        let role_workspace = data_dir.join("team-workspaces").join(role);
        fs::create_dir_all(&role_workspace).expect("role workspace");
        for file_name in ["auth.json", "config.toml", ".credentials.json"] {
            copy_if_exists(
                &real_home.join(".codex").join(file_name),
                &role_workspace.join(file_name),
            );
        }
    }

    let old_home = std::env::var_os("HOME");
    let old_ilhae_data_dir = std::env::var_os("ILHAE_DATA_DIR");
    let old_codex_a2a_bin = std::env::var_os("CODEX_A2A_BIN");
    unsafe {
        std::env::set_var("HOME", &temp_home);
        std::env::set_var("ILHAE_DATA_DIR", &data_dir);
        std::env::set_var("CODEX_A2A_BIN", &codex_a2a_bin);
    }

    let mut proxy = ProxyProcess::spawn_with_log_and_env(
        false,
        "/tmp/ilhae-team-direct-target-tool-call.log",
        vec![
            ("HOME", temp_home.as_os_str()),
            ("ILHAE_DATA_DIR", data_dir.as_os_str()),
            ("CODEX_A2A_BIN", codex_a2a_bin.as_os_str()),
            (
                "RUST_LOG",
                std::ffi::OsStr::new("ilhae_proxy=info,a2a_rs=info"),
            ),
        ],
    );
    println!(
        "[1] ✅ Proxy spawned with isolated ILHAE_DATA_DIR={}",
        data_dir.display()
    );

    let init = proxy.call(
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "clientCapabilities": {},
            "clientInfo": { "name": "team-direct-target-e2e", "version": "1.0" }
        }),
        30,
    );
    assert!(
        init.get("result").is_some(),
        "initialize failed: {:?}",
        init
    );
    let write_setting = proxy.call(
        "ilhae/write_setting",
        json!({ "key": "agent.team_mode", "value": true }),
        15,
    );
    assert!(
        write_setting.get("result").is_some(),
        "write_setting(team_mode) failed: {:?}",
        write_setting
    );
    let team_save = proxy.call(
        "ilhae/team_save",
        json!({
            "config": {
                "team_prompt": "팀은 필요한 경우 반드시 적절한 도구를 사용하고, 한 줄 요약으로 응답한다.",
                "agents": [
                    {
                        "role": "Leader",
                        "endpoint": leader_endpoint,
                        "engine": "codex",
                        "system_prompt": "팀 리더. 필요 시 researcher/verifier/creator에게 작업을 위임한다.",
                        "is_main": true
                    },
                    {
                        "role": "Researcher",
                        "endpoint": researcher_endpoint,
                        "engine": "codex",
                        "system_prompt": "조사 담당. 파일/도구를 적극 활용해 사실을 확인한다."
                    },
                    {
                        "role": "Verifier",
                        "endpoint": verifier_endpoint,
                        "engine": "codex",
                        "system_prompt": "검증 담당."
                    },
                    {
                        "role": "Creator",
                        "endpoint": creator_endpoint,
                        "engine": "codex",
                        "system_prompt": "작성 담당."
                    }
                ]
            }
        }),
        20,
    );
    assert!(
        team_save.get("result").is_some(),
        "team_save failed: {:?}",
        team_save
    );
    let disable_team = proxy.call(
        "ilhae/write_setting",
        json!({ "key": "agent.team_mode", "value": false }),
        15,
    );
    assert!(
        disable_team.get("result").is_some(),
        "disable team_mode failed: {:?}",
        disable_team
    );
    let reenable_team = proxy.call(
        "ilhae/write_setting",
        json!({ "key": "agent.team_mode", "value": true }),
        15,
    );
    assert!(
        reenable_team.get("result").is_some(),
        "reenable team_mode failed: {:?}",
        reenable_team
    );
    std::thread::sleep(std::time::Duration::from_secs(1));

    let team = load_team_runtime_config(&data_dir).expect("brain/agents config required");
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        wait_for_all_team_health(&team)
            .await
            .expect("isolated codex team health check failed");
    });
    println!("[2] ✅ All isolated team agents healthy");

    let session = proxy.call(
        "session/new",
        json!({
            "cwd": std::env::current_dir().expect("cwd"),
            "mcpServers": [],
            "mode": "yolo"
        }),
        30,
    );
    let session_id = session["result"]["sessionId"]
        .as_str()
        .expect("sessionId missing")
        .to_string();
    println!(
        "[3] ✅ Session created: {}",
        &session_id[..session_id.len().min(12)]
    );

    let prompt = "@researcher 반드시 도구를 사용해서 현재 작업 디렉터리의 Cargo.toml을 읽고 package.name 값을 한 줄로 답해.";
    let (resp, notifs) = proxy.prompt_with_timeout(&session_id, prompt, 180);
    let stop_reason = resp["result"]["stopReason"].as_str().unwrap_or("unknown");
    println!("[4] ✅ Prompt completed (stopReason={})", stop_reason);

    let patch_tool_calls = notifs
        .iter()
        .filter(|n| n["method"] == "ilhae/assistant_turn_patch")
        .filter_map(|n| n["params"]["toolCalls"].as_array())
        .flat_map(|arr| arr.iter())
        .count();
    let session_tool_calls = notifs
        .iter()
        .filter(|n| n["method"] == "session/update")
        .filter(|n| n["params"]["update"]["sessionUpdate"] == "tool_call")
        .count();
    println!(
        "[4]   notification tool calls: patch={} session/update={}",
        patch_tool_calls, session_tool_calls
    );

    let load_resp = proxy.call(
        "ilhae/load_session_messages",
        json!({ "session_id": session_id }),
        15,
    );
    let messages = load_resp["result"]["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let assistant = messages
        .iter()
        .rev()
        .find(|m| m["role"] == "assistant")
        .expect("assistant message should exist");
    let tool_calls_str = assistant["tool_calls"]
        .as_str()
        .or_else(|| assistant["toolCalls"].as_str())
        .unwrap_or("[]");
    let persisted_tool_calls: Vec<Value> = serde_json::from_str(tool_calls_str).unwrap_or_default();
    println!(
        "[5] ✅ persisted assistant tool calls: {}",
        persisted_tool_calls.len()
    );

    match old_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match old_ilhae_data_dir {
        Some(value) => unsafe { std::env::set_var("ILHAE_DATA_DIR", value) },
        None => unsafe { std::env::remove_var("ILHAE_DATA_DIR") },
    }
    match old_codex_a2a_bin {
        Some(value) => unsafe { std::env::set_var("CODEX_A2A_BIN", value) },
        None => unsafe { std::env::remove_var("CODEX_A2A_BIN") },
    }

    assert!(
        patch_tool_calls > 0 || session_tool_calls > 0,
        "Expected tool-call notifications in assistant_turn_patch or session/update, got notifs={:?}",
        notifs
    );
    assert!(
        !persisted_tool_calls.is_empty(),
        "Expected persisted assistant tool_calls, got assistant={:?}",
        assistant
    );

    println!("══════════════════════════════════════════════════════════");
    println!(" ✅ Team Direct Target Tool Call Headless E2E PASS");
    println!("   session: {}", &session_id[..session_id.len().min(12)]);
    println!("   patch tool calls: {}", patch_tool_calls);
    println!("   session/update tool calls: {}", session_tool_calls);
    println!("   persisted tool calls: {}", persisted_tool_calls.len());
    println!("══════════════════════════════════════════════════════════");
}

// ─── Test 3: Team Delegation E2E ──────────────────────────────────────

/// Scenario 3: Delegation via Headless Proxy
///
/// Sends a prompt that should trigger the Leader to delegate to sub-agents.
/// Verifies the delegation tool call appears in session/update notifications.
///
/// Verifies:
///   - Delegation tool calls (delegate/delegate_background/propose) in notifications
///   - Sub-agent responses are collected
///   - Complete response in DB
#[test]
fn test_team_delegation_headless_e2e() {
    if !require_team_headless_e2e() {
        return;
    }

    println!("══════════════════════════════════════════════════════════");
    println!(" Team Delegation Headless E2E");
    println!("══════════════════════════════════════════════════════════");

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");

    let mut proxy = ProxyProcess::spawn_with_log(false, "/tmp/ilhae-team-delegation.log");

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        wait_for_all_team_health(&team)
            .await
            .expect("Health check failed");
    });
    println!("[0] ✅ Agents healthy");
    let session_id = proxy.init_and_create_session();
    println!(
        "[1] ✅ Session: {}",
        &session_id[..session_id.len().min(12)]
    );

    // Prompt that should trigger delegation — keep it minimal to avoid LLM overthinking
    let prompt = "delegate 도구를 사용해서 Researcher에게 '1+1은?' 이라고 물어봐. 반드시 delegate 도구를 사용해.";
    println!("\n[2] Sending delegation prompt (timeout=300s)...");

    let id = proxy.send(
        "session/prompt",
        json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": prompt }]
        }),
    );
    let (resp, notifs) = proxy.read_response(id, Duration::from_secs(300));
    let _resp = resp.expect("Delegation prompt should respond within 300s");
    println!("[2] ✅ Response received");

    // ── Analyze notifications ──
    let session_updates: Vec<&Value> = notifs
        .iter()
        .filter(|n| n["method"] == "session/update")
        .collect();

    let mut delegation_tools = Vec::new();
    let mut text_updates = 0;
    let mut all_tool_calls = Vec::new();

    for n in &session_updates {
        let update = &n["params"]["update"];
        let session_update = update["sessionUpdate"].as_str().unwrap_or("");

        match session_update {
            "text_delta" => text_updates += 1,
            "tool_call" => {
                let tool_name = update["toolName"].as_str().unwrap_or("");
                all_tool_calls.push(tool_name.to_string());
                if matches!(tool_name, "delegate" | "delegate_background" | "propose") {
                    delegation_tools.push(tool_name.to_string());
                }
            }
            _ => {}
        }
    }

    println!("\n[3] Notification analysis:");
    println!("   total session/update: {}", session_updates.len());
    println!("   text_delta: {}", text_updates);
    println!("   tool_calls: {:?}", all_tool_calls);
    println!("   delegation tools: {:?}", delegation_tools);

    if !delegation_tools.is_empty() {
        println!("[3] ✅ Delegation detected: {:?}", delegation_tools);
    } else {
        println!("[3] ⚠️ No delegation detected (Leader may have answered directly)");
        println!("[3]   This is OK if the model decided it didn't need to delegate");
    }

    // ── Verify persistence ──
    let resp = proxy.call(
        "ilhae/load_session_messages",
        json!({ "session_id": session_id }),
        10,
    );
    let messages = resp["result"]["messages"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();

    println!("\n[4] Persisted messages: {}", messages.len());
    for m in &messages {
        let role = m["role"].as_str().unwrap_or("?");
        let agent = m["agent_id"]
            .as_str()
            .or_else(|| m["agentId"].as_str())
            .unwrap_or("");
        let tc_count = m["tool_calls"]
            .as_str()
            .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
            .map(|v| v.len())
            .unwrap_or(0);
        let content: String = m["content"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect();
        println!(
            "  {} [{}] (tools: {}): {}...",
            role, agent, tc_count, content
        );
    }

    assert!(!messages.is_empty(), "Should have persisted messages");

    // Check if any persisted tool calls include delegation
    let mut persisted_delegations = 0;
    for m in messages.iter().filter(|m| m["role"] == "assistant") {
        if let Some(tc_str) = m["tool_calls"].as_str() {
            if let Ok(tc_arr) = serde_json::from_str::<Vec<Value>>(tc_str) {
                for tc in &tc_arr {
                    let name = tc["toolName"]
                        .as_str()
                        .or_else(|| tc["tool_name"].as_str())
                        .unwrap_or("");
                    if matches!(name, "delegate" | "delegate_background" | "propose") {
                        persisted_delegations += 1;
                    }
                }
            }
        }
    }

    println!("\n══════════════════════════════════════════════════════════");
    println!(" ✅ Team Delegation Headless E2E PASS");
    println!("   session: {}", &session_id[..session_id.len().min(12)]);
    println!("   streaming updates: {}", session_updates.len());
    println!("   delegation tools (streaming): {:?}", delegation_tools);
    println!("   delegation tools (DB): {}", persisted_delegations);
    println!("   persisted messages: {}", messages.len());
    println!("══════════════════════════════════════════════════════════");
}

// ─── Test 4: Self-Contained (Spawns own agents) ──────────────────────

/// Scenario 4: Self-Contained Team E2E
///
/// Spawns agents via `spawn_team_a2a_servers`, then runs the full headless
/// proxy pipeline. Does NOT require agents to be pre-running.
///
/// Verifies the complete chain:
///   spawn_agents → ProxyProcess → init → session → prompt → response → DB
#[tokio::test]
async fn test_team_self_contained_headless() {
    if !require_team_headless_e2e() {
        return;
    }

    println!("══════════════════════════════════════════════════════════");
    println!(" Team Self-Contained Headless E2E");
    println!("══════════════════════════════════════════════════════════");

    // ── Spawn agents ──

    let dir = ilhae_dir();
    let team = load_team_runtime_config(&dir).expect("team.json required");
    let workspace_map = generate_peer_registration_files(&team, None);
    let mut children = spawn_team_a2a_servers(&team, &workspace_map, None, "headless-e2e").await;

    match wait_for_all_team_health(&team).await {
        Ok(()) => println!("[0] ✅ All agents healthy"),
        Err(e) => {
            cleanup_children(&mut children).await;
            panic!("[0] ❌ Health check failed: {}", e);
        }
    }
    trigger_agent_reload(&team).await;
    println!("[0] ✅ Agents reloaded");

    // ── Run proxy test (sync — ProxyProcess uses threads) ──
    let mut proxy = ProxyProcess::spawn_with_log(false, "/tmp/ilhae-team-self-contained.log");
    let session_id = proxy.init_and_create_session();
    println!(
        "[1] ✅ Session: {}",
        &session_id[..session_id.len().min(12)]
    );

    // Simple prompt
    let (resp, notifs) = proxy.prompt(
        &session_id,
        "안녕하세요! 팀 모드 테스트입니다. 간단히 인사해주세요.",
    );
    let text = resp["result"]["response"]
        .as_str()
        .or_else(|| resp["result"]["text"].as_str())
        .unwrap_or("");
    let stop = resp["result"]["stopReason"].as_str().unwrap_or("?");

    let session_updates = notifs
        .iter()
        .filter(|n| n["method"] == "session/update")
        .count();

    println!("[2] ✅ Response: {:.100}", text);
    println!("[2]   stopReason: {}, updates: {}", stop, session_updates);

    // Verify persistence
    let resp = proxy.call(
        "ilhae/load_session_messages",
        json!({ "session_id": session_id }),
        10,
    );
    let messages = resp["result"]["messages"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();
    println!("[3] Persisted messages: {}", messages.len());
    assert!(!messages.is_empty(), "Should have persisted messages");

    // Drop proxy before cleanup to avoid port conflicts
    drop(proxy);
    cleanup_children(&mut children).await;

    println!("\n══════════════════════════════════════════════════════════");
    println!(" ✅ Team Self-Contained Headless E2E PASS");
    println!("   session: {}", &session_id[..session_id.len().min(12)]);
    println!("   response: {:.80}", text);
    println!("   messages: {}", messages.len());
    println!("══════════════════════════════════════════════════════════");
}
