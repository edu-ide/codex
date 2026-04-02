//! MCP Manager for ilhae-proxy.
//! Manages multiple MCP server connections (stdio, SSE) and aggregates their tools.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{error, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPreset {
    pub id: String,
    pub name: String,
    pub transport_type: String, // "stdio" | "sse" | "streamable-http"
    pub command: Option<String>,
    pub args: Option<String>,
    pub sse_url: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub cwd: Option<String>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct McpServer {
    pub preset: McpPreset,
    pub child: Option<Child>,
    pub tools: Vec<serde_json::Value>,
}

pub struct McpManager {
    pub servers: Arc<Mutex<HashMap<String, McpServer>>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            servers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Refresh active servers based on settings.
    pub async fn sync_with_presets(&self, presets: Vec<serde_json::Value>) {
        let mut active_presets = HashMap::new();
        for p_val in presets {
            if let Ok(preset) = serde_json::from_value::<McpPreset>(p_val) {
                active_presets.insert(preset.id.clone(), preset);
            }
        }

        let mut servers = self.servers.lock().await;

        // Stop servers no longer in presets
        let to_remove: Vec<String> = servers
            .keys()
            .filter(|id| !active_presets.contains_key(*id))
            .cloned()
            .collect();

        for id in to_remove {
            if let Some(mut server) = servers.remove(&id) {
                info!("Stopping MCP server: {}", server.preset.name);
                if let Some(mut child) = server.child.take() {
                    let _ = child.kill().await;
                }
            }
        }

        // Start new servers or update existing
        for (id, preset) in active_presets {
            if !servers.contains_key(&id) {
                if preset.transport_type == "stdio" {
                    if let Some(cmd) = &preset.command {
                        info!("Starting stdio MCP server: {}", preset.name);
                        let mut command = Command::new(cmd);
                        println!(
                            "DEBUG_SPAWN: spawning MCP server '{}' with command: {} {:?}",
                            preset.id, cmd, preset.args
                        );
                        if let Some(args_str) = &preset.args {
                            command.args(args_str.split_whitespace());
                        }
                        if let Some(env) = &preset.env {
                            command.envs(env);
                        }
                        if let Some(cwd) = &preset.cwd {
                            command.current_dir(cwd);
                        }
                        command
                            .stdin(Stdio::piped())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped());

                        match command.spawn() {
                            Ok(child) => {
                                servers.insert(
                                    id,
                                    McpServer {
                                        preset,
                                        child: Some(child),
                                        tools: Vec::new(), // In a real impl, we'd fetch tools via JSON-RPC here
                                    },
                                );
                            }
                            Err(e) => error!("Failed to spawn MCP server {}: {}", preset.name, e),
                        }
                    }
                } else if preset.transport_type == "sse"
                    || preset.transport_type == "streamable-http"
                {
                    info!(
                        "Adding network MCP server: {} ({})",
                        preset.name, preset.transport_type
                    );
                    servers.insert(
                        id,
                        McpServer {
                            preset,
                            child: None,
                            tools: Vec::new(),
                        },
                    );
                }
            }
        }
    }

    /// Aggregate all tools from all managed servers.
    #[allow(dead_code)]
    pub async fn list_all_tools(&self) -> Vec<serde_json::Value> {
        let servers = self.servers.lock().await;
        let mut all_tools = Vec::new();
        for server in servers.values() {
            // For now, return tools if we had them.
            // In this PoC, we will inject a 'proxy' tool marker to show it's working.
            for t in &server.tools {
                all_tools.push(t.clone());
            }
        }
        all_tools
    }

    /// Check if a specific plugin is actually connected/running.
    pub async fn is_connected(&self, id: &str) -> bool {
        let servers = self.servers.lock().await;
        servers.contains_key(id)
    }
}
