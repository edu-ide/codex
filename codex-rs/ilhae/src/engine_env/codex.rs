// Copyright 2026 Edu-IDE. Codex engine environment configuration.

use super::EngineEnv;
use tracing::info;

/// Codex (codex-ilhae) engine environment.
pub struct CodexEnv {
    pub engine_name: String,
}

impl EngineEnv for CodexEnv {
    fn label(&self) -> &str {
        if self.engine_name.starts_with("codex-ilhae") {
            "Codex-Ilhae"
        } else {
            "Codex"
        }
    }

    fn apply(&self, cmd: &mut tokio::process::Command) {
        if self.engine_name.starts_with("codex-ilhae") {
            return;
        }

        // 1. If OPENAI_API_KEY is already set, use it directly
        if std::env::var("OPENAI_API_KEY")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
        {
            return;
        }

        // 2. Read OAuth access_token from ~/.codex/auth.json (ChatGPT OAuth flow)
        if let Some(token) = read_codex_oauth_token() {
            info!("[CodexEnv] Using OAuth token from ~/.codex/auth.json");
            cmd.env("OPENAI_API_KEY", token);
            return;
        }

        // 3. Fallback: API key from auth-profiles or keychain
        if let Ok(Some(api_key)) =
            ilhae_common::auth::read_any_api_key_value(&["openai", "openai-codex"])
        {
            cmd.env("OPENAI_API_KEY", api_key);
        }
    }

    fn build_spawn_command(&self, port: u16, role: &str) -> tokio::process::Command {
        let codex_bin = if self.engine_name.starts_with("codex-ilhae") {
            resolve_native_codex_wrapper()
        } else {
            crate::context_proxy::resolve_codex_a2a_bin()
        };
        info!(bin = %codex_bin, engine_name = %self.engine_name, "[CodexEnv] Spawning a2a server");
        let mut c = tokio::process::Command::new(&codex_bin);
        c.arg("a2a-server");
        c.arg("--port")
            .arg(port.to_string())
            .arg("--role")
            .arg(role);
        c.arg("--codex-bin")
            .arg(resolve_codex_cli_bin(&self.engine_name));
        if let Some(profile) = resolve_codex_cli_profile() {
            c.arg("--codex-profile").arg(profile);
        }
        c
    }

    fn default_port(&self) -> u16 {
        crate::port_config::codex_a2a_port()
    }
}

fn resolve_codex_cli_bin(engine_name: &str) -> String {
    if let Ok(from_env) = std::env::var("CODEX_CLI_BIN") {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if engine_name.eq_ignore_ascii_case("ilhae") {
        return "ilhae".to_string();
    }

    if engine_name.starts_with("codex-ilhae") {
        return "ilhae".to_string();
    }

    if let Some(home) = dirs::home_dir() {
        let wrapper = home.join("bin").join("codex-ilhae-cli");
        if wrapper.exists() {
            return wrapper.to_string_lossy().to_string();
        }
    }

    "codex".to_string()
}

fn resolve_codex_cli_profile() -> Option<String> {
    if let Ok(from_env) = std::env::var("CODEX_CLI_PROFILE") {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    Some("gemma-local".to_string())
}

fn resolve_native_codex_wrapper() -> String {
    if let Ok(from_env) = std::env::var("ILHAE_NATIVE_CODEX_WRAPPER") {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if let Some(home) = dirs::home_dir() {
        let ilhae_wrapper = home.join("bin").join("ilhae");
        if ilhae_wrapper.exists() {
            return ilhae_wrapper.to_string_lossy().to_string();
        }
        let wrapper = home.join("bin").join("codex-ilhae-llama-nemotron");
        if wrapper.exists() {
            return wrapper.to_string_lossy().to_string();
        }
    }

    "ilhae".to_string()
}

/// Read OAuth access_token from existing Codex CLI credentials.
/// Codex CLI stores its OAuth tokens at ~/.codex/auth.json (ChatGPT OAuth flow).
fn read_codex_oauth_token() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let auth_path = std::path::PathBuf::from(&home)
        .join(".codex")
        .join("auth.json");
    let content = std::fs::read_to_string(&auth_path).ok()?;
    let auth: serde_json::Value = serde_json::from_str(&content).ok()?;

    // Try tokens.access_token first (ChatGPT OAuth)
    if let Some(token) = auth
        .pointer("/tokens/access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        return Some(token.to_string());
    }

    // Fallback: top-level OPENAI_API_KEY
    auth.get("OPENAI_API_KEY")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Symlink Codex auth files from ~/.codex/ into a team workspace directory.
/// This ensures the codex CLI subprocess can find credentials when CODEX_HOME
/// points to the team workspace instead of the default ~/.codex/.
pub fn symlink_codex_auth_to_workspace(workspace: &std::path::Path) {
    let home = match std::env::var("HOME") {
        Ok(h) => std::path::PathBuf::from(h),
        Err(_) => return,
    };
    let source_dir = home.join(".codex");
    let target_dir = workspace;

    let _ = std::fs::create_dir_all(target_dir);

    // Files to symlink: auth.json, config.toml, model_catalog.json
    for file_name in &["auth.json", "config.toml", "model_catalog.json"] {
        let src = source_dir.join(file_name);
        let dst = target_dir.join(file_name);
        if src.exists() && !dst.exists() {
            match std::os::unix::fs::symlink(&src, &dst) {
                Ok(()) => info!("[CodexEnv] Symlinked {} → {:?}", file_name, dst),
                Err(e) => tracing::warn!("[CodexEnv] Failed to symlink {}: {}", file_name, e),
            }
        }
    }
}
