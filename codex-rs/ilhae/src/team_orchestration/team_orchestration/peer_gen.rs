//! Peer registration file generation for team A2A agents.
//!
//! Creates per-agent workspace directories with `.gemini/agents/{peer}.md`
//! files so that each agent's AgentRegistry auto-discovers its peers.
//! Also resolves the orch-team-mcp-server binary path.

use serde_json::Value;
use tracing::{info, warn};

use super::team_config::*;

/// Generate per-agent workspace directories with `.gemini/agents/{peer}.md` files.
/// Uses gemini-cli's `agentLoader.ts` frontmatter format so `AgentRegistry` auto-discovers
/// remote agents during `Config.initialize()`, giving each a2a-server A2A **client** capability.
///
/// Returns a map of role -> workspace_path so spawn can set CODER_AGENT_WORKSPACE_PATH.
pub fn generate_peer_registration_files(team: &TeamRuntimeConfig) -> std::collections::HashMap<String, std::path::PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let ilhae_dir = crate::config::resolve_ilhae_data_dir();
    let base_dir = ilhae_dir.join("team-workspaces");
    let ilhae_dir_str = ilhae_dir.to_string_lossy().to_string();
    let mut workspace_map = std::collections::HashMap::new();

    let source_gemini_dir = std::path::PathBuf::from(&home).join(".gemini");
    for agent in &team.agents {
        let my_name = agent.role.to_lowercase();
        let is_main = agent.is_main;
        // Each agent gets its own workspace: ~/ilhae/team-workspaces/{role}/
        let workspace = base_dir.join(&my_name);
        let gemini_dir = workspace.join(".gemini");
        let agents_dir = gemini_dir.join("agents");
        let _ = std::fs::create_dir_all(&agents_dir);

        // Seed auth + config files so isolated GEMINI_CLI_HOME can still authenticate.
        // Always overwrite auth tokens to keep them fresh (they expire within an hour).
        for file_name in ["oauth_creds.json", "google_accounts.json", "settings.json", "trustedFolders.json", "mcp-server-enablement.json"] {
            let src = source_gemini_dir.join(file_name);
            let dst = gemini_dir.join(file_name);
            if src.exists() {
                // DO NOT overwrite settings.json or mcp-server-enablement.json if they already exist,
                // so that per-agent isolated capabilities (MCP toggles) are preserved!
                let is_settings = file_name.ends_with(".json") && (file_name.starts_with("settings") || file_name.starts_with("mcp-server"));
                if is_settings && dst.exists() {
                    continue;
                }
                if let Err(e) = std::fs::copy(&src, &dst) {
                    warn!("[PeerGen] Failed to copy {:?} -> {:?}: {}", src, dst, e);
                }
            }
        }

        // Inject artifact MCP stdio server into settings.json
        // so each team agent can use artifact_save/artifact_list tools.
        let settings_path = gemini_dir.join("settings.json");
        if let Some(parent) = settings_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let settings_str = std::fs::read_to_string(&settings_path).unwrap_or_else(|_| "{}".to_string());
        
        let mut settings = match serde_json::from_str::<Value>(&settings_str) {
            Ok(s) => {
                if s.is_object() { s } else { serde_json::json!({}) }
            }
            Err(e) => {
                tracing::warn!("[PeerGen] Invalid JSON in settings.json, resetting for injection: {}", e);
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
            servers.clear(); // Remove user's broken servers from the test environment!
            
            // Resolve binary path: prefer target/debug, then target/release, then PATH
            let mcp_bin = resolve_brain_mcp_server_bin();
            servers.insert(
                "ilhae-tools".to_string(),
                serde_json::json!({
                    "command": mcp_bin,
                    "args": [],
                    "trust": true,
                    "env": {
                        "ILHAE_DIR": ilhae_dir_str,
                    }
                }),
            );
            if let Ok(updated) = serde_json::to_string_pretty(&settings) {
                if let Err(e) = std::fs::write(&settings_path, updated) {
                    tracing::error!("[PeerGen] Failed to write settings to {:?}: {}", settings_path, e);
                 } else {
                    tracing::info!("[PeerGen] {} -> injected ilhae-tools (orch-team-mcp-server) into settings.json", my_name);
                }
            }
        }

        // ── Governance: Role-based MCP tool enablement ──
        // Leader gets all tools; sub-agents get restricted by role
        let enablement_path = gemini_dir.join("mcp-server-enablement.json");
        let allowed_tools = role_based_tool_allowlist(&my_name, is_main);
        let enablement = serde_json::json!({
            "ilhae-tools": {
                "enabled": true,
                "allowedTools": allowed_tools
            }
        });
        if let Ok(content) = serde_json::to_string_pretty(&enablement) {
            let _ = std::fs::write(&enablement_path, &content);
            info!("[PeerGen] {} -> governance: {} tools allowed", my_name, allowed_tools.as_array().map(|a| a.len()).unwrap_or(0));
        }

        // ── Shared Context: team-wide shared_notes.json ──
        let shared_context_dir = base_dir.join("_shared");
        let _ = std::fs::create_dir_all(&shared_context_dir);
        let shared_notes_src = shared_context_dir.join("shared_notes.json");
        if !shared_notes_src.exists() {
            let _ = std::fs::write(&shared_notes_src, "{}");
        }
        let shared_notes_dst = workspace.join("shared_notes.json");
        // Symlink so all agents read/write the same file
        if !shared_notes_dst.exists() {
            let _ = std::os::unix::fs::symlink(&shared_notes_src, &shared_notes_dst);
            info!("[PeerGen] {} -> symlinked shared_notes.json", my_name);
        }
        let source_agents_dir = std::path::PathBuf::from(&home).join(".agents");
        let dest_agents_dir = workspace.join(".agents");
        if is_main {
            let _ = std::fs::remove_dir_all(&dest_agents_dir);
            let _ = std::fs::remove_dir_all(&agents_dir);
            let _ = std::fs::create_dir_all(&agents_dir);
        } else if source_agents_dir.exists() && !dest_agents_dir.exists() {
            let _ = std::process::Command::new("cp")
                .args(["-r", source_agents_dir.to_string_lossy().as_ref(), dest_agents_dir.to_string_lossy().as_ref()])
                .output();
        }

        // Create an individual .md file for each peer (agentLoader.ts format)
        let mut peer_specs: Vec<(String, String, String)> = Vec::new();
        if !is_main {
            for peer in &team.agents {
                let peer_name = peer.role.to_lowercase();
                if peer_name == my_name {
                    continue; // Skip self
                }
                let peer_desc = match peer_name.as_str() {
                    "leader" | "manager" => "팀 리더. 전체 계획 수립 및 최종 통합 담당. 작업 분배와 결과 종합을 요청할 수 있습니다.",
                    "researcher" => "자료 조사 전문가. 근거 수집, 사실 확인, 배경 조사를 위임할 수 있습니다.",
                    "verifier" | "reviewer" => "검증 전문가. 결과 검증, 정확성 확인, 리스크 분석을 위임할 수 있습니다.",
                    "creator" | "coder" => "콘텐츠 작성 전문가. 최종 답변 초안, 문서 작성, 코드 생성을 위임할 수 있습니다.",
                    _ => "팀 에이전트",
                };
                peer_specs.push((
                    peer_name.to_string(),
                    peer.endpoint.trim_end_matches('/').to_string(),
                    peer_desc.to_string(),
                ));
            }
        }

        for (peer_name, peer_endpoint, peer_desc) in peer_specs {
            let peer_file = agents_dir.join(format!("{}.md", peer_name));
            let content = format!(
                r#"---
name: {name}
kind: remote
description: "{desc}"
agent_card_url: "{endpoint}/.well-known/agent.json"
---

# {name}

{desc}

## 협업 가이드

- 이 에이전트는 A2A 프로토콜로 연결된 팀원입니다.
- 필요한 경우 이 에이전트를 tool로 직접 호출하여 작업을 위임하세요.
- 자신의 전문 영역이 아닌 작업이 필요할 때 적극적으로 다른 팀원에게 위임하세요.
- 여러 팀원에게 동시에 작업을 위임할 수 있습니다.

## 구조화된 인수인계 프로토콜

위임 시 다음 형식으로 query를 작성하면 context 유실을 방지합니다:
```
[HANDOFF]
FROM: {{자신의 role}}
SUMMARY: {{지금까지 한 작업 요약}}
FINDINGS:
- {{핵심 발견 1}}
- {{핵심 발견 2}}
REFERENCES: {{관련 파일/URL/세션ID}}
INSTRUCTIONS: {{구체적 요청사항}}
PRIORITY: normal
[/HANDOFF]
```

## 진행 보고 프로토콜

장시간 작업 시 중간 결과를 shared_notes.json에 기록하세요:
```json
{{"role": "{{자신의 role}}", "status": "in_progress", "percent": 50, "summary": "현재까지 결과 요약"}}
```
"#,
                name = peer_name,
                desc = peer_desc,
                endpoint = peer_endpoint
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
agent_card_url: "{endpoint}/.well-known/agent.json"
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
                endpoint = peer_endpoint
            );
            match std::fs::write(&codex_skill_file, &codex_content) {
                Ok(()) => info!("[PeerGen] {} -> codex skill file {:?}", my_name, codex_skill_file),
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
            "\n## Common Rules\n\
             - Teammates are registered as A2A agents in `.gemini/agents/`.\n\
             - Call them as tools to delegate work.\n\
             - Full-Mesh P2P: every agent can call every other agent directly.\n\
             - Actively delegate schedules outside your expertise.\n"
                .to_string()
        };

        let gemini_md_path = workspace.join(".gemini").join("GEMINI.md");
        let gemini_md_content = format!(
            r#"# Team Agent: {role}

{instruction}
{protocol}
{collaboration_surface}
- When creating artifacts (task.md, implementation_plan.md, walkthrough.md), use the `artifact_save` MCP tool. It handles file paths and versioning automatically.
{artifact_rules}

## A2A Communication Methodologies
When calling another agent via tools, use these modes appropriately:
1. Synchronous (async: false): Use when you need the result immediately to proceed with your logic. You will wait for the result.
2. Fire-and-forget (async: true, subscribe: false): Use when you want to trigger a background task and do not need to know when it finishes or what the result is.
3. Pub/Sub (async: true, subscribe: true): Use for long-running schedules. You will delegate the task and then immediately finish your turn (go to sleep). When the sub-agent finishes, the system will wake you up with a `[System Alert]` containing the result, allowing you to synthesize the final answer.
4. If the request explicitly says background, async, subscribe, alert, wake up later, or long-running, you MUST choose the background/subscribe-capable delegation mode rather than synchronous waiting.
5. In those cases, do not block the same turn waiting for the final answer. Start the task, then rely on the follow-up alert/update to continue the orchestration.
"#,
            role = my_name,
            instruction = role_instruction,
            protocol = team_protocol_section,
            collaboration_surface = collaboration_surface_section,
            artifact_rules = crate::ARTIFACT_RULES_SHORT,
        );
        match std::fs::write(&gemini_md_path, &gemini_md_content) {
            Ok(()) => info!("[PeerGen] {} -> GEMINI.md {:?}", my_name, gemini_md_path),
            Err(e) => warn!("[PeerGen] Failed to write GEMINI.md {:?}: {}", gemini_md_path, e),
        }

        workspace_map.insert(my_name.to_string(), workspace);
    }

    workspace_map
}

// ── Governance: Role-based MCP tool allow-list ───────────────────────────

/// Return a JSON array of allowed MCP tool names for the given role.
/// Leader/main agents get all tools ("*"); sub-agents get restricted sets.
fn role_based_tool_allowlist(role: &str, is_main: bool) -> serde_json::Value {
    use serde_json::json;

    if is_main {
        // Leader: unrestricted — all tools available
        return json!(["*"]);
    }

    match role {
        "researcher" => json!([
            "memory_search", "memory_read", "knowledge_search",
            "session_recall", "artifact_list",
            "web_search", "read_url"
        ]),
        "creator" | "coder" => json!([
            "artifact_save", "artifact_edit", "artifact_list",
            "memory_write", "memory_read",
            "write_file", "read_file"
        ]),
        "verifier" | "reviewer" => json!([
            "memory_read", "memory_search", "knowledge_search",
            "artifact_list", "session_recall",
            "read_file"
        ]),
        _ => json!(["*"]), // Unknown roles get full access as safety fallback
    }
}

/// Resolve the orch-team-mcp-server binary path.
pub fn resolve_brain_mcp_server_bin() -> String {
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
            let candidate = cur.join("orch-team-mcp-server");
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
            let candidate_release = cur.parent().unwrap_or(cur).join("release").join("orch-team-mcp-server");
            if candidate_release.exists() {
                return candidate_release.to_string_lossy().to_string();
            }
            let candidate_debug = cur.parent().unwrap_or(cur).join("debug").join("orch-team-mcp-server");
            if candidate_debug.exists() {
                return candidate_debug.to_string_lossy().to_string();
            }
        }
    }
    
    // Fallback to searching from CWD
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
    "orch-team-mcp-server".to_string()
}

