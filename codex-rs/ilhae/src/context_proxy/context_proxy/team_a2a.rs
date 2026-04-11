use std::path::Path;
use std::time::Duration;

use agent_client_protocol_schema::ContentBlock;
use serde::Deserialize;
use tracing::{info, warn};

// Allow unused imports since we just dump all common imports
#[allow(unused_imports)]
use crate::context_proxy::role_parser::*;
use regex::Regex;

pub fn sync_user_agent_model_inline(engine: &str, model: &str, ilhae_dir: &Path) {
    let agents_dir = ilhae_dir.join("brain").join("agents");
    let user_agent_path = agents_dir.join("user_agent.md");

    if !agents_dir.exists() {
        let _ = std::fs::create_dir_all(&agents_dir);
    }

    let mut content = if user_agent_path.exists() {
        std::fs::read_to_string(&user_agent_path).unwrap_or_default()
    } else {
        "---\ntype: agent\nendpoint: http://localhost:4325\nengine: gemini\nmodel: gemini-2.5-pro\ncolor: \"#FF5733\"\navatar: \"🤖\"\nis_main: false\n---\n\n당신은 이 프로젝트를 끊임없이 발전시키고 완벽을 추구하는 'Autonomous Tech Lead & Visionary Product Manager'입니다.\n".to_string()
    };

    if content.contains("engine:") {
        let re = Regex::new(r"(?m)^engine:\s*.*$").unwrap();
        content = re
            .replace(&content, format!("engine: {}", engine))
            .to_string();
    } else {
        content = content.replacen("---\n", &format!("---\nengine: {}\n", engine), 1);
    }

    if content.contains("model:") {
        let re = Regex::new(r"(?m)^model:\s*.*$").unwrap();
        content = re
            .replace(&content, format!("model: {}", model))
            .to_string();
    } else {
        content = content.replacen("---\n", &format!("---\nmodel: {}\n", model), 1);
    }

    let _ = std::fs::write(&user_agent_path, content);
    info!(
        "[team_a2a] Synced user_agent.md -> engine: {}, model: {}",
        engine, model
    );
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TeamAgentConfig {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub system_prompt: String,
    /// "gemini" (default) or "codex"
    #[serde(default = "default_engine")]
    pub engine: String,
    /// Specific model override (e.g. "gemini-2.5-pro")
    #[serde(default)]
    pub model: String,
    /// True if this agent is the main entry point (receives user messages first)
    #[serde(default)]
    pub is_main: bool,
}

pub fn default_engine() -> String {
    "gemini".to_string()
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TeamConfigFile {
    #[serde(default)]
    pub team_prompt: String,
    #[serde(default)]
    pub agents: Vec<TeamAgentConfig>,
}

#[derive(Debug, Clone)]
pub struct TeamRoleTarget {
    pub role: String,
    pub endpoint: String,
    pub system_prompt: String,
    /// "gemini" or "codex"
    pub engine: String,
    /// Specific model override (e.g. "gemini-2.5-pro")
    pub model: String,
    /// Skill IDs available to this agent (e.g. ["vault", "code_review"])
    pub skills: Vec<String>,
    /// MCP server IDs available to this agent (e.g. ["filesystem", "github"])
    pub mcp_servers: Vec<String>,
    /// True if this agent is the main entry point
    pub is_main: bool,
}

#[derive(Debug, Clone)]
pub struct TeamRuntimeConfig {
    pub team_prompt: String,
    pub agents: Vec<TeamRoleTarget>,
}

#[derive(Debug, Clone)]
pub struct TeamOrchestrationResult {
    pub assistant_text: String,
    pub structured: serde_json::Value,
}

pub fn prompt_blocks_to_text(prompt: &[ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(t) => {
                if t.text.starts_with("__MCP_WIDGET_CTX__:") {
                    None
                } else {
                    Some(t.text.trim().to_string())
                }
            }
            _ => None,
        })
        .filter(|chunk| !chunk.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Parse a single agent markdown file from `brain/agents/*.md`.
/// Frontmatter fields: endpoint, engine, model, color, avatar, type.
/// The markdown body (after frontmatter) is used as the system_prompt.
fn parse_agent_markdown(path: &std::path::Path) -> Option<TeamRoleTarget> {
    let content = std::fs::read_to_string(path).ok()?;
    let role = path.file_stem()?.to_string_lossy().to_string();

    // Must have YAML frontmatter
    if !content.starts_with("---\n") {
        return None;
    }
    let end_idx = content[4..].find("\n---\n")?;
    let yaml_str = &content[4..4 + end_idx];
    let body = content[4 + end_idx + 5..].trim().to_string();

    // Simple key: value parsing (no serde_yaml dependency needed)
    let mut fm = std::collections::HashMap::new();
    for line in yaml_str.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_string();
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            fm.insert(key, value);
        }
    }

    // Only include files with type: agent (or missing type defaults to agent if endpoint exists)
    let file_type = fm.get("type").map(|s| s.as_str()).unwrap_or("agent");
    if file_type != "agent" {
        return None;
    }

    let endpoint = fm
        .get("endpoint")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if endpoint.is_empty() {
        return None;
    }

    let engine = fm
        .get("engine")
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_else(|| "gemini".to_string());
    let model = fm
        .get("model")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let skills = parse_fm_list(&fm, "skills");
    let mcp_servers = parse_fm_list(&fm, "mcp_servers");

    let is_main = fm
        .get("is_main")
        .map(|s| s.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    Some(TeamRoleTarget {
        role,
        endpoint,
        system_prompt: body,
        engine,
        model,
        skills,
        mcp_servers,
        is_main,
    })
}

/// Parse a comma-separated or YAML-list frontmatter value into Vec<String>.
/// Supports: `skills: vault, code_review` or `skills: [vault, code_review]`
fn parse_fm_list(fm: &std::collections::HashMap<String, String>, key: &str) -> Vec<String> {
    let Some(val) = fm.get(key) else {
        return Vec::new();
    };
    let val = val.trim();
    if val.is_empty() {
        return Vec::new();
    }
    // Strip surrounding brackets if present
    let val = val
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(val);
    val.split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn load_team_runtime_config(ilhae_dir: &Path) -> Option<TeamRuntimeConfig> {
    // ── Try brain/agents/*.md first ──
    let agents_dir = ilhae_dir.join("brain").join("agents");
    let mut agents = Vec::new();

    if agents_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
            let mut paths: Vec<_> = entries
                .flatten()
                .filter(|e| {
                    e.path().extension().and_then(|ext| ext.to_str()) == Some("md")
                        && e.path().is_file()
                })
                .collect();
            paths.sort_by_key(|e| e.file_name());

            for entry in paths {
                if let Some(target) = parse_agent_markdown(&entry.path()) {
                    info!(
                        "[TeamBrain] Loaded agent from {:?}: role={} endpoint={}",
                        entry.path(),
                        target.role,
                        target.endpoint
                    );
                    agents.push(target);
                }
            }
        }
    }

    // Read team prompt from brain/context/TEAM.md
    let team_prompt = {
        let team_md = ilhae_dir.join("brain").join("context").join("TEAM.md");
        std::fs::read_to_string(&team_md)
            .unwrap_or_default()
            .trim()
            .to_string()
    };

    if !agents.is_empty() {
        info!(
            "[TeamBrain] Loaded {} agents from brain/agents/",
            agents.len()
        );
        return Some(TeamRuntimeConfig {
            team_prompt,
            agents,
        });
    }

    // ── Fallback: legacy team.json ──
    let path = ilhae_dir
        .join("brain")
        .join("settings")
        .join("team_config.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let parsed: TeamConfigFile = serde_json::from_str(&raw).ok()?;

    for agent_cfg in &parsed.agents {
        let endpoint = agent_cfg.endpoint.trim().to_string();
        if !endpoint.is_empty() {
            agents.push(TeamRoleTarget {
                role: agent_cfg.role.trim().to_string(),
                endpoint,
                system_prompt: agent_cfg.system_prompt.trim().to_string(),
                engine: if agent_cfg.engine.trim().is_empty() {
                    "gemini".to_string()
                } else {
                    agent_cfg.engine.trim().to_lowercase()
                },
                model: agent_cfg.model.trim().to_string(),
                skills: Vec::new(),
                mcp_servers: Vec::new(),
                is_main: agent_cfg.is_main,
            });
        }
    }

    if agents.is_empty() {
        return None;
    }

    Some(TeamRuntimeConfig {
        team_prompt: if team_prompt.is_empty() {
            parsed.team_prompt.trim().to_string()
        } else {
            team_prompt
        },
        agents,
    })
}

pub fn load_user_agent_runtime_target(ilhae_dir: &Path) -> Option<TeamRoleTarget> {
    let runtime_path = ilhae_dir.join("brain").join("agents").join("user_agent.md");
    if runtime_path.exists() {
        return parse_agent_markdown(&runtime_path);
    }

    let fallback_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("brain").join("agents").join("user_agent.md"))?;
    if fallback_path.exists() {
        return parse_agent_markdown(&fallback_path);
    }
    None
}

#[derive(Debug, Clone)]
pub struct A2AResponseParsed {
    pub text: String,
    pub state: Option<String>,
    pub task_id: Option<String>,
    pub schedule_id: Option<String>,
    pub context_id: Option<String>,
}

pub fn normalize_a2a_state(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace('_', "-")
}

pub fn is_terminal_a2a_state(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "canceled" | "cancelled")
}

