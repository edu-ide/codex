//! Adapters for port traits — real I/O and mock implementations.

use crate::helpers::is_ilhae_native_engine_name;
use crate::helpers::spawn_local_a2a_server;
use crate::ports::{
    AgentSpawner, AgentTransportFactory, AgentTransportPreference, AgentTransportRequest,
    BuiltAgentTransport, SpawnedAgent, TeamSpawnEnv,
};
use codex_app_server::in_process::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
use codex_app_server_client::AppServerEvent;
use codex_app_server_protocol::ClientRequest;
use sacp::DynConnectTo;
use sacp_tokio::AcpHttpAgent;
use std::sync::{Arc, Mutex};
use tracing::info;
use tracing::warn;

// ─── Real Adapter ────────────────────────────────────────────────────────────

/// Production adapter: spawns real OS processes via `spawn_local_a2a_server`.
pub struct RealAgentSpawner;

#[async_trait::async_trait]
impl AgentSpawner for RealAgentSpawner {
    async fn spawn_agent(
        &self,
        port: u16,
        engine: &str,
        card_name: Option<&str>,
        team_env: Option<TeamSpawnEnv>,
    ) -> anyhow::Result<SpawnedAgent> {
        let child = spawn_local_a2a_server(port, engine, card_name, team_env).await?;
        Ok(SpawnedAgent {
            pid: child.id(),
            port,
        })
    }

    async fn health_check(&self, port: u16) -> bool {
        crate::helpers::probe_tcp("127.0.0.1", port)
    }
}

/// Production adapter: builds ACP-backed transports for Codex/Gemini/A2A.
pub struct AcpTransportFactory {
    supervisor_handle: crate::process_supervisor::SupervisorHandle,
}

impl AcpTransportFactory {
    pub fn new(supervisor_handle: crate::process_supervisor::SupervisorHandle) -> Self {
        Self { supervisor_handle }
    }
}

#[async_trait::async_trait]
impl AgentTransportFactory for AcpTransportFactory {
    async fn build_transport(
        &self,
        request: &AgentTransportRequest,
    ) -> anyhow::Result<BuiltAgentTransport> {
        let (host, port) = crate::helpers::parse_host_port(&request.endpoint);
        let is_local = matches!(host.as_str(), "127.0.0.1" | "localhost" | "0.0.0.0" | "::1");
        let already_running = crate::helpers::probe_tcp(&host, port);

        info!(port, already_running, "A2A endpoint probe result");

        if is_ilhae_native_engine_name(&request.engine_name) {
            info!(
                "Native ACP integration mode: spawning ilhae-agent internally over tokio::io::duplex"
            );
            let (a_side, b_side) = tokio::io::duplex(1024 * 1024);
            let (read_a, write_a) = tokio::io::split(a_side);
            let (read_b, write_b) = tokio::io::split(b_side);

            let ilhae_dir = crate::config::resolve_ilhae_data_dir();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();

                let config = rt
                    .block_on(codex_core::config::Config::load_default_with_cli_overrides(
                        Vec::new(),
                    ))
                    .unwrap_or_else(|e| {
                        panic!("Failed to load ilhae config from {:?}: {:?}", ilhae_dir, e)
                    });

                tracing::info!("ilhae-agent background thread started.");
                let _ = rt.block_on(async move {
                    if let Err(e) = codex_acp::run_stream(config, read_b, write_b).await {
                        tracing::error!("ilhae-agent stream ran into error: {:?}", e);
                    }
                });
            });

            use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
            let compat_write = write_a.compat_write();
            let compat_read = read_a.compat();
            let agent = sacp::ByteStreams::new(compat_write, compat_read);
            return Ok(BuiltAgentTransport {
                transport: DynConnectTo::new(agent),
                spawned_child: None,
            });
        }

        if request.is_team && is_local {
            info!(
                endpoint = %request.endpoint,
                already_running,
                "Team mode: using A2A transport (deferred — leader will auto-connect when ready)"
            );
            return Ok(BuiltAgentTransport {
                transport: DynConnectTo::new(crate::a2a_transport::A2AAgent::new(
                    request.endpoint.clone(),
                )),
                spawned_child: None,
            });
        }

        if already_running {
            let acp_endpoint = format!("{}/acp", request.endpoint.trim_end_matches('/'));
            info!(endpoint = %acp_endpoint, "Using ACP/HTTP transport (existing server)");
            return Ok(BuiltAgentTransport {
                transport: DynConnectTo::new(AcpHttpAgent::new(acp_endpoint)),
                spawned_child: None,
            });
        }

        if is_local {
            if request.is_team {
                info!(
                    endpoint = %request.endpoint,
                    "Team mode: using A2A transport (deferred — leader spawning)"
                );
                return Ok(BuiltAgentTransport {
                    transport: DynConnectTo::new(crate::a2a_transport::A2AAgent::new(
                        request.endpoint.clone(),
                    )),
                    spawned_child: None,
                });
            }

            info!(
                port,
                "A2A endpoint unreachable, spawning local a2a-server (deferred)"
            );

            let _ = std::process::Command::new("fuser")
                .args(["-k", &format!("{}/tcp", port)])
                .output();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            let sv_handle = self.supervisor_handle.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    crate::process_supervisor::ensure_agent_healthy(&sv_handle, port).await
                {
                    warn!(
                        port,
                        "Failed to ensure agent healthy during transport build: {}", e
                    );
                } else {
                    info!(
                        port,
                        "a2a-server spawned/healthy, AcpHttpAgent will auto-connect"
                    );
                }
            });

            let acp_endpoint = format!("{}/acp", request.endpoint.trim_end_matches('/'));
            info!(
                endpoint = %acp_endpoint,
                "Using ACP/HTTP transport (deferred — a2a-server spawning in background)"
            );
            return Ok(BuiltAgentTransport {
                transport: DynConnectTo::new(AcpHttpAgent::new(acp_endpoint)),
                spawned_child: None,
            });
        }

        Err(anyhow::anyhow!(
            "Remote A2A endpoint {} is unreachable. Cannot start agent.",
            request.endpoint
        ))
    }
}

