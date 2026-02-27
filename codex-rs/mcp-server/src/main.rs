use clap::Parser;
use codex_arg0::arg0_dispatch_or_else;
use codex_mcp_server::{TransportOptions, run_main_with_transport};
use codex_utils_cli::CliConfigOverrides;

/// Codex MCP server with optional HTTP transport.
#[derive(Parser)]
#[command(name = "codex-mcp-server")]
struct Cli {
    /// Start an HTTP server on this port (in addition to stdin/stdout).
    /// Example: --port 9100
    #[arg(long)]
    port: Option<u16>,

    /// Disable stdin/stdout transport; run HTTP-only mode.
    /// Requires --port to be set.
    #[arg(long)]
    http_only: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.http_only && cli.port.is_none() {
        anyhow::bail!("--http-only requires --port to be set");
    }

    let transport = TransportOptions {
        http_port: cli.port,
        http_only: cli.http_only,
    };

    arg0_dispatch_or_else(|arg0_paths| async move {
        run_main_with_transport(
            arg0_paths,
            CliConfigOverrides::default(),
            transport,
        )
        .await?;
        Ok(())
    })
}
