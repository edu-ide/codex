// commands

use crate::SharedState;
use crate::relay_server::RelayEvent;
use crate::{
    BUILTIN_PLUGINS, PluginInfo, broadcast_event, builtin_plugin_list, mcp_preset_description,
    normalize_mcp_preset_for_store,
};
use serde_json::json;
use std::sync::atomic::Ordering;
pub async fn handle_plugin_list(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let cfg = ctx.infra.settings_store.get();
    for preset in &cfg.mcp.presets {
        if let Some(normalized) = normalize_mcp_preset_for_store(preset) {
            let _ = ctx.infra.brain.preset_upsert(normalized);
        }
    }
    let db_presets = ctx.infra.brain.preset_list().unwrap_or_default();
    let mut plugins = Vec::new();

    for def in BUILTIN_PLUGINS {
        let enabled = if def.id == "browser" {
            cfg.browser.enabled
        } else {
            let default_enabled = true;
            cfg.plugins.get(def.id).copied().unwrap_or(default_enabled)
        };
        plugins.push(PluginInfo {
            id: def.id.to_string(),
            label: def.label.to_string(),
            description: def.description.to_string(),
            enabled,
            connected: if def.id == "browser" {
                ctx.infra.browser_mgr.get_status().running
            } else {
                false
            },
        });
    }

    plugins.push(PluginInfo {
        id: "yolo".to_string(),
        label: "YOLO".to_string(),
        description: "권한 자동 승인 모드".to_string(),
        enabled: cfg.permissions.approval_preset == "full-access",
        connected: false,
    });

    for preset in db_presets {
        if let Some(id) = preset.get("id").and_then(|v| v.as_str()) {
            plugins.push(PluginInfo {
                id: id.to_string(),
                label: preset
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(id)
                    .to_string(),
                description: mcp_preset_description(&preset),
                enabled: preset
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                connected: ctx.infra.mcp_mgr.is_connected(id).await,
            });
        }
    }

    let builtin_plugins = builtin_plugin_list(
        &cfg.plugins,
        &cfg.permissions.auto_approve_plugins,
        cfg.browser.enabled,
    );
    maybe_respond(
        cmd.request_id.as_deref(),
        json!({
            "plugins": plugins,
            "builtin_plugins": builtin_plugins,
        }),
        None,
    );
}

pub async fn handle_plugin_toggle(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let plugin_id = cmd
        .payload
        .get("plugin_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let enabled = cmd
        .payload
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if plugin_id.is_empty() {
        maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some("plugin_id is required".to_string()),
        );
        return;
    }

    let cfg_for_sync = ctx.infra.settings_store.get();
    for preset in &cfg_for_sync.mcp.presets {
        if let Some(normalized) = normalize_mcp_preset_for_store(preset) {
            let _ = ctx.infra.brain.preset_upsert(normalized);
        }
    }

    if plugin_id == "yolo" {
        let preset = if enabled { "full-access" } else { "auto" };
        let _ = ctx.infra.settings_store.set_value(
            "permissions.approval_preset",
            serde_json::Value::String(preset.to_string()),
        );
    } else if BUILTIN_PLUGINS.iter().any(|def| def.id == plugin_id) {
        let _ = ctx.infra.settings_store.set_value(
            &format!("plugins.{}", plugin_id),
            serde_json::Value::Bool(enabled),
        );
        if plugin_id == "browser" {
            let _ = ctx
                .infra
                .settings_store
                .set_value("browser.enabled", serde_json::Value::Bool(enabled));
            ctx.infra
                .browser_mgr
                .react_to_settings(&ctx.infra.settings_store.get().browser);
        }
    } else {
        let _ = ctx.infra.brain.preset_toggle(plugin_id, enabled);
    }

    let active = ctx
        .infra
        .brain
        .preset_list()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false))
        .collect::<Vec<_>>();
    ctx.infra.mcp_mgr.sync_with_presets(active).await;

    let db_presets = ctx.infra.brain.preset_list().unwrap_or_default();
    let cfg = ctx.infra.settings_store.get();
    let mut plugins = Vec::new();

    for def in BUILTIN_PLUGINS {
        let enabled = if def.id == "browser" {
            cfg.browser.enabled
        } else {
            let default_enabled = true;
            cfg.plugins.get(def.id).copied().unwrap_or(default_enabled)
        };
        plugins.push(PluginInfo {
            id: def.id.to_string(),
            label: def.label.to_string(),
            description: def.description.to_string(),
            enabled,
            connected: if def.id == "browser" {
                ctx.infra.browser_mgr.get_status().running
            } else {
                false
            },
        });
    }

    plugins.push(PluginInfo {
        id: "yolo".to_string(),
        label: "YOLO".to_string(),
        description: "권한 자동 승인 모드".to_string(),
        enabled: cfg.permissions.approval_preset == "full-access",
        connected: false,
    });

    for preset in db_presets {
        if let Some(id) = preset.get("id").and_then(|v| v.as_str()) {
            plugins.push(PluginInfo {
                id: id.to_string(),
                label: preset
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(id)
                    .to_string(),
                description: mcp_preset_description(&preset),
                enabled: preset
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                connected: ctx.infra.mcp_mgr.is_connected(id).await,
            });
        }
    }

    // Manual broadcast only for MCP presets (toggle_preset bypasses set_value).
    // Builtin plugins + YOLO use set_value() which auto-broadcasts via unified channel.
    let is_mcp_preset =
        !BUILTIN_PLUGINS.iter().any(|def| def.id == plugin_id) && plugin_id != "yolo";
    if is_mcp_preset {
        broadcast_event(
            &ctx.infra.relay_tx,
            RelayEvent::SettingsChanged {
                key: format!("plugins.{}", plugin_id),
                value: serde_json::Value::Bool(enabled),
            },
        );
    }
    ctx.sessions
        .instructions_version
        .fetch_add(1, Ordering::Relaxed);

    // Sync to brain/mcp.json after MCP preset toggle
    if is_mcp_preset {
        ctx.infra.settings_store.sync_to_brain_mcp_json();
    }

    let builtin_plugins = builtin_plugin_list(
        &cfg.plugins,
        &cfg.permissions.auto_approve_plugins,
        cfg.browser.enabled,
    );
    maybe_respond(
        cmd.request_id.as_deref(),
        json!({
            "plugins": plugins,
            "builtin_plugins": builtin_plugins,
        }),
        None,
    );
}
