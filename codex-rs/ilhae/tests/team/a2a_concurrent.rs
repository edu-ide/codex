use super::common::test_gate::require_team_local_a2a_spawn;
use reqwest::Client;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[tokio::test]
async fn test_4_concurrent_a2a_servers() {
    if !require_team_local_a2a_spawn() {
        return;
    }

    let workspace_root = std::env::current_dir().expect("Failed to get current dir");

    // Simulate the true git root
    let project_root = workspace_root.parent().unwrap();
    let a2a_server_js = project_root.join("gemini-cli/packages/a2a-server/dist/src/http/server.js");

    let mut children = vec![];
    let ports = [4450, 4451, 4452, 4453];

    for port in ports {
        let mut cmd = Command::new("node");
        cmd.arg(&a2a_server_js);
        cmd.env("CODER_AGENT_PORT", port.to_string());
        cmd.env(
            "CODER_AGENT_WORKSPACE_PATH",
            project_root.to_string_lossy().as_ref(),
        );
        cmd.env("GEMINI_FOLDER_TRUST", "true");
        cmd.env("USE_CCPA", "1");

        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());

        children.push(cmd.spawn().expect("Failed to spawn a2a-server"));
    }

    let client = Client::new();
    let timeout = Duration::from_secs(35); // longer than 30s proxy timeout
    let start = std::time::Instant::now();
    let mut all_healthy = false;

    loop {
        let mut healthy_count = 0;
        for port in ports {
            let url = format!("http://localhost:{}/.well-known/agent.json", port);
            if let Ok(resp) = client
                .get(&url)
                .timeout(Duration::from_secs(1))
                .send()
                .await
            {
                if resp.status().is_success() {
                    healthy_count += 1;
                }
            }
        }

        if healthy_count == ports.len() {
            all_healthy = true;
            break;
        }

        if start.elapsed() >= timeout {
            break;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    for mut child in children {
        child.kill().await.expect("Failed to kill a2a-server");
    }

    assert!(
        all_healthy,
        "Not all 4 concurrent A2A servers became healthy within 35 seconds. This replicates the hang!"
    );
}
