//! Auto Mode (User Agent Interceptor) Tests
//!
//! Tests the core logic used in run_team_orchestration_live when
//! autonomous mode detects an `input-required` state and calls the
//! User Agent to continue the loop.
//!
//! These are unit-level integration tests that validate:
//!   1. input-required state detection from SSE buffer
//!   2. User Agent HTTP call + response parsing
//!   3. Fallback to RALPH_LOOP_TEMPLATE when User Agent is unavailable

use brain_session_rs::session_store::SessionStore;
use ilhae_proxy::context_proxy::autonomy::AutoDirectiveGate;
use ilhae_proxy::context_proxy::autonomy::AutoLoopDecision;
use ilhae_proxy::context_proxy::autonomy::AutoUnusableKind;
use ilhae_proxy::context_proxy::autonomy::ClassifiedUserAgentOutcome;
use ilhae_proxy::context_proxy::autonomy::STOP_REASON_MAX_TURNS;
use ilhae_proxy::context_proxy::autonomy::STOP_REASON_STALL;
use ilhae_proxy::context_proxy::autonomy::STOP_REASON_TIMEBOX;
use ilhae_proxy::context_proxy::autonomy::STOP_REASON_USER_AGENT_FAILURE;
use ilhae_proxy::context_proxy::autonomy::budget_stop_reason;
use ilhae_proxy::context_proxy::autonomy::classify_user_agent_error;
use ilhae_proxy::context_proxy::autonomy::classify_user_agent_text;
use ilhae_proxy::context_proxy::autonomy::followup_text_to_persist;
use ilhae_proxy::context_proxy::autonomy::persist_autonomous_followup;
use ilhae_proxy::context_proxy::autonomy::user_agent::parse_user_agent_response_text;
use serde_json::json;
use tempfile::TempDir;

/// Simulates the SSE buffer parsing logic from runner.rs (lines 1655-1669)
/// to detect if the last event has an `input-required` state.
fn detect_input_required(buffer: &str) -> bool {
    for line in buffer.lines().rev() {
        if let Some(json_str) = line.strip_prefix("data: ") {
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(json_str.trim()) {
                let ev_root = if event.get("kind").is_some() {
                    &event
                } else if let Some(r) = event.get("result") {
                    r
                } else {
                    &event
                };
                if let Some(state) = ev_root.pointer("/status/state").and_then(|v| v.as_str()) {
                    return matches!(state, "input-required" | "TASK_STATE_INPUT_REQUIRED");
                }
            }
        }
    }
    false
}

// ──────────────────────────────────────────────────────────────────────
// 1. input-required detection from SSE buffer
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_detect_input_required_from_sse_buffer() {
    let buffer = r#"data: {"kind":"status-update","status":{"state":"working","message":{"parts":[{"kind":"text","text":"Processing..."}]}}}
data: {"kind":"status-update","status":{"state":"input-required","message":{"parts":[{"kind":"text","text":"I need your confirmation."}]}}}"#;

    assert!(
        detect_input_required(buffer),
        "Should detect input-required from SSE buffer"
    );
}

#[test]
fn test_detect_input_required_from_result_wrapper() {
    // When the event is wrapped in a JSONRPC result
    let buffer = r#"data: {"result":{"status":{"state":"input-required","message":{"parts":[{"kind":"text","text":"Waiting for input."}]}}}}"#;

    assert!(
        detect_input_required(buffer),
        "Should detect input-required from result-wrapped event"
    );
}

#[test]
fn test_detect_completed_is_not_input_required() {
    let buffer = r#"data: {"kind":"status-update","status":{"state":"completed","message":{"parts":[{"kind":"text","text":"Done."}]}}}"#;

    assert!(
        !detect_input_required(buffer),
        "Completed state should not be detected as input-required"
    );
}

#[test]
fn test_detect_working_is_not_input_required() {
    let buffer = r#"data: {"kind":"status-update","status":{"state":"working","message":{"parts":[{"kind":"text","text":"Working..."}]}}}"#;

    assert!(
        !detect_input_required(buffer),
        "Working state should not be detected as input-required"
    );
}

#[test]
fn test_detect_input_required_legacy_state_name() {
    // Legacy A2A state name
    let buffer = r#"data: {"kind":"status-update","status":{"state":"TASK_STATE_INPUT_REQUIRED","message":{"parts":[{"kind":"text","text":"Need input"}]}}}"#;

    assert!(
        detect_input_required(buffer),
        "Should detect legacy TASK_STATE_INPUT_REQUIRED"
    );
}

#[test]
fn test_detect_empty_buffer() {
    assert!(
        !detect_input_required(""),
        "Empty buffer should not be input-required"
    );
}

#[test]
fn test_detect_mixed_buffer_last_event_wins() {
    // Multiple events; the LAST one with a state determines the result
    let buffer = r#"data: {"kind":"status-update","status":{"state":"input-required","message":{"parts":[{"kind":"text","text":"First: need input"}]}}}
data: {"kind":"status-update","status":{"state":"completed","message":{"parts":[{"kind":"text","text":"Actually done."}]}}}"#;

    assert!(
        !detect_input_required(buffer),
        "Last event is completed, so should NOT be input-required"
    );
}

