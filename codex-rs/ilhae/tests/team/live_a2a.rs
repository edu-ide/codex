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

// ═══════════════════════════════════════════════════════════════════════
// 1. Team Config Loading Tests (uses ilhae_proxy::context_proxy::team_a2a)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_load_team_runtime_config() {
    let config = load_team_runtime_config(&ilhae_dir());
    assert!(config.is_some(), "team.json should exist and be parseable");

    let config = config.unwrap();
    assert!(
        !config.agents.is_empty(),
        "team.json should define at least one agent"
    );
    assert_eq!(
        config.agents.len(),
        4,
        "Expected 4 team agents (Leader, Researcher, Verifier, Creator)"
    );

    // Verify all expected roles exist
    let roles: Vec<String> = config.agents.iter().map(|a| a.role.clone()).collect();
    for expected in ["Leader", "Researcher", "Verifier", "Creator"] {
        assert!(
            roles.contains(&expected.to_string()),
            "Missing role: {}",
            expected
        );
    }
}

#[test]
fn test_extract_port_from_endpoint() {
    assert_eq!(
        extract_port_from_endpoint("http://localhost:4321"),
        Some(4321)
    );
    assert_eq!(
        extract_port_from_endpoint("http://localhost:4321/"),
        Some(4321)
    );
    assert_eq!(extract_port_from_endpoint("invalid"), None);
}

