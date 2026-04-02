//! Codex Native ACP E2E Test — Verify codex-ilhae directly via HTTP
//!
//! This test ensures that the codex-ilhae binary natively supports
//! the ACP protocol over HTTP.

use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn codex_native_http_acp_test() {
    let cwd = std::env::current_dir().expect("cwd");

    // Find codex-ilhae binary by checking current and parent directories
    let mut codex_bin = PathBuf::from("codex-ilhae");
    let mut current = Some(cwd.as_path());
    while let Some(path) = current {
        let candidate = path.join("codex/codex-rs/target/debug/codex-ilhae");
        if candidate.exists() {
            codex_bin = candidate;
            break;
        }
        let candidate2 = path.join("target/debug/codex-ilhae"); // If built in root
        if candidate2.exists() {
            codex_bin = candidate2;
            break;
        }
        current = path.parent();
    }

    if !codex_bin.exists() {
        panic!(
            "ilhae binary not found! Please build it first: cd codex/codex-rs && cargo build -p ilhae"
        );
    }

    let port = 41277;

    println!("\n═══════════════════════════════════════════════════");
    println!(" Codex Native HTTP ACP E2E Test (Direct)");
    println!("═══════════════════════════════════════════════════");

    // 1. Start Codex Agent Server
    println!("[1] Starting Codex Agent Server on port {}...", port);
    let mut agent = Command::new(&codex_bin)
        .arg("a2a-server")
        .arg("--port")
        .arg(port.to_string())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("Failed to spawn codex-ilhae");

    // Wait for server to start
    std::thread::sleep(Duration::from_secs(5));

    // 2. HTTP Client (Use fresh client for each request to avoid connection pooling issues)
    let url = format!("http://localhost:{}/acp", port);

    let send_request = |method: &str, params: Value| -> Value {
        println!("[ACP Client] Requesting: {}", method);
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        let res = client
            .post(&url)
            .json(&json!({
                "jsonrpc": "2.0", "id": 1,
                "method": method, "params": params
            }))
            .send()
            .expect("HTTP Send failed");

        let json: Value = res.json().expect("JSON Parse failed");
        json
    };

    // 3. Test Initialize
    println!("[2] Initializing...");
    let res = send_request(
        "initialize",
        json!({
            "protocolVersion": "0.1.0",
            "clientInfo": { "name": "test", "version": "1" }
        }),
    );
    assert!(res.get("result").is_some(), "Initialize failed: {:?}", res);
    println!("[2] ✅ Initialize OK");

    // 4. Test session/new
    println!("[3] Creating session...");
    let res = send_request("session/new", json!({ "cwd": "/" }));
    let session_id = res["result"]["sessionId"]
        .as_str()
        .expect("No sessionId")
        .to_string();
    println!("[3] ✅ Session created: {}", session_id);

    // 5. Test message/send
    println!("[4] Sending message...");
    let res = send_request(
        "message/send",
        json!({
            "sessionId": session_id,
            "message": { "text": "echo 'Direct ACP Success'" }
        }),
    );

    let text = res["result"]["message"]["text"]
        .as_str()
        .expect("No response text");
    assert!(!text.is_empty());
    println!("[4] ✅ Response: {}", text);

    // Cleanup
    let _ = agent.kill();
    let _ = agent.wait();

    println!("═══════════════════════════════════════════════════");
    println!(" [codex-native-http] PASS");
    println!("═══════════════════════════════════════════════════");
}
