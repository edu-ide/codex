//! AgentRouter — Conductor chain proxy that routes requests to the correct agent.
//!
//! Sits at the end of the proxy chain (after Relay, before Agent).
//! In solo mode: pass-through to the default Agent.
//! In team mode: this is now primarily a compatibility/fallback layer.
//! The canonical direct-target path lives earlier in `context_proxy::prompt`
//! and uses A2A southbound. AgentRouter remains useful when requests reach
//! the end of the conductor chain and still need defensive fallback routing.
//!
//! ```text
//! Client → [Admin → Tools → Persist → Context → Relay → AgentRouter] → Agent
//!                                                          ↓ (fallback)
//!                                                       compat A2A route
//! ```

use std::sync::Arc;

use a2a_acp_adapter::{AcpEventMapper, AcpMappedEvent};
use a2a_rs::proxy::SessionContext;
use a2a_rs::{A2aProxy, StreamEvent};
use agent_client_protocol_schema::{ContentBlock, PromptRequest, PromptResponse, StopReason};
use sacp::{Agent, Client, Conductor, ConnectTo, ConnectionTo, Proxy, Responder, UntypedMessage};
use serde_json::json;
use tracing::{debug, info, warn};

use crate::context_proxy::{TeamRuntimeConfig, load_team_runtime_config};

// ─── AgentRouter state ──────────────────────────────────────────────────

pub struct AgentRouter {
    pub state: Arc<crate::SharedState>,
}

