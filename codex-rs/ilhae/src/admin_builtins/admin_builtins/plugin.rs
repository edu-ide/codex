#[macro_export]
macro_rules! register_admin_plugin_handlers {
    ($builder:expr, $state:expr) => {{
        let s = $state.clone();
        $builder
            // ═══ Plugin List ═══
            .on_receive_request_from(sacp::Client, {
                let brain = s.infra.brain.clone();
                let settings = s.infra.settings_store.clone();
                let mcp_mgr = s.infra.mcp_mgr.clone();
                let browser = s.infra.browser_mgr.clone();
                async move |_req: ListPluginsRequest, responder: Responder<ListPluginsResponse>, cx: ConnectionTo<Conductor>| {
                    info!("ilhae/list_plugins RPC");
                    let brain = brain.clone();
                    let settings = settings.clone();
                    let mcp_mgr = mcp_mgr.clone();
                    let browser = browser.clone();
                    cx.spawn(async move {
                        let db_presets = brain.preset_list().unwrap_or_default();
                        let cfg = settings.get();
                        let mut plugins = Vec::new();
                        for def in BUILTIN_PLUGINS {
                            let enabled = if def.id == "browser" {
                                cfg.browser.enabled
                            } else {
                                let default_enabled = true;
                                cfg.plugins.get(def.id).copied().unwrap_or(default_enabled)
                            };
                            plugins.push(PluginInfo {
                                id: def.id.to_string(), label: def.label.to_string(), description: def.description.to_string(),
                                enabled,
                                connected: if def.id == "browser" { browser.get_status().running } else { false },
                            });
                        }
                        plugins.push(PluginInfo {
                            id: "yolo".to_string(), label: "YOLO".to_string(), description: "권한 자동 승인 모드".to_string(),
                            enabled: cfg.permissions.approval_preset == "full-access",
                            connected: false,
                        });
                        for p in db_presets {
                            if let Some(id) = p.get("id").and_then(|v| v.as_str()) {
                                plugins.push(PluginInfo {
                                    id: id.to_string(),
                                    label: p.get("name").and_then(|v| v.as_str()).unwrap_or(id).to_string(),
                                    description: mcp_preset_description(&p),
                                    enabled: p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
                                    connected: mcp_mgr.is_connected(id).await,
                                });
                            }
                        }
                        let bp = builtin_plugin_list(&cfg.plugins, &cfg.permissions.auto_approve_plugins, cfg.browser.enabled);
                        responder.respond(ListPluginsResponse {
                            plugins,
                            builtin_plugins: bp,
                        })
                    })
                }
            }, sacp::on_receive_request!())
            // ═══ Plugin Toggle ═══
            .on_receive_request_from(sacp::Client, {
                let brain = s.infra.brain.clone();
                let settings = s.infra.settings_store.clone();
                let mcp_mgr = s.infra.mcp_mgr.clone();
                let browser = s.infra.browser_mgr.clone();
                let relay_tx = s.infra.relay_tx.clone();
                async move |req: TogglePluginRequest, responder: Responder<TogglePluginResponse>, cx: ConnectionTo<Conductor>| {
                    info!("ilhae/toggle_plugin RPC plugin_id={} enabled={}", req.plugin_id, req.enabled);
                    let settings = settings.clone();
                    let brain = brain.clone();
                    let mcp_mgr = mcp_mgr.clone();
                    let browser = browser.clone();
                    let relay_tx = relay_tx.clone();
                    cx.spawn(async move {
                        if req.plugin_id == "yolo" {
                            let preset = if req.enabled { "full-access" } else { "auto" };
                            let _ = settings.set_value("permissions.approval_preset", serde_json::Value::String(preset.to_string()));
                        } else if BUILTIN_PLUGINS.iter().any(|def| def.id == req.plugin_id) {
                            let _ = settings.set_value(&format!("plugins.{}", req.plugin_id), serde_json::Value::Bool(req.enabled));
                            if req.plugin_id == "browser" {
                                let _ = settings.set_value("browser.enabled", serde_json::Value::Bool(req.enabled));
                                let browser_cfg = settings.get().browser;
                                let browser_bg = browser.clone();
                                let _ = tokio::task::spawn_blocking(move || {
                                    browser_bg.react_to_settings(&browser_cfg);
                                });
                            }
                        } else {
                            let _ = brain.preset_toggle(&req.plugin_id, req.enabled);
                        }
                        let is_mcp_preset = !BUILTIN_PLUGINS.iter().any(|def| def.id == req.plugin_id)
                            && req.plugin_id != "yolo";
                        if is_mcp_preset {
                            let active = brain.preset_list().unwrap_or_default().into_iter()
                                .filter(|p| p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)).collect();
                            mcp_mgr.sync_with_presets(active).await;
                        }

                        let db_presets = brain.preset_list().unwrap_or_default();
                        let cfg = settings.get();
                        let mut plugins = Vec::new();
                        for def in BUILTIN_PLUGINS {
                            let enabled = if def.id == "browser" {
                                cfg.browser.enabled
                            } else {
                                let default_enabled = true;
                                cfg.plugins.get(def.id).copied().unwrap_or(default_enabled)
                            };
                            plugins.push(PluginInfo {
                                id: def.id.to_string(), label: def.label.to_string(), description: def.description.to_string(),
                                enabled,
                                connected: if def.id == "browser" { browser.get_status().running } else { false },
                            });
                        }
                        plugins.push(PluginInfo {
                            id: "yolo".to_string(), label: "YOLO".to_string(), description: "권한 자동 승인 모드".to_string(),
                            enabled: cfg.permissions.approval_preset == "full-access",
                            connected: false,
                        });
                        for p in db_presets {
                            if let Some(id) = p.get("id").and_then(|v| v.as_str()) {
                                plugins.push(PluginInfo {
                                    id: id.to_string(),
                                    label: p.get("name").and_then(|v| v.as_str()).unwrap_or(id).to_string(),
                                    description: mcp_preset_description(&p),
                                    enabled: p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
                                    connected: mcp_mgr.is_connected(id).await,
                                });
                            }
                        }
                        // Manual broadcast only for MCP presets (toggle_preset bypasses set_value)
                        // Builtin plugins + YOLO use set_value() which auto-broadcasts.
                        if is_mcp_preset {
                            broadcast_event(&relay_tx, RelayEvent::SettingsChanged {
                                key: format!("plugins.{}", req.plugin_id),
                                value: serde_json::Value::Bool(req.enabled),
                            });
                        }
                        // Sync to brain/mcp.json after MCP preset toggle
                        if is_mcp_preset {
                            settings.sync_to_brain_mcp_json();
                        }
                        responder.respond(TogglePluginResponse { plugins })
                    })
                }
            }, sacp::on_receive_request!())
    }};
}
