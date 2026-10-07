//! Start the configured team directly, without starting the desktop proxy.
use codex_ilhae::context_proxy::generate_peer_registration_files;
use codex_ilhae::context_proxy::load_team_runtime_config;
use codex_ilhae::context_proxy::spawn_team_a2a_servers;
use codex_ilhae::context_proxy::wait_for_all_team_health;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Usage: ilhae_team_runtime [--prepare-only]\nPrepare private role configurations and start the team from ILHAE_DATA_DIR."
        );
        return Ok(());
    }
    anyhow::ensure!(
        args.is_empty() || args == ["--prepare-only"],
        "Unknown arguments; use --help"
    );
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let data_dir = codex_ilhae::config::resolve_ilhae_data_dir();
    let team = load_team_runtime_config(&data_dir)
        .ok_or_else(|| anyhow::anyhow!("No configured team in {}", data_dir.display()))?;
    let workspaces = generate_peer_registration_files(&team, None);
    anyhow::ensure!(
        workspaces.len() == team.agents.len(),
        "Some role configurations could not be prepared; refusing partial team launch"
    );
    println!("Prepared {} role workspaces", workspaces.len());
    if !args.is_empty() {
        return Ok(());
    }
    let mut children = spawn_team_a2a_servers(&team, &workspaces, None, "team-runtime").await;
    let health = wait_for_all_team_health(&team).await;
    let result = match health {
        Ok(()) => {
            println!("Team ready: {} agents", team.agents.len());
            wait_for_shutdown().await
        }
        Err(error) => Err(anyhow::anyhow!(error)),
    };
    // Only stop child processes launched by this invocation.
    for child in &mut children {
        let _ = child.start_kill();
    }
    for child in &mut children {
        let _ = child.wait().await;
    }
    result
}

async fn wait_for_shutdown() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}
