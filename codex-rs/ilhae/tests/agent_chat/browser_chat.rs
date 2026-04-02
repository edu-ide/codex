//! Agent Chat E2E Test — Browser (BotBrowser/Chrome) integration
//!
//! Verifies the agent chat flow triggers browser tool usage.
//! The test validates:
//!   1. Proxy initialization and session creation
//!   2. Agent attempts browser_navigate tool call when prompted
//!   3. Handles CDP connection failure gracefully (expected without --bot-profile)
//!
//! Run: `cargo test --test agent_chat browser_chat_e2e -- --ignored --nocapture`

use serde_json::{Value, json};
use std::time::Duration;

use super::common::proxy_harness::ProxyProcess;
use super::common::team_helpers::*;

// ─── Test ────────────────────────────────────────────────────────────────

#[ignore]
#[test]
fn browser_chat_e2e() {
    println!("═══════════════════════════════════════════════════");
    println!(" Agent Chat E2E: Browser Integration (BotBrowser/Chrome)");
    println!("═══════════════════════════════════════════════════");

    let _dir = ilhae_dir();

    // ── Step 1: Spawn proxy and create session ───────────────────────────
    let mut proxy = ProxyProcess::spawn();
    let session_id = proxy.init_and_create_session();
    println!("[1] ✅ Session created: {}", session_id);

    // ── Step 2: Send prompt to trigger browser tool ──────────────────────
    let user_prompt = "브라우저 도구(browser_navigate)를 사용하여 https://example.com 에 접속하고, \
                       페이지의 <h1> 태그 텍스트를 추출해주세요.";

    println!("\n[2] Sending prompt: \"{}\"", user_prompt);

    let id = proxy.send(
        "session/prompt",
        json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": user_prompt }]
        }),
    );

    // Use 60s timeout — the agent typically responds within 30s.
    // We collect ALL notifications (session/update events) to check for tool calls.
    let (resp, notifs) = proxy.read_response(id, Duration::from_secs(60));

    // ── Step 3: Analyze notifications for browser tool usage ─────────────
    println!("\n[3] Analyzing {} notifications...", notifs.len());

    let mut browser_tool_attempted = false;
    let mut tool_call_names: Vec<String> = Vec::new();

    for notif in &notifs {
        // Check for tool_call events in session/update notifications
        let method = notif["method"].as_str().unwrap_or("");
        if method == "session/update" {
            let update = &notif["params"]["update"];

            // Look for tool_call status updates
            if let Some(title) = update.get("title").and_then(|t| t.as_str()) {
                if title.contains("browser") || title.contains("navigate") {
                    browser_tool_attempted = true;
                    tool_call_names.push(title.to_string());
                    println!("  🔧 Tool call detected: {}", title);
                }
            }

            // Also check toolCallId patterns
            if let Some(tc_id) = update.get("toolCallId").and_then(|t| t.as_str()) {
                if tc_id.contains("browser") {
                    browser_tool_attempted = true;
                    println!("  🔧 Browser tool call ID: {}", tc_id);
                }
            }
        }
    }

    // Also scan raw notification JSON for browser-related content
    if !browser_tool_attempted {
        let all_notifs_str = serde_json::to_string(&notifs).unwrap_or_default();
        if all_notifs_str.contains("browser_navigate") || all_notifs_str.contains("browser-tools") {
            browser_tool_attempted = true;
            println!("  🔧 Browser tool reference found in notification stream");
        }
    }

    // ── Step 4: Evaluate result ──────────────────────────────────────────
    println!("\n[4] Evaluating result...");
    println!("  Browser tool attempted: {}", browser_tool_attempted);
    println!("  Tool calls found: {:?}", tool_call_names);

    match resp {
        Some(val) => {
            if val.get("error").is_some() {
                let err = &val["error"];
                println!("  ⚠️ Prompt returned error: {}", err);
                // Errors related to browser/CDP are expected
                let err_str = err.to_string();
                if err_str.contains("browser")
                    || err_str.contains("CDP")
                    || err_str.contains("timed out")
                {
                    println!("  ✅ Error is browser-related (expected without --bot-profile)");
                } else {
                    panic!("❌ Unexpected error: {}", err);
                }
            } else {
                println!("  ✅ Prompt completed successfully");
                let result_str = val["result"].to_string();
                println!(
                    "  Response: {}",
                    result_str.chars().take(200).collect::<String>()
                );
            }
        }
        None => {
            // Timeout is acceptable — agent may be stuck retrying browser tool
            println!("  ⚠️ Prompt timed out after 60s");
            if browser_tool_attempted {
                println!("  ✅ But browser tool WAS attempted before timeout — test passes");
            } else {
                println!(
                    "  ⚠️ Browser tool was NOT detected. Checking if agent was still processing..."
                );
                // Even timeout is acceptable as long as we can see the agent tried something
            }
        }
    }

    // ── Final verdict ────────────────────────────────────────────────────
    // The key assertion: the agent MUST have attempted to use a browser tool.
    // Whether it succeeded or failed depends on the runtime browser availability.
    assert!(
        browser_tool_attempted,
        "❌ Test failed: Agent did not attempt to use any browser tool. \
         Expected at least one browser_navigate or browser-tools call."
    );

    println!("\n═══════════════════════════════════════════════════");
    println!(" [browser_chat_e2e] PASS ✅");
    println!("  Agent correctly attempted browser tool usage.");
    println!("═══════════════════════════════════════════════════");
}
