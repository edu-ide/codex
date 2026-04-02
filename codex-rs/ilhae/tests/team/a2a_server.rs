use reqwest::Client;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[tokio::test]
async fn test_a2a_server_startup_ignores_target_dir() {
    let port = 4444;
    let endpoint = format!("http://localhost:{}", port);

    // We run the test from the workspace root by grabbing the current directory
    let workspace_root = std::env::current_dir().expect("Failed to get current dir");

    // Spawn the a2a-server just like spawn_team_a2a_servers does
    let mut cmd = Command::new("node");

    // We assume this runs from within ilhae-proxy directory
    let a2a_server_js = workspace_root
        .parent()
        .unwrap()
        .join("gemini-cli/packages/a2a-server/dist/src/http/server.js");

    cmd.arg(a2a_server_js);
    cmd.env("CODER_AGENT_PORT", port.to_string());
    cmd.env(
        "CODER_AGENT_WORKSPACE_PATH",
        workspace_root.parent().unwrap().to_string_lossy().as_ref(),
    );
    cmd.env("GEMINI_FOLDER_TRUST", "true");
    cmd.env("USE_CCPA", "1"); // Use dummy auth approach to bypass API key requirement for local test

    // Suppress output
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    let mut child = cmd.spawn().expect("Failed to spawn a2a-server");

    // Poll for health
    let url = format!("{}/.well-known/agent.json", endpoint);
    let client = Client::new();
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(10); // Should be very fast if ignoring properly
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

    // Clean up
    child.kill().await.expect("Failed to kill a2a-server");

    assert!(
        is_healthy,
        "a2a-server failed to become healthy within {} seconds. It might be hanging on file indexing.",
        timeout.as_secs()
    );
}