/// Discovered peer info from runtime Agent Card fetch.
#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    pub role: String,
    pub endpoint: String,
    pub name: String,
    pub description: String,
    pub skills: Vec<String>,
    pub supported_modes: Vec<String>,
}

/// Fetch Agent Cards from all team endpoints and refresh peer files if capabilities changed.
/// Returns the number of peers whose capabilities were updated.
pub async fn discover_and_refresh_peers(
    team: &TeamRuntimeConfig,
    supervisor_handle: &crate::process_supervisor::SupervisorHandle,
) -> usize {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("[PeerDiscovery] Failed to create HTTP client: {}", e);
            return 0;
        }
    };

    let mut updated_count = 0;

    for agent in &team.agents {
        let endpoint = agent.endpoint.trim_end_matches('/');
        let card_url = format!("{}/.well-known/agent.json", endpoint);

        let card: serde_json::Value = match client
            .get(&card_url)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(res) if res.status().is_success() => {
                match res.json().await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("[PeerDiscovery] Failed to parse card for {}: {}", agent.role, e);
                        continue;
                    }
                }
            }
            Ok(res) => {
                warn!("[PeerDiscovery] Non-200 from {} card: {}", agent.role, res.status());
                continue;
            }
            Err(e) => {
                // Agent not reachable — skip silently (supervisor handles restart)
                tracing::trace!("[PeerDiscovery] {} unreachable: {}", agent.role, e);
                continue;
            }
        };

        // Compare with cached card in supervisor
        let card_changed = {
            let sv = supervisor_handle.read().await;
            let key = format!("team-{}", agent.role.to_lowercase());
            match sv.processes.get(&key) {
                Some(proc) => {
                    proc.cached_agent_card.as_ref() != Some(&card)
                }
                None => true,
            }
        };

        if card_changed {
            info!(
                "[PeerDiscovery] Agent Card changed for '{}', updating cache",
                agent.role
            );

            // Update cache in supervisor
            {
                let mut sv = supervisor_handle.write().await;
                let key = format!("team-{}", agent.role.to_lowercase());
                if let Some(proc) = sv.processes.get_mut(&key) {
                    proc.cached_agent_card = Some(card.clone());
                    proc.card_last_fetched = Some(std::time::Instant::now());
                }
            }

            updated_count += 1;
        }
    }

    // If any cards changed, regenerate peer registration files
    if updated_count > 0 {
        info!(
            "[PeerDiscovery] {} agent card(s) changed, regenerating peer files",
            updated_count
        );
        let _ = generate_peer_registration_files(team);
    }

    updated_count
}
