use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

use sacp::{Agent, Conductor, ConnectionTo, Responder, UntypedMessage};

use crate::SharedState;
use crate::{
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, apply_codex_profile_to_config,
    helpers::{LEGACY_CODEX_AGENT_ID, is_ilhae_native_agent_id},
    infer_agent_id_from_command,
};

fn ilhae_profile_model_name(profile_id: &str) -> Option<String> {
    let config = crate::config::load_ilhae_toml_config();
    let profile = config.profiles.get(profile_id)?;
    if profile.native_runtime.enabled {
        if let Some(model) = Path::new(&profile.native_runtime.model_path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .filter(|stem| !stem.trim().is_empty())
        {
            return Some(model);
        }
    }
    profile
        .agent
        .engine_id
        .clone()
        .or_else(|| profile.agent.command.clone())
        .or_else(|| Some(profile_id.to_string()))
}

pub async fn handle_set_session_config_option(
    req: SetSessionConfigOptionRequest,
    responder: Responder<SetSessionConfigOptionResponse>,
    cx: ConnectionTo<Conductor>,
    s: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    let settings = &s.infra.settings_store;
    let cx_cache = &s.infra.relay_conductor_cx;

    cx_cache.try_add(cx.clone()).await;
    let agent_id = infer_agent_id_from_command(&settings.get().agent.command);
    info!(
        "[SetConfigOption] agent={}, configId={}, value={}",
        agent_id, req.config_id, req.value
    );

    let ok_response = SetSessionConfigOptionResponse {
        config_options: vec![],
    };

    // ─── Model change: unified path for ALL engines ──────────────────────
    if req.config_id == "model" {
        let model_id = if is_ilhae_native_agent_id(&agent_id) {
            // Codex/Ilhae: apply profile, then move the managed local runtime.
            let profile_name = req.value.clone();
            let previous_active = crate::config::load_ilhae_toml_config().profile.active;
            let resolved = match crate::config::set_active_ilhae_profile(&profile_name) {
                Ok(profile) => {
                    if let Err(e) =
                        crate::config::apply_ilhae_profile_projection(settings, &profile)
                    {
                        warn!("[SetConfigOption] Failed to project Ilhae profile: {}", e);
                    }
                    if let Err(e) = crate::config::prepare_ilhae_codex_home() {
                        warn!(
                            "[SetConfigOption] Failed to refresh Ilhae Codex home: {}",
                            e
                        );
                    }
                    if let Err(e) = crate::switch_native_runtime_for_cli(
                        previous_active.as_deref(),
                        Some(profile.id.as_str()),
                    )
                    .await
                    {
                        warn!("[SetConfigOption] Failed to switch native runtime: {}", e);
                    }
                    crate::notify_engine_state(cx_cache, settings).await;
                    ilhae_profile_model_name(&profile_name).unwrap_or_else(|| profile_name.clone())
                }
                Err(e) => {
                    warn!("[SetConfigOption] Failed to apply Ilhae profile: {}", e);
                    if let Err(e) = apply_codex_profile_to_config(&profile_name) {
                        warn!("[SetConfigOption] Failed to apply Codex profile: {}", e);
                    }
                    std::fs::read_to_string(
                        dirs::home_dir()
                            .map(|h| h.join(".codex/config.toml"))
                            .unwrap_or_default(),
                    )
                    .ok()
                    .and_then(|s| s.parse::<toml::Value>().ok())
                    .and_then(|c| {
                        c.get("profiles")?
                            .get(&profile_name)?
                            .get("model")?
                            .as_str()
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| profile_name.clone())
                }
            };
            info!(
                "[SetConfigOption] Switching Codex model to: {} (profile: {})",
                resolved, profile_name
            );

            // Sync user_agent.md to match the new codex model
            crate::context_proxy::team_a2a::sync_user_agent_model_inline(
                LEGACY_CODEX_AGENT_ID,
                &resolved,
                &crate::config::resolve_ilhae_data_dir(),
            );

            resolved
        } else {
            // Non-Codex (Gemini etc.): use the value directly as model ID
            info!(
                "[SetConfigOption] Switching {} model to: {}",
                agent_id, req.value
            );

            // Sync user_agent.md to match the new engine's model
            crate::context_proxy::team_a2a::sync_user_agent_model_inline(
                &agent_id,
                &req.value,
                &crate::config::resolve_ilhae_data_dir(),
            );

            req.value.clone()
        };

        // Send session/set_model to the agent (works for both Gemini and Codex)
        let model_req = UntypedMessage::new(
            crate::types::REQ_SESSION_SET_MODEL,
            json!({
                "sessionId": req.session_id,
                "modelId": model_id,
            }),
        )
        .unwrap();
        return cx
            .send_request_to(Agent, model_req)
            .on_receiving_result(async move |result| match result {
                Ok(_) => responder.respond(ok_response),
                Err(e) => {
                    warn!("[SetConfigOption] Agent rejected model change: {}", e);
                    // Still respond OK so UI can update optimistically
                    responder.respond(ok_response)
                }
            });
    }

    if req.config_id == "thinking" && is_ilhae_native_agent_id(&agent_id) {
        let thinking_mode = crate::settings_types::normalize_thinking_mode(&req.value);
        if let Err(e) = settings.set_value("agent.thinking_mode", json!(thinking_mode.clone())) {
            return responder.respond_with_error(sacp::util::internal_error(e));
        }

        let active_profile = settings
            .get()
            .agent
            .active_profile
            .or_else(|| crate::config::load_ilhae_toml_config().profile.active);
        if active_profile
            .as_deref()
            .and_then(|profile_id| crate::config::get_native_runtime_config(Some(profile_id)))
            .is_some()
        {
            if let Err(e) = crate::stop_native_runtime_for_cli(active_profile.as_deref()).await {
                warn!(
                    "[SetConfigOption] Failed to stop native runtime for thinking change: {}",
                    e
                );
            }
            if let Err(e) = crate::ensure_native_runtime_for_cli(active_profile.as_deref()).await {
                warn!(
                    "[SetConfigOption] Failed to restart native runtime for thinking change: {}",
                    e
                );
            }
        }

        crate::notify_engine_state(cx_cache, settings).await;
        info!(
            "[SetConfigOption] Updated local thinking mode to {}",
            thinking_mode
        );
        return responder.respond(ok_response);
    }

    // ─── Non-model config options ────────────────────────────────────────
    if is_ilhae_native_agent_id(&agent_id) {
        // Codex: write to config.toml
        let config_path = dirs::home_dir()
            .map(|h| h.join(".codex/config.toml"))
            .unwrap_or_default();
        if let Ok(config_str) = std::fs::read_to_string(&config_path)
            && let Ok(mut doc) = config_str.parse::<toml_edit::DocumentMut>()
        {
            doc[&req.config_id] = toml_edit::value(&req.value);
            let _ = std::fs::write(&config_path, doc.to_string());
            info!(
                "[SetConfigOption] Updated config.toml: {}={}",
                req.config_id, req.value
            );
        }
        return responder.respond(ok_response);
    }

    // Non-Codex, non-model: try forwarding to agent, respond OK on any outcome
    info!(
        "[SetConfigOption] Forwarding {} config to agent: {}={}",
        agent_id, req.config_id, req.value
    );
    return cx
        .send_request_to(Agent, req)
        .on_receiving_result(async move |result| match result {
            Ok(resp) => responder.respond(resp),
            Err(e) => {
                warn!("[SetConfigOption] Agent rejected config change: {}", e);
                responder.respond(ok_response)
            }
        });
}
