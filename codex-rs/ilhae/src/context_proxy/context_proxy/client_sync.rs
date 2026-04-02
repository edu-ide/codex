use serde_json::json;
use std::sync::Arc;
use tracing::{info, warn};

use sacp::{Agent, Conductor, ConnectionTo, Responder, UntypedMessage};

use crate::SharedState;
use crate::{
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, apply_codex_profile_to_config,
    infer_agent_id_from_command,
};

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
        let model_id = if agent_id == "codex" {
            // Codex: apply profile → resolve actual model name
            let profile_name = req.value.clone();
            if let Err(e) = apply_codex_profile_to_config(&profile_name) {
                warn!("[SetConfigOption] Failed to apply profile: {}", e);
            }
            let resolved = {
                let config_path = dirs::home_dir()
                    .map(|h| h.join(".codex/config.toml"))
                    .unwrap_or_default();
                std::fs::read_to_string(&config_path)
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
            };
            info!(
                "[SetConfigOption] Switching Codex model to: {} (profile: {})",
                resolved, profile_name
            );
            
            // Sync user_agent.md to match the new codex model
            crate::context_proxy::team_a2a::sync_user_agent_model_inline(
                "codex",
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

    // ─── Non-model config options ────────────────────────────────────────
    if agent_id == "codex" {
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