#[test]
fn test_team_config_endpoints_match_expected() {
    let config = load_team_runtime_config(&ilhae_dir()).expect("team.json must exist");

    for (expected_role, expected_endpoint) in TEAM_ENDPOINTS {
        let agent = config
            .agents
            .iter()
            .find(|a| a.role == *expected_role)
            .unwrap_or_else(|| panic!("Agent {} not found in team.json", expected_role));

        assert_eq!(
            agent.endpoint, *expected_endpoint,
            "{} endpoint mismatch",
            expected_role
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 2. Agent Card Discovery Tests (live HTTP against running servers)
// ═══════════════════════════════════════════════════════════════════════

#[ignore]
#[tokio::test]
async fn test_agent_card_discovery_all_agents() {
    let client = reqwest::Client::new();

    for (role, endpoint) in TEAM_ENDPOINTS {
        let url = format!("{}/.well-known/agent.json", endpoint);
        let resp = client
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await;

        match resp {
            Ok(r) => {
                assert!(
                    r.status().is_success(),
                    "{} agent card returned {}: {}",
                    role,
                    r.status(),
                    url
                );

                let body: serde_json::Value = r.json().await.unwrap();
                assert!(
                    body.get("name").is_some(),
                    "{} agent card missing 'name' field",
                    role
                );
                assert!(
                    body.get("url").is_some(),
                    "{} agent card missing 'url' field",
                    role
                );

                let name = body["name"].as_str().unwrap();
                println!("✅ {} agent card: name={}", role, name);
            }
            Err(e) => {
                panic!(
                    "❌ {} at {} unreachable: {}. Is `pnpm tauri dev` running?",
                    role, url, e
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 3. A2A Health Check Tests (uses ilhae_proxy::context_proxy::team_a2a)
// ═══════════════════════════════════════════════════════════════════════

#[ignore]
#[tokio::test]
async fn test_wait_for_a2a_health_all_agents() {
    let config = load_team_runtime_config(&ilhae_dir()).expect("team.json must exist");

    for agent in &config.agents {
        let result = wait_for_a2a_health(&agent.endpoint, Duration::from_secs(10)).await;

        assert!(
            result.is_ok(),
            "Health check failed for {}: {:?}",
            agent.role,
            result.err()
        );
        println!("✅ {} health check passed", agent.role);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 4. A2A Task Send/Receive Test (live communication)
// ═══════════════════════════════════════════════════════════════════════

use super::common::a2a_test_helpers::send_a2a_message;

#[ignore]
#[tokio::test]
async fn test_a2a_message_send_to_leader() {
    let client = reqwest::Client::new();
    let body = send_a2a_message(&client, "http://localhost:4321", "Say hello in one word.").await;

    println!(
        "📨 Leader response: {}",
        serde_json::to_string_pretty(&body).unwrap()
    );

    // Verify JSON-RPC response structure
    assert!(
        body.get("result").is_some(),
        "Expected 'result' field in response, got error: {:?}",
        body.get("error")
    );

    let result = &body["result"];
    let parsed = parse_a2a_result(result);

    println!("📝 Parsed state: {:?}", parsed.state);
    println!("📝 Parsed task_id: {:?}", parsed.task_id);
    println!("📝 Parsed context_id: {:?}", parsed.context_id);

    // Must have a task ID and state
    assert!(parsed.task_id.is_some(), "Expected task_id in A2A response");
    assert!(parsed.state.is_some(), "Expected state in A2A response");
    println!(
        "✅ Leader accepted message, state={}",
        parsed.state.unwrap()
    );
}

#[ignore]
#[tokio::test]
async fn test_a2a_message_send_all_agents() {
    let client = reqwest::Client::new();

    for (role, endpoint) in TEAM_ENDPOINTS {
        let body = send_a2a_message(&client, endpoint, "Respond with OK.").await;

        if let Some(result) = body.get("result") {
            let parsed = parse_a2a_result(result);
            assert!(parsed.task_id.is_some(), "{}: missing task_id", role);
            println!(
                "✅ {} accepted message: task_id={}, state={:?}",
                role,
                parsed.task_id.unwrap(),
                parsed.state
            );
        } else {
            panic!("❌ {} returned error: {:?}", role, body.get("error"));
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 5. Settings Variable Resolution Test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_settings_path_variables_resolve() {
    let settings_path = ilhae_dir()
        .join("brain")
        .join("settings")
        .join("app_settings.json");
    if !settings_path.exists() {
        eprintln!("⚠️ settings.json not found, skipping");
        return;
    }

    let raw = std::fs::read_to_string(&settings_path).unwrap();
    let settings: Settings = serde_json::from_str(&raw).unwrap();

    // Verify no raw ${VARIABLE} tokens remain in preset cwd fields after
    // resolve_path_variables would be called (we test the source data)
    for preset in &settings.mcp.presets {
        if let Some(cwd) = preset.get("cwd").and_then(|v| v.as_str()) {
            // After variable resolution, paths should start with / and
            // not contain raw ${...} tokens
            let has_variable = cwd.contains("${");
            println!("  preset cwd: {} (has_variable={})", cwd, has_variable);
            // It's OK to have variables in the raw file — they get resolved at runtime
            // But verify they are valid known variables
            if has_variable {
                assert!(
                    cwd.contains("${HOME}")
                        || cwd.contains("${MONOREPO}")
                        || cwd.contains("${PROJECTS}"),
                    "Unknown variable in cwd: {}",
                    cwd
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 6. A2A Response Parsing Tests (unit-level using lib crate)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_parse_a2a_result_completed() {
    let result = serde_json::json!({
        "id": "task-123",
        "status": {
            "state": "completed",
            "message": {
                "role": "agent",
                "parts": [{"text": "Hello, I completed the task."}]
            }
        }
    });

    let parsed = parse_a2a_result(&result);
    assert_eq!(parsed.state.as_deref(), Some("completed"));
    assert!(
        parsed.text.contains("Hello"),
        "Expected text to contain 'Hello', got: {}",
        parsed.text
    );
    assert_eq!(parsed.task_id.as_deref(), Some("task-123"));
}

#[test]
fn test_parse_a2a_result_with_artifacts() {
    let result = serde_json::json!({
        "id": "task-456",
        "status": {"state": "completed"},
        "artifacts": [{
            "parts": [
                {"text": "Line 1\n"},
                {"text": "Line 2\n"}
            ]
        }]
    });

    let parsed = parse_a2a_result(&result);
    assert_eq!(parsed.state.as_deref(), Some("completed"));
    assert!(parsed.text.contains("Line 1"));
    assert!(parsed.text.contains("Line 2"));
}

#[test]
fn test_parse_a2a_result_input_required() {
    use ilhae_proxy::context_proxy::team_a2a::is_input_required_a2a_state;

    assert!(is_input_required_a2a_state("input-required"));
    assert!(is_input_required_a2a_state("inputrequired"));
    assert!(!is_input_required_a2a_state("completed"));
}
