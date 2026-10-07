use std::collections::HashMap;
use std::sync::Arc;

use sacp::Conductor;
use sacp::ConnectionTo;
use sacp::Responder;
use serde_json::json;
use tracing::info;

use crate::SharedState;
use crate::types::CapabilitiesRequest;
use crate::types::CapabilitiesResponse;
use crate::types::SetTeamAgentEngineRequest;
use crate::types::SetTeamAgentEngineResponse;
use crate::types::ToggleMcpRequest;
use crate::types::ToggleMcpResponse;
use crate::types::ToggleSkillRequest;
use crate::types::ToggleSkillResponse;

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

    // Brain skills and configured MCP servers are shared across engines and profiles.
    let (mut skills, mut mcps) = crate::capabilities::read_gemini_capabilities();

    // Sync all discovered skills into ~/ilhae/brain/skills/ for persistence
    crate::capabilities::sync_acp_skills_to_brain(&skills);
    // Also sync Gemini CLI built-in skills from source tree
    crate::capabilities::sync_gemini_builtin_skills_from_source();

    if req.agent_id.is_none()
        && let Some(session_id) = req.session_id.as_deref()
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

    if let Some(role) = req.agent_id.as_deref() {
        let data_dir = crate::config::resolve_ilhae_data_dir();
        let target = crate::context_proxy::load_team_runtime_config(&data_dir)
            .and_then(|team| {
                team.agents
                    .into_iter()
                    .find(|agent| agent.role.eq_ignore_ascii_case(role))
            })
            .unwrap_or_else(|| crate::context_proxy::TeamRoleTarget {
                role: role.to_owned(),
                endpoint: String::new(),
                system_prompt: String::new(),
                engine: String::new(),
                model: String::new(),
                skills: Vec::new(),
                mcp_servers: Vec::new(),
                is_main: false,
            });
        match crate::context_proxy::role_parser::team_capabilities::resolve_agent_capabilities(
            &data_dir, &target,
        ) {
            Ok(selected) => {
                for skill in &mut skills {
                    if let Some(name) = skill.get("name").and_then(serde_json::Value::as_str) {
                        let excluded = selected
                            .skills
                            .as_ref()
                            .is_some_and(|names| !names.iter().any(|allowed| allowed == name))
                            || selected
                                .disabled_skills
                                .iter()
                                .any(|disabled| disabled == name);
                        if selected.skills.is_some() || excluded {
                            skill["disabled"] = json!(excluded);
                        }
                    }
                }
                for mcp in &mut mcps {
                    if let Some(name) = mcp.get("name").and_then(serde_json::Value::as_str) {
                        let excluded = selected
                            .mcp_servers
                            .as_ref()
                            .is_some_and(|names| !names.iter().any(|allowed| allowed == name))
                            || selected
                                .disabled_mcps
                                .iter()
                                .any(|disabled| disabled == name);
                        if selected.mcp_servers.is_some() || excluded {
                            mcp["disabled"] = json!(excluded);
                        }
                    }
                }
            }
            Err(error) => {
                tracing::warn!("Cannot resolve {} capabilities: {}", role, error);
                for capability in skills.iter_mut().chain(mcps.iter_mut()) {
                    capability["disabled"] = json!(true);
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
    if req.agent_id.is_none()
        && let Some(session_id) = req.session_id.as_deref()
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

    let result = crate::capabilities::toggle_skill(&req.name, !req.enable, req.agent_id.as_deref());
    responder.respond(ToggleSkillResponse {
        success: result.is_ok(),
        error: result.err(),
    })
}

pub async fn handle_toggle_mcp_request(
    req: ToggleMcpRequest,
    responder: Responder<ToggleMcpResponse>,
    _cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    if req.agent_id.is_none()
        && let Some(session_id) = req.session_id.as_deref()
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

    let result = crate::capabilities::toggle_mcp(&req.name, !req.enable, req.agent_id.as_deref());
    responder.respond(ToggleMcpResponse {
        success: result.is_ok(),
        error: result.err(),
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
