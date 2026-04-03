#[macro_export]
macro_rules! register_admin_settings_handlers {
    ($builder:expr, $state:expr) => {{
        let s = $state.clone();
        $builder
            // ═══ Settings Read ═══
            .on_receive_request_from(sacp::Client, {
                let settings = s.infra.settings_store.clone();
                async move |_req: ReadSettingsRequest, responder: Responder<ReadSettingsResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/read_settings RPC");
                    let all = serde_json::to_value(&settings.get()).unwrap_or(json!({}));
                    responder.respond(ReadSettingsResponse { settings: all })
                }
            }, sacp::on_receive_request!())
            // ═══ Engine Capabilities Read ═══
            .on_receive_request_from(sacp::Client, {
                let settings = s.infra.settings_store.clone();
                async move |req: GetEngineCapabilitiesRequest, responder: Responder<GetEngineCapabilitiesResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/engine/get_capabilities RPC");
                    let settings_snapshot = settings.get();
                    let current_engine = crate::helpers::infer_agent_id_from_command(&settings_snapshot.agent.command);
                    let target_engine = req.engine_id.unwrap_or_else(|| current_engine.clone());
                    responder.respond(GetEngineCapabilitiesResponse {
                        current_engine,
                        profile: crate::capabilities::engine_capability_profile_json(&target_engine),
                        matrix: crate::capabilities::engine_capability_matrix_json(),
                    })
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(sacp::Client, {
                let settings = s.infra.settings_store.clone();
                async move |req: crate::IlhaeAppEngineGetRequest, responder: Responder<crate::IlhaeAppEngineGetResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/engine/get RPC");
                    let settings_snapshot = settings.get();
                    let current_engine = crate::helpers::infer_agent_id_from_command(&settings_snapshot.agent.command);
                    let target_engine = req.engine_id.unwrap_or_else(|| current_engine.clone());
                    let resolved_engine = crate::engine_env::resolve_engine_env(&target_engine);
                    let team_backend = crate::config::normalize_team_backend(&settings_snapshot.agent.team_backend);
                    let use_remote_team = settings_snapshot.agent.team_mode
                        && crate::config::team_backend_uses_remote_transport(&team_backend);
                    let endpoint = if use_remote_team {
                        let ep = settings_snapshot.agent.a2a_endpoint.trim();
                        if ep.is_empty() {
                            format!("http://127.0.0.1:{}", crate::port_config::team_base_port())
                        } else {
                            ep.to_string()
                        }
                    } else {
                        let ep = settings_snapshot.agent.a2a_endpoint.trim();
                        if ep.is_empty() {
                            format!("http://127.0.0.1:{}", resolved_engine.default_port())
                        } else {
                            ep.to_string()
                        }
                    };
                    responder.respond(crate::IlhaeAppEngineGetResponse {
                        current_engine,
                        command: settings_snapshot.agent.command.clone(),
                        team_mode: settings_snapshot.agent.team_mode,
                        team_backend,
                        endpoint,
                        enabled_engines: settings_snapshot.agent.enabled_engines.clone(),
                        profile: crate::capabilities::engine_capability_profile_json(&target_engine),
                        matrix: crate::capabilities::engine_capability_matrix_json(),
                    })
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(sacp::Client, {
                let settings = s.infra.settings_store.clone();
                let cx_cache = s.infra.relay_conductor_cx.clone();
                async move |req: crate::IlhaeAppEngineSetRequest, responder: Responder<crate::IlhaeAppEngineSetResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/engine/set RPC engine={}", req.engine_id);
                    let Some(command) = crate::helpers::resolve_engine_command(&req.engine_id, req.command.as_deref()) else {
                        return responder.respond_with_error(sacp::Error::new(
                            -32602,
                            "unknown engine id; provide explicit command".to_string(),
                        ));
                    };

                    if let Err(e) = settings.set_value("agent.command", serde_json::Value::String(command.clone())) {
                        return responder.respond_with_error(sacp::util::internal_error(e));
                    }

                    let mut enabled_engines = settings.get().agent.enabled_engines;
                    if !enabled_engines.iter().any(|engine| engine == &req.engine_id) {
                        enabled_engines.push(req.engine_id.clone());
                        let _ = settings.set_value("agent.enabled_engines", serde_json::json!(enabled_engines));
                    }

                    crate::notify_engine_state(&cx_cache, &settings).await;

                    let updated = settings.get();
                    let current_engine = crate::helpers::infer_agent_id_from_command(&updated.agent.command);
                    let resolved_engine = crate::engine_env::resolve_engine_env(&current_engine);
                    let team_backend = crate::config::normalize_team_backend(&updated.agent.team_backend);
                    let use_remote_team = updated.agent.team_mode
                        && crate::config::team_backend_uses_remote_transport(&team_backend);
                    let endpoint = if use_remote_team {
                        let ep = updated.agent.a2a_endpoint.trim();
                        if ep.is_empty() {
                            format!("http://127.0.0.1:{}", crate::port_config::team_base_port())
                        } else {
                            ep.to_string()
                        }
                    } else {
                        let ep = updated.agent.a2a_endpoint.trim();
                        if ep.is_empty() {
                            format!("http://127.0.0.1:{}", resolved_engine.default_port())
                        } else {
                            ep.to_string()
                        }
                    };

                    responder.respond(crate::IlhaeAppEngineSetResponse {
                        ok: true,
                        current_engine: current_engine.clone(),
                        command: updated.agent.command.clone(),
                        team_mode: updated.agent.team_mode,
                        team_backend,
                        endpoint,
                        enabled_engines: updated.agent.enabled_engines.clone(),
                        profile: crate::capabilities::engine_capability_profile_json(&current_engine),
                        matrix: crate::capabilities::engine_capability_matrix_json(),
                    })
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(sacp::Client, {
                async move |_req: crate::IlhaeAppProfileListRequest, responder: Responder<crate::IlhaeAppProfileListResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/profile/list RPC");
                    let (active_profile, profiles) = crate::config::list_ilhae_profiles();
                    responder.respond(crate::IlhaeAppProfileListResponse {
                        active_profile,
                        profiles,
                    })
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(sacp::Client, {
                async move |req: crate::IlhaeAppProfileGetRequest, responder: Responder<crate::IlhaeAppProfileGetResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/profile/get RPC");
                    let (active_profile, profile) = crate::config::get_ilhae_profile(req.profile_id.as_deref());
                    responder.respond(crate::IlhaeAppProfileGetResponse {
                        active_profile,
                        profile,
                    })
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(sacp::Client, {
                let settings = s.infra.settings_store.clone();
                let cx_cache = s.infra.relay_conductor_cx.clone();
                async move |req: crate::IlhaeAppProfileSetRequest, responder: Responder<crate::IlhaeAppProfileSetResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/profile/set RPC profile={}", req.profile_id);
                    let profile = match crate::config::set_active_ilhae_profile(&req.profile_id) {
                        Ok(profile) => profile,
                        Err(e) => {
                            return responder.respond_with_error(sacp::Error::new(-32602, e));
                        }
                    };
                    if let Err(e) = crate::config::apply_ilhae_profile_projection(&settings, &profile) {
                        return responder.respond_with_error(sacp::util::internal_error(e));
                    }
                    crate::notify_engine_state(&cx_cache, &settings).await;
                    responder.respond(crate::IlhaeAppProfileSetResponse {
                        ok: true,
                        active_profile: profile.id.clone(),
                        profile: Some(profile),
                    })
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(sacp::Client, {
                let settings = s.infra.settings_store.clone();
                let cx_cache = s.infra.relay_conductor_cx.clone();
                async move |req: crate::IlhaeAppProfileUpsertRequest, responder: Responder<crate::IlhaeAppProfileUpsertResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/app/profile/upsert RPC profile={}", req.profile.id);
                    let (active_profile, profile) = match crate::config::upsert_ilhae_profile(req.profile, req.activate) {
                        Ok(result) => result,
                        Err(e) => {
                            return responder.respond_with_error(sacp::Error::new(-32602, e));
                        }
                    };

                    if active_profile.as_deref() == Some(profile.id.as_str()) {
                        if let Err(e) = crate::config::apply_ilhae_profile_projection(&settings, &profile) {
                            return responder.respond_with_error(sacp::util::internal_error(e));
                        }
                        crate::notify_engine_state(&cx_cache, &settings).await;
                    }

                    responder.respond(crate::IlhaeAppProfileUpsertResponse {
                        ok: true,
                        active_profile,
                        profile: Some(profile),
                    })
                }
            }, sacp::on_receive_request!())
            // ═══ Settings Write ═══
            .on_receive_request_from(sacp::Client, {
                let settings = s.infra.settings_store.clone();
                let mcp_mgr = s.infra.mcp_mgr.clone();
                let brain = s.infra.brain.clone();
                let instr_version = s.sessions.instructions_version.clone();
                let browser_mgr_ws = s.infra.browser_mgr.clone();
                async move |req: WriteSettingRequest, responder: Responder<WriteSettingResponse>, cx: ConnectionTo<Conductor>| {
                    info!("ilhae/write_setting RPC key={}", req.key);
                    let settings = settings.clone();
                    let brain = brain.clone();
                    let mcp_mgr = mcp_mgr.clone();
                    let instr_version = instr_version.clone();
                    let browser_mgr_ws = browser_mgr_ws.clone();
                    cx.spawn(async move {
                        match settings.set_value(&req.key, req.value.clone()) {
                            Ok(()) => {
                                if req.key == "mcp.presets" {
                                    if let Some(presets) = req.value.as_array() {
                                        for p in presets { let _ = brain.preset_upsert(p.clone()); }
                                    }
                                }
                                if let Some(id_str) = req.key.strip_prefix("plugins.") {
                                    let is_known = BUILTIN_PLUGINS.iter().any(|def| def.id == id_str);
                                    if !is_known {
                                        let val = req.value.as_bool().unwrap_or(false);
                                        let _ = brain.preset_toggle(id_str, val);
                                    }
                                }
                                if req.key == "mcp.presets" || req.key.starts_with("plugins.") {
                                    let active = brain.preset_list().unwrap_or_default().into_iter()
                                        .filter(|p| p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false))
                                        .collect();
                                    mcp_mgr.sync_with_presets(active).await;
                                }
                                if req.key == "browser.enabled" {
                                    let _ = settings.set_value("plugins.browser", req.value.clone());
                                }
                                if req.key == "plugins.browser" {
                                    let _ = settings.set_value("browser.enabled", req.value.clone());
                                }

                                if req.key.starts_with("browser.") {
                                    let cfg = settings.get();
                                    let browser_cfg = cfg.browser.clone();
                                    let browser_mgr_bg = browser_mgr_ws.clone();
                                    let _ = tokio::task::spawn_blocking(move || {
                                        browser_mgr_bg.react_to_settings(&browser_cfg);
                                    });
                                    instr_version.fetch_add(1, Ordering::Relaxed);
                                }
                                if req.key.starts_with("plugins.") {
                                    instr_version.fetch_add(1, Ordering::Relaxed);
                                    info!("Plugin setting changed ({}), instructions version bumped", req.key);
                                }
                                let all = serde_json::to_value(&settings.get()).unwrap_or(json!({}));
                                // NOTE: broadcast removed — SettingsStore::set_value() auto-broadcasts
                                responder.respond(WriteSettingResponse { settings: all })
                            }
                            Err(e) => responder.respond_with_error(sacp::util::internal_error(e.to_string())),
                        }
                    })
                }
            }, sacp::on_receive_request!())
            // ═══ Read brain/mcp.json ═══
            .on_receive_request_from(sacp::Client, {
                let settings = s.infra.settings_store.clone();
                async move |_req: ReadMcpJsonRequest, responder: Responder<ReadMcpJsonResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/read_mcp_json RPC");
                    let content = settings.read_brain_mcp_json();
                    responder.respond(ReadMcpJsonResponse { content })
                }
            }, sacp::on_receive_request!())
            // ═══ Write brain/mcp.json ═══
            .on_receive_request_from(sacp::Client, {
                let settings = s.infra.settings_store.clone();
                let brain = s.infra.brain.clone();
                let mcp_mgr = s.infra.mcp_mgr.clone();
                async move |req: WriteMcpJsonRequest, responder: Responder<WriteMcpJsonResponse>, cx: ConnectionTo<Conductor>| {
                    info!("ilhae/write_mcp_json RPC");
                    let settings = settings.clone();
                    let brain = brain.clone();
                    let mcp_mgr = mcp_mgr.clone();
                    cx.spawn(async move {
                        match settings.write_brain_mcp_json(&req.content) {
                            Ok(()) => {
                                // Sync DB presets from updated settings
                                let cfg = settings.get();
                                for p in &cfg.mcp.presets {
                                    let _ = brain.preset_upsert(p.clone());
                                }
                                let active = brain.preset_list().unwrap_or_default().into_iter()
                                    .filter(|p| p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)).collect();
                                mcp_mgr.sync_with_presets(active).await;
                                responder.respond(WriteMcpJsonResponse { ok: true, error: None })
                            }
                            Err(e) => {
                                responder.respond(WriteMcpJsonResponse { ok: false, error: Some(e) })
                            }
                        }
                    })
                }
            }, sacp::on_receive_request!())
            // ═══ Get Cached ConfigOptions ═══
            .on_receive_request_from(sacp::Client, {
                let config_cache = s.infra.cached_config_options.clone();
                async move |_req: GetConfigOptionsRequest, responder: Responder<GetConfigOptionsResponse>, _cx: ConnectionTo<Conductor>| {
                    let cache = config_cache.read().await;
                    let options = if cache.is_empty() {
                        // Fallback: read from ~/.gemini/settings.json before first session/new
                        crate::helpers::build_gemini_config_options()
                    } else {
                        cache.clone()
                    };
                    responder.respond(GetConfigOptionsResponse {
                        config_options: options,
                    })
                }
            }, sacp::on_receive_request!())
    }};
}
