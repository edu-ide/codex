use std::collections::HashMap;
use std::sync::Arc;

use sacp::{Conductor, ConnectionTo, Responder};
use serde_json::json;
use tracing::info;

use crate::SharedState;
use crate::types::{
    CapabilitiesRequest, CapabilitiesResponse, SetTeamAgentEngineRequest,
    SetTeamAgentEngineResponse, ToggleMcpRequest, ToggleMcpResponse, ToggleSkillRequest,
    ToggleSkillResponse,
};

pub fn bind_routes<H>(
    builder: sacp::Builder<sacp::Proxy, H>,
    state: Arc<SharedState>,
) -> sacp::Builder<sacp::Proxy, impl sacp::HandleDispatchFrom<sacp::Conductor>>
where
    H: sacp::HandleDispatchFrom<sacp::Conductor> + 'static,
{
    builder
        .on_receive_request_from(
            sacp::Client,
            {
                let state = state.clone();
                async move |req: CapabilitiesRequest,
                            responder: Responder<CapabilitiesResponse>,
                            cx: ConnectionTo<Conductor>| {
                    handle_capabilities_request(req, responder, cx, state.clone()).await
                }
            },
            sacp::on_receive_request!(),
        )
        .on_receive_request_from(
            sacp::Client,
            {
                let state = state.clone();
                async move |req: ToggleSkillRequest,
                            responder: Responder<ToggleSkillResponse>,
                            cx: ConnectionTo<Conductor>| {
                    handle_toggle_skill_request(req, responder, cx, state.clone()).await
                }
            },
            sacp::on_receive_request!(),
        )
        .on_receive_request_from(
            sacp::Client,
            {
                let state = state.clone();
                async move |req: ToggleMcpRequest,
                            responder: Responder<ToggleMcpResponse>,
                            cx: ConnectionTo<Conductor>| {
                    handle_toggle_mcp_request(req, responder, cx, state.clone()).await
                }
            },
            sacp::on_receive_request!(),
        )
        .on_receive_request_from(
            sacp::Client,
            {
                let state = state.clone();
                async move |req: SetTeamAgentEngineRequest,
                            responder: Responder<SetTeamAgentEngineResponse>,
                            cx: ConnectionTo<Conductor>| {
                    handle_set_team_agent_engine_request(req, responder, cx, state.clone()).await
                }
            },
            sacp::on_receive_request!(),
        )
}

pub async fn handle_capabilities_request(
    req: CapabilitiesRequest,
    responder: Responder<CapabilitiesResponse>,
    _cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    info!("Intercepted CapabilitiesRequest");

    let settings_snapshot = state.infra.settings_store.get();
    let enabled_engines = settings_snapshot.agent.enabled_engines;
    let team_disabled = settings_snapshot.agent.team_agent_disabled_capabilities;

    let mut skills = Vec::new();
    let mut mcps = Vec::new();

    if enabled_engines.contains(&"gemini".to_string()) {
        let (g_skills, g_mcps) = crate::capabilities::read_gemini_capabilities();
        skills.extend(g_skills);
        mcps.extend(g_mcps);
    }

    if enabled_engines.contains(&"codex".to_string()) {
        skills.push(json!({
            "name": "codex",
            "description": "Codex default coding skill",
            "isBuiltin": true,
            "disabled": false
        }));
    }

    if let Some(agent) = req.agent_id.as_deref().filter(|s| {
        let ilhae_dir = dirs::home_dir()
            .map(|h| h.join(crate::helpers::ILHAE_DIR_NAME))
            .unwrap_or_default();
        !crate::context_proxy::load_team_runtime_config(&ilhae_dir)
            .map(|cfg| {
                cfg.agents
                    .iter()
                    .any(|a| a.role.to_lowercase() == s.to_lowercase() && a.is_main)
            })
            .unwrap_or(false)
    }) && let Some(overrides) = team_disabled.get(agent)
    {
        for skill in &mut skills {
            if let Some(name) = skill.get("name").and_then(|v| v.as_str())
                && overrides.skills.contains(&name.to_string())
            {
                skill["disabled"] = json!(true);
            }
        }
        for mcp in &mut mcps {
            if let Some(name) = mcp.get("name").and_then(|v| v.as_str())
                && overrides.mcps.contains(&name.to_string())
            {
                mcp["disabled"] = json!(true);
            }
        }
    }

    // Sync all discovered skills into ~/ilhae/brain/skills/ for persistence
    crate::capabilities::sync_acp_skills_to_brain(&skills);
    // Also sync Gemini CLI built-in skills from source tree
    crate::capabilities::sync_gemini_builtin_skills_from_source();

    if let Some(session_id) = req.session_id.as_deref()
        && let Ok(Some(session)) = state.infra.brain.session_get_raw(session_id)
    {
        let override_obj: serde_json::Value =
            serde_json::from_str(&session.capabilities_override).unwrap_or(json!({}));

        let session_disabled_skills: Vec<String> = override_obj
            .pointer("/skills/disabled")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        for skill in &mut skills {
            if let Some(name) = skill.get("name").and_then(|v| v.as_str()) {
                if session_disabled_skills.contains(&name.to_string()) {
                    skill["disabled"] = json!(true);
                } else {
                    // Session override scope is explicit; if not listed, treat as enabled.
                    skill["disabled"] = json!(false);
                }
            }
        }

        if let Some(mcps_obj) = override_obj.get("mcps").and_then(|v| v.as_object()) {
            for mcp in &mut mcps {
                if let Some(name) = mcp.get("name").and_then(|v| v.as_str())
                    && let Some(state_obj) = mcps_obj.get(name).and_then(|v| v.as_object())
                    && let Some(enabled) = state_obj.get("enabled").and_then(|v| v.as_bool())
                {
                    mcp["disabled"] = json!(!enabled);
                }
            }
        }
    }

    responder.respond(CapabilitiesResponse {
        skills,
        mcps,
        engines: HashMap::new(),
    })
}

