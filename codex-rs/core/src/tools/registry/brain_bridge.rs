pub(super) async fn sync_with_brain_cli() -> Result<(), String> {
    tracing::info!("Synchronizing tool registry with Brain MCP bridge...");
    match tokio::process::Command::new("brain")
        .arg("sync")
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                tracing::info!("Successfully synchronized tools with Brain.");
                Ok(())
            } else {
                let err = String::from_utf8_lossy(&output.stderr).to_string();
                tracing::warn!("Failed to synchronize with Brain: {}", err);
                Err(err)
            }
        }
        Err(e) => {
            tracing::warn!("Failed to execute brain CLI: {}", e);
            Err(e.to_string())
        }
    }
}