// ──────────────────────────────────────────────────────────────────────
// 2. User Agent HTTP call simulation
// ──────────────────────────────────────────────────────────────────────

/// Starts a tiny mock HTTP server that returns a fixed A2A response.
async fn start_mock_user_agent(port: u16, response_text: &str) -> tokio::task::JoinHandle<()> {
    let response_body = json!({
        "jsonrpc": "2.0",
        "id": "mock-ua-response",
        "result": {
            "status": {
                "state": "completed",
                "message": {
                    "parts": [{ "kind": "text", "text": response_text }]
                }
            }
        }
    });
    let body_str = serde_json::to_string(&response_body).unwrap();

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .unwrap_or_else(|_| panic!("Failed to bind mock UA on port {}", port));

    tokio::spawn(async move {
        use axum::Router;
        use axum::routing::post;
        let body_clone = body_str.clone();
        let app = Router::new().route(
            "/",
            post(move || {
                let b = body_clone.clone();
                async move {
                    axum::response::Json(serde_json::from_str::<serde_json::Value>(&b).unwrap())
                }
            }),
        );
        let _ = axum::serve(listener, app).await;
    })
}

#[tokio::test]
async fn test_user_agent_http_call_returns_directive() {
    // Start mock UA on a random high port (avoid colliding with real 4325)
    let port = 14325;
    let expected_text = "Please proceed with the next step of implementation.";
    let _handle = start_mock_user_agent(port, expected_text).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Simulate the same HTTP call that runner.rs makes (lines 1688-1709)
    let ua_prompt = "Leader agent is waiting for your instruction.\n\n[Previous Output / Context]\nSome progress text\n\nPlease provide your next directive.";
    let ua_request = json!({
        "jsonrpc": "2.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "parts": [{ "kind": "text", "text": ua_prompt }],
                "messageId": uuid::Uuid::new_v4().to_string(),
                "contextId": "test-session"
            }
        }
    });

    let http_client = reqwest::Client::new();
    let resp = http_client
        .post(format!("http://localhost:{}", port))
        .json(&ua_request)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .expect("User Agent HTTP call should succeed");

    assert!(resp.status().is_success(), "UA should return 200");

    let body: serde_json::Value = resp.json().await.expect("Should parse JSON");
    let ua_response_text = parse_user_agent_response_text(&body);

    assert_eq!(
        ua_response_text, expected_text,
        "Parsed User Agent response should match expected text"
    );
    assert!(matches!(
        classify_user_agent_text(&ua_response_text),
        ClassifiedUserAgentOutcome::Continue(text) if text == expected_text
    ));
}

#[tokio::test]
async fn test_user_agent_fallback_when_unavailable() {
    // Simulate calling a User Agent that doesn't exist (port with no listener)
    let http_client = reqwest::Client::new();
    let ua_request = json!({
        "jsonrpc": "2.0",
        "id": "test",
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "parts": [{ "kind": "text", "text": "test" }],
                "messageId": "msg-1",
                "contextId": "test-session"
            }
        }
    });

    // Port 14399 should have nothing listening
    let result = http_client
        .post("http://localhost:14399")
        .json(&ua_request)
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    // Should be Err (connection refused) — this is the fallback path in runner.rs
    assert!(
        result.is_err(),
        "Should fail when User Agent is not running — triggers RALPH_LOOP_TEMPLATE fallback"
    );
}

// ──────────────────────────────────────────────────────────────────────
// 3. End-to-end: detect + dispatch flow
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_auto_mode_e2e_detect_and_dispatch() {
    // 1. Simulate an SSE buffer with input-required
    let buffer = r#"data: {"kind":"status-update","status":{"state":"working","message":{"parts":[{"kind":"text","text":"Analyzing..."}]}}}
data: {"kind":"status-update","status":{"state":"input-required","message":{"parts":[{"kind":"text","text":"Shall I deploy to production?"}]}}}"#;

    let is_input_required = detect_input_required(buffer);
    assert!(is_input_required, "Step 1: Should detect input-required");

    // 2. Start mock User Agent and call it
    let port = 14326;
    let _handle = start_mock_user_agent(port, "Yes, deploy to production.").await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // 3. Simulate the User Agent call (same as runner.rs auto-mode block)
    let progress = "Previous work: analyzed deployment checklist";
    let ua_prompt = format!(
        "Leader agent is waiting for your instruction.\n\n[Previous Output / Context]\n{}\n\nPlease provide your next directive.",
        progress
    );

    let ua_request = json!({
        "jsonrpc": "2.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "parts": [{ "kind": "text", "text": ua_prompt }],
                "messageId": uuid::Uuid::new_v4().to_string(),
                "contextId": "e2e-test-session"
            }
        }
    });

    let http_client = reqwest::Client::new();
    let resp = http_client
        .post(format!("http://localhost:{}", port))
        .json(&ua_request)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .expect("UA call should succeed");

    let body: serde_json::Value = resp.json().await.unwrap();
    let current_prompt = parse_user_agent_response_text(&body);

    assert!(
        !current_prompt.is_empty(),
        "Step 3: User Agent should provide a non-empty directive"
    );
    assert_eq!(
        current_prompt, "Yes, deploy to production.",
        "Step 3: User Agent directive should match"
    );
    assert!(
        followup_text_to_persist(
            &ilhae_proxy::context_proxy::autonomy::decide_auto_loop_action(
                classify_user_agent_text(&current_prompt),
                0,
                progress,
            )
        )
        .is_some()
    );

    // 4. This prompt would be fed back to the Leader as the next iteration
    // In the actual runner.rs, this becomes `current_prompt` for the next loop iteration
    println!(
        "✅ Auto Mode E2E: input-required detected → User Agent returned: {}",
        current_prompt
    );
}

