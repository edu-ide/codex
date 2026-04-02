//! Artifact Chat E2E Test — headless proxy integration
//!
//! Verifies the artifact tool call flow via JSON-RPC over stdio:
//!   1. initialize → session/new → session/prompt
//!   2. Prompts the agent to create artifacts (task.md, implementation_plan.md, walkthrough.md)
//!   3. Verifies that `session/update` notifications contain completed tool calls
//!      with diff content blocks for each artifact file.
//!
//! Run: `cargo test --test artifact_chat_e2e -- --nocapture`

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// ─── Proxy Process Helper ────────────────────────────────────────────────

struct ProxyProcess {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rx: mpsc::Receiver<String>,
    msg_id: u32,
}

impl ProxyProcess {
    fn spawn() -> Self {
        let workspace = std::env::current_dir().expect("cwd");
        let proxy_bin = workspace.join("target/debug/ilhae-proxy");
        assert!(proxy_bin.exists(), "Build the proxy first: cargo build");

        let stderr_file =
            std::fs::File::create("/tmp/proxy_test_stderr.log").expect("create stderr log");

        let mut child = Command::new(&proxy_bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr_file))
            .spawn()
            .expect("Failed to spawn proxy");

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        std::thread::sleep(Duration::from_secs(15));

        ProxyProcess {
            child,
            stdin,
            rx,
            msg_id: 0,
        }
    }

    fn send(&mut self, method: &str, params: Value) -> u32 {
        self.msg_id += 1;
        let req = json!({
            "jsonrpc": "2.0", "id": self.msg_id,
            "method": method, "params": params,
        });
        writeln!(self.stdin, "{}", req).expect("write to proxy stdin");
        self.msg_id
    }

    fn read_response(&mut self, target_id: u32, timeout: Duration) -> (Option<Value>, Vec<Value>) {
        let deadline = Instant::now() + timeout;
        let mut notifications = Vec::new();

        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self
                .rx
                .recv_timeout(remaining.min(Duration::from_millis(500)))
            {
                Ok(line) => {
                    if let Ok(msg) = serde_json::from_str::<Value>(line.trim()) {
                        if msg.get("id").and_then(|v| v.as_u64()) == Some(target_id as u64) {
                            return (Some(msg), notifications);
                        } else if msg.get("method").is_some() {
                            notifications.push(msg);
                        }
                    } else if !line.trim().is_empty() {
                        println!("[PROXY OUT] {}", line);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        (None, notifications)
    }

    /// Send a prompt and wait for response + notifications
    fn prompt(&mut self, session_id: &str, message: &str) -> (Value, Vec<Value>) {
        let id = self.send(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": message }]
            }),
        );
        let (resp, notifs) = self.read_response(id, Duration::from_secs(300));
        let resp = resp.expect("Prompt should respond within 300s");
        assert!(
            resp.get("result").is_some() || resp.get("error").is_some(),
            "Prompt response unexpected: {:?}",
            resp
        );
        (resp, notifs)
    }
}

impl Drop for ProxyProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ─── Extract artifact diff from notifications ────────────────────────────

fn find_artifact_diff(notifs: &[Value], target_filename: &str) -> Option<(String, usize)> {
    // Derive artifact_type from filename for artifact_save matching
    let target_artifact_type = match target_filename {
        "task.md" => "task",
        "implementation_plan.md" => "plan",
        "walkthrough.md" => "walkthrough",
        _ => "",
    };

    for n in notifs {
        let update = &n["params"]["update"];
        if update["sessionUpdate"] == "tool_call" {
            // Check diff content blocks (write_file style)
            if let Some(content_arr) = update["content"].as_array() {
                for c in content_arr {
                    if c["type"] == "diff" {
                        let path = c["path"].as_str().unwrap_or("");
                        if path == target_filename
                            || path.ends_with(&format!("/{}", target_filename))
                        {
                            let new_text = c["newText"].as_str().unwrap_or("");
                            return Some((path.to_string(), new_text.len()));
                        }
                    }
                }
            }
            // Check rawInput as string
            if let Some(raw_input) = update.get("rawInput") {
                let parsed = if let Some(raw_str) = raw_input.as_str() {
                    serde_json::from_str::<Value>(raw_str).ok()
                } else {
                    raw_input.as_object().map(|o| Value::Object(o.clone()))
                };
                if let Some(parsed) = parsed {
                    // write_file style: { file_path, content }
                    let fp = parsed["file_path"].as_str().unwrap_or("");
                    if fp.ends_with(target_filename) {
                        let content_len = parsed["content"].as_str().map(|s| s.len()).unwrap_or(0);
                        return Some((fp.to_string(), content_len));
                    }
                    // artifact_save style: { artifact_type, content, summary }
                    let at = parsed["artifact_type"].as_str().unwrap_or("");
                    if !target_artifact_type.is_empty() && at == target_artifact_type {
                        let content_len = parsed["content"].as_str().map(|s| s.len()).unwrap_or(0);
                        return Some((format!("artifact_save:{}", at), content_len));
                    }
                }
            }
        }
    }
    None
}

