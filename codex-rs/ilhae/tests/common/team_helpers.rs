//! Shared helpers for real-server team tests.
//!
//! Contains `ilhae_dir()`, `cleanup_children()`, and common re-exports
//! used by delegation_real, persistence, real_agent, and acp_tool_call tests.

#![allow(dead_code)]

use std::path::PathBuf;

pub use std::time::Duration;

pub use a2a_rs::proxy::A2aProxy;
pub use ilhae_proxy::context_proxy::{
    generate_peer_registration_files, load_team_runtime_config, spawn_team_a2a_servers,
    trigger_agent_reload, wait_for_all_team_health,
};
pub use ilhae_proxy::session_store::SessionStore;

/// Returns ~/ilhae directory path.
pub fn ilhae_dir() -> PathBuf {
    dirs::home_dir().expect("HOME not set").join("ilhae")
}

/// Cleanup spawned child processes + kill known team ports.
pub async fn cleanup_children(children: &mut Vec<tokio::process::Child>) {
    for child in children.iter_mut() {
        let _ = child.kill().await;
    }
    // Also kill known team ports
    for port in [4321, 4322, 4323, 4324] {
        let _ = std::process::Command::new("fuser")
            .args(["-k", &format!("{}/tcp", port)])
            .output();
    }
}