pub fn is_input_required_a2a_state(state: &str) -> bool {
    matches!(state, "input-required" | "inputrequired")
}

pub fn extract_text_from_a2a_part(part: &serde_json::Value) -> Option<String> {
    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let data = part.get("data")?;
    if let Some(text) = data.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let obj = data.as_object()?;
    for key in ["text", "description", "content", "summary", "message"] {
        if let Some(text) = obj.get(key).and_then(|v| v.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub fn collect_text_from_a2a_parts(parts: &[serde_json::Value], delimiter: &str) -> String {
    parts
        .iter()
        .filter_map(extract_text_from_a2a_part)
        .collect::<Vec<_>>()
        .join(delimiter)
}

pub fn parse_a2a_message_text(result: &serde_json::Value) -> String {
    let parts_text = result
        .get("parts")
        .and_then(|v| v.as_array())
        .map(|parts| collect_text_from_a2a_parts(parts, ""))
        .unwrap_or_default();
    if !parts_text.trim().is_empty() {
        return parts_text.trim().to_string();
    }

    let status_text = result
        .get("status")
        .and_then(|v| v.get("message"))
        .and_then(|v| v.get("parts"))
        .and_then(|v| v.as_array())
        .map(|parts| collect_text_from_a2a_parts(parts, ""))
        .unwrap_or_default();
    if !status_text.trim().is_empty() {
        return status_text.trim().to_string();
    }

    let history_text = result
        .get("history")
        .and_then(|v| v.as_array())
        .map(|history| {
            history
                .iter()
                .filter(|msg| msg.get("role").and_then(|v| v.as_str()) == Some("agent"))
                .flat_map(|msg| {
                    msg.get("parts")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|part| extract_text_from_a2a_part(&part))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if !history_text.trim().is_empty() {
        return history_text.trim().to_string();
    }

    let artifact_text = result
        .get("artifacts")
        .and_then(|v| v.as_array())
        .map(|artifacts| {
            artifacts
                .iter()
                .flat_map(|artifact| {
                    artifact
                        .get("parts")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|part| extract_text_from_a2a_part(&part))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    artifact_text.trim().to_string()
}

pub fn parse_a2a_result(result: &serde_json::Value) -> A2AResponseParsed {
    let state = result
        .get("status")
        .and_then(|v| v.get("state"))
        .and_then(|v| v.as_str())
        .map(normalize_a2a_state)
        .or_else(|| {
            result
                .get("state")
                .and_then(|v| v.as_str())
                .map(normalize_a2a_state)
        });

    let schedule_id = result
        .get("taskId")
        .and_then(|v| v.as_str())
        .or_else(|| result.get("schedule_id").and_then(|v| v.as_str()))
        .or_else(|| result.get("id").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    let context_id = result
        .get("contextId")
        .and_then(|v| v.as_str())
        .or_else(|| result.get("context_id").and_then(|v| v.as_str()))
        .map(|s| s.to_string());
    let task_id = schedule_id.clone();

    A2AResponseParsed {
        text: parse_a2a_message_text(result),
        state,
        task_id,
        schedule_id,
        context_id,
    }
}

// ── A2A server auto-spawn helpers (binary + env now in engine_env/) ──
/// Extract port number from an endpoint URL like "http://localhost:4321"
pub fn extract_port_from_endpoint(endpoint: &str) -> Option<u16> {
    endpoint
        .trim_end_matches('/')
        .rsplit(':')
        .next()
        .and_then(|s| s.parse::<u16>().ok())
}

pub fn is_gemini_cli_root(path: &std::path::Path) -> bool {
    path.join("packages")
        .join("a2a-server")
        .join("package.json")
        .exists()
}

pub fn resolve_gemini_cli_root() -> Option<std::path::PathBuf> {
    if let Ok(from_env) = std::env::var("GEMINI_CLI_ROOT") {
        let p = std::path::PathBuf::from(from_env);
        if is_gemini_cli_root(&p) {
            return Some(p);
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let mut cur: Option<&std::path::Path> = Some(cwd.as_path());
        while let Some(dir) = cur {
            if is_gemini_cli_root(dir) {
                return Some(dir.to_path_buf());
            }
            let sibling = dir.join("gemini-cli");
            if is_gemini_cli_root(&sibling) {
                return Some(sibling);
            }
            cur = dir.parent();
        }
    }

    // Compile-time fallback: use CARGO_MANIFEST_DIR to locate gemini-cli
    // relative to the ilhae-proxy crate (sibling directory in monorepo).
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(ilhae_agent) = manifest_dir.parent() {
        let sibling = ilhae_agent.join("gemini-cli");
        if is_gemini_cli_root(&sibling) {
            return Some(sibling);
        }
    }

    None
}

/// Resolve the codex-mcp-server binary path.
/// Checks CODEX_MCP_SERVER_BIN env var, then sibling paths, then PATH.
pub fn resolve_codex_a2a_bin() -> String {
    if let Ok(from_env) = std::env::var("CODEX_A2A_BIN") {
        if std::path::Path::new(&from_env).exists() {
            return from_env;
        }
    }
    if let Ok(current_exe) = std::env::current_exe()
        && let Some(bin_dir) = current_exe.parent()
    {
        let sibling = bin_dir.join("codex-ilhae");
        if sibling.exists() {
            info!("[TeamSpawn] Found codex A2A sibling binary: {:?}", sibling);
            return sibling.to_string_lossy().to_string();
        }
    }
    // Try co-located path relative to ilhae-agent directory
    if let Ok(cwd) = std::env::current_dir() {
        let mut cur: Option<&std::path::Path> = Some(cwd.as_path());
        while let Some(dir) = cur {
            for subpath in [
                "target/debug/codex-ilhae",
                "target/release/codex-ilhae",
                "target/debug/codex-a2a",
                "target/release/codex-a2a",
                "services/ilhae-agent/target/debug/codex-a2a",
                "services/ilhae-agent/target/release/codex-a2a",
                "codex/codex-rs/target/debug/codex-ilhae",
                "codex/codex-rs/target/release/codex-ilhae",
                "codex-a2a/target/debug/codex-ilhae",
                "codex-a2a/target/release/codex-ilhae",
            ] {
                let candidate = dir.join(subpath);
                if candidate.exists() {
                    info!("[TeamSpawn] Found codex A2A binary: {:?}", candidate);
                    return candidate.to_string_lossy().to_string();
                }
            }
            cur = dir.parent();
        }
    }
    // Fallback to PATH
    "codex-ilhae".to_string()
}

/// Resolve the `node` binary path.
/// Checks `which node`, common nvm locations, and `/usr/local/bin/node`.
/// Falls back to `"node"` if nothing found.
pub fn resolve_node_binary() -> String {
    // 1. Try `which node`
    if let Ok(output) = std::process::Command::new("which").arg("node").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() && std::path::Path::new(&path).exists() {
                return path;
            }
        }
    }

    // 2. Check common locations
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let candidates = [
        format!("{}/.nvm/versions/node/v20.19.0/bin/node", home),
        format!("{}/.nvm/versions/node/v22.14.0/bin/node", home),
        "/usr/local/bin/node".to_string(),
        "/opt/homebrew/bin/node".to_string(),
    ];
    for candidate in &candidates {
        if std::path::Path::new(candidate).exists() {
            info!("[NodeResolve] Found node at: {}", candidate);
            return candidate.clone();
        }
    }

    // 3. Walk nvm versions dir for any installed version
    let nvm_dir = format!("{}/.nvm/versions/node", home);
    if let Ok(entries) = std::fs::read_dir(&nvm_dir) {
        let mut versions: Vec<_> = entries.flatten().collect();
        versions.sort_by(|a, b| b.file_name().cmp(&a.file_name())); // newest first
        for entry in versions {
            let bin = entry.path().join("bin").join("node");
            if bin.exists() {
                let path = bin.to_string_lossy().to_string();
                info!("[NodeResolve] Found node via nvm scan: {}", path);
                return path;
            }
        }
    }

    warn!("[NodeResolve] Could not find node binary, falling back to 'node'");
    "node".to_string()
}

/// Spawn a gemini-cli-a2a-server process for each team agent.
/// `workspace_map` maps role -> workspace path (from `generate_peer_registration_files`).
/// Returns the list of child processes (to be cleaned up later).
pub async fn spawn_team_a2a_servers(
    team: &TeamRuntimeConfig,
    workspace_map: &std::collections::HashMap<String, std::path::PathBuf>,
    webhook_url: Option<&str>,
    session_id: &str,
) -> Vec<tokio::process::Child> {
    let gemini_cli_root = resolve_gemini_cli_root();
    if let Some(root) = gemini_cli_root.as_ref() {
        info!(
            "[TeamSpawn] Using gemini-cli workspace launcher at {:?}",
            root
        );
    } else {
        warn!("[TeamSpawn] GEMINI_CLI_ROOT unresolved; fallback to `gemini-cli-a2a-server` binary");
    }

    let futures = team.agents.iter().map(|target| {
        let role = target.role.clone();
        let engine = target.engine.clone();
        let model = target.model.clone();
        let endpoint = target.endpoint.clone();
        let is_main = target.is_main;
        let _gemini_cli_root = gemini_cli_root.clone();
        let workspace_map = workspace_map.clone();
        let webhook_url_opt = webhook_url.map(|s| s.to_string());
        let session_id = session_id.to_string();

        async move {
            let port = match extract_port_from_endpoint(&endpoint) {
                Some(p) => p,
                None => {
                    warn!(
                        "[TeamSpawn] Cannot extract port from {}: {}",
                        role, endpoint
                    );
                    return None;
                }
            };

            // Check if the server is already running and healthy.
            let check_url = format!("{}/.well-known/agent.json", endpoint.trim_end_matches('/'));
            if reqwest::Client::new()
                .get(&check_url)
                .timeout(Duration::from_millis(500))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false)
            {
                info!(
                    "[TeamSpawn] {} already running on port {}, skipping spawn",
                    role, port
                );
                return None;
            }

            info!(
                "[TeamSpawn] Spawning A2A server for {} on port {} (engine={})",
                role, port, engine
            );

            // Kill any existing DEAD/zombie process occupying this port
            let _ = std::process::Command::new("fuser")
                .args(["-k", &format!("{}/tcp", port)])
                .output();

            let engine_env = crate::engine_env::resolve_engine_env(&engine);
            let mut cmd = engine_env.build_spawn_command(port, &role);

            // Ensure PATH is inherited (critical for `sh -c node …` in test environments)
            if let Ok(path) = std::env::var("PATH") {
                cmd.env("PATH", &path);
            }

            // ── Canonical A2A env (single source of truth) ──
            crate::engine_env::apply_engine_env(&mut cmd, &engine).await;

            if engine.eq_ignore_ascii_case("gemini") {
                cmd.env("USE_CCPA", "true");
                cmd.env("GEMINI_CLI_USE_COMPUTE_ADC", "true");
            }

            // ── Spawn-specific vars ──
            let engine_env = crate::engine_env::resolve_engine_env(&engine);
            let engine_label = engine_env.label();
            cmd.env("AGENT_CARD_NAME", format!("{} ({})", role, engine_label));

            if !model.is_empty() {
                cmd.env("GEMINI_MODEL", &model);
                info!("[TeamSpawn] {} using model: {}", role, model);
            }

            let caller_role = role.to_lowercase();
            cmd.env("CODER_AGENT_NAME", caller_role.clone());
            cmd.env("A2A_CONTEXT_ID", &session_id);
            cmd.env(
                "CODER_AGENT_ENABLE_AGENTS",
                if is_main { "false" } else { "true" },
            );
            cmd.env("CODER_AGENT_IGNORE_WORKSPACE_SETTINGS", "true");

            if let Some(workspace) = workspace_map.get(&caller_role) {
                if let Ok(cwd) = std::env::current_dir() {
                    let mut current = cwd.clone();
                    let mut project_root = cwd.clone();
                    while let Some(parent) = current.parent() {
                        if current.join(".git").exists() {
                            project_root = current.clone();
                            break;
                        }
                        current = parent.to_path_buf();
                    }
                    cmd.env(
                        "CODER_AGENT_WORKSPACE_PATH",
                        project_root.to_string_lossy().as_ref(),
                    );
                } else {
                    cmd.env(
                        "CODER_AGENT_WORKSPACE_PATH",
                        workspace.to_string_lossy().as_ref(),
                    );
                }

                // Symlink ~/.gemini auth files into team workspace so
                // gemini CLI can find OAuth credentials when GEMINI_CLI_HOME
                // points to the workspace instead of ~/.gemini.
                if engine.eq_ignore_ascii_case("gemini") {
                    symlink_gemini_auth_to_workspace(workspace);
                }
                if engine.eq_ignore_ascii_case("codex") {
                    crate::engine_env::codex::symlink_codex_auth_to_workspace(workspace);
                }

                cmd.env("GEMINI_CLI_HOME", workspace.to_string_lossy().as_ref());
                cmd.env("CODEX_HOME", workspace.to_string_lossy().as_ref());
            }

            // Brain directory access
            if let Some(workspace) = workspace_map.get(&caller_role) {
                if let Some(team_ws_dir) = workspace.parent() {
                    if let Some(ilhae_root) = team_ws_dir.parent() {
                        let brain_dir = ilhae_root.join("brain");
                        if brain_dir.is_dir() {
                            cmd.env(
                                "CODER_AGENT_INCLUDE_DIRS",
                                brain_dir.to_string_lossy().as_ref(),
                            );
                            info!("[TeamSpawn] {} include brain dir: {:?}", role, brain_dir);
                        }
                    }
                }
            }

            if let Some(url) = webhook_url_opt.as_ref() {
                cmd.env("TEAM_EVENT_WEBHOOK", url);
                let base_url = url.trim_end_matches("/events");
                let caller_endpoint = endpoint.trim_end_matches('/');
                cmd.env(
                    "A2A_WEBHOOK_URL",
                    format!(
                        "{}/a2a_callback?caller={}&caller_endpoint={}",
                        base_url, caller_role, caller_endpoint
                    ),
                );
            }

            let log_dir = workspace_map
                .get(&role.to_lowercase())
                .cloned()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
            let log_path = log_dir.join(format!("a2a-server-{}.log", role.to_lowercase()));
            let log_file = match std::fs::File::create(&log_path) {
                Ok(f) => f,
                Err(e) => {
                    warn!("[TeamSpawn] Cannot create log {:?}: {}", log_path, e);
                    cmd.stdin(std::process::Stdio::null());
                    cmd.stdout(std::process::Stdio::null());
                    cmd.stderr(std::process::Stdio::null());
                    match cmd.spawn() {
                        Ok(child) => {
                            info!(
                                "[TeamSpawn] {} spawned (pid: {:?}) (no log)",
                                role,
                                child.id()
                            );
                            return Some(child);
                        }
                        Err(e2) => {
                            warn!("[TeamSpawn] Failed to spawn {} A2A server: {}", role, e2);
                            return None;
                        }
                    }
                }
            };
            let log_file2 = log_file
                .try_clone()
                .unwrap_or_else(|_| std::fs::File::create("/dev/null").expect("open /dev/null"));
            info!("[TeamSpawn] {} log: {:?}", role, log_path);
            cmd.stdin(std::process::Stdio::null());
            cmd.stdout(std::process::Stdio::from(log_file));
            cmd.stderr(std::process::Stdio::from(log_file2));

            match cmd.spawn() {
                Ok(child) => {
                    info!("[TeamSpawn] {} spawned (pid: {:?})", role, child.id());
                    Some(child)
                }
                Err(e) => {
                    warn!("[TeamSpawn] Failed to spawn {} A2A server: {}", role, e);
                    None
                }
            }
        }
    });

    let results = futures::future::join_all(futures).await;
    results.into_iter().flatten().collect()
}

pub async fn ensure_user_agent_server(
    ilhae_dir: &Path,
    proxy_base_url: Option<&str>,
    session_id: &str,
) -> Option<tokio::process::Child> {
    let target = load_user_agent_runtime_target(ilhae_dir)?;
    if let Some(port) = extract_port_from_endpoint(&target.endpoint) {
        tracing::info!(
            "[UserAgent] Forcing restart on port {} to reload latest autonomous settings",
            port
        );
        crate::process_supervisor::kill_port(port);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let team = TeamRuntimeConfig {
        team_prompt: String::new(),
        agents: vec![target.clone()],
    };
    let workspace_map = generate_peer_registration_files(&team, proxy_base_url);
    let mut children = spawn_team_a2a_servers(&team, &workspace_map, None, session_id).await;
    if wait_for_a2a_health(&target.endpoint, Duration::from_secs(30))
        .await
        .is_ok()
    {
        info!("[UserAgent] Ready at {}", target.endpoint);
    } else {
        warn!(
            "[UserAgent] Not ready after spawn attempt: {}",
            target.endpoint
        );
    }
    children.pop()
}

/// Wait for an A2A server to become healthy by polling its agent-card endpoint.
pub async fn wait_for_a2a_health(endpoint: &str, timeout: Duration) -> Result<(), String> {
    let url = format!("{}/.well-known/agent.json", endpoint.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(500);

    loop {
        match client
            .get(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            _ => {}
        }
        if start.elapsed() >= timeout {
            return Err(format!(
                "A2A server at {} not ready after {:?}",
                endpoint, timeout
            ));
        }
        tokio::time::sleep(poll_interval).await;
    }
}

/// Wait for all team A2A servers to be healthy.
pub async fn wait_for_all_team_health(team: &TeamRuntimeConfig) -> Result<(), String> {
    let timeout = Duration::from_secs(90);

    for agent in &team.agents {
        let role = &agent.role;
        let endpoint = &agent.endpoint;
        info!("[TeamHealth] Waiting for {} at {}", role, endpoint);
        wait_for_a2a_health(endpoint, timeout)
            .await
            .map_err(|e| format!("{} health check failed: {}", role, e))?;
        info!("[TeamHealth] {} is ready", role);
    }
    Ok(())
}

/// Kill all spawned team A2A server processes.
pub async fn cleanup_team_processes(
    mut children: Vec<tokio::process::Child>,
    team: &TeamRuntimeConfig,
) {
    let client = reqwest::Client::new();

    // Step 1: Try HTTP /shutdown for each server (graceful)
    for agent in &team.agents {
        let role = &agent.role;
        let endpoint = &agent.endpoint;
        let url = format!("{}/shutdown", endpoint.trim_end_matches('/'));
        match client
            .post(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                info!("[TeamCleanup] {} shutdown via HTTP OK", role);
            }
            _ => {
                info!(
                    "[TeamCleanup] {} HTTP shutdown failed, will use signal",
                    role
                );
            }
        }
    }

    // Step 2: Wait briefly then kill remaining processes
    tokio::time::sleep(Duration::from_millis(800)).await;
    for child in children.iter_mut() {
        let pid = child.id();
        // Try SIGTERM first (Unix only)
        #[cfg(unix)]
        if let Some(pid_val) = pid {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &pid_val.to_string()])
                .spawn();
            info!("[TeamCleanup] Sent SIGTERM to pid={}", pid_val);
        }
        // Give 1s for graceful exit, then force kill
        match tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
            Ok(_) => info!("[TeamCleanup] Process {:?} exited gracefully", pid),
            Err(_) => {
                info!("[TeamCleanup] Process {:?} didn't exit, force killing", pid);
                let _ = child.kill().await;
            }
        }
    }
}

/// Trigger AgentRegistry reload on all team A2A servers via JSON-RPC.
/// This re-scans `.gemini/agents/` so each server discovers its now-healthy peers.
pub async fn trigger_agent_reload(team: &TeamRuntimeConfig) {
    let futures = team.agents.iter().map(|agent| {
        let role = agent.role.clone();
        let endpoint = agent.endpoint.clone();
        async move {
            let proxy = a2a_rs::A2aProxy::new(&endpoint, &role);
            match proxy.reload_agents().await {
                Ok(result) => {
                    info!("[AgentReload] {} reloaded: {}", role, result);
                }
                Err(e) => {
                    warn!("[AgentReload] {} reload failed: {}", role, e);
                }
            }
        }
    });

    futures::future::join_all(futures).await;
}

/// Dynamically add brain directory to all team A2A agents via JSON-RPC.
/// This gives agents file-system access to brain/ and triggers skill discovery
/// from brain/skills/.
pub async fn add_brain_directories(team: &TeamRuntimeConfig, ilhae_dir: &std::path::Path) {
    let brain_dir = ilhae_dir.join("brain");
    if !brain_dir.is_dir() {
        info!("[AddBrainDir] brain directory not found at {:?}", brain_dir);
        return;
    }
    let brain_path = brain_dir.to_string_lossy().to_string();

    let futures = team.agents.iter().map(|agent| {
        let role = agent.role.clone();
        let endpoint = agent.endpoint.clone();
        let path = brain_path.clone();
        async move {
            let proxy = a2a_rs::A2aProxy::new(&endpoint, &role);
            match proxy.add_directory(&path, true).await {
                Ok(result) => {
                    info!("[AddBrainDir] {} added brain dir: {}", role, result);
                }
                Err(e) => {
                    warn!("[AddBrainDir] {} failed to add brain dir: {}", role, e);
                }
            }
        }
    });

    futures::future::join_all(futures).await;
}

/// Symlink gemini CLI auth files from ~/.gemini/ into a team workspace directory.
///
/// When `GEMINI_CLI_HOME` is set to a team workspace, the gemini a2a-server
/// looks for OAuth credentials there instead of `~/.gemini/`. This function
/// symlinks the existing auth files so each team agent reuses the user's
/// existing authentication without requiring separate login.
fn symlink_gemini_auth_to_workspace(workspace: &std::path::Path) {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return,
    };
    let gemini_home = std::path::PathBuf::from(&home).join(".gemini");
    if !gemini_home.is_dir() {
        return;
    }

    // Auth-related files to symlink
    let auth_files = [
        "oauth_creds.json",
        ".credentials.json",
        "credentials.json",
        ".session_data.json",
    ];

    for filename in &auth_files {
        let src = gemini_home.join(filename);
        if !src.exists() {
            continue;
        }
        let dst = workspace.join(filename);
        if dst.exists() {
            continue; // Already exists (symlink or real file)
        }
        match std::os::unix::fs::symlink(&src, &dst) {
            Ok(()) => {
                info!(
                    "[TeamSpawn] Symlinked auth file: {} -> {}",
                    dst.display(),
                    src.display()
                );
            }
            Err(e) => {
                warn!("[TeamSpawn] Failed to symlink {}: {}", dst.display(), e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_a2a_result;

    #[test]
    fn parse_a2a_result_keeps_task_id_alias_for_schedule_id() {
        let result = serde_json::json!({
            "id": "task-123",
            "status": {
                "state": "completed",
                "message": {
                    "role": "agent",
                    "parts": [{"text": "done"}]
                }
            }
        });

        let parsed = parse_a2a_result(&result);
        assert_eq!(parsed.schedule_id.as_deref(), Some("task-123"));
        assert_eq!(parsed.task_id.as_deref(), Some("task-123"));
    }
}
