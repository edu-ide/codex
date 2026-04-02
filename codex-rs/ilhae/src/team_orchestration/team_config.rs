//! Team configuration types and config-file parsing.
//!
//! Extracted from `runner.rs` — contains struct definitions for team
//! agent configuration (TeamAgentConfig, TeamConfigFile, TeamRoleTarget,
//! TeamRuntimeConfig, TeamOrchestrationResult, A2AResponseParsed) and
//! the config-loading functions.

use std::path::Path;

use agent_client_protocol_schema::ContentBlock;
use serde::Deserialize;
use serde_json::json;
use tracing::info;

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
    /// Skill IDs available to this agent
    pub skills: Vec<String>,
    /// MCP server IDs available to this agent
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
fn parse_agent_markdown_runner(path: &std::path::Path) -> Option<TeamRoleTarget> {
    let content = std::fs::read_to_string(path).ok()?;
    let role = path.file_stem()?.to_string_lossy().to_string();

    if !content.starts_with("---\n") {
        return None;
    }
    let end_idx = content[4..].find("\n---\n")?;
    let yaml_str = &content[4..4 + end_idx];
    let body = content[4 + end_idx + 5..].trim().to_string();

    let mut fm = std::collections::HashMap::new();
    for line in yaml_str.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if let Some((key, value)) = line.split_once(':') {
            fm.insert(key.trim().to_string(), value.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }

    let file_type = fm.get("type").map(|s| s.as_str()).unwrap_or("agent");
    if file_type != "agent" { return None; }

    let endpoint = fm.get("endpoint").map(|s| s.trim().to_string()).unwrap_or_default();
    if endpoint.is_empty() { return None; }

    let engine = fm.get("engine").map(|s| s.trim().to_lowercase()).unwrap_or_else(|| "gemini".to_string());

    let parse_list = |key: &str| -> Vec<String> {
        let Some(val) = fm.get(key) else { return Vec::new() };
        let val = val.trim();
        if val.is_empty() { return Vec::new(); }
        let val = val.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(val);
        val.split(',').map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string()).filter(|s| !s.is_empty()).collect()
    };
    let skills = parse_list("skills");
    let mcp_servers = parse_list("mcp_servers");

    Some(TeamRoleTarget {
        role,
        endpoint,
        system_prompt: body,
        engine,
        skills,
        mcp_servers,
    })
}

pub fn load_team_runtime_config(ilhae_dir: &Path) -> Option<TeamRuntimeConfig> {
    // ── Try brain/agents/*.md first ──
    let agents_dir = ilhae_dir.join("brain").join("agents");
    let mut agents = Vec::new();

    if agents_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
            let mut paths: Vec<_> = entries.flatten()
                .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md") && e.path().is_file())
                .collect();
            paths.sort_by_key(|e| e.file_name());
            for entry in paths {
                if let Some(target) = parse_agent_markdown_runner(&entry.path()) {
                    info!("[TeamBrain] Loaded agent from {:?}: role={} endpoint={}", entry.path(), target.role, target.endpoint);
                    agents.push(target);
                }
            }
        }
    }

    let team_prompt = {
        let team_md = ilhae_dir.join("brain").join("context").join("TEAM.md");
        std::fs::read_to_string(&team_md).unwrap_or_default().trim().to_string()
    };

    if !agents.is_empty() {
        return Some(TeamRuntimeConfig { team_prompt, agents });
    }

    // ── Fallback: legacy team.json ──
    let path = ilhae_dir.join("brain").join("settings").join("team_config.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let parsed: TeamConfigFile = serde_json::from_str(&raw).ok()?;

    for agent_cfg in &parsed.agents {
        let endpoint = agent_cfg.endpoint.trim().to_string();
        if !endpoint.is_empty() {
            agents.push(TeamRoleTarget {
                role: agent_cfg.role.trim().to_string(),
                endpoint,
                system_prompt: agent_cfg.system_prompt.trim().to_string(),
                engine: if agent_cfg.engine.trim().is_empty() { "gemini".to_string() } else { agent_cfg.engine.trim().to_lowercase() },
                skills: Vec::new(),
                mcp_servers: Vec::new(),
            });
        }
    }

    if agents.is_empty() {
        return None;
    }

    Some(TeamRuntimeConfig {
        team_prompt: if team_prompt.is_empty() { parsed.team_prompt.trim().to_string() } else { team_prompt },
        agents,
    })
}

#[derive(Debug, Clone)]
pub struct A2AResponseParsed {
    pub text: String,
    pub state: Option<String>,
    pub schedule_id: Option<String>,
    pub context_id: Option<String>,
}

pub fn extract_role_sections(text: &str) -> Vec<serde_json::Value> {
    let role_pattern = regex::Regex::new(r"(?m)^\*\*(\w+)\s*(?:\([^)]*\))?\s*:\*\*\s*(.*)$")
        .unwrap_or_else(|_| regex::Regex::new(r"^$").unwrap());

    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_role: Option<String> = None;
    let mut current_content = String::new();

    for line in text.lines() {
        if let Some(caps) = role_pattern.captures(line) {
            // Save previous section
            if let Some(role) = current_role.take()
                && !current_content.trim().is_empty() {
                    sections.push((role, current_content.trim().to_string()));
                }
            let role_name = caps.get(1).map(|m| m.as_str()).unwrap_or("Leader").to_string();
            let first_line = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            current_role = Some(role_name);
            current_content = first_line;
        } else if current_role.is_some() {
            current_content.push('\n');
            current_content.push_str(line);
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    // Flush last section
    if let Some(role) = current_role {
        if !current_content.trim().is_empty() {
            sections.push((role, current_content.trim().to_string()));
        }
    } else if !current_content.trim().is_empty() {
        // No role markers found — attribute everything to Leader
        sections.push(("Leader".to_string(), current_content.trim().to_string()));
    }

    sections
        .iter()
        .map(|(role, content)| json!({ "role": role, "content": content }))
        .collect()
}
