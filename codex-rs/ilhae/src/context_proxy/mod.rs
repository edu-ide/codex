//! ContextProxy — Permission handling, config sync, and prompt context injection.
//!
//! Handles:
//! - Agent → Client: Permission requests (YOLO / allowlist auto-approval)
//! - Client → Proxy: SetSessionConfigOption (Codex config.toml sync)
//! - Client → Agent: PromptRequest (context injection + response save)

use std::sync::Arc;
use std::sync::atomic::Ordering;

use agent_client_protocol_schema::{
    CancelNotification, ContentBlock, PromptRequest, SessionId, SetSessionModeRequest,
    SetSessionModeResponse, StopReason, TextContent,
};
use sacp::{Agent, Client, Conductor, ConnectTo, ConnectionTo, Proxy, Responder};
use tracing::debug;

use crate::approval_manager::ApprovalEvent;
use crate::{
    IlhaeAppTimelineSubscribeRequest, IlhaeAppTimelineSubscribeResponse,
    IlhaeAppTurnInterruptRequest, IlhaeAppTurnInterruptResponse, IlhaeAppTurnStartRequest,
    IlhaeAppTurnStartResponse, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
};

pub mod autonomy;
pub mod capabilities;
pub mod client_sync;
pub mod execution_mode;
pub mod fs_handlers;
pub mod middleware;
pub mod permissions;
pub mod prompt;
pub mod prompt_finalize;
pub mod role_parser;
pub mod routing;
pub mod team_a2a;
pub mod team_preflight;
pub mod team_utils;
pub mod terminal_handlers;

use self::client_sync::handle_set_session_config_option;

pub use role_parser::*;
pub use routing::*;
pub use team_a2a::*;

fn split_tool_name(tool_call: &serde_json::Value) -> String {
    let raw = tool_call
        .get("name")
        .or_else(|| tool_call.get("toolName"))
        .or_else(|| tool_call.get("tool_name"))
        .or_else(|| tool_call.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let raw = raw
        .replace("(ilhae-tools mcp server)", "")
        .trim()
        .to_string();
    let raw = raw
        .strip_prefix("mcp_ilhae-tools_")
        .unwrap_or(&raw)
        .to_string();
    raw.strip_prefix("ilhae-tools__")
        .unwrap_or(&raw)
        .to_string()
}

fn split_tool_input<'a>(tool_call: &'a serde_json::Value) -> Option<&'a serde_json::Value> {
    tool_call
        .get("rawInput")
        .or_else(|| tool_call.get("raw_input"))
        .or_else(|| tool_call.get("input"))
}

fn split_tool_output(tool_call: &serde_json::Value) -> Option<serde_json::Value> {
    let raw = tool_call
        .get("rawOutput")
        .or_else(|| tool_call.get("raw_output"))
        .or_else(|| tool_call.get("output"))
        .or_else(|| tool_call.get("responseText"))?;
    if let Some(s) = raw.as_str() {
        serde_json::from_str::<serde_json::Value>(s)
            .ok()
            .or_else(|| Some(serde_json::json!(s)))
    } else {
        Some(raw.clone())
    }
}

