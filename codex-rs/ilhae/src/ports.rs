//! Port traits for hexagonal architecture.
//!
//! These traits define the boundaries between business logic and I/O.
//! Production code uses real adapters; tests inject mocks.

use std::path::PathBuf;

use sacp::DynConnectTo;

/// Environment for spawning team agents (workspace isolation).
#[derive(Debug, Clone)]
pub struct TeamSpawnEnv {
    pub workspace_path: PathBuf,
    pub role: String,
}

/// Result of spawning an agent process.
#[derive(Debug, Clone)]
pub struct SpawnedAgent {
    pub pid: Option<u32>,
    pub port: u16,
}

/// Request for building an interactive agent transport.
#[derive(Debug, Clone)]
pub struct AgentTransportRequest {
    pub engine_name: String,
    pub endpoint: String,
    pub is_team: bool,
    pub preference: AgentTransportPreference,
}

/// Built interactive agent transport plus any owned child process.
pub struct BuiltAgentTransport {
    pub transport: DynConnectTo<sacp::Client>,
    pub spawned_child: Option<tokio::process::Child>,
}

/// Preferred transport family for the upstream agent connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTransportPreference {
    Auto,
    Acp,
    AppServer,
}

/// Port: agent process lifecycle management.
///
/// Abstracts how agent processes (gemini-cli, codex-acp) are spawned,
/// enabling unit tests to verify supervisor logic without real processes.
#[async_trait::async_trait]
pub trait AgentSpawner: Send + Sync {
    /// Spawn a new agent process on the given port.
    async fn spawn_agent(
        &self,
        port: u16,
        engine: &str,
        card_name: Option<&str>,
        team_env: Option<TeamSpawnEnv>,
    ) -> anyhow::Result<SpawnedAgent>;

    /// Check if an agent is healthy (responds to HTTP).
    async fn health_check(&self, port: u16) -> bool;
}

/// Port: interactive agent transport construction.
///
/// This abstracts the transport family used by the orchestration layer.
/// Today ACP-backed transports implement it; next phase adds Codex app-server
/// without changing startup policy or context orchestration code.
#[async_trait::async_trait]
pub trait AgentTransportFactory: Send + Sync {
    async fn build_transport(
        &self,
        request: &AgentTransportRequest,
    ) -> anyhow::Result<BuiltAgentTransport>;
}
