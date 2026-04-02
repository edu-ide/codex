//! Centralized port configuration with environment variable overrides.
//!
//! All default port numbers live here. Each can be overridden via an
//! environment variable so users can customize without rebuilding.

/// Read a port from an environment variable, falling back to a default.
fn env_port(var: &str, default: u16) -> u16 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// ── Solo agent A2A server ports ─────────────────────────────────────────

/// Gemini CLI A2A server port (default: 41241).
pub fn gemini_a2a_port() -> u16 {
    env_port("GEMINI_A2A_PORT", 41241)
}

/// Codex A2A server port (default: 41242).
pub fn codex_a2a_port() -> u16 {
    env_port("CODEX_A2A_PORT", 41242)
}

/// Claude A2A server port (default: 41243).
pub fn claude_a2a_port() -> u16 {
    env_port("CLAUDE_A2A_PORT", 41243)
}

// ── Infrastructure ports ────────────────────────────────────────────────

/// SACP relay server port (default: 18790).
pub fn sacp_port() -> u16 {
    env_port("ILHAE_SACP_PORT", 18790)
}

/// Health/push-notification HTTP server port (default: 18791).
pub fn health_port() -> u16 {
    env_port("ILHAE_HEALTH_PORT", 18791)
}

// ── Team agent port range ───────────────────────────────────────────────

/// Base port for team agent A2A servers (default: 4321).
/// Team agents use sequential ports starting from this base (4321, 4322, 4323, 4324).
pub fn team_base_port() -> u16 {
    env_port("ILHAE_TEAM_BASE_PORT", 4321)
}

/// Team agent ports: sequential from base, `count` entries.
pub fn team_ports(count: u16) -> Vec<u16> {
    let base = team_base_port();
    (0..count).map(|i| base + i).collect()
}

/// All known agent ports (solo + team), for process cleanup.
pub fn all_agent_ports() -> Vec<u16> {
    let mut ports = vec![gemini_a2a_port(), codex_a2a_port()];
    ports.extend(team_ports(4));
    ports
}

/// Infrastructure ports (SACP + health).
pub fn infra_ports() -> Vec<u16> {
    vec![sacp_port(), health_port()]
}