/// Placeholder adapter boundary for future native Codex app-server transport.
///
/// The current conductor still requires `sacp::Client`, so this adapter cannot
/// yet return a live transport without an app-server -> ACP bridge.
pub struct CodexAppServerTransportFactory;

impl CodexAppServerTransportFactory {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl AgentTransportFactory for CodexAppServerTransportFactory {
    async fn build_transport(
        &self,
        request: &AgentTransportRequest,
    ) -> anyhow::Result<BuiltAgentTransport> {
        let _channel_capacity = DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
        let _event_type = std::any::type_name::<AppServerEvent>();
        let _request_type = std::any::type_name::<ClientRequest>();

        Err(anyhow::anyhow!(
            "Native app-server transport is selected for engine '{}' at '{}' but the ilhae conductor still requires sacp::Client. Implement an app-server -> ACP bridge before enabling this path.",
            request.engine_name,
            request.endpoint
        ))
    }
}

pub async fn build_transport_with_preference(
    acp_factory: &AcpTransportFactory,
    app_server_factory: &CodexAppServerTransportFactory,
    request: &AgentTransportRequest,
) -> anyhow::Result<BuiltAgentTransport> {
    if matches!(
        request.preference,
        AgentTransportPreference::AppServer | AgentTransportPreference::Auto
    ) && is_ilhae_native_engine_name(&request.engine_name)
    {
        match app_server_factory.build_transport(request).await {
            Ok(built) => return Ok(built),
            Err(err) if request.preference == AgentTransportPreference::AppServer => {
                return Err(err);
            }
            Err(err) => {
                warn!(
                    engine = %request.engine_name,
                    endpoint = %request.endpoint,
                    "Native app-server transport unavailable, falling back to ACP: {}",
                    err
                );
            }
        }
    }

    acp_factory.build_transport(request).await
}

// ─── Mock Adapter ────────────────────────────────────────────────────────────

/// Record of a spawn_agent call for test assertions.
#[derive(Debug, Clone)]
pub struct SpawnCall {
    pub port: u16,
    pub engine: String,
    pub card_name: Option<String>,
}

/// Test adapter: records calls and returns configurable results.
pub struct MockAgentSpawner {
    calls: Arc<Mutex<Vec<SpawnCall>>>,
    next_result: Arc<Mutex<Result<SpawnedAgent, String>>>,
    healthy_ports: Arc<Mutex<Vec<u16>>>,
}

impl MockAgentSpawner {
    /// Create a mock that always returns the given result.
    pub fn new(result: Result<SpawnedAgent, String>) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            next_result: Arc::new(Mutex::new(result)),
            healthy_ports: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a mock that always succeeds with a fake PID.
    pub fn always_ok() -> Self {
        Self::new(Ok(SpawnedAgent {
            pid: Some(12345),
            port: 0, // will be overridden per call
        }))
    }

    /// Get the recorded spawn calls.
    pub fn calls(&self) -> Vec<SpawnCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Mark a port as healthy for health_check.
    pub fn mark_healthy(&self, port: u16) {
        self.healthy_ports.lock().unwrap().push(port);
    }
}

#[async_trait::async_trait]
impl AgentSpawner for MockAgentSpawner {
    async fn spawn_agent(
        &self,
        port: u16,
        engine: &str,
        card_name: Option<&str>,
        _team_env: Option<TeamSpawnEnv>,
    ) -> anyhow::Result<SpawnedAgent> {
        self.calls.lock().unwrap().push(SpawnCall {
            port,
            engine: engine.to_string(),
            card_name: card_name.map(|s| s.to_string()),
        });
        let result = self.next_result.lock().unwrap().clone();
        match result {
            Ok(mut agent) => {
                agent.port = port;
                Ok(agent)
            }
            Err(msg) => Err(anyhow::anyhow!(msg)),
        }
    }

    async fn health_check(&self, port: u16) -> bool {
        self.healthy_ports.lock().unwrap().contains(&port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_spawner_records_calls() {
        let mock = MockAgentSpawner::always_ok();
        let result = mock
            .spawn_agent(4321, "gemini", Some("Test Agent"), None)
            .await;
        assert!(result.is_ok());
        let agent = result.unwrap();
        assert_eq!(agent.port, 4321);
        assert_eq!(agent.pid, Some(12345));

        let calls = mock.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].port, 4321);
        assert_eq!(calls[0].engine, "gemini");
    }

    #[tokio::test]
    async fn mock_spawner_failure() {
        let mock = MockAgentSpawner::new(Err("spawn failed".to_string()));
        let result = mock.spawn_agent(4322, "codex", None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("spawn failed"));
    }

    #[tokio::test]
    async fn mock_health_check() {
        let mock = MockAgentSpawner::always_ok();
        assert!(!mock.health_check(4321).await);
        mock.mark_healthy(4321);
        assert!(mock.health_check(4321).await);
    }
}