#[test]
fn shipped_classifier_rejects_quota_empty_and_unreachable() {
    let accepted = classify_user_agent_text("첨부 원문을 근거로 회신 초안을 작성하세요.");
    let quota = classify_user_agent_text(
        "Agent Error, unknown agent message: quota will reset at midnight UTC",
    );
    let empty = classify_user_agent_text("");
    let unreachable = classify_user_agent_error("User Agent timed out after 5s");

    assert!(matches!(accepted, ClassifiedUserAgentOutcome::Continue(_)));
    assert!(matches!(
        quota,
        ClassifiedUserAgentOutcome::Unusable {
            kind: AutoUnusableKind::QuotaOrAgentError,
            ..
        }
    ));
    assert!(matches!(
        empty,
        ClassifiedUserAgentOutcome::Unusable {
            kind: AutoUnusableKind::Empty,
            ..
        }
    ));
    assert!(matches!(
        unreachable,
        ClassifiedUserAgentOutcome::Unusable {
            kind: AutoUnusableKind::Unreachable,
            ..
        }
    ));

    for outcome in [quota, empty, unreachable] {
        let decision =
            ilhae_proxy::context_proxy::autonomy::decide_auto_loop_action(outcome, 0, "progress");
        assert!(
            followup_text_to_persist(&decision).is_none(),
            "rejected outcomes must not produce a persistable follow-up, got {decision:?}"
        );
    }
}

#[test]
fn shipped_followup_persist_survives_fresh_store_and_surfaces_errors() {
    let tmp = TempDir::new().expect("tmpdir");
    let data_dir = tmp.path().join("ilhae");
    let session_id = "auto-mode-followup";
    let directive = "다음 단계는 검토 의견을 표로 정리하세요.";

    {
        let store = SessionStore::new(&data_dir).expect("store");
        store
            .create_session(session_id, "auto mode", "leader", "/")
            .expect("create session");
        persist_autonomous_followup(&store, session_id, directive).expect("persist");
    }

    let fresh = SessionStore::new(&data_dir).expect("fresh store");
    let messages = fresh
        .load_session_messages(session_id)
        .expect("load messages");
    assert!(
        messages.iter().any(|message| {
            message.role == "system" && message.agent_id == "leader" && message.content == directive
        }),
        "fresh store missing follow-up row: {messages:?}"
    );

    let error = persist_autonomous_followup(&fresh, "no-such-session", directive)
        .expect_err("missing session must surface a persist error");
    assert!(
        error.contains("not found"),
        "unexpected persist error: {error}"
    );
}

#[test]
fn shipped_stop_reason_ends_after_two_unusable_outcomes() {
    let mut gate = AutoDirectiveGate::default();
    let mut persisted = Vec::new();
    let outcomes = [
        classify_user_agent_text("Agent Error, unknown agent message: quota will reset soon"),
        classify_user_agent_text(""),
        classify_user_agent_text("Agent Error, unknown agent message: quota will reset soon"),
    ];

    let mut last = None;
    for outcome in outcomes {
        let decision = gate.step(outcome, "progress");
        if let Some(text) = followup_text_to_persist(&decision) {
            persisted.push(text.to_string());
        }
        let stop = matches!(decision, AutoLoopDecision::Stop { .. });
        last = Some(decision);
        if stop {
            break;
        }
    }

    match last {
        Some(AutoLoopDecision::Stop { stop_reason }) => {
            assert_eq!(stop_reason, STOP_REASON_USER_AGENT_FAILURE);
        }
        other => panic!("expected user-agent failure stop, got {other:?}"),
    }
    assert!(
        persisted.is_empty(),
        "no identical Ralph/error rows should be written: {persisted:?}"
    );
    assert_eq!(budget_stop_reason(true, 1, 8, 0), Some(STOP_REASON_TIMEBOX));
    assert_eq!(
        budget_stop_reason(false, 8, 8, 0),
        Some(STOP_REASON_MAX_TURNS)
    );
    assert_eq!(budget_stop_reason(false, 3, 8, 2), Some(STOP_REASON_STALL));
}
