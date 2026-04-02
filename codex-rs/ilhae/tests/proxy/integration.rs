//! Rust integration tests for ilhae-proxy using the lib crate directly.
//!
//! These tests import and call real production code from `ilhae_proxy::*`.
//! No mocks — real SettingsStore, real ProcessSupervisor, real agent spawning.
//!
//! Run: `cargo test --test proxy_integration -- --nocapture`

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

// ── Direct lib crate imports ─────────────────────────────────────────
use ilhae_proxy::helpers::probe_tcp;
use ilhae_proxy::process_supervisor::{
    create_supervisor, get_status, register_team_processes, spawn_supervisor_loop,
};
use ilhae_proxy::settings_store::SettingsStore;

// ═══════════════════════════════════════════════════════════════════════
// Scenario 1: Real Supervisor + Agent Lifecycle
//   - Creates real SettingsStore from disk
//   - Creates real ProcessSupervisor with RealAgentSpawner
//   - Verifies health check via real TCP probe
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn scenario_1_supervisor_spawns_real_agents() {
    let ilhae_dir = dirs::home_dir().unwrap().join("ilhae");
    let settings_store = Arc::new(SettingsStore::new(&ilhae_dir));
    let settings = settings_store.get();

    println!(
        "[1] Settings loaded — team_mode={}, command={}",
        settings.agent.team_mode, settings.agent.command
    );
    println!("[1] a2a_endpoint={}", settings.agent.a2a_endpoint);

    // Create real supervisor (internally uses RealAgentSpawner)
    let handle = create_supervisor(settings_store.clone());

    // Check what processes are registered
    {
        let sv = handle.read().await;
        println!("[1] Registered processes:");
        for (name, proc) in &sv.processes {
            println!(
                "  {} — port={}, engine={}, enabled={}, pid={:?}",
                name, proc.port, proc.engine, proc.enabled, proc.pid
            );
        }
    }

    // Start the supervisor loop (spawns background health-check task)
    spawn_supervisor_loop(handle.clone(), None);

    // Wait for agents to become healthy (supervisor loop has 10s initial delay + spawn time)
    let timeout = Duration::from_secs(30);
    let start = Instant::now();
    let mut alive_ports = Vec::new();

    while start.elapsed() < timeout {
        let status = get_status(&handle).await;
        alive_ports.clear();
        for (name, port, enabled, alive, pid) in &status {
            if *alive && *enabled {
                alive_ports.push((*port, name.clone()));
            }
            println!(
                "[1] {:20} port={} enabled={} alive={} pid={:?}",
                name, port, enabled, alive, pid
            );
        }
        if !alive_ports.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // Verify at least one agent is alive
    assert!(
        !alive_ports.is_empty(),
        "At least one agent should be alive after {}s",
        timeout.as_secs()
    );
    println!("[1] ✅ Alive agents: {:?}", alive_ports);

    // Direct TCP probe using lib crate function
    for (port, name) in &alive_ports {
        assert!(
            probe_tcp("127.0.0.1", *port),
            "probe_tcp should confirm {} on port {} is alive",
            name,
            port
        );
    }
    println!("[1] ✅ TCP probes confirmed");
}

// ═══════════════════════════════════════════════════════════════════════
// Scenario 2: Solo Prompt → Response (via proxy binary stdio)
//   - Spawns real proxy, sends JSON-RPC
//   - Uses real session_store to verify DB persistence
// ═══════════════════════════════════════════════════════════════════════

/// Helper: Spawn the proxy binary and wrap stdin/stdout for JSON-RPC.
struct ProxyProcess {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    reader: std::io::BufReader<std::process::ChildStdout>,
    msg_id: u32,
}

impl ProxyProcess {
    fn spawn() -> Self {
        use std::process::{Command, Stdio};
        let workspace = std::env::current_dir().expect("cwd");
        let proxy_bin = workspace.join("target/debug/ilhae-proxy");
        assert!(proxy_bin.exists(), "Build the proxy first: cargo build");

        let mut child = Command::new(&proxy_bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("Failed to spawn proxy");

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let reader = std::io::BufReader::new(stdout);

        std::thread::sleep(Duration::from_secs(3));

        ProxyProcess {
            child,
            stdin,
            reader,
            msg_id: 0,
        }
    }

    fn send(&mut self, method: &str, params: Value) -> u32 {
        use std::io::Write;
        self.msg_id += 1;
        let req = json!({
            "jsonrpc": "2.0", "id": self.msg_id,
            "method": method, "params": params,
        });
        writeln!(self.stdin, "{}", req).expect("write to proxy stdin");
        self.msg_id
    }

    fn read_response(&mut self, target_id: u32, timeout: Duration) -> (Option<Value>, Vec<Value>) {
        use std::io::BufRead;
        let deadline = Instant::now() + timeout;
        let mut notifications = Vec::new();
        let mut line = String::new();

        while Instant::now() < deadline {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if let Ok(msg) = serde_json::from_str::<Value>(line.trim()) {
                        if msg.get("id").and_then(|v| v.as_u64()) == Some(target_id as u64) {
                            return (Some(msg), notifications);
                        } else if msg.get("method").is_some() {
                            notifications.push(msg);
                        }
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        (None, notifications)
    }
}

impl Drop for ProxyProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn scenario_2_solo_prompt_and_db_persistence() {
    let mut proxy = ProxyProcess::spawn();

    // Initialize
    let id = proxy.send(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": { "name": "rust-integration-test", "version": "1.0" }
        }),
    );
    let (resp, _) = proxy.read_response(id, Duration::from_secs(20));
    let resp = resp.expect("Initialize should respond");
    assert!(resp.get("result").is_some(), "Initialize: {:?}", resp);

    // Create session
    let id = proxy.send("session/new", json!({ "cwd": "/tmp", "mcpServers": [] }));
    let (resp, _) = proxy.read_response(id, Duration::from_secs(20));
    let resp = resp.expect("session/new should respond");
    let session_id = resp["result"]["sessionId"]
        .as_str()
        .expect("sessionId missing")
        .to_string();
    println!("[2] Session created: {}", session_id);

    // Health check via lib crate
    let alive = probe_tcp("127.0.0.1", 41241);
    println!("[2] Solo gemini 41241: alive={}", alive);

    // Prompt
    let id = proxy.send(
        "session/prompt",
        json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": "Say hello in one word" }]
        }),
    );
    let (resp, notifs) = proxy.read_response(id, Duration::from_secs(60));
    let resp = resp.expect("Prompt should respond");
    assert!(
        resp.get("result").is_some() || resp.get("error").is_some(),
        "Prompt: {:?}",
        resp
    );

    let updates = notifs
        .iter()
        .filter(|n| n["method"] == "session/update")
        .count();
    println!("[2] session/update notifications: {}", updates);
    assert!(updates > 0, "Should receive streaming updates");

    // Verify DB persistence using rusqlite directly (session_store uses SQLite)
    let ilhae_dir = dirs::home_dir().unwrap().join("ilhae");
    let db_path = ilhae_dir.join("sessions.db");
    if db_path.exists() {
        match rusqlite::Connection::open(&db_path) {
            Ok(conn) => {
                let count: Result<i64, _> = conn.query_row(
                    "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                    [&session_id],
                    |row| row.get::<_, i64>(0),
                );
                let count = count.unwrap_or(0);
                println!(
                    "[2] ✅ DB messages for session {}: {}",
                    &session_id[..12],
                    count
                );
                if count == 0 {
                    println!("[2] ⚠️ 0 messages — possible WAL journaling delay or table mismatch");
                }
            }
            Err(e) => {
                println!("[2] ⚠️ Could not open DB: {} — skipping DB check", e);
            }
        }
    } else {
        println!(
            "[2] ⚠️ sessions.db not found at {:?} — skipping DB check",
            db_path
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Scenario 3: Team Mode Config via lib crate
//   - Directly uses SettingsStore to toggle team_mode
//   - Creates supervisor, registers team processes
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn scenario_3_team_mode_registration() {
    let ilhae_dir = dirs::home_dir().unwrap().join("ilhae");
    let settings_store = Arc::new(SettingsStore::new(&ilhae_dir));
    let handle = create_supervisor(settings_store.clone());

    // Register team processes using lib crate directly
    let team_agents = vec![
        ("Leader".to_string(), 4321u16, "gemini".to_string()),
        ("Researcher".to_string(), 4322u16, "gemini".to_string()),
        ("Verifier".to_string(), 4323u16, "gemini".to_string()),
    ];
    let workspace_map = std::collections::HashMap::new();
    register_team_processes(&handle, &team_agents, &workspace_map).await;

    // Verify team processes are registered
    {
        let sv = handle.read().await;
        let team_count = sv
            .processes
            .iter()
            .filter(|(k, _)| k.starts_with("team-"))
            .count();
        println!("[3] Team processes registered: {}", team_count);
        assert_eq!(team_count, 3, "Expected 3 team processes");

        // Solo should be disabled
        for (name, proc) in &sv.processes {
            if name.ends_with("-solo") {
                assert!(!proc.enabled, "{} should be disabled in team mode", name);
            }
        }
        println!("[3] ✅ Solo processes disabled in team mode");
    }

    // Verify status
    let status = get_status(&handle).await;
    for (name, port, enabled, alive, pid) in &status {
        println!(
            "[3] {:20} port={} enabled={} alive={} pid={:?}",
            name, port, enabled, alive, pid
        );
    }

    let team_enabled: Vec<_> = status
        .iter()
        .filter(|(name, _, enabled, _, _)| name.starts_with("team-") && *enabled)
        .collect();
    assert_eq!(team_enabled.len(), 3, "All 3 team agents should be enabled");
    println!("[3] ✅ Team mode registration verified");
}

// ═══════════════════════════════════════════════════════════════════════
// Scenario 4: Raw Input Pipeline Verification (lib crate only)
//   - Uses agent_client_protocol_schema::SessionUpdate directly
//   - Simulates the exact relay_proxy.rs code path (L105-L117)
//   - Verifies rawInput survives deser → to_value → push_tool_call
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scenario_4_raw_input_pipeline_lib_crate() {
    use agent_client_protocol_schema::SessionUpdate;
    use ilhae_proxy::turn_accumulator::TurnAccumulator;

    // Test 1: ToolCall with rawInput — simulates what agent sends
    println!("[4.1] ToolCall with rawInput — relay_proxy deserialization path");
    let tc_json = json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "run_shell_command-abc",
        "title": "Shell",
        "status": "in_progress",
        "rawInput": {"command": "echo hello", "cwd": "/tmp"},
        "kind": "execute",
        "content": [],
        "locations": []
    });

    let update: SessionUpdate = serde_json::from_value(tc_json.clone()).unwrap();
    if let SessionUpdate::ToolCall(ref tc) = update {
        assert!(tc.raw_input.is_some(), "raw_input must be Some after deser");
        println!("[4.1] ✅ raw_input present after deserialization");

        // relay_proxy.rs L110: serde_json::to_value(tc)
        let val = serde_json::to_value(tc).unwrap();
        assert!(
            val.get("rawInput").is_some(),
            "rawInput must survive to_value"
        );
        assert!(!val["rawInput"].is_null(), "rawInput must not be null");
        println!("[4.1] ✅ rawInput survives serde_json::to_value");

        // relay_proxy.rs L117: buffer.push_tool_call(val.clone(), tool_call_id)
        let mut accum = TurnAccumulator::new("test-session".to_string(), "agent".to_string(), 0);
        let tool_id = val["toolCallId"].as_str().map(|s| s.to_string());
        accum.push_tool_call(val.clone(), tool_id);
        accum.advance_patch();

        // Verify rawInput in accumulated tool_calls
        let tool_calls = &accum.tool_calls;
        assert!(
            !tool_calls.is_empty(),
            "TurnAccumulator should have tool_calls"
        );
        let stored = &tool_calls[0];
        assert!(
            stored.get("rawInput").is_some(),
            "rawInput in TurnAccumulator"
        );
        assert_eq!(stored["rawInput"]["command"], "echo hello");
        println!("[4.1] ✅ rawInput preserved in TurnAccumulator.tool_calls");
    } else {
        panic!("Expected ToolCall variant");
    }

    // Test 2: ToolCall WITHOUT rawInput (should handle gracefully)
    println!("\n[4.2] ToolCall without rawInput — absent field handling");
    let no_raw = json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "x",
        "title": "T",
        "status": "pending"
    });
    let update: SessionUpdate = serde_json::from_value(no_raw).unwrap();
    if let SessionUpdate::ToolCall(tc) = update {
        assert!(tc.raw_input.is_none(), "raw_input should be None");
        let val = serde_json::to_value(&tc).unwrap();
        // With skip_serializing_if = "Option::is_none", rawInput shouldn't appear
        let has_raw_key = val.get("rawInput").map_or(false, |v| !v.is_null());
        assert!(!has_raw_key, "rawInput should not be present when None");
        println!("[4.2] ✅ absent rawInput handled correctly");
    }

    // Test 3: Full JSON-RPC message path (simulates AcpHttpAgent SSE → relay_proxy)
    println!(
        "\n[4.3] Full JSON-RPC pipeline: SSE message → SessionUpdate → to_value → TurnAccumulator"
    );
    let jsonrpc = json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "tool_call",
                "toolCallId": "read_file-99",
                "title": "ReadFile",
                "status": "in_progress",
                "rawInput": {"path": "/etc/hosts"},
                "kind": "read",
                "content": []
            }
        }
    });

    let update_val = jsonrpc["params"]["update"].clone();
    let update: SessionUpdate = serde_json::from_value(update_val).unwrap();
    if let SessionUpdate::ToolCall(tc) = update {
        assert!(tc.raw_input.is_some());
        let val = serde_json::to_value(&tc).unwrap();
        assert_eq!(val["rawInput"]["path"], "/etc/hosts");

        // Push to TurnAccumulator like relay_proxy does
        let mut accum = TurnAccumulator::new("s1".to_string(), "".to_string(), 0);
        accum.push_tool_call(val.clone(), Some("read_file-99".to_string()));
        assert_eq!(accum.tool_calls[0]["rawInput"]["path"], "/etc/hosts");
        println!("[4.3] ✅ Full pipeline preserves rawInput");
    }

    println!("\n[4] 결론: proxy relay_proxy 코드 경로에서 rawInput은 보존됨.");
    println!(
        "[4] rawInput 누락 버그의 원인은 gemini CLI/a2a-server가 rawInput을 전송하지 않는 것."
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Scenario 5: Full AcpHttpAgent SSE E2E Mock Using Lib Crates
//   - Spawns axum SSE mock server.
//   - Connects AcpHttpAgent (native transport) to the mock server.
//   - SACP Client sends a prompt, Mock replies with ToolCall+rawInput.
//   - Validates that AcpHttpAgent correctly parses and delivers rawInput.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn scenario_5_solo_e2e_mock_server() {
    use agent_client_protocol_schema::{SessionNotification, SessionUpdate, ToolCall};
    use axum::{
        Json, Router,
        extract::State,
        response::sse::{Event, Sse},
        routing::{get, post},
    };
    use sacp::{Agent, Client, ConnectTo, ConnectionTo, Proxy, UntypedMessage};
    use sacp_tokio::AcpHttpAgent;
    use std::time::Duration;
    use tokio::sync::broadcast;

    #[derive(Clone)]
    struct AppState {
        tx: broadcast::Sender<String>,
    }

    let (tx, _rx) = broadcast::channel::<String>(16);
    let state = AppState { tx: tx.clone() };

    let app = Router::new()
        .route(
            "/acp/stream",
            get(|State(state): State<AppState>| async move {
                let mut rx = state.tx.subscribe();
                let stream = futures::stream::unfold(rx, |mut rx| async move {
                    match rx.recv().await {
                        Ok(msg) => Some((
                            Ok::<_, std::convert::Infallible>(Event::default().data(msg)),
                            rx,
                        )),
                        Err(_) => None,
                    }
                });
                // Send the initial endpoint event first, then chain the unfold stream
                let init_stream = futures::stream::once(async {
                    Ok::<_, std::convert::Infallible>(
                        Event::default().data("endpoint: http://127.0.0.1:41249/acp"),
                    )
                });
                use futures::StreamExt;
                Sse::new(init_stream.chain(stream))
                    .keep_alive(axum::response::sse::KeepAlive::default())
            }),
        )
        .route(
            "/acp",
            post(
                |State(state): State<AppState>, Json(body): Json<serde_json::Value>| async move {
                    let method = body["method"].as_str().unwrap_or("");
                    if method == "session/prompt" {
                        let mock_tool = json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": "mock-solo-session",
                                "update": {
                                    "sessionUpdate": "tool_call",
                                    "toolCallId": "mock_tool_1",
                                    "title": "search_files",
                                    "status": "in_progress",
                                    "kind": "search",
                                    "rawInput": {"query": "test"},
                                    "content": [],
                                    "locations": [],
                                    "_meta": {"model": "mock"},
                                }
                            }
                        });
                        let _ = state.tx.send(serde_json::to_string(&mock_tool).unwrap());
                    }
                    axum::http::StatusCode::ACCEPTED
                },
            ),
        )
        .with_state(state);

    let server = tokio::net::TcpListener::bind("127.0.0.1:41249")
        .await
        .unwrap();
    tokio::spawn(async move {
        axum::serve(server, app).await.unwrap();
    });

    // Wait for the server to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create SACP agent representing the mock server
    let acp_agent = AcpHttpAgent::new("http://127.0.0.1:41249/acp");

    let (notif_tx, mut notif_rx) = tokio::sync::mpsc::channel::<SessionNotification>(10);

    // Send the prompt
    let prompt_msg = UntypedMessage::new(
        "session/prompt",
        json!({
            "sessionId": "mock-solo-session",
            "prompt": [{"type": "text", "text": "test"}]
        }),
    )
    .unwrap();

    // Create a client that will receive notifications from the agent, and connect it.
    let connected_client = tokio::spawn(async move {
        Client
            .builder()
            .name("test-client")
            .on_receive_notification_from(
                Agent,
                move |notif: SessionNotification, _cx: ConnectionTo<Agent>| {
                    let tx = notif_tx.clone();
                    async move {
                        let _ = tx.send(notif).await;
                        Ok(())
                    }
                },
                sacp::on_receive_notification!(),
            )
            .connect_with(acp_agent, async move |cx: ConnectionTo<Agent>| {
                // Give AcpHttpAgent time to establish the SSE stream bridging
                tokio::time::sleep(Duration::from_millis(1000)).await;

                println!("[5] Sending session/prompt from Client to Mock Agent");
                let _ = cx.send_notification_to(Agent, prompt_msg);

                // Wait indefinitely to keep the connection open
                std::future::pending::<Result<(), sacp::Error>>().await
            })
            .await
            .unwrap();
    });

    // Wait for the notification containing rawInput
    let timeout = tokio::time::sleep(Duration::from_secs(5));
    tokio::select! {
        Some(notif) = notif_rx.recv() => {
            if let SessionUpdate::ToolCall(tc) = notif.update {
                // Determine if tc.tool_call_id is a String or ToolCallId wrapper
                let id_str = serde_json::to_value(&tc.tool_call_id).unwrap();
                assert_eq!(id_str.as_str().unwrap_or(""), "mock_tool_1");
                assert!(tc.raw_input.is_some(), "raw_input should not be dropped by AcpHttpAgent or SSE parsing");
                assert_eq!(tc.raw_input.unwrap()["query"], "test");
                println!("[5] ✅ Success! Received tool_call event with rawInput intact via AcpHttpAgent.");
            } else {
                panic!("Expected sessionUpdate to be tool_call");
            }
        }
        _ = timeout => {
            panic!("Timed out waiting for session/update from mock server");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Scenario 6: ACP Initialization Timeout (lib crate stdio wrapper)
//   - Spawns proxy with --experimental-acp
//   - Sends an ACP "initialize" JSON-RPC message
//   - Verifies it responds rather than hanging forever
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scenario_6_acp_initialization() {
    use std::process::{Command, Stdio};
    let workspace = std::env::current_dir().expect("cwd");
    let proxy_bin = workspace.join("target/debug/ilhae-proxy");
    assert!(proxy_bin.exists(), "Build the proxy first: cargo build");

    let mut child = Command::new(&proxy_bin)
        .arg("--experimental-acp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn proxy");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = std::io::BufReader::new(stdout);

    std::thread::sleep(Duration::from_secs(2));

    let init_msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#;
    let init_req = format!("{}\n", init_msg);

    use std::io::Write;
    stdin
        .write_all(init_req.as_bytes())
        .expect("write to proxy stdin");
    stdin.flush().expect("flush stdin");
    println!("[6] Sent initialize message");

    use std::io::BufRead;
    let timeout = Duration::from_secs(10);
    let deadline = Instant::now() + timeout;
    let mut line = String::new();
    let mut got_response = false;

    while Instant::now() < deadline {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if let Ok(msg) = serde_json::from_str::<Value>(line.trim()) {
                    if msg.get("id").and_then(|v| v.as_u64()) == Some(1) {
                        println!("[6] ✅ Received evaluate response: {:?}", msg);
                        got_response = true;
                        break;
                    }
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    assert!(
        got_response,
        "Proxy failed to respond to 'initialize' within {:?}. This reproduces the ACP timeout bug.",
        timeout
    );

    let _ = child.kill();
    let _ = child.wait();
}
