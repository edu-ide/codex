use reqwest::Client;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[tokio::test]
#[ignore]
async fn test_a2a_server_startup_hangs_on_target_dir() {
    let port = 4445;
    let endpoint = format!("http://localhost:{}", port);

    let workspace_root = std::env::current_dir().expect("Failed to get current dir");

    // Simulate the BUG: passing desktop/src-tauri as the workspace root.
    // Because it lacks a .git folder directly inside it, gemini-cli-core ignores .gitignore
    // and scans the massive target/ directory.
    let buggy_workspace = workspace_root.parent().unwrap().join("desktop/src-tauri");

    let a2a_server_js = workspace_root
        .parent()
        .unwrap()
        .join("gemini-cli/packages/a2a-server/dist/src/http/server.js");

    let mut cmd = Command::new("node");
    cmd.arg(a2a_server_js);
    cmd.env("CODER_AGENT_PORT", port.to_string());
    cmd.env(
        "CODER_AGENT_WORKSPACE_PATH",
        buggy_workspace.to_string_lossy().as_ref(),
    );
    cmd.env("GEMINI_FOLDER_TRUST", "true");
    cmd.env("USE_CCPA", "1");

    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    let mut child = cmd.spawn().expect("Failed to spawn a2a-server");

    let url = format!("{}/.well-known/agent.json", endpoint);
    let client = Client::new();
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(10); // It will fail this timeout
    let mut is_healthy = false;

    loop {
        if let Ok(resp) = client
            .get(&url)
            .timeout(Duration::from_secs(1))
            .send()
            .await
        {
            if resp.status().is_success() {
                is_healthy = true;
                break;
            }
        }

        if start.elapsed() >= timeout {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    child.kill().await.expect("Failed to kill a2a-server");

    // We ASSERT that it is NOT healthy within 10 seconds because it is hanging on the target/ directory
    assert!(
        !is_healthy,
        "a2a-server became healthy too quickly! The bug might not be reproducing."
    );
}