pub fn persist_team_split_messages(
    store: &crate::session_store::SessionStore,
    parent_session_id: &str,
    _user_text: &str,
    _assistant_text: &str,
    _parent_agent_id: &str,
    tool_calls_json: &str,
    structured: Option<&serde_json::Value>,
) {
    let mut tool_calls: Vec<serde_json::Value> =
        serde_json::from_str(tool_calls_json).unwrap_or_default();

    if tool_calls.is_empty()
        && let Some(structured) = structured
        && let Some((_content, structured_tool_calls)) =
            crate::context_proxy::team_utils::synthesize_assistant_from_a2a_structured(structured)
    {
        tool_calls = structured_tool_calls;
    }

    let split_messages =
        crate::context_proxy::team_utils::extract_team_split_messages_from_tool_calls(&tool_calls);

    for split in split_messages {
        let tool_name = split_tool_name(&split.tool_call);
        let input = split_tool_input(&split.tool_call);
        let output = split_tool_output(&split.tool_call).unwrap_or(serde_json::Value::Null);
        let request = input
            .and_then(|v| {
                v.get("query")
                    .or_else(|| v.get("request"))
                    .or_else(|| v.get("message"))
                    .or_else(|| v.get("proposal"))
            })
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let task_id = input
            .and_then(|v| v.get("task_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                output
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        let mode = output
            .get("mode")
            .and_then(|v| v.as_str())
            .or_else(|| input.and_then(|v| v.get("mode")).and_then(|v| v.as_str()))
            .unwrap_or(if tool_name == "delegate_background" {
                "background"
            } else if tool_name == "subscribe_task" {
                "subscribe"
            } else {
                "sync"
            });
        let single_tool_call_json = serde_json::to_string(&vec![split.tool_call.clone()])
            .unwrap_or_else(|_| "[]".to_string());

        let events = crate::team_timeline::project_split_message_events(
            &split,
            &tool_name,
            &request,
            &task_id,
            mode,
            &output,
            &single_tool_call_json,
        );
        crate::team_timeline::persist_events(store, parent_session_id, events);
    }
}

const PINNED_FETCH_TIMEOUT_MS: u64 = 400;
const MEMORY_SEARCH_TIMEOUT_MS: u64 = 500;
const PROMPT_PREFLIGHT_WARN_THRESHOLD_MS: u64 = 800;
// ─── ContextProxy state ─────────────────────────────────────────────────

pub struct ContextProxy {
    pub state: Arc<crate::SharedState>,
}

impl ConnectTo<Conductor> for ContextProxy {
    async fn connect_to(self, conductor: impl ConnectTo<Proxy>) -> Result<(), sacp::Error> {
        let s = self.state;

        // ═══ Approval Event Forwarder ═══
        // Subscribe to ApprovalManager broadcast and forward events to desktop
        // via extNotification. This is the architectural sync mechanism — all
        // channels (Telegram, future Discord/Slack) resolve through ApprovalManager,
        // and this listener ensures desktop always gets notified.
        {
            let mut approval_rx = s.infra.approval_manager.subscribe();
            let cx_cache = s.infra.relay_conductor_cx.clone();
            tokio::spawn(async move {
                while let Ok(event) = approval_rx.recv().await {
                    let (method, payload) = match &event {
                        ApprovalEvent::Resolved {
                            permission_id,
                            option_id,
                            resolved_by,
                        } => (
                            "ilhae/approval_resolved",
                            serde_json::json!({
                                "permission_id": permission_id,
                                "option_id": option_id,
                                "resolved_by": resolved_by,
                            }),
                        ),
                        ApprovalEvent::Expired { permission_id } => (
                            "ilhae/approval_expired",
                            serde_json::json!({
                                "permission_id": permission_id,
                            }),
                        ),
                        _ => continue, // NewRequest is handled by channel clients directly
                    };
                    cx_cache.notify_desktop(method, payload).await;
                }
            });
        }

        let builder = Proxy.builder()
            .name("context-proxy")
            // ═══ Client → Agent: session/cancel (record cancel version + forward) ═══
            .on_receive_notification_from(Client, {
                let session_cancel_ver = s.sessions.cancel_ver.clone();
                let cancel_version = s.sessions.cancel_version.clone();
                move |notif: CancelNotification, cx: ConnectionTo<Conductor>| {
                    let session_cancel_ver = session_cancel_ver.clone();
                    let cancel_version = cancel_version.clone();
                    async move {
                        let session_id = notif.session_id.0.to_string();
                        let ver = cancel_version.fetch_add(1, Ordering::Relaxed) + 1;
                        {
                            session_cancel_ver.insert(session_id.clone(), ver);
                        }
                        debug!(
                            "[Cancel] Marked session {} cancel version={} and forwarding session/cancel",
                            session_id, ver
                        );
                        cx.send_notification_to(Agent, notif)?;
                        Ok(())
                    }
                }
            }, sacp::on_receive_notification!());

        // ═══ Apply Middlewares ═══
        let builder = permissions::bind_routes(builder, s.clone());
        let builder = capabilities::bind_routes(builder, s.clone());
        let builder = prompt::bind_routes(builder, s.clone());
        let builder = fs_handlers::bind_routes(builder, s.clone());
        let builder = terminal_handlers::bind_routes(builder, s.clone());

        builder
            // ═══ Client → Proxy: SetSessionConfigOption (intercept for Codex config.toml sync) ═══
            .on_receive_request_from(
                Client,
                {
                    let state = s.clone();
                    async move |req: SetSessionConfigOptionRequest,
                                responder: Responder<SetSessionConfigOptionResponse>,
                                cx: ConnectionTo<Conductor>| {
                        handle_set_session_config_option(req, responder, cx, state.clone()).await
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Client → Agent: SetSessionMode (passthrough) ═══
            .on_receive_request_from(
                Client,
                {
                    async move |req: SetSessionModeRequest,
                                responder: Responder<SetSessionModeResponse>,
                                cx: ConnectionTo<Conductor>| {
                        debug!("[session/set_mode] modeId={}", req.mode_id);
                        cx.send_request_to(Agent, req)
                            .forward_response_to(responder)
                    }
                },
                sacp::on_receive_request!(),
            )
            .on_receive_request_from(
                Client,
                {
                    let cx_cache = s.infra.relay_conductor_cx.clone();
                    async move |req: IlhaeAppTimelineSubscribeRequest,
                                responder: Responder<IlhaeAppTimelineSubscribeResponse>,
                                cx: ConnectionTo<Conductor>| {
                        let session_ids = req.normalized_session_ids();
                        let primary_session_id = session_ids.first().cloned().unwrap_or_default();
                        cx_cache.try_add(cx.clone()).await;
                        cx_cache
                            .set_timeline_subscriptions(&cx, session_ids.clone())
                            .await;
                        responder.respond(IlhaeAppTimelineSubscribeResponse {
                            ok: true,
                            session_id: primary_session_id,
                            session_ids,
                            notification_method: crate::types::NOTIF_APP_SESSION_EVENT.to_string(),
                        })
                    }
                },
                sacp::on_receive_request!(),
            )
            .on_receive_request_from(
                Client,
                {
                    async move |req: IlhaeAppTurnStartRequest,
                                responder: Responder<IlhaeAppTurnStartResponse>,
                                cx: ConnectionTo<Conductor>| {
                        let session_id = req
                            .session_id
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                        let mut prompt_req = PromptRequest::new(
                            SessionId::new(session_id.clone()),
                            vec![ContentBlock::Text(TextContent::new(req.input))],
                        );
                        if let Some(agent_id) = req.agent_id.filter(|s| !s.trim().is_empty()) {
                            prompt_req.meta = serde_json::json!({ "agentId": agent_id })
                                .as_object()
                                .cloned();
                        }
                        let response = cx.send_request_to(Agent, prompt_req).block_task().await?;
                        let stop_reason = match response.stop_reason {
                            StopReason::Cancelled => "cancelled",
                            StopReason::EndTurn => "completed",
                            StopReason::MaxTokens => "max_tokens",
                            StopReason::MaxTurnRequests => "max_turn_requests",
                            StopReason::Refusal => "refusal",
                            _ => "unknown",
                        }
                        .to_string();
                        responder.respond(IlhaeAppTurnStartResponse {
                            session_id,
                            stop_reason,
                            meta: serde_json::to_value(response.meta)
                                .unwrap_or(serde_json::Value::Null),
                        })
                    }
                },
                sacp::on_receive_request!(),
            )
            .on_receive_request_from(
                Client,
                {
                    async move |req: IlhaeAppTurnInterruptRequest,
                                responder: Responder<IlhaeAppTurnInterruptResponse>,
                                cx: ConnectionTo<Conductor>| {
                        cx.send_notification_to(
                            Agent,
                            CancelNotification::new(SessionId::new(req.session_id.clone())),
                        )?;
                        responder.respond(IlhaeAppTurnInterruptResponse {
                            ok: true,
                            session_id: req.session_id,
                            turn_id: req.turn_id,
                        })
                    }
                },
                sacp::on_receive_request!(),
            )
            .connect_with(conductor, async move |_cx: ConnectionTo<Conductor>| {
                std::future::pending::<Result<(), sacp::Error>>().await
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::extract_team_role_sections;

    #[test]
    fn parses_bold_team_sections() {
        let text = "**Leader (계획):** 계획\n**Researcher (자료 조사):** 조사\n**Verifier (검증):** 검증\n**Creator (최종 정리):** 정리";
        let sections = extract_team_role_sections(text);
        assert_eq!(sections.len(), 4);
        assert_eq!(sections[0].0, "Leader");
        assert_eq!(sections[1].0, "Researcher");
        assert_eq!(sections[2].0, "Verifier");
        assert_eq!(sections[3].0, "Creator");
    }
}