impl ConnectTo<Conductor> for AgentRouter {
    async fn connect_to(self, conductor: impl ConnectTo<Proxy>) -> Result<(), sacp::Error> {
        let s = self.state;

        Proxy.builder()
            .name("agent-router")
            // ═══ Client → Agent: PromptRequest (route by agent_id or @mention) ═══
            .on_receive_request_from(Client, {
                let state = s.clone();
                async move |req: PromptRequest, responder: Responder<PromptResponse>, cx: ConnectionTo<Conductor>| {
                    // 1. Check if team mode is enabled
                    let settings = state.infra.settings_store.get();
                    if !settings.agent.team_mode {
                        // Solo mode: pass-through to default agent
                        debug!("[AgentRouter] Solo mode — forwarding to default agent");
                        let _ = cx.send_request_to(Agent, req).forward_response_to(responder);
                        return Ok(());
                    }

                    // 2. Determine target agent from _meta.agentId or @mention
                    let target_agent = resolve_target_agent(&req, &state.infra.ilhae_dir);

                    let Some((target_role, clean_prompt)) = target_agent else {
                        // No specific target — forward to default agent (leader)
                        debug!("[AgentRouter] Team mode, no target — forwarding to leader agent");
                        let _ = cx.send_request_to(Agent, req).forward_response_to(responder);
                        return Ok(());
                    };

                    info!("[AgentRouter] Compatibility routing to '{}' via A2A fallback", target_role);

                    // 3. Resolve target agent from team config and route southbound via A2A
                    let Some(team_cfg) = load_team_runtime_config(&state.infra.ilhae_dir) else {
                        warn!("[AgentRouter] team config missing, falling back to leader");
                        let _ = cx.send_request_to(Agent, req).forward_response_to(responder);
                        return Ok(());
                    };
                    let Some(target_agent) = team_cfg.agents.iter().find(|a| a.role.eq_ignore_ascii_case(&target_role)).cloned() else {
                        warn!("[AgentRouter] Agent '{}' not found in team config, falling back to leader", target_role);
                        let _ = cx.send_request_to(Agent, req).forward_response_to(responder);
                        return Ok(());
                    };

                    // 4. Build A2A session context from current proxy session
                    let session_id = req.session_id.0.to_string();
                    let cwd = state.infra.brain.sessions()
                        .get_session(&session_id)
                        .ok()
                        .and_then(|s| s.map(|v| v.cwd))
                        .filter(|c| !c.trim().is_empty())
                        .unwrap_or_else(|| "/".to_string());

                    let mcp_servers_json = {
                        let map = &state.sessions.mcp_servers;
                        map.get(&session_id).and_then(|servers| serde_json::to_value(servers).ok())
                    };

                    let mut session_ctx = SessionContext::new()
                        .with_cwd(cwd.clone())
                        .with_admin_skills(true)
                        .with_disabled_skills(vec![])
                        .with_extra_skills_dirs(vec![
                            crate::config::get_active_vault_dir()
                                .join("skills")
                                .to_string_lossy()
                                .to_string(),
                        ]);
                    if let Some(mcp_json) = mcp_servers_json {
                        session_ctx = session_ctx.with_mcp_servers(mcp_json);
                    }

                    let proxy = A2aProxy::with_context(&target_agent.endpoint, &target_agent.role, session_ctx);

                    // 5. Send prompt via A2A (southbound canonical path)
                    let prompt_text = if clean_prompt.is_empty() {
                        prompt_blocks_to_text(&req.prompt)
                    } else {
                        clean_prompt
                    };

                    match proxy.send_and_observe(&prompt_text, Some(session_id.clone()), None).await {
                        Ok((result_text, events)) => {
                            let mut mapper = AcpEventMapper::new(&["researcher", "verifier", "creator", "creator_1", "creator_2", "leader"]);
                            let mut mapped_text = result_text.clone();
                            for event in &events {
                                match event {
                                    StreamEvent::StatusUpdate(su) => {
                                        match mapper.map_status_update(su) {
                                            AcpMappedEvent::TextUpdate(text) => {
                                                if !text.trim().is_empty() {
                                                    mapped_text = text;
                                                }
                                            }
                                            AcpMappedEvent::ToolCallUpdate { .. }
                                            | AcpMappedEvent::DelegationEvent { .. }
                                            | AcpMappedEvent::None => {}
                                        }
                                    }
                                    StreamEvent::ArtifactUpdate(au) => {
                                        if let Some(text) = mapper.map_artifact_update(au) {
                                            if !text.trim().is_empty() {
                                                mapped_text = text;
                                            }
                                        }
                                    }
                                    StreamEvent::Task(task) => {
                                        if let Some(msg) = &task.status.message {
                                            let text = extract_text_from_parts(&msg.parts);
                                            if !text.trim().is_empty() {
                                                mapped_text = text;
                                            }
                                        }
                                    }
                                    StreamEvent::Message(msg) => {
                                        let text = extract_text_from_parts(&msg.parts);
                                        if !text.trim().is_empty() {
                                            mapped_text = text;
                                        }
                                    }
                                }
                            }
                            // Emit session update notification for the routed agent
                            let turn_id = format!("agent-router-{}", uuid::Uuid::new_v4());
                            let item_id = format!("{turn_id}:{target_role}");
                            if let Ok(notif) = UntypedMessage::new(
                                crate::types::NOTIF_APP_SESSION_EVENT,
                                crate::types::IlhaeAppSessionEventNotification {
                                    engine: target_role.to_string(),
                                    event: crate::types::IlhaeAppSessionEventDto::MessageDelta {
                                        thread_id: session_id.clone(),
                                        turn_id: turn_id.clone(),
                                        item_id,
                                        channel: "assistant".to_string(),
                                        delta: mapped_text.clone(),
                                    },
                                },
                            ) {
                                let _ = cx.send_notification_to(Client, notif);
                            }
                            if let Ok(notif) = UntypedMessage::new(
                                crate::types::NOTIF_APP_SESSION_EVENT,
                                crate::types::IlhaeAppSessionEventNotification {
                                    engine: target_role.to_string(),
                                    event: crate::types::IlhaeAppSessionEventDto::TurnCompleted {
                                        thread_id: session_id.clone(),
                                        turn_id,
                                        status: "completed".to_string(),
                                    },
                                },
                            ) {
                                let _ = cx.send_notification_to(Client, notif);
                            }

                            // Persist to DB
                            state.infra.brain.sessions().add_full_message(
                                &session_id, "assistant", &mapped_text, &target_role,
                                "", &mapper.tool_calls_json(), 0, 0, 0, 0,
                            ).ok();

                            let response_meta = json!({
                                "direct_agent": target_role,
                                "transport": "a2a",
                            })
                            .as_object()
                            .cloned()
                            .unwrap_or_default();
                            responder.respond(PromptResponse::new(StopReason::EndTurn).meta(response_meta))?;
                        }
                        Err(e) => {
                            warn!("[AgentRouter] AgentPool prompt failed for {}: {}, falling back to leader", target_role, e);
                            // Fall back to default agent
                            let _ = cx.send_request_to(Agent, req).forward_response_to(responder);
                        }
                    }

                    Ok(())
                }
            }, sacp::on_receive_request!())
            .connect_with(conductor, async move |_cx: ConnectionTo<Conductor>| {
                std::future::pending::<Result<(), sacp::Error>>().await
            })
            .await
    }
}

