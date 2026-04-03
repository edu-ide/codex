// commands

use serde_json::json;

pub async fn handle_team_presets(
    _ctx: &crate::SharedState,
    cmd: &crate::relay_server::RelayCommand,
    client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    // Use built-in presets from admin_proxy
    let presets = crate::admin_proxy::team_presets();
    let event = crate::relay_server::RelayEvent::CommandResponse {
        request_id: cmd.request_id.clone().unwrap_or_default(),
        result: json!({ "presets": presets }),
        error: None,
    };
    if let Ok(s) = serde_json::to_string(&event) {
        let state = _ctx.infra.relay_state.clone();
        state.send_to_client(client_id.into(), &s).await;
    } else {
        maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some("serialization error".to_string()),
        );
    }
}

pub async fn handle_team_save(
    ctx: &crate::SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    // Accept either a full `config` or a `preset_name` to source from presets
    let payload = &cmd.payload;
    let config_val = if let Some(cfg) = payload.get("config") {
        cfg.clone()
    } else if let Some(name) = payload.get("preset_name").and_then(|v| v.as_str()) {
        let presets = crate::admin_proxy::team_presets();
        match presets.get(name) {
            Some(p) => json!({
                "agents": p.get("agents").cloned().unwrap_or(json!([])),
                "team_prompt": p.get("team_prompt").cloned().unwrap_or(json!(null)),
                "auto_approve": true
            }),
            None => {
                return maybe_respond(
                    cmd.request_id.as_deref(),
                    serde_json::Value::Null,
                    Some(format!("unknown preset: {}", name)),
                );
            }
        }
    } else {
        return maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some("config or preset_name is required".to_string()),
        );
    };

    // Write legacy JSON (brain/settings/team_config.json). This is sufficient because
    // load_team_runtime_config() falls back to this file when brain/agents/*.md is absent.
    let ilhae_dir = &ctx.infra.ilhae_dir;
    let team_path = ilhae_dir
        .join("brain")
        .join("settings")
        .join("team_config.json");
    if let Some(parent) = team_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(
        &team_path,
        serde_json::to_string_pretty(&config_val).unwrap_or_default(),
    ) {
        Ok(_) => {
            // Hot-apply if team_mode enabled
            let settings = ctx.infra.settings_store.get();
            let team_backend = crate::config::normalize_team_backend(&settings.agent.team_backend);
            let use_remote_team = settings.agent.team_mode
                && crate::config::team_backend_uses_remote_transport(&team_backend);
            if use_remote_team {
                let dir = ilhae_dir.clone();
                let sv = ctx.team.supervisor.clone();
                tokio::spawn(async move {
                    if let Some(team) = crate::context_proxy::load_team_runtime_config(&dir) {
                        let entries: Vec<(String, u16, String)> = team
                            .agents
                            .iter()
                            .filter_map(|a| {
                                crate::context_proxy::extract_port_from_endpoint(&a.endpoint)
                                    .map(|port| (a.role.clone(), port, a.engine.clone()))
                            })
                            .collect();
                        crate::process_supervisor::restart_team_agents(&sv, &entries).await;
                        let workspace_map =
                            crate::context_proxy::generate_peer_registration_files(&team, None);
                        let _ = crate::context_proxy::spawn_team_a2a_servers(
                            &team,
                            &workspace_map,
                            None,
                            "team-save",
                        )
                        .await;
                        let _ = crate::context_proxy::wait_for_all_team_health(&team).await;
                    }
                });
            }
            maybe_respond(cmd.request_id.as_deref(), json!({"ok": true}), None);
        }
        Err(e) => {
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some(e.to_string()),
            );
        }
    }
}
