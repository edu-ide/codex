// Copyright 2026 Edu-IDE. Engine-specific A2A server environment variables.
//
//! Engine-agnostic environment variable application for A2A server processes.
//!
//! **Architecture**: Each engine (Gemini, Codex, etc.) implements [`EngineEnv`]
//! to declare its required environment variables. `apply_engine_env()` dispatches
//! to the correct implementation, then applies common variables shared by all engines.
//!
//! Adding a new engine:
//! 1. Create `engine_env/{engine}.rs` implementing `EngineEnv`
//! 2. Add the variant to `resolve_engine_env()`
//! 3. Run `cargo build --release`

pub mod codex;
pub mod gemini;

use tracing::{debug, warn};

/// Trait for engine-specific environment variable configuration.
///
/// Each engine declares what env vars it needs. The common vars (GEMINI_FOLDER_TRUST,
/// GEMINI_YOLO_MODE, etc.) are applied by [`apply_engine_env`] after the engine-specific ones.
pub trait EngineEnv {
    /// Human-readable label for logs and UI (e.g. "Gemini", "Codex")
    fn label(&self) -> &str;

    /// Apply engine-specific env vars to the command
    fn apply(&self, cmd: &mut tokio::process::Command);

    /// Build the tokio::process::Command for spawning this engine's A2A server.
    /// `port` is the port to listen on. `role` is the agent role (e.g. "leader").
    fn build_spawn_command(&self, port: u16, role: &str) -> tokio::process::Command;

    /// Default A2A server port for solo mode.
    fn default_port(&self) -> u16;

    /// Pre-launch hook called before spawning. Override for engines that need
    /// credential refresh or other setup. Default: no-op.
    fn pre_launch(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

/// Resolve engine name to its env configurator.
pub fn resolve_engine_env(engine: &str) -> Box<dyn EngineEnv + Send + Sync> {
    match engine {
        "ilhae" | "codex" | "codex-ilhae" | "codex-ilhae-llama-nemotron" => Box::new(codex::CodexEnv {
            engine_name: engine.to_string(),
        }),
        _ => Box::new(gemini::GeminiEnv), // default
    }
}

/// Apply all required env vars for an A2A server process.
///
/// This is the **single entry point** — both `helpers::spawn_a2a_server_daemon`
/// and `team_a2a::spawn_team_a2a_servers` call this function.
pub async fn apply_engine_env(cmd: &mut tokio::process::Command, engine: &str) {
    let engine_env = resolve_engine_env(engine);

    debug!(
        engine = engine_env.label(),
        "Applying A2A server engine env"
    );

    // 0. Pre-launch hook (e.g. token refresh) — failure is non-fatal
    if let Err(e) = engine_env.pre_launch().await {
        warn!(engine = engine_env.label(), error = %e, "pre_launch hook failed, continuing");
    }

    // 1. Common vars (all engines) — clear inherited shell noise first
    apply_common_env(cmd);

    // 2. Engine-specific vars
    engine_env.apply(cmd);
}

/// Environment variables shared by ALL engines.
fn apply_common_env(cmd: &mut tokio::process::Command) {
    // Security / automation
    cmd.env("GEMINI_FOLDER_TRUST", "true");
    cmd.env("GEMINI_YOLO_MODE", "true");
    cmd.env("A2A_AUTO_COMPLETE_ON_TURN_END", "true");
    cmd.env("A2A_SERVER", "true");

    // Strip explicit API keys — engine-specific auth provides credentials
    cmd.env_remove("GEMINI_API_KEY");
    cmd.env_remove("ANTHROPIC_API_KEY");

    // CRITICAL: Remove USE_CCPA if inherited from parent shell profile.
    // USE_CCPA triggers GOOGLE_APPLICATION_CREDENTIALS file auth → LOGIN_WITH_GOOGLE
    // fallback → crash in non-interactive. Engine-specific auth (e.g. ADC) is correct.
    cmd.env_remove("USE_CCPA");
}