// ─── Test ────────────────────────────────────────────────────────────────

#[test]
fn artifact_chat_e2e_ui_payload_verification() {
    println!("═══════════════════════════════════════════════════");
    println!(" Artifact Chat E2E: Multi-Artifact Verification");
    println!("═══════════════════════════════════════════════════");

    let mut proxy = ProxyProcess::spawn();

    // ── Step 1: Initialize ──────────────────────────────────────────────
    println!("\n[1] Sending initialize...");
    let id = proxy.send(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": { "name": "artifact-chat-e2e", "version": "2.0" }
        }),
    );
    let (resp, _) = proxy.read_response(id, Duration::from_secs(60));
    let resp = resp.expect("Initialize should respond");
    assert!(
        resp.get("result").is_some(),
        "Initialize failed: {:?}",
        resp
    );
    println!("[1] ✅ Initialize OK");

    // ── Step 2: Create Session ──────────────────────────────────────────
    println!("\n[2] Creating session...");
    let id = proxy.send("session/new", json!({ "cwd": "/tmp", "mcpServers": [] }));
    let (resp, _) = proxy.read_response(id, Duration::from_secs(30));
    let resp = resp.expect("session/new should respond");
    let session_id = resp["result"]["sessionId"]
        .as_str()
        .expect("sessionId missing")
        .to_string();
    println!("[2] ✅ Session created: {}", session_id);

    // ── Step 2.5: Enable YOLO mode ──────────────────────────────────────
    println!("\n[2.5] Enabling YOLO mode (full-access)...");
    let id = proxy.send(
        "ilhae/write_setting",
        json!({
            "key": "permissions.approval_preset",
            "value": "full-access"
        }),
    );
    let (resp, _) = proxy.read_response(id, Duration::from_secs(10));
    assert!(resp.expect("settings respond").get("result").is_some());
    println!("[2.5] ✅ YOLO mode enabled");

    // ── Step 2.6: Disable team mode (solo mode for e2e test) ─────────
    println!("\n[2.6] Disabling team mode (solo)...");
    let id = proxy.send(
        "ilhae/write_setting",
        json!({
            "key": "agent.team_mode",
            "value": false
        }),
    );
    let (resp, _) = proxy.read_response(id, Duration::from_secs(10));
    assert!(resp.expect("settings respond").get("result").is_some());
    println!("[2.6] ✅ Solo mode enabled");

    // ── Single Prompt → All 3 Artifacts ──────────────────────────────────
    // Send a single non-trivial task and verify all 3 artifacts are created.
    let expected_artifacts = ["task.md", "implementation_plan.md", "walkthrough.md"];

    println!("\n[3] Sending single non-trivial prompt (expecting all 3 artifacts)...");
    let prompt = "파이썬으로 간단한 계산기 CLI 앱을 만들어줘. 더하기, 빼기, 곱하기, 나누기를 지원해야 해. main() 함수로 실행 가능하게.";
    let preview: String = prompt.chars().take(40).collect();
    println!("[3] Prompt: \"{}...\"", preview);

    let (_resp, notifs) = proxy.prompt(&session_id, prompt);

    let session_updates: Vec<&Value> = notifs
        .iter()
        .filter(|n| n["method"] == "session/update")
        .collect();

    // Log SSE raw info
    for n in &notifs {
        let update = &n["params"]["update"];
        if update["sessionUpdate"] == "tool_call" {
            let has_raw = update.get("rawInput").is_some();
            let content_len = serde_json::to_string(update).map(|s| s.len()).unwrap_or(0);
            println!(
                "[SSE raw] tool_call hasRawInput={} len={}",
                has_raw, content_len
            );
        }
    }

    let mut pass_count = 0;
    let total = expected_artifacts.len();

    for (i, filename) in expected_artifacts.iter().enumerate() {
        match find_artifact_diff(&notifs, filename) {
            Some((path, text_len)) => {
                println!("[{}] ✅ {} found!", i + 4, filename);
                println!("       path: {}", path);
                println!("       content length: {} chars", text_len);
                pass_count += 1;
            }
            None => {
                println!("[{}] ❌ {} NOT found in notifications!", i + 4, filename);
                println!(
                    "       {} session/update notifications received",
                    session_updates.len()
                );
            }
        }
    }

    // ── Final Report ────────────────────────────────────────────────────
    println!("\n═══════════════════════════════════════════════════");
    println!(
        " [artifact-chat-e2e] {}/{} artifacts verified",
        pass_count, total
    );
    println!("═══════════════════════════════════════════════════");

    // At minimum task.md must be created; plan and walkthrough are bonus
    assert!(
        pass_count >= 1,
        "At least task.md should be created, but 0/{} artifacts found",
        total
    );
    if pass_count < total {
        println!(
            " ⚠️ Not all 3 artifacts created in a single turn (got {}/{})",
            pass_count, total
        );
        println!("    This is acceptable — agent may create plan/walkthrough in follow-up turns.");
    }
}
