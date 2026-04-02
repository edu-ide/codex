// commands

use crate::SharedState;
use crate::{
    BUILTIN_PLUGINS, normalize_mcp_preset_for_store, read_codex_runtime_options,
    write_codex_runtime_option,
};
use serde_json::json;
use std::sync::atomic::Ordering;
pub async fn handle_settings_get(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let settings = ctx.infra.settings_store.get();
    let result = serde_json::to_value(settings).unwrap_or(serde_json::Value::Null);
    maybe_respond(cmd.request_id.as_deref(), result, None);
}

pub async fn handle_settings_set(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    if let (Some(key), Some(value)) = (
        cmd.payload.get("key").and_then(|v| v.as_str()),
        cmd.payload.get("value"),
    ) {
        if let Err(e) = ctx.infra.settings_store.set_value(key, value.clone()) {
            maybe_respond(cmd.request_id.as_deref(), serde_json::Value::Null, Some(e));
            return;
        }

        if key == "mcp.presets"
            && let Some(presets) = value.as_array()
        {
            for preset in presets {
                if let Some(normalized) = normalize_mcp_preset_for_store(preset) {
                    let _ = ctx.infra.brain.preset_upsert(normalized);
                }
            }
        }

        if let Some(id_str) = key.strip_prefix("plugins.") {
            let is_known = BUILTIN_PLUGINS.iter().any(|def| def.id == id_str);
            if !is_known {
                let val = value.as_bool().unwrap_or(false);
                let _ = ctx.infra.brain.preset_toggle(id_str, val);
            }
        }

        if key == "mcp.presets" || key.starts_with("plugins.") {
            let active = ctx
                .infra
                .brain
                .preset_list()
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false))
                .collect::<Vec<_>>();
            ctx.infra.mcp_mgr.sync_with_presets(active).await;
        }

        if key == "browser.enabled" {
            let _ = ctx
                .infra
                .settings_store
                .set_value("plugins.browser", value.clone());
        }
        if key == "plugins.browser" {
            let _ = ctx
                .infra
                .settings_store
                .set_value("browser.enabled", value.clone());
        }

        // Dynamic browser launch/stop when browser.* settings change via relay
        if key.starts_with("browser.") {
            let cfg = ctx.infra.settings_store.get();
            ctx.infra.browser_mgr.react_to_settings(&cfg.browser);
            ctx.sessions
                .instructions_version
                .fetch_add(1, Ordering::Relaxed);
        }

        if key.starts_with("plugins.") {
            ctx.sessions
                .instructions_version
                .fetch_add(1, Ordering::Relaxed);
        }
        // Note: set_value() auto-broadcasts via unified SettingsEvent channel.
        // Bridges in main.rs forward to relay (mobile) and desktop (SACP).
        // Manual broadcast is only needed for toggle_preset() which bypasses set_value().
        maybe_respond(cmd.request_id.as_deref(), json!({ "ok": true }), None);
    }
}

pub async fn handle_codex_config_get(
    _ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let (sandbox_mode, approval_policy) = read_codex_runtime_options();
    maybe_respond(
        cmd.request_id.as_deref(),
        json!({
            "sandbox_mode": sandbox_mode,
            "approval_policy": approval_policy,
        }),
        None,
    );
}

pub async fn handle_codex_config_set(
    _ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let key = cmd
        .payload
        .get("key")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let value = cmd
        .payload
        .get("value")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if key.is_empty() || value.is_empty() {
        maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some("key and value are required".to_string()),
        );
        return;
    }
    if key != "sandbox_mode" && key != "approval_policy" {
        maybe_respond(
            cmd.request_id.as_deref(),
            serde_json::Value::Null,
            Some(format!("unsupported codex config key: {}", key)),
        );
        return;
    }

    match write_codex_runtime_option(key, value) {
        Ok(()) => {
            maybe_respond(
                cmd.request_id.as_deref(),
                json!({
                    "ok": true,
                    "key": key,
                    "value": value,
                }),
                None,
            );
        }
        Err(e) => {
            maybe_respond(cmd.request_id.as_deref(), serde_json::Value::Null, Some(e));
        }
    }
}
