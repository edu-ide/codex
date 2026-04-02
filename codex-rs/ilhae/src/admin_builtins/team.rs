#[macro_export]
macro_rules! register_admin_team_handlers {
    ($builder:expr, $state:expr) => {{
        let s = $state.clone();
        $builder
            // ═══ Team List ═══
            .on_receive_request_from(sacp::Client, {
                let infra_ctx = s.infra_context().clone();
                let ilhae_dir = infra_ctx.ilhae_dir.clone();
                let vault_dir = infra_ctx.brain.vault_dir();
                async move |_req: TeamListRequest, responder: Responder<TeamListResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/team_list RPC");
                    // Read from brain/agents/*.md first, then fallback to team.json
                    let agents_dir = vault_dir.join("agents");
                    let mut agents = Vec::new();

                    if agents_dir.is_dir() {
                        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
                            let mut paths: Vec<_> = entries.flatten()
                                .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md") && e.path().is_file())
                                .collect();
                            paths.sort_by_key(|e| e.file_name());
                            for entry in paths {
                                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                                    if !content.starts_with("---\n") { continue; }
                                    let Some(end_idx) = content[4..].find("\n---\n") else { continue; };
                                    let yaml_str = &content[4..4 + end_idx];
                                    let body = content[4 + end_idx + 5..].trim().to_string();
                                    let role = entry.path().file_stem()
                                        .map(|s| s.to_string_lossy().to_string())
                                        .unwrap_or_default();

                                    let mut fm = std::collections::HashMap::new();
                                    for line in yaml_str.lines() {
                                        let line = line.trim();
                                        if line.is_empty() || line.starts_with('#') { continue; }
                                        if let Some((k, v)) = line.split_once(':') {
                                            fm.insert(k.trim().to_string(), v.trim().trim_matches('"').trim_matches('\'').to_string());
                                        }
                                    }

                                    let file_type = fm.get("type").map(|s| s.as_str()).unwrap_or("agent");
                                    if file_type != "agent" { continue; }
                                    let endpoint = fm.get("endpoint").map(|s| s.trim().to_string()).unwrap_or_default();
                                    if endpoint.is_empty() { continue; }

                                    // Parse list fields (skills, mcp_servers)
                                    let parse_list_field = |fm: &std::collections::HashMap<String, String>, key: &str| -> Vec<String> {
                                        let Some(val) = fm.get(key) else { return Vec::new() };
                                        let val = val.trim();
                                        if val.is_empty() { return Vec::new(); }
                                        let val = val.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(val);
                                        val.split(',').map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string()).filter(|s| !s.is_empty()).collect()
                                    };
                                    let skills = parse_list_field(&fm, "skills");
                                    let mcp_servers = parse_list_field(&fm, "mcp_servers");

                                    let is_main = fm.get("is_main").map(|s| s.trim().eq_ignore_ascii_case("true")).unwrap_or(false);

                                    agents.push(json!({
                                        "role": role,
                                        "endpoint": endpoint,
                                        "engine": fm.get("engine").map(|s| s.as_str()).unwrap_or("gemini"),
                                        "model": fm.get("model").map(|s| s.as_str()).unwrap_or(""),
                                        "color": fm.get("color").map(|s| s.as_str()).unwrap_or("#7c3aed"),
                                        "avatar": fm.get("avatar").map(|s| s.as_str()).unwrap_or("🤖"),
                                        "is_main": is_main,
                                        "system_prompt": body,
                                        "skills": skills,
                                        "mcp_servers": mcp_servers,
                                    }));
                                }
                            }
                        }
                    }

                    let team_prompt = {
                        let team_md = vault_dir.join("context").join("TEAM.md");
                        std::fs::read_to_string(&team_md).unwrap_or_default().trim().to_string()
                    };

                    if !agents.is_empty() {
                        let config = json!({
                            "agents": agents,
                            "team_prompt": if team_prompt.is_empty() { serde_json::Value::Null } else { json!(team_prompt) },
                            "auto_approve": true,
                        });
                        return responder.respond(TeamListResponse { config });
                    }

                    // Fallback: legacy team.json
                    let team_path = ilhae_dir.join("brain").join("settings").join("team_config.json");
                    let config = match std::fs::read_to_string(&team_path) {
                        Ok(s) => serde_json::from_str(&s).unwrap_or(crate::admin_proxy::default_team_config()),
                        Err(_) => crate::admin_proxy::default_team_config(),
                    };
                    responder.respond(TeamListResponse { config })
                }
            }, sacp::on_receive_request!())
            // ═══ Team Save ═══
            .on_receive_request_from(sacp::Client, {
                let infra_ctx = s.infra_context().clone();
                let team_state = s.team_state().clone();
                let ilhae_dir = infra_ctx.ilhae_dir.clone();
                let vault_dir_save = infra_ctx.brain.vault_dir();
                let settings_store = infra_ctx.settings_store.clone();
                let supervisor_handle = team_state.supervisor.clone();
                let agent_refresh_tx = infra_ctx.agent_refresh_tx.clone();
                let a2a_routing_map = team_state.a2a_routing_map.clone();
                async move |req: TeamSaveRequest, responder: Responder<TeamSaveResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/team_save RPC");

                    // Write agents to brain/agents/*.md
                    let agents_dir = vault_dir_save.join("agents");
                    let _ = std::fs::create_dir_all(&agents_dir);

                    // Collect existing md files to remove deleted agents
                    let mut existing_files: std::collections::HashSet<String> = std::collections::HashSet::new();
                    if let Ok(entries) = std::fs::read_dir(&agents_dir) {
                        for entry in entries.flatten() {
                            if entry.path().extension().and_then(|ext| ext.to_str()) == Some("md") {
                                if let Some(stem) = entry.path().file_stem().map(|s| s.to_string_lossy().to_string()) {
                                    existing_files.insert(stem);
                                }
                            }
                        }
                    }

                    let mut saved_roles = std::collections::HashSet::new();
                    if let Some(agents) = req.config.get("agents").and_then(|v| v.as_array()) {
                        for agent in agents {
                            let role = agent.get("role").and_then(|v| v.as_str()).unwrap_or("").trim();
                            if role.is_empty() { continue; }
                            let endpoint = agent.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").trim();
                            let engine = agent.get("engine").and_then(|v| v.as_str()).unwrap_or("gemini").trim();
                            let model = agent.get("model").and_then(|v| v.as_str()).unwrap_or("").trim();
                            let color = agent.get("color").and_then(|v| v.as_str()).unwrap_or("#7c3aed").trim();
                            let avatar = agent.get("avatar").and_then(|v| v.as_str()).unwrap_or("🤖").trim();
                            let system_prompt = agent.get("system_prompt").and_then(|v| v.as_str()).unwrap_or("").trim();
                            let skills: Vec<String> = agent.get("skills")
                                .and_then(|v| v.as_array())
                                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                                .unwrap_or_default();
                            let mcp_servers: Vec<String> = agent.get("mcp_servers")
                                .and_then(|v| v.as_array())
                                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                                .unwrap_or_default();

                            let skills_line = if skills.is_empty() { String::new() } else {
                                format!("skills: [{}]\n", skills.join(", "))
                            };
                            let mcp_line = if mcp_servers.is_empty() { String::new() } else {
                                format!("mcp_servers: [{}]\n", mcp_servers.join(", "))
                            };

                            let is_main = agent.get("is_main").and_then(|v| v.as_bool()).unwrap_or(false);
                            let is_main_line = if is_main { "is_main: true\n" } else { "" };

                            let md_content = format!(
                                "---\n{}type: agent\nendpoint: \"{}\"\nengine: {}\nmodel: \"{}\"\ncolor: \"{}\"\navatar: \"{}\"\n{}{}---\n\n{}\n",
                                is_main_line, endpoint, engine, model, color, avatar, skills_line, mcp_line, system_prompt
                            );

                            let file_path = agents_dir.join(format!("{}.md", role));
                            if let Err(e) = std::fs::write(&file_path, &md_content) {
                                warn!("team_save write agent {:?} error: {}", file_path, e);
                            }
                            saved_roles.insert(role.to_string());
                        }
                    }

                    // Remove agents that were deleted
                    for old_role in &existing_files {
                        if !saved_roles.contains(old_role) {
                            let old_path = agents_dir.join(format!("{}.md", old_role));
                            info!("[TeamSave] Removing deleted agent: {:?}", old_path);
                            let _ = std::fs::remove_file(&old_path);
                        }
                    }

                    // Write team prompt to brain/context/TEAM.md
                    if let Some(prompt) = req.config.get("team_prompt").and_then(|v| v.as_str()) {
                        let context_dir = vault_dir_save.join("context");
                        let _ = std::fs::create_dir_all(&context_dir);
                        let team_md = context_dir.join("TEAM.md");
                        let _ = std::fs::write(&team_md, prompt.trim());
                    }

                    // Also write legacy team.json for backward compatibility
                    let team_path = ilhae_dir.join("brain").join("settings").join("team_config.json");
                    let content = serde_json::to_string_pretty(&req.config).unwrap_or_default();
                    let _ = std::fs::write(&team_path, content);

                    // Hot-apply team config: kill changed processes, then re-spawn
                    if settings_store.get().agent.team_mode {
                        let dir = ilhae_dir.clone();
                        let sv = supervisor_handle.clone();
                        let settings_store_for_refresh = settings_store.clone();
                        let Some(team) = crate::context_proxy::load_team_runtime_config(&dir) else {
                            warn!("[TeamSave] team config invalid after save, skip hot spawn");
                            return responder.respond(TeamSaveResponse { ok: true });
                        };

                        let team_entries: Vec<(String, u16, String)> = team
                            .agents
                            .iter()
                            .filter_map(|a| {
                                crate::context_proxy::extract_port_from_endpoint(&a.endpoint)
                                    .map(|port| (a.role.clone(), port, a.engine.clone()))
                            })
                            .collect();

                        crate::process_supervisor::restart_team_agents(&sv, &team_entries).await;

                        let agent_count = team.agents.len();
                        info!("[TeamSave] Hot-spawning {} team A2A agents", agent_count);
                        let workspace_map = crate::context_proxy::generate_peer_registration_files(&team, None);
                        let _children = crate::context_proxy::spawn_team_a2a_servers(
                            &team,
                            &workspace_map,
                            None,
                            "team-save",
                        )
                        .await;
                        match crate::context_proxy::wait_for_all_team_health(&team).await {
                            Ok(()) => {
                                info!("[TeamSave] All {} team agents are healthy", agent_count);

                                // ── Update routing table for A2A proxy ──
                                if let Some(ref routing_map) = a2a_routing_map {
                                    crate::a2a_persistence::update_routing_map(routing_map, &team).await;
                                    info!("[TeamSave] A2A proxy routing table updated ({} agents)", agent_count);
                                }

                                settings_store_for_refresh.emit_current_value("agent.team_mode");
                                let _ = agent_refresh_tx.send(());
                            }
                            Err(e) => warn!("[TeamSave] Team health check failed: {}", e),
                        }
                    }
                    responder.respond(TeamSaveResponse { ok: true })
                }
            }, sacp::on_receive_request!())
            // ═══ Team Presets ═══
            .on_receive_request_from(sacp::Client, {
                async move |_req: TeamPresetsRequest, responder: Responder<TeamPresetsResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/team_presets RPC");
                    responder.respond(TeamPresetsResponse { presets: crate::admin_proxy::team_presets() })
                }
            }, sacp::on_receive_request!())
            // ═══ A2A Task Aggregation (fan-out) ═══
            .on_receive_request_from(sacp::Client, {
                let ilhae_dir = s.infra_context().ilhae_dir.clone();
                async move |req: ListA2ATasksRequest, responder: Responder<ListA2ATasksResponse>, cx: ConnectionTo<Conductor>| {
                    info!("ilhae/list_a2a_schedules RPC status_filter={:?}", req.status_filter);
                    let ilhae_dir = ilhae_dir.clone();
                    let status_filter = req.status_filter.clone();
                    cx.spawn(async move {
                        // Load team config to get agent endpoints
                        let team_path = ilhae_dir.join("brain").join("settings").join("team_config.json");
                        let config: serde_json::Value = match std::fs::read_to_string(&team_path) {
                            Ok(s) => serde_json::from_str(&s).unwrap_or(crate::admin_proxy::default_team_config()),
                            Err(_) => crate::admin_proxy::default_team_config(),
                        };

                        let agents = config.get("agents")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(5))
                            .build()
                            .unwrap_or_default();

                        // Fan-out: call A2A schedules/list on each agent in parallel
                        let mut handles = Vec::new();
                        for agent in &agents {
                            let role = agent.get("role").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                            let endpoint = agent.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            if endpoint.is_empty() { continue; }

                            let client = client.clone();
                            let status_filter = status_filter.clone();
                            handles.push(tokio::spawn(async move {
                                let body = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": 1,
                                    "method": "schedules/list",
                                    "params": {}
                                });
                                match client.post(&endpoint).json(&body).send().await {
                                    Ok(res) => {
                                        match res.json::<serde_json::Value>().await {
                                            Ok(json) => {
                                                let schedules = json.get("result")
                                                    .and_then(|r| r.get("schedules"))
                                                    .and_then(|t| t.as_array())
                                                    .cloned()
                                                    .unwrap_or_default();
                                                // Optional status filtering
                                                let filtered: Vec<serde_json::Value> = if let Some(ref filter) = status_filter {
                                                    schedules.into_iter().filter(|t| {
                                                        t.get("status").and_then(|s| s.get("state")).and_then(|s| s.as_str()) == Some(filter)
                                                    }).collect()
                                                } else {
                                                    schedules
                                                };
                                                let count = filtered.len();
                                                AgentTasksDto { role, endpoint, schedules: filtered, task_count: count, error: None }
                                            }
                                            Err(e) => AgentTasksDto { role, endpoint, schedules: vec![], task_count: 0, error: Some(e.to_string()) },
                                        }
                                    }
                                    Err(e) => AgentTasksDto { role, endpoint, schedules: vec![], task_count: 0, error: Some(e.to_string()) },
                                }
                            }));
                        }

                        let mut all_agents = Vec::new();
                        let mut total = 0;
                        for handle in handles {
                            match handle.await {
                                Ok(dto) => {
                                    total += dto.task_count;
                                    all_agents.push(dto);
                                }
                                Err(e) => {
                                    warn!("A2A task fan-out join error: {}", e);
                                }
                            }
                        }

                        responder.respond(ListA2ATasksResponse { agents: all_agents, total_schedules: total })
                    })
                }
            }, sacp::on_receive_request!())
            // ═══ A2A Timeline (from Session Store) ═══
            .on_receive_request_from(sacp::Client, {
                let brain = s.infra_context().brain.clone();
                async move |req: GetA2ATimelineRequest, responder: Responder<GetA2ATimelineResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/get_a2a_timeline RPC session={}", req.session_id);
                    let events = brain.session_get_a2a_timeline(&req.session_id).unwrap_or_else(|e| {
                        warn!("DB error loading timeline: {}", e);
                        vec![]
                    });
                    responder.respond(GetA2ATimelineResponse {
                        session_id: req.session_id,
                        events,
                    })
                }
            }, sacp::on_receive_request!())
            // ═══ A2A Card Fetch (server-side) ═══
            .on_receive_request_from(sacp::Client, {
                let settings_store = s.infra_context().settings_store.clone();
                let supervisor = s.team_state().supervisor.clone();
                async move |req: A2ACardRequest, responder: Responder<A2ACardResponse>, cx: ConnectionTo<Conductor>| {
                    let settings_store = settings_store.clone();
                    let supervisor = supervisor.clone();
                    info!("ilhae/a2a_card RPC endpoint={}", req.endpoint);
                    cx.spawn(async move {
                        let base = req.endpoint.trim_end_matches('/');
                        let urls = [
                            format!("{}/.well-known/agent.json", base),
                            format!("{}/.well-known/agent.json", base),
                        ];
                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(2))
                            .build()
                            .unwrap_or_default();
                        for url in &urls {
                            match client.get(url).header("Accept", "application/json").send().await {
                                Ok(res) if res.status().is_success() => {
                                    match res.json::<serde_json::Value>().await {
                                        Ok(card) => {
                                            return responder.respond(A2ACardResponse { card: Some(card) });
                                        }
                                        Err(e) => {
                                            warn!("a2a_card parse error for {}: {}", url, e);
                                        }
                                    }
                                }
                                Ok(res) => {
                                    warn!("a2a_card non-200 for {}: {}", url, res.status());
                                }
                                Err(e) => {
                                    warn!("a2a_card fetch error for {}: {}", url, e);
                                }
                            }
                        }

                        // If unreachable, attempt to auto-recover if it's a local endpoint.
                        // In team mode, the supervisor manages team agent lifecycle — skip auto-recovery.
                        if !settings_store.get().agent.team_mode {
                            let (host, port) = crate::parse_host_port(&base);
                            let is_local = matches!(host.as_str(), "127.0.0.1" | "localhost" | "0.0.0.0" | "::1");
                            if is_local {
                                warn!("A2A card unreachable on port {}, attempting to auto-recover...", port);
                                if let Err(e) = crate::process_supervisor::ensure_agent_healthy(&supervisor, port).await {
                                    warn!("Auto-recovery spawn failed: {}", e);
                                } else {
                                    info!("Auto-recovery spawn succeeded for port {}", port);
                                }
                            }
                        }

                        responder.respond(A2ACardResponse { card: None })
                    })
                }
            }, sacp::on_receive_request!())
    }};
}