pub async fn handle_toggle_skill_request(
    req: ToggleSkillRequest,
    responder: Responder<ToggleSkillResponse>,
    _cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    if let Some(session_id) = req.session_id.as_deref()
        && let Ok(Some(session)) = state.infra.brain.session_get_raw(session_id)
    {
        let mut override_obj: serde_json::Value =
            serde_json::from_str(&session.capabilities_override).unwrap_or(json!({}));

        let mut disabled_skills: Vec<String> = override_obj
            .pointer("/skills/disabled")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        if !req.enable {
            if !disabled_skills.contains(&req.name) {
                disabled_skills.push(req.name.clone());
            }
        } else {
            disabled_skills.retain(|x| x != &req.name);
        }

        if !override_obj.is_object() {
            override_obj = json!({});
        }
        let obj = override_obj
            .as_object_mut()
            .expect("override object already normalized");
        let skills_obj = obj
            .entry("skills")
            .or_insert(json!({}))
            .as_object_mut()
            .expect("skills object must be json object");
        skills_obj.insert("disabled".to_string(), json!(disabled_skills));

        let _ = state.infra.brain.session_update_capabilities(
            session_id,
            &serde_json::to_string(&override_obj).unwrap_or_default(),
        );
        return responder.respond(ToggleSkillResponse {
            success: true,
            error: None,
        });
    }

    let agent = req.agent_id.as_deref().filter(|s| {
        let ilhae_dir = dirs::home_dir()
            .map(|h| h.join(crate::helpers::ILHAE_DIR_NAME))
            .unwrap_or_default();
        !crate::context_proxy::load_team_runtime_config(&ilhae_dir)
            .map(|cfg| {
                cfg.agents
                    .iter()
                    .any(|a| a.role.to_lowercase() == s.to_lowercase() && a.is_main)
            })
            .unwrap_or(false)
    });
    let _ = crate::capabilities::toggle_skill(&req.name, !req.enable, agent);
    responder.respond(ToggleSkillResponse {
        success: true,
        error: None,
    })
}

pub async fn handle_toggle_mcp_request(
    req: ToggleMcpRequest,
    responder: Responder<ToggleMcpResponse>,
    _cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    if let Some(session_id) = req.session_id.as_deref()
        && let Ok(Some(session)) = state.infra.brain.session_get_raw(session_id)
    {
        let mut override_obj: serde_json::Value =
            serde_json::from_str(&session.capabilities_override).unwrap_or(json!({}));

        if !override_obj.is_object() {
            override_obj = json!({});
        }
        let obj = override_obj
            .as_object_mut()
            .expect("override object already normalized");
        let mcps_obj = obj
            .entry("mcps")
            .or_insert(json!({}))
            .as_object_mut()
            .expect("mcps object must be json object");
        mcps_obj.insert(req.name.clone(), json!({ "enabled": req.enable }));

        let _ = state.infra.brain.session_update_capabilities(
            session_id,
            &serde_json::to_string(&override_obj).unwrap_or_default(),
        );
        return responder.respond(ToggleMcpResponse {
            success: true,
            error: None,
        });
    }

    let agent = req.agent_id.as_deref().filter(|s| {
        let ilhae_dir = dirs::home_dir()
            .map(|h| h.join(crate::helpers::ILHAE_DIR_NAME))
            .unwrap_or_default();
        !crate::context_proxy::load_team_runtime_config(&ilhae_dir)
            .map(|cfg| {
                cfg.agents
                    .iter()
                    .any(|a| a.role.to_lowercase() == s.to_lowercase() && a.is_main)
            })
            .unwrap_or(false)
    });
    let _ = crate::capabilities::toggle_mcp(&req.name, !req.enable, agent);
    responder.respond(ToggleMcpResponse {
        success: true,
        error: None,
    })
}

pub async fn handle_set_team_agent_engine_request(
    req: SetTeamAgentEngineRequest,
    responder: Responder<SetTeamAgentEngineResponse>,
    _cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    if let Ok(Some(session)) = state.infra.brain.session_get_raw(&req.session_id) {
        let mut override_obj: serde_json::Value =
            serde_json::from_str(&session.capabilities_override).unwrap_or(json!({}));
        if !override_obj.is_object() {
            override_obj = json!({});
        }

        let obj = override_obj
            .as_object_mut()
            .expect("override object already normalized");
        let engines_obj = obj
            .entry("engines")
            .or_insert(json!({}))
            .as_object_mut()
            .expect("engines object must be json object");

        let role_key = match req.role.to_lowercase().as_str() {
            "leader" | "manager" => "Leader",
            "researcher" => "Researcher",
            "verifier" | "reviewer" => "Verifier",
            "creator" | "coder" => "Creator",
            _ => req.role.as_str(),
        };
        engines_obj.insert(role_key.to_string(), json!(req.engine));

        let _ = state.infra.brain.session_update_capabilities(
            &req.session_id,
            &serde_json::to_string(&override_obj).unwrap_or_default(),
        );
    }

    responder.respond(SetTeamAgentEngineResponse { success: true })
}
