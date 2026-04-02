use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn test_workflow_pipeline_integration() {
    let home_dir = dirs::home_dir().unwrap();
    let vault_dir = home_dir.join("ilhae/vault/workflow");
    let _ = std::fs::create_dir_all(&vault_dir);

    // Create a mock script to simulate the A2A_AGENT_COMMAND (gemini)
    let mock_script_path = std::env::temp_dir().join("mock_gemini_agent.sh");
    std::fs::write(
        &mock_script_path,
        r#"#!/usr/bin/env bash
    echo "Mock AI Output: Success"
    exit 0
    "#,
    )
    .unwrap();

    // ensure executable
    Command::new("chmod")
        .arg("+x")
        .arg(&mock_script_path)
        .status()
        .unwrap();

    let mut child = Command::new("cargo")
        .args(&["run", "--bin", "ilhae-proxy"])
        .env("RUST_LOG", "warn")
        .env("A2A_AGENT_COMMAND", &mock_script_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start proxy");

    // allow startup
    std::thread::sleep(Duration::from_secs(3));

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    let mut send_request = |req: serde_json::Value, expected_id: u64| {
        let mut msg = serde_json::to_string(&req).unwrap();
        msg.push('\n');
        stdin.write_all(msg.as_bytes()).unwrap();
        stdin.flush().unwrap();

        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 {
                panic!("EOF reached before getting response for id {}", expected_id);
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                if val.get("id").and_then(|i| i.as_u64()) == Some(expected_id) {
                    return val;
                }
            }
        }
    };

    // 1. initialize
    let res = send_request(
        json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            },
            "id": 1
        }),
        1,
    );
    assert!(res["result"].is_object(), "Initialize failed");

    // Clear workflow directory before tests
    for entry in std::fs::read_dir(&vault_dir).unwrap() {
        if let Ok(e) = entry {
            let _ = std::fs::remove_file(e.path());
        }
    }

    // 2. Call brainstorm_design
    let res = send_request(
        json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": "brainstorm_design",
                "arguments": {
                    "objective": "Design a new login system",
                    "context": "React frontend, Rust backend"
                }
            },
            "id": 2
        }),
        2,
    );

    // Assert response is ok
    assert!(
        res["result"].is_object(),
        "tools/call brainstorm_design failed: {:?}",
        res
    );
    let tool_res = res["result"].as_object().unwrap();
    assert_eq!(
        tool_res.get("isError").and_then(|b| b.as_bool()),
        Some(false)
    );

    // Check DESIGN file exists
    let files: Vec<PathBuf> = std::fs::read_dir(&vault_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("DESIGN_")
        })
        .collect();
    assert_eq!(
        files.len(),
        1,
        "Expected exactly one DESIGN_ file to be created"
    );

    // 3. Call create_execution_plan
    let res = send_request(
        json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": "create_execution_plan",
                "arguments": {
                    "design_document": "Mock design",
                    "constraints": "Keep it simple"
                }
            },
            "id": 3
        }),
        3,
    );

    assert!(
        res["result"].is_object(),
        "tools/call create_execution_plan failed: {:?}",
        res
    );
    let tool_res = res["result"].as_object().unwrap();
    assert_eq!(
        tool_res.get("isError").and_then(|b| b.as_bool()),
        Some(false)
    );

    // Check PLAN file exists
    let files: Vec<PathBuf> = std::fs::read_dir(&vault_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("PLAN_")
        })
        .collect();
    assert_eq!(
        files.len(),
        1,
        "Expected exactly one PLAN_ file to be created"
    );

    // 4. Call verify_execution
    let res = send_request(
        json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": "verify_execution",
                "arguments": {
                    "original_requirements": "Login system",
                    "execution_plan": "Mock plan",
                    "artifacts_to_verify": "/path/to/code"
                }
            },
            "id": 4
        }),
        4,
    );

    assert!(
        res["result"].is_object(),
        "tools/call verify_execution failed: {:?}",
        res
    );
    let tool_res = res["result"].as_object().unwrap();
    assert_eq!(
        tool_res.get("isError").and_then(|b| b.as_bool()),
        Some(false)
    );

    // Check VERIFICATION file exists
    let files: Vec<PathBuf> = std::fs::read_dir(&vault_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("VERIFICATION_")
        })
        .collect();
    assert_eq!(
        files.len(),
        1,
        "Expected exactly one VERIFICATION_ file to be created"
    );

    println!("All pipeline tests passed!");
    child.kill().unwrap();

    // Clean up
    let _ = std::fs::remove_file(&mock_script_path);
}
