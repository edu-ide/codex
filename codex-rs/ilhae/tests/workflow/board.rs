use ilhae_proxy::types::*;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn test_workflow_board_integration() {
    let mut child = Command::new("cargo")
        .args(&["run", "--bin", "ilhae-proxy"])
        .env("RUST_LOG", "warn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start proxy");

    // allow startup
    std::thread::sleep(Duration::from_secs(3));

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // Setup dummy artifact
    let vault_dir = dirs::home_dir().unwrap().join("ilhae/vault/workflow");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let dummy_file = vault_dir.join("DESIGN_rs_headless_test.md");
    std::fs::write(&dummy_file, "---\nproject_path: \"/tmp/headless\"\ndate: \"2099-01-01T00:00:00Z\"\ntype: \"DESIGN\"\n---\n# RS Test\nContent data.").unwrap();

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
    assert!(res["result"].is_object(), "Initialize failed: {:?}", res);

    // 2. session/new
    let res = send_request(
        json!({
            "jsonrpc": "2.0",
            "method": "session/new",
            "params": { "cwd": "/tmp", "mcpServers": [] },
            "id": 2
        }),
        2,
    );
    assert!(
        res["result"]["sessionId"].is_string(),
        "Session/new failed: {:?}",
        res
    );

    // 3. list_workflow_artifacts
    let res = send_request(
        json!({
            "jsonrpc": "2.0",
            "method": "ilhae/list_workflow_artifacts",
            "params": {},
            "id": 3
        }),
        3,
    );
    let artifacts = res["result"]["artifacts"]
        .as_array()
        .expect(&format!("List should return array: {:?}", res));
    assert!(
        artifacts
            .iter()
            .any(|a| a["id"] == "DESIGN_rs_headless_test.md"),
        "Document not found in list: {:?}",
        res
    );

    // 4. read_workflow_artifact
    let res = send_request(
        json!({
            "jsonrpc": "2.0",
            "method": "ilhae/read_workflow_artifact",
            "params": { "id": "DESIGN_rs_headless_test.md" },
            "id": 4
        }),
        4,
    );
    let content = res["result"]["content"]
        .as_str()
        .expect("Read should return content");
    assert!(
        content.contains("Content data."),
        "Content did not match dummy content"
    );

    println!("All tests passed!");
    child.kill().unwrap();
    std::fs::remove_file(dummy_file).unwrap();
}
