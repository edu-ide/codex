//! Codex ACP - An Agent Client Protocol implementation for Codex.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use agent_client_protocol::AgentSideConnection;
use codex_core::config::{Config, ConfigOverrides};
use codex_utils_cli::CliConfigOverrides;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::{io::Result as IoResult, rc::Rc};
use tokio::task::LocalSet;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing_subscriber::EnvFilter;

mod codex_agent;
mod thread;

pub static ACP_CLIENT: OnceLock<Arc<AgentSideConnection>> = OnceLock::new();

use tokio::io::{AsyncRead, AsyncWrite};

/// Run the Codex ACP agent using arbitrary async streams.
/// This allows embedding the agent into other processes (like ilhae-proxy).
pub async fn run_stream<R, W>(config: Config, reader: R, writer: W) -> IoResult<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    // Create our Agent implementation
    let agent = Rc::new(codex_agent::CodexAgent::new(config));

    let compat_reader = reader.compat();
    let compat_writer = writer.compat_write();

    // Run the I/O task to handle the actual communication
    LocalSet::new()
        .run_until(async move {
            // Create the ACP connection
            let (client, io_task) =
                AgentSideConnection::new(agent.clone(), compat_writer, compat_reader, |fut| {
                    tokio::task::spawn_local(fut);
                });

            if ACP_CLIENT.set(Arc::new(client)).is_err() {
                // Ignore if it was already set (e.g. multi-tenant execution)
            }

            io_task
                .await
                .map_err(|e| std::io::Error::other(format!("ACP I/O error: {e}")))
        })
        .await?;

    Ok(())
}

/// Run the Codex ACP agent over stdio (CLI binary usage).
///
/// This sets up an ACP agent that communicates over stdio, bridging
/// the ACP protocol with the existing codex-rs infrastructure.
///
/// # Errors
///
/// If unable to parse the config or start the program.
pub async fn run_main(
    codex_linux_sandbox_exe: Option<PathBuf>,
    cli_config_overrides: CliConfigOverrides,
) -> IoResult<()> {
    // Install a simple subscriber so `tracing` output is visible.
    // Users can control the log level with `RUST_LOG`.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Parse CLI overrides and load configuration
    let cli_kv_overrides = cli_config_overrides.parse_overrides().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("error parsing -c overrides: {e}"),
        )
    })?;

    let config_overrides = ConfigOverrides {
        codex_linux_sandbox_exe,
        ..ConfigOverrides::default()
    };

    let config =
        Config::load_with_cli_overrides_and_harness_overrides(cli_kv_overrides, config_overrides)
            .await
            .map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("error loading config: {e}"),
                )
            })?;

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    run_stream(config, stdin, stdout).await
}

// Re-export the MCP server types for compatibility
pub use codex_mcp_server::{
    CodexToolCallParam, CodexToolCallReplyParam, ExecApprovalElicitRequestParams,
    ExecApprovalResponse, PatchApprovalElicitRequestParams, PatchApprovalResponse,
};
