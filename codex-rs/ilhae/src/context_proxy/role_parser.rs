use regex::Regex;
use serde_json::json;
use tracing::{info, warn};

use crate::context_proxy::routing::*;
#[allow(unused_imports)]
use crate::context_proxy::team_a2a::*;

fn resolve_team_mcp_server_bin() -> String {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(target_dir) = exe_path.parent() {
            let mut cur = target_dir;
            while cur.file_name().and_then(|n| n.to_str()) == Some("deps") {
                if let Some(parent) = cur.parent() {
                    cur = parent;
                } else {
                    break;
                }
            }
            for candidate in [
                cur.join("orch-team-mcp-server"),
                cur.parent()
                    .unwrap_or(cur)
                    .join("release")
                    .join("orch-team-mcp-server"),
                cur.parent()
                    .unwrap_or(cur)
                    .join("debug")
                    .join("orch-team-mcp-server"),
            ] {
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let mut cur: Option<&std::path::Path> = Some(cwd.as_path());
        while let Some(dir) = cur {
            for subpath in [
                "orch-team-mcp-server/target/debug/orch-team-mcp-server",
                "orch-team-mcp-server/target/release/orch-team-mcp-server",
                "target/debug/orch-team-mcp-server",
                "target/release/orch-team-mcp-server",
            ] {
                let candidate = dir.join(subpath);
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
            cur = dir.parent();
        }
    }

    {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let service_root = manifest_dir
            .parent()
            .map(|path| path.to_path_buf())
            .unwrap_or(manifest_dir.clone());
        for candidate in [
            service_root
                .join("orch-team-mcp-server")
                .join("target")
                .join("debug")
                .join("orch-team-mcp-server"),
            service_root
                .join("orch-team-mcp-server")
                .join("target")
                .join("release")
                .join("orch-team-mcp-server"),
        ] {
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }

    "orch-team-mcp-server".to_string()
}

/// Extract role-based sections from the Leader's aggregated response.
/// The Leader may structure its response with role markers like:
///   **Leader (계획):** ...
///   **Researcher (자료 조사):** ...
/// If no such markers are found, the entire response is attributed to Leader.
pub fn extract_role_sections(text: &str) -> Vec<serde_json::Value> {
    let role_pattern = regex::Regex::new(r"(?m)^\*\*(\w+)\s*(?:\([^)]*\))?\s*:\*\*\s*(.*)$")
        .unwrap_or_else(|_| regex::Regex::new(r"^$").unwrap());

    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_role: Option<String> = None;
    let mut current_content = String::new();

    for line in text.lines() {
        if let Some(caps) = role_pattern.captures(line) {
            // Save previous section
            if let Some(role) = current_role.take() {
                if !current_content.trim().is_empty() {
                    sections.push((role, current_content.trim().to_string()));
                }
            }
            let role_name = caps
                .get(1)
                .map(|m| m.as_str())
                .unwrap_or("Leader")
                .to_string();
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

/// Generate per-agent workspace directories with `.gemini/agents/{peer}.md` files.
/// Uses gemini-cli's `agentLoader.ts` frontmatter format so `AgentRegistry` auto-discovers
/// remote agents during `Config.initialize()`, giving each a2a-server A2A **client** capability.
///
/// Returns a map of role -> workspace_path so spawn can set CODER_AGENT_WORKSPACE_PATH.
pub fn generate_peer_registration_files(
    team: &TeamRuntimeConfig,
    proxy_base_url: Option<&str>,
) -> std::collections::HashMap<String, std::path::PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let ilhae_dir = crate::config::resolve_ilhae_data_dir();
    let base_dir = ilhae_dir.join("team-workspaces");
    let mut workspace_map = std::collections::HashMap::new();

    let source_gemini_dir = std::path::PathBuf::from(&home).join(".gemini");
    for agent in &team.agents {
        let my_name = agent.role.to_lowercase();
        // Each agent gets its own workspace: ~/ilhae/team-workspaces/{role}/
        let workspace = base_dir.join(&my_name);
        let gemini_dir = workspace.join(".gemini");
        let agents_dir = gemini_dir.join("agents");
        let _ = std::fs::create_dir_all(&agents_dir);
        let is_main = agent.is_main;

        // Seed auth + config files so isolated GEMINI_CLI_HOME can still authenticate.
        // Always overwrite auth tokens to keep them fresh (they expire within an hour).
        for file_name in [
            "oauth_creds.json",
            "google_accounts.json",
            "settings.json",
            "trustedFolders.json",
            "mcp-server-enablement.json",
        ] {
            let src = source_gemini_dir.join(file_name);
            let dst = gemini_dir.join(file_name);
            if src.exists() {
                // DO NOT overwrite settings.json or mcp-server-enablement.json if they already exist,
                // so that per-agent isolated capabilities (MCP toggles) are preserved!
                let is_settings = file_name.ends_with(".json")
                    && (file_name.starts_with("settings") || file_name.starts_with("mcp-server"));
                if is_settings && dst.exists() {
                    continue;
                }
                if let Err(e) = std::fs::copy(&src, &dst) {
                    warn!("[PeerGen] Failed to copy {:?} -> {:?}: {}", src, dst, e);
                }
            }
        }

        // Provision Superpowers skill files into brain/skills/ if not already present
        crate::superpowers_skills::provision_superpowers_skills();

        // Copy .agents/skills only for sub-agents.
        // The main Leader should not see peer role skills or generic global agent skills;
        // team delegation must go through `team-tools` MCP surface.
        let source_agents_dir = std::path::PathBuf::from(&home).join(".agents");
        let dest_agents_dir = workspace.join(".agents");
        if is_main {
            let _ = std::fs::remove_dir_all(&dest_agents_dir);
            let _ = std::fs::remove_dir_all(&agents_dir);
            let _ = std::fs::create_dir_all(&agents_dir);
        } else if source_agents_dir.exists() && !dest_agents_dir.exists() {
            let _ = std::process::Command::new("cp")
                .args([
                    "-r",
                    source_agents_dir.to_string_lossy().as_ref(),
                    dest_agents_dir.to_string_lossy().as_ref(),
                ])
                .output();
        }

        // Inject ilhae-tools MCP server into settings.json, except for user_agent.
        // user_agent must return plain-text next directives only; giving it tool surface
        // causes it to delegate instead of steering.
        let settings_path = gemini_dir.join("settings.json");
        let settings_str =
            std::fs::read_to_string(&settings_path).unwrap_or_else(|_| "{}".to_string());

        let mut settings = match serde_json::from_str::<serde_json::Value>(&settings_str) {
            Ok(s) => {
                if s.is_object() {
                    s
                } else {
                    serde_json::json!({})
                }
            }
            Err(e) => {
                tracing::warn!(
                    "[PeerGen] Invalid JSON in settings.json, resetting for injection: {}",
                    e
                );
                serde_json::json!({})
            }
        };

        let mcp_servers = settings
            .as_object_mut()
            .and_then(|o| {
                o.entry("mcpServers")
                    .or_insert_with(|| serde_json::json!({}));
                o.get_mut("mcpServers")
            })
            .and_then(|v| v.as_object_mut());

        if let Some(servers) = mcp_servers {
            servers.clear();

            if my_name != "user_agent" {
                let mcp_bin = resolve_team_mcp_server_bin();
                servers.insert(
                    "ilhae-tools".to_string(),
                    serde_json::json!({
                        "command": mcp_bin,
                        "args": [],
                        "trust": true,
                        "env": {
                            "ILHAE_DIR": ilhae_dir.to_string_lossy().to_string(),
                        }
                    }),
                );
                tracing::info!(
                    "[PeerGen] {} -> injected ilhae-tools (orch-team-mcp-server) into settings.json",
                    my_name
                );
            } else {
                tracing::info!(
                    "[PeerGen] {} -> cleared MCP servers for plain-text autonomous directives",
                    my_name
                );
            }

            if let Ok(updated) = serde_json::to_string_pretty(&settings) {
                if let Err(e) = std::fs::write(&settings_path, updated) {
                    tracing::error!(
                        "[PeerGen] Failed to write settings to {:?}: {}",
                        settings_path,
                        e
                    );
                }
            }
        }

        // Create individual peer files only for sub-agents.
        // The Leader must delegate via team-tools, not directly via peer agent files.
        let mut peer_specs: Vec<(String, String, String, String)> = Vec::new();
        if !is_main && my_name != "user_agent" {
            for peer in &team.agents {
                let peer_name = peer.role.to_lowercase();
                if peer_name == my_name {
                    continue; // Skip self
                }
                let peer_desc: String = if !peer.system_prompt.trim().is_empty() {
                    let first_line = peer
                        .system_prompt
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or(&peer.system_prompt)
                        .trim();
                    if first_line.len() > 100 {
                        format!("{}…", &first_line[..100])
                    } else {
                        first_line.to_string()
                    }
                } else {
                    format!("팀 에이전트 ({})", peer_name)
                };
                let effective_endpoint = if let Some(proxy_url) = proxy_base_url {
                    format!("{}/a2a/{}", proxy_url.trim_end_matches('/'), peer_name)
                } else {
                    peer.endpoint.trim_end_matches('/').to_string()
                };
                let agent_card_url = if let Some(proxy_url) = proxy_base_url {
                    format!(
                        "{}/a2a/{}/.well-known/agent.json",
                        proxy_url.trim_end_matches('/'),
                        peer_name
                    )
                } else {
                    format!(
                        "{}/.well-known/agent.json",
                        peer.endpoint.trim_end_matches('/')
                    )
                };
                peer_specs.push((
                    peer_name.to_string(),
                    effective_endpoint,
                    peer_desc.to_string(),
                    agent_card_url,
                ));
            }
        }

        for (peer_name, _peer_endpoint, peer_desc, peer_card_url) in peer_specs {
            let peer_file = agents_dir.join(format!("{}.md", peer_name));
            let content = format!(
                r#"---
name: {name}
description: {desc}
kind: remote
agent_card_url: "{card_url}"
---

# {name}

{desc}

## 협업 가이드

- 이 에이전트는 A2A 프로토콜로 연결된 팀원입니다.
- 필요한 경우 이 에이전트를 tool로 직접 호출하여 작업을 위임하세요.
- 자신의 전문 영역이 아닌 작업이 필요할 때 적극적으로 다른 팀원에게 위임하세요.
- 여러 팀원에게 동시에 작업을 위임할 수 있습니다.
"#,
                name = peer_name,
                desc = peer_desc,
                card_url = peer_card_url
            );
            match std::fs::write(&peer_file, &content) {
                Ok(()) => info!("[PeerGen] {} -> peer file {:?}", my_name, peer_file),
                Err(e) => warn!("[PeerGen] Failed to write {:?}: {}", peer_file, e),
            }

            // Also generate a SKILL.md for Codex compatibility
            let codex_skill_dir = dest_agents_dir.join("skills").join(&peer_name);
            let _ = std::fs::create_dir_all(&codex_skill_dir);
            let codex_skill_file = codex_skill_dir.join("SKILL.md");
            let codex_content = format!(
                r#"---
name: {name}
kind: remote
agent_card_url: "{card_url}"
description: "{desc}"
metadata:
  short-description: "{desc}"
interface:
  display_name: {name}
---

# {name}

{desc}

## 협업 가이드
- 이 에이전트는 A2A 프로토콜로 연결된 팀원입니다.
- 필요한 경우 이 에이전트를 tool로 직접 호출하여 작업을 위임하세요.
"#,
                name = peer_name,
                desc = peer_desc,
                card_url = peer_card_url
            );
            match std::fs::write(&codex_skill_file, &codex_content) {
                Ok(()) => info!(
                    "[PeerGen] {} -> codex skill file {:?}",
                    my_name, codex_skill_file
                ),
                Err(e) => warn!("[PeerGen] Failed to write {:?}: {}", codex_skill_file, e),
            }
        }

        // Create GEMINI.md with the preset's system_prompt for this agent.
        // For the Leader, also inject the team_prompt (delegation protocol) so it
        // follows the mandatory Researcher→Verifier→Creator pipeline.
        let role_instruction = if agent.system_prompt.trim().is_empty() {
            format!("You are the {} agent on this team.", my_name)
        } else {
            agent.system_prompt.clone()
        };

        let team_protocol_section = if is_main && !team.team_prompt.trim().is_empty() {
            format!(
                "\n## Delegation Protocol (MANDATORY)\n\n{}\n",
                team.team_prompt.trim()
            )
        } else {
            String::new()
        };
        let collaboration_surface_section = if is_main {
            "\n## Team Coordination Surface (CRITICAL)\n\
             - You are the Leader. Use ONLY the `team-tools` MCP tools for teammate coordination.\n\
             - Valid team delegation tools are: `delegate`, `delegate_background`, and `propose`.\n\
             - DO NOT call teammate skills or remote agent tools directly.\n\
             - DO NOT use `activate_skill` for team roles such as researcher, verifier, creator, creator_1, creator_2, or leader.\n\
             - DO NOT use `generalist`, `cli_help`, or `codebase_investigator` as substitutes for team delegation.\n\
             - When the user asks to use Researcher/Verifier/Creator in natural language, convert that intent into a `team-tools` delegation.\n"
                .to_string()
        } else {
            "\n## Team Coordination Surface\n\
             - Teammates are registered as A2A agents in `.gemini/agents/`.\n\
             - Call them as tools to delegate work.\n\
             - Full-Mesh P2P: every agent can call every other agent directly.\n\
             - Actively delegate schedules outside your expertise.\n"
                .to_string()
        };

        // ── Read brain/memory/global/*.md and inject into GEMINI.md ──
        let global_context = {
            let global_dir = std::path::PathBuf::from(&home)
                .join(".ilhae")
                .join("brain")
                .join("memory")
                .join("global");
            let mut sections = String::new();
            if global_dir.is_dir() {
                // Read files in deterministic order
                let mut entries: Vec<_> = std::fs::read_dir(&global_dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
                    .collect();
                entries.sort_by_key(|e| e.file_name());
                for entry in entries {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        let fname = entry.file_name().to_string_lossy().to_string();
                        sections.push_str(&format!(
                            "\n## [Global Context: {}]\n\n{}\n",
                            fname.trim_end_matches(".md"),
                            content.trim()
                        ));
                    }
                }
            }
            sections
        };

        let gemini_md_path = workspace.join(".gemini").join("GEMINI.md");
        let gemini_md_content = format!(
            r#"# Team Agent: {role}

{instruction}
{protocol}
## Common Rules
{collaboration_surface}

## Artifact System (MANDATORY)
You MUST automatically create and maintain the following artifact files during every task:

### 1. task.md — Task Checklist
- **When:** At the very start of any non-trivial task (more than a simple question/answer).
- **What:** A detailed checklist to organize your work. Use `[ ]` for incomplete, `[/]` for in-progress, `[x]` for completed items.
- **Format:**
  ```yaml
  ---
  tags: [artifact, task, checklist]
  status: in_progress
  ---
  ```
  followed by a markdown checklist.
- **Maintain:** Update items as `[/]` when starting and `[x]` when done.

### 2. implementation_plan.md — Technical Plan
- **When:** Before starting complex code changes (new features, refactoring, multi-file edits).
- **What:** A structured technical design document.
- **Format:**
  ```yaml
  ---
  tags: [artifact, plan, implementation]
  status: draft
  ---
  ```
  Include: Goal, Proposed Changes (grouped by component), Verification Plan.
- **Skip:** For simple bug fixes, single-line changes, or questions.

### 3. walkthrough.md — Completion Summary
- **When:** After completing significant work.
- **What:** Summary of what was accomplished, what was tested, and validation results.
- **Format:**
  ```yaml
  ---
  tags: [artifact, walkthrough, summary]
  status: complete
  ---
  ```

### Rules
{}
- **Always include YAML frontmatter** with tags and status.
- **Minimum requirement:** `task.md` must be created for every non-trivial task.
- `implementation_plan.md` is required for complex changes only.
- `walkthrough.md` is required when work involves significant code changes.

## Obsidian Vault Integration (MANDATORY)
- You have access to an Obsidian-style Knowledge Graph Vault via the `vault_` tools.
- **Always** wrap important concepts, project names, or related topics in `[[wikilinks]]` when writing notes or responding to the user. This builds the necessary knowledge graph.
- For daily status updates, task completions, or chronological logs, use `vault_append_daily_note` to append to today's note.
- For persisting permanent knowledge, use `vault_write_note`. Use `vault_read_note` and `vault_search_notes` to retrieve context before answering.

## Superpowers Workflow (On-Demand Skills)
아래 스킬들은 `brain/skills/`에서 필요할 때 불러올 수 있습니다. `skills_list`로 목록을 보고 `skill_view`로 필요한 스킬만 로드하세요:
1. **brainstorming** — 코드 작성 전 반드시 이 스킬을 참조하여 사용자와 대화를 통해 설계를 먼저 합니다.
2. **writing-plans** — 설계 승인 후 구체적인 bite-sized 실행 계획을 작성합니다.
3. **executing-plans** — 계획에 따라 배치 실행 및 체크포인트 리뷰를 진행합니다.
4. **verification-before-completion** — 작업 완료 선언 전 반드시 실제 명령 실행 결과로 검증 증거를 확보합니다.
5. **subagent-driven-development** — 독립적인 태스크를 서브에이전트에 위임하고 2단계(spec → quality) 코드 리뷰를 거칩니다.

## A2A Communication Methodologies
When calling another agent via tools, use these modes appropriately:
1. Synchronous (async: false): Use when you need the result immediately to proceed with your logic. You will wait for the result.
2. Fire-and-forget (async: true, subscribe: false): Use when you want to trigger a background task and do not need to know when it finishes or what the result is.
3. Pub/Sub (async: true, subscribe: true): Use for long-running schedules. You will delegate the task and then immediately finish your turn (go to sleep). When the sub-agent finishes, the system will wake you up with a `[System Alert]` containing the result, allowing you to synthesize the final answer.
4. If the user explicitly asks for background, async, subscribe, alert, wake-up-on-completion, or long-running delegation, you MUST choose the background/subscribe-capable delegation tool instead of synchronous delegation.
5. If the user also wants the completed result in this same conversation, call `subscribe_task(task_id)` (or the equivalent await/subscribe tool) immediately after `delegate_background` using the returned task id.
{global_context}
\"#,
            crate::ARTIFACT_RULES_SHORT,
            role = my_name,
            instruction = role_instruction,
            protocol = team_protocol_section,
            collaboration_surface = collaboration_surface_section,
            global_context = global_context,
        );
        match std::fs::write(&gemini_md_path, &gemini_md_content) {
            Ok(()) => info!("[PeerGen] {} -> GEMINI.md {:?}", my_name, gemini_md_path),
            Err(e) => warn!(
                "[PeerGen] Failed to write GEMINI.md {:?}: {}",
                gemini_md_path, e
            ),
        }

        workspace_map.insert(my_name.to_string(), workspace);
    }

    workspace_map
}

pub fn normalize_team_role(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "leader" => Some("Leader"),
        "researcher" => Some("Researcher"),
        "verifier" => Some("Verifier"),
        "creator" => Some("Creator"),
        _ => None,
    }
}

pub fn extract_team_role_sections(text: &str) -> Vec<(String, String)> {
    let heading_re = match Regex::new(
        r"(?i)^\s*[\p{P}\p{S}\d\s]*(leader|researcher|verifier|creator)\b(?:\s*\([^\n)]*\))?\s*:?\s*\*{0,2}\s*(.*)$",
    ) {
        Ok(v) => v,
        Err(e) => {
            warn!("[TeamSplit] failed to compile heading regex: {}", e);
            return Vec::new();
        }
    };

    let lines: Vec<&str> = text.lines().collect();
    let mut headers: Vec<(usize, String, String)> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let Some(cap) = heading_re.captures(line) else {
            continue;
        };
        let Some(role_raw) = cap.get(1).map(|m| m.as_str()) else {
            continue;
        };
        let Some(role) = normalize_team_role(role_raw) else {
            continue;
        };
        let mut inline = cap
            .get(2)
            .map(|m| {
                m.as_str()
                    .trim()
                    .trim_start_matches(|ch: char| {
                        matches!(
                            ch,
                            ']' | ')' | '>' | '}' | ':' | '-' | '–' | '—' | '·' | '•' | '|'
                        )
                    })
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();
        if let Some(idx) = inline.find(']') {
            let before = inline[..idx].trim();
            if idx == 0 || (!before.contains(' ') && before.chars().count() <= 12) {
                inline = inline[idx + 1..].trim().to_string();
            }
        }
        headers.push((idx, role.to_string(), inline));
    }
    if headers.len() < 2 {
        return Vec::new();
    }

    let mut sections: Vec<(String, String)> = Vec::new();
    for (i, (line_idx, role, inline)) in headers.iter().enumerate() {
        let next_line_idx = headers.get(i + 1).map(|h| h.0).unwrap_or(lines.len());
        let mut body_parts: Vec<String> = Vec::new();
        if !inline.trim().is_empty() {
            body_parts.push(inline.trim().to_string());
        }
        for line in lines.iter().take(next_line_idx).skip(line_idx + 1) {
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                body_parts.push(trimmed.to_string());
            }
        }
        let body = body_parts
            .join("\n")
            .trim()
            .trim_matches('*')
            .trim()
            .to_string();
        if body.is_empty() {
            continue;
        }
        if let Some((_, existing_body)) = sections.iter_mut().find(|(r, _)| r == role) {
            if !existing_body.is_empty() {
                existing_body.push_str("\n\n");
            }
            existing_body.push_str(&body);
        } else {
            sections.push((role.clone(), body));
        }
    }
    sections
}

pub fn extract_a2a_role_events(text: &str) -> Vec<(String, String)> {
    let line_re = match Regex::new(r"^\s*\[([A-Za-z][A-Za-z0-9_-]{1,31})\]\s*(.+?)\s*$") {
        Ok(v) => v,
        Err(e) => {
            warn!("[TeamSplit] failed to compile a2a event regex: {}", e);
            return Vec::new();
        }
    };

    text.lines()
        .filter_map(|line| {
            let cap = line_re.captures(line)?;
            let role = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            let msg = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            if role.is_empty() || msg.is_empty() {
                return None;
            }
            Some((role.to_string(), msg.to_string()))
        })
        .collect()
}

pub fn upsert_role_section(sections: &mut Vec<(String, String)>, role: String, content: String) {
    if let Some((_, existing)) = sections.iter_mut().find(|(r, _)| *r == role) {
        if !existing.is_empty() {
            existing.push_str("\n\n");
        }
        existing.push_str(content.trim());
    } else {
        sections.push((role, content.trim().to_string()));
    }
}

pub fn sanitize_role_section_content(role: &str, content: &str) -> String {
    let mut out = content.replace("\r\n", "\n").trim().to_string();
    if out.is_empty() {
        return out;
    }

    let role_escaped = regex::escape(role);
    let mut patterns = Vec::with_capacity(3);
    for raw in [
        format!(r"(?is)^\s*\[\s*{}\s*\]\s*", role_escaped),
        format!(r"(?is)^\s*{}\s*\]\s*", role_escaped),
        format!(
            r"(?is)^\s*\*{{0,2}}\s*{}(?:\s*\([^\n)]*\))?\s*:?\s*\*{{0,2}}\s*",
            role_escaped
        ),
    ] {
        if let Ok(re) = Regex::new(&raw) {
            patterns.push(re);
        }
    }

    for _ in 0..6 {
        let mut changed = false;
        for re in &patterns {
            if re.is_match(&out) {
                out = re.replace(&out, "").to_string().trim_start().to_string();
                changed = true;
            }
        }
        let trimmed = out
            .trim_start_matches(|ch: char| {
                matches!(
                    ch,
                    ']' | ')' | '}' | '>' | ':' | '-' | '–' | '—' | '•' | '·' | '|'
                )
            })
            .trim_start()
            .to_string();
        if trimmed != out {
            out = trimmed;
            changed = true;
        }
        if !changed {
            break;
        }
    }

    out.trim().to_string()
}

pub fn normalize_role_sections(sections: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (role, content) in sections {
        let cleaned = sanitize_role_section_content(&role, &content);
        if cleaned.is_empty() {
            continue;
        }
        upsert_role_section(&mut out, role, cleaned);
    }
    out
}

pub fn has_malformed_leading_role_markers(sections: &[(String, String)]) -> bool {
    let heading_like = Regex::new(r"(?i)^\s*\[(leader|researcher|verifier|creator)\b").ok();
    sections.iter().any(|(_, body)| {
        let trimmed = body.trim_start();
        trimmed.starts_with(']')
            || heading_like
                .as_ref()
                .map(|re| re.is_match(trimmed))
                .unwrap_or(false)
    })
}

pub fn extract_team_role_sections_from_structured(
    _structured: Option<&serde_json::Value>,
    _role_names: &[String],
    events: &mut Vec<serde_json::Value>,
) {
    let items = _structured
        .and_then(|s| s.get("role_sections"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let role_raw = obj
            .get("role")
            .or_else(|| obj.get("team_role"))
            .or_else(|| obj.get("agent_role"))
            .or_else(|| obj.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let Some(role) = normalize_team_role(role_raw).map(str::to_string) else {
            continue;
        };
        let content = obj
            .get("content")
            .or_else(|| obj.get("text"))
            .or_else(|| obj.get("body"))
            .or_else(|| obj.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if content.is_empty() {
            continue;
        }
        events.push(serde_json::json!({
            "source_role": role,
            "message": content
        }));
    }
}

pub fn extract_a2a_role_events_from_structured(
    structured: &serde_json::Value,
) -> Vec<(String, String)> {
    let mut events = Vec::new();
    let items = structured
        .get("events")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let source_role = obj
            .get("source_role")
            .or_else(|| obj.get("role"))
            .or_else(|| obj.get("agent"))
            .or_else(|| obj.get("from"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let message = obj
            .get("message")
            .or_else(|| obj.get("content"))
            .or_else(|| obj.get("text"))
            .or_else(|| obj.get("body"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let message = sanitize_role_section_content(&source_role, &message);
        if source_role.eq_ignore_ascii_case("agent") && looks_like_aggregated_team_payload(&message)
        {
            continue;
        }
        if looks_like_verbose_role_event_payload(&source_role, &message) {
            continue;
        }
        if source_role.is_empty() || message.is_empty() {
            continue;
        }
        events.push((source_role, message));
    }
    events
}
