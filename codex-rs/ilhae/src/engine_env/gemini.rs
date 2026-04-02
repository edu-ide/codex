// Copyright 2026 Edu-IDE. Gemini engine environment configuration.

use super::EngineEnv;
use anyhow::Result;
use tracing::info;

/// Gemini (gemini-ilhae) engine environment.
///
/// Auth: Relies on external authentication mechanisms (like ACP Desktop Client)
/// to provide or refresh valid credentials before spawning.
pub struct GeminiEnv;

impl EngineEnv for GeminiEnv {
    fn label(&self) -> &str {
        "Gemini"
    }

    fn apply(&self, cmd: &mut tokio::process::Command) {
        // ADC auth (gcloud auth application-default login)
        cmd.env("USE_CCPA", "true");
        cmd.env("GEMINI_CLI_USE_COMPUTE_ADC", "true");

        // 1. Inject the valid token provided by the ACP Client (Desktop)
        if let Ok(token) = std::env::var("GEMINI_OAUTH_ACCESS_TOKEN") {
            cmd.env("GEMINI_ACCESS_TOKEN", token);
            return;
        }

        // 2. Auto-read from existing gemini CLI OAuth credentials (~/.gemini/oauth_creds.json)
        if let Some(token) = read_gemini_oauth_token() {
            info!("[GeminiEnv] Using OAuth token from ~/.gemini/oauth_creds.json");
            cmd.env("GEMINI_ACCESS_TOKEN", token);
            return;
        }

        // 3. Fallback: API key from auth-profiles or keychain
        if let Ok(Some(api_key)) =
            ilhae_common::auth::read_any_api_key_value(&["google", "google-gemini-cli"])
        {
            cmd.env("GEMINI_API_KEY", api_key);
        }
    }

    fn build_spawn_command(&self, port: u16, _role: &str) -> tokio::process::Command {
        // Priority: dedicated a2a-server binary > workspace node > legacy subcommand fallback
        if std::process::Command::new("which")
            .arg("gemini-ilhae-a2a-server")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            info!("[GeminiEnv] Spawning via gemini-ilhae-a2a-server");
            let mut c = tokio::process::Command::new("gemini-ilhae-a2a-server");
            c.env("CODER_AGENT_PORT", port.to_string());
            // Ensure wrapper script can find a2a-server.mjs
            if let Some(root) = crate::context_proxy::resolve_gemini_cli_root() {
                c.env("GEMINI_CLI_ROOT", root.to_string_lossy().as_ref());
            }
            c
        } else if let Some(root) = crate::context_proxy::resolve_gemini_cli_root() {
            info!(root = %root.display(), "[GeminiEnv] Spawning via workspace node");
            let node_bin = crate::context_proxy::resolve_node_binary();
            let mut c = tokio::process::Command::new(&node_bin);
            c.arg("packages/a2a-server/dist/a2a-server.mjs");
            c.current_dir(&root);
            c.env("CODER_AGENT_PORT", port.to_string());
            c
        } else if std::process::Command::new("which")
            .arg("gemini-ilhae")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            info!("[GeminiEnv] Spawning via legacy gemini-ilhae a2a-server");
            let mut c = tokio::process::Command::new("gemini-ilhae");
            c.arg("a2a-server");
            c.env("CODER_AGENT_PORT", port.to_string());
            c
        } else {
            info!("[GeminiEnv] Spawning via gemini-ilhae-a2a-server fallback");
            let mut c = tokio::process::Command::new("gemini-ilhae-a2a-server");
            c.env("CODER_AGENT_PORT", port.to_string());
            if let Some(root) = crate::context_proxy::resolve_gemini_cli_root() {
                c.env("GEMINI_CLI_ROOT", root.to_string_lossy().as_ref());
            }
            c
        }
    }

    fn default_port(&self) -> u16 {
        crate::port_config::gemini_a2a_port()
    }

    fn pre_launch(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

/// Read OAuth access_token from existing Gemini CLI credentials.
/// The gemini CLI stores its OAuth tokens at ~/.gemini/oauth_creds.json.
/// If the user has already authenticated via `gemini`, we reuse that token.
fn read_gemini_oauth_token() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let creds_path = std::path::PathBuf::from(&home)
        .join(".gemini")
        .join("oauth_creds.json");
    let content = std::fs::read_to_string(&creds_path).ok()?;
    let creds: serde_json::Value = serde_json::from_str(&content).ok()?;
    creds
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}
