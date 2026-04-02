//! Startup helpers — extracted from `main.rs`.
//!
//! Contains initialization functions used during proxy startup:
//! - `cleanup_redundant_sessions`: remove duplicate "Untitled" sessions
//! - `resolve_team_main_target`: resolve main agent engine/endpoint from team config
//! - `build_agent_transport`: resolve and build the agent transport (ACP/HTTP/A2A)

use std::sync::Arc;

use sacp::DynConnectTo;
use sacp_tokio::AcpHttpAgent;
use tracing::info;

use crate::adapters::{
    AcpTransportFactory, CodexAppServerTransportFactory, build_transport_with_preference,
};
use crate::context_proxy;
use crate::helpers::{
    ILHAE_NATIVE_TRANSPORT_ENV, LEGACY_CODEX_TRANSPORT_ENV, infer_agent_id_from_command,
    is_ilhae_native_command, is_ilhae_native_engine_name,
};
use crate::ports::{AgentTransportPreference, AgentTransportRequest};
use crate::session_store::SessionStore;
use crate::settings_store::SettingsStore;

/// Remove duplicate "Untitled" empty sessions, keeping only the first one.
pub fn cleanup_redundant_sessions(store: &Arc<SessionStore>) {
    if let Ok(sessions) = store.list_sessions() {
        let candidates: Vec<_> = sessions
            .iter()
            .filter(|s| s.title == "Untitled" && s.message_count == 0)
            .collect();
        if candidates.len() > 1 {
            info!(
                "Cleaning up {} redundant 'Untitled' sessions",
                candidates.len() - 1
            );
            for s in candidates.iter().skip(1) {
                let _ = store.delete_session(&s.id);
            }
        }
    }
}

/// Resolve team main agent's engine and endpoint from team runtime config.
pub fn resolve_team_main_target(ilhae_dir: &std::path::Path) -> Option<(String, String)> {
    context_proxy::load_team_runtime_config(&ilhae_dir.to_path_buf()).and_then(|team| {
        team.agents
            .iter()
            .find(|agent| agent.is_main)
            .or_else(|| team.agents.first())
            .map(|agent| (agent.engine.clone(), agent.endpoint.clone()))
    })
}

/// Resolve agent transport from settings.
/// If `agent.a2a_endpoint` is set, uses ACP/HTTP transport (spawning a2a-server if needed).
/// Otherwise spawns a stdio-based ACP agent process.
///
/// Returns the transport and optionally the spawned a2a-server child process.
pub async fn build_agent_transport(
    settings_store: &Arc<SettingsStore>,
    supervisor_handle: crate::process_supervisor::SupervisorHandle,
) -> anyhow::Result<(DynConnectTo<sacp::Client>, Option<tokio::process::Child>)> {
    // Mock mode: skip all health checks and return a dummy AcpHttpAgent
    // that will never be called — mock responses are injected by the proxy chain.
    if crate::mock_provider::is_mock_mode() {
        info!("[MockAgent] Mock mode — skipping agent transport build");
        let dummy = AcpHttpAgent::new("http://127.0.0.1:1/acp".to_string());
        return Ok((DynConnectTo::new(dummy), None));
    }

    let settings_snapshot = settings_store.get();

    let is_team = settings_snapshot.agent.team_mode;
    let engine_name = if is_team {
        let ilhae_dir = crate::config::resolve_ilhae_data_dir();
        resolve_team_main_target(&ilhae_dir)
            .map(|(engine, _)| engine)
            .unwrap_or_else(|| {
                if is_ilhae_native_command(&settings_snapshot.agent.command) {
                    infer_agent_id_from_command(&settings_snapshot.agent.command)
                } else {
                    "gemini".to_string()
                }
            })
    } else if is_ilhae_native_command(&settings_snapshot.agent.command) {
        infer_agent_id_from_command(&settings_snapshot.agent.command)
    } else {
        "gemini".to_string()
    };

    let resolved_engine = crate::engine_env::resolve_engine_env(&engine_name);

    // SSoT: Solo mode always uses engine-based ports.
    // a2a_endpoint is only used in team mode (points to leader).
    let a2a_endpoint = if is_team {
        let ep = settings_snapshot.agent.a2a_endpoint.trim();
        if ep.is_empty() {
            let ilhae_dir = crate::config::resolve_ilhae_data_dir();
            resolve_team_main_target(&ilhae_dir)
                .map(|(_, endpoint)| endpoint)
                .unwrap_or_else(|| {
                    format!("http://localhost:{}", crate::port_config::team_base_port())
                })
        } else {
            ep.to_string()
        }
    } else {
        format!("http://localhost:{}", resolved_engine.default_port())
    };
    info!(
        engine = resolved_engine.label(),
        endpoint = %a2a_endpoint,
        "Build agent transport: resolved engine endpoint"
    );

    if !a2a_endpoint.is_empty() {
        let preference = codex_transport_preference(&engine_name);
        let acp_factory = AcpTransportFactory::new(supervisor_handle.clone());
        let app_server_factory = CodexAppServerTransportFactory::new();
        let built = build_transport_with_preference(
            &acp_factory,
            &app_server_factory,
            &AgentTransportRequest {
                engine_name: engine_name.clone(),
                endpoint: a2a_endpoint.clone(),
                is_team,
                preference,
            },
        )
        .await?;
        return Ok((built.transport, built.spawned_child));
    }

    // a2a_endpoint was empty — this shouldn't happen with current settings logic,
    // but return error rather than falling back to stdio.
    Err(anyhow::anyhow!(
        "No A2A endpoint configured and no fallback available"
    ))
}

fn codex_transport_preference(engine_name: &str) -> AgentTransportPreference {
    if !is_ilhae_native_engine_name(engine_name) {
        return AgentTransportPreference::Acp;
    }

    match std::env::var(ILHAE_NATIVE_TRANSPORT_ENV)
        .or_else(|_| std::env::var(LEGACY_CODEX_TRANSPORT_ENV))
    {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "app-server" | "app_server" | "appserver" => AgentTransportPreference::AppServer,
            "acp" => AgentTransportPreference::Acp,
            _ => AgentTransportPreference::Auto,
        },
        Err(_) => AgentTransportPreference::Auto,
    }
}