// ─── Helper functions ───────────────────────────────────────────────────

fn extract_text_from_parts(parts: &[a2a_rs::Part]) -> String {
    a2a_rs::proxy::extract_text_from_parts(parts)
}

/// Resolve the target agent from PromptRequest.
/// Checks: 1) `_meta.agentId`, 2) `@mention` in first text block.
/// Returns `(role_key, clean_prompt_text)` or None.
pub fn resolve_target_agent(
    req: &PromptRequest,
    ilhae_dir: &std::path::Path,
) -> Option<(String, String)> {
    // 1. Check _meta.agentId
    if let Some(meta) = &req.meta {
        if let Some(agent_id) = meta.get("agentId").and_then(|v| v.as_str()) {
            let role = agent_id.to_lowercase();
            // Only route to non-main agents; main agent is the default fallback
            if !role.is_empty() {
                let is_main_agent = load_team_runtime_config(ilhae_dir)
                    .map(|cfg| {
                        cfg.agents
                            .iter()
                            .any(|a| a.role.to_lowercase() == role && a.is_main)
                    })
                    .unwrap_or(false);
                if !is_main_agent {
                    let text = prompt_blocks_to_text(&req.prompt);
                    return Some((role, text));
                }
            }
        }
    }

    // 2. Check @mention in prompt text
    let text = prompt_blocks_to_text(&req.prompt);
    let trimmed = text.trim();
    if !trimmed.starts_with('@') {
        return None;
    }

    let team_cfg = load_team_runtime_config(ilhae_dir)?;
    extract_mention(&trimmed, &team_cfg)
}

/// Extract @mention from text, matching against team config roles.
pub fn extract_mention(text: &str, team: &TeamRuntimeConfig) -> Option<(String, String)> {
    let after_at = &text[1..];
    let mention_end = after_at
        .find(|c: char| c.is_whitespace())
        .unwrap_or(after_at.len());
    let mentioned = after_at[..mention_end].to_lowercase();

    if mentioned.is_empty() {
        return None;
    }

    let matched = team.agents.iter().find(|a| {
        let role_key = a.role.to_lowercase();
        !a.is_main && (role_key == mentioned || role_key.starts_with(&mentioned))
    });

    matched.map(|agent| {
        let role_key = agent.role.to_lowercase();
        let remaining = after_at[mention_end..].trim().to_string();
        (role_key, remaining)
    })
}

/// Extract text content from prompt ContentBlocks.
pub fn prompt_blocks_to_text(blocks: &[ContentBlock]) -> String {
    let mut parts = Vec::new();
    for block in blocks {
        if let ContentBlock::Text(t) = block {
            parts.push(t.text.as_str());
        }
    }
    parts.join("\n")
}
