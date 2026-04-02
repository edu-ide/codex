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
