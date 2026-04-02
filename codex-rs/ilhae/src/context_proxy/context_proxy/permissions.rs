use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_client_protocol_schema::{
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome,
};
use sacp::{Agent, Client, Conductor, ConnectionTo, Responder, UntypedMessage};
use serde_json::json;
use tracing::{debug, info, warn};

use crate::SharedState;
use crate::approval_manager::{ApprovalOption, ApprovalRequest};
use crate::context_proxy::autonomy::state::{
    AutonomousPhase, current_autonomous_iteration, set_autonomous_phase,
    transition_autonomous_phase,
};
use crate::relay_server::{self, RelayEvent};
use crate::send_synthetic_tool_call;

const DESKTOP_CANCEL_SENTINEL_OPTION_ID: &str = "__ilhae_cancelled_by_desktop__";

fn is_auto_safe_self_improvement_tool(tool_title: &str, cfg: &crate::settings_types::Settings) -> bool {
    if !cfg.agent.self_improvement_enabled
        || !cfg
            .agent
            .self_improvement_preset
            .eq_ignore_ascii_case("safe_apply")
    {
        return false;
    }
    matches!(
        tool_title.trim(),
        "memory_promote" | "memory_extract"
    )
}

pub fn bind_routes<H>(
    builder: sacp::Builder<sacp::Proxy, H>,
    state: Arc<SharedState>,
) -> sacp::Builder<sacp::Proxy, impl sacp::HandleDispatchFrom<sacp::Conductor>>
where
    H: sacp::HandleDispatchFrom<sacp::Conductor> + 'static,
{
    builder.on_receive_request_from(
        Agent,
        {
            let state = state.clone();
            async move |req: RequestPermissionRequest,
                        responder: Responder<RequestPermissionResponse>,
                        cx: ConnectionTo<Conductor>| {
                handle_permission_request(req, responder, cx, state.clone()).await
            }
        },
        sacp::on_receive_request!(),
    )
}

pub async fn handle_permission_request(
    req: RequestPermissionRequest,
    responder: Responder<RequestPermissionResponse>,
    cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    let settings = state.infra.settings_store.clone();
    let cx_cache = state.infra.relay_conductor_cx.clone();
    let relay_state = state.infra.relay_state.clone();
    let relay_tx_perm = state.infra.relay_tx.clone();
    let approval_mgr = state.infra.approval_manager.clone();

    let full_access = settings.is_full_access();
    if !full_access {
        cx_cache.try_add(cx.clone()).await;
    }

    if full_access {
        if let Some(id) = req.options.first().map(|opt| opt.option_id.clone()) {
            debug!(
                "[FullAccess] Auto-approving permission request with option: {}",
                id
            );
            return responder.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
            ));
        }
        warn!("[FullAccess] Permission request had no options; returning Cancelled");
        return responder.respond(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ));
    }

    let cfg = settings.get();
    let track_autonomous_state = cfg.agent.autonomous_mode;

    let tool_title = req.tool_call.fields.title.clone().unwrap_or_default();
    let acp_session_id_str = req.session_id.0.to_string();
    let tool_call_id = serde_json::to_value(&req.tool_call).ok().and_then(|v| {
        v.get("toolCallId")
            .or_else(|| v.get("tool_call_id"))
            .and_then(|id| id.as_str())
            .map(|id| id.to_string())
    });
    let trace_id = uuid::Uuid::new_v4().to_string();
    let proxy_received_epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut auto_approved_plugin: Option<&str> = None;
    if let Some(plugin_id) = crate::plugins::tool_to_plugin_id(&tool_title)
        && cfg
            .permissions
            .auto_approve_plugins
            .get(plugin_id)
            .copied()
            .unwrap_or(false)
    {
        auto_approved_plugin = Some(plugin_id);
    }

    if auto_approved_plugin.is_none() && !tool_title.is_empty() {
        for (key, &enabled) in &cfg.permissions.auto_approve_plugins {
            if enabled && tool_title.starts_with(key.as_str()) {
                auto_approved_plugin = Some(key.as_str());
                break;
            }
        }
    }

    if let Some(plugin_id) = auto_approved_plugin {
        let option_id = req.options.first().map(|opt| opt.option_id.clone());
        if let Some(id) = option_id {
            info!(
                "[AutoApprove] Auto-approving tool (plugin={}): {}",
                plugin_id, tool_title
            );
            send_synthetic_tool_call(&req.session_id, &req.tool_call, &cx);
            return responder.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
            ));
        }
    }

    if is_auto_safe_self_improvement_tool(&tool_title, &cfg)
        && let Some(id) = req.options.first().map(|opt| opt.option_id.clone())
    {
        info!(
            "[SelfImprovementPolicy] Auto-approving safe apply tool: {}",
            tool_title
        );
        send_synthetic_tool_call(&req.session_id, &req.tool_call, &cx);
        return responder.respond(RequestPermissionResponse::new(
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
        ));
    }

    if !tool_title.is_empty()
        && let Some((option_id, kind)) = settings.check_allowlist(&tool_title)
    {
        info!("[Allowlist] Auto-{} for tool: {}", kind, tool_title);
        send_synthetic_tool_call(&req.session_id, &req.tool_call, &cx);
        return responder.respond(RequestPermissionResponse::new(
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id)),
        ));
    }

    let has_relay_clients = relay_state.has_connected_clients().await;
    let has_telegram_bridge = relay_state.has_telegram_bridge().await;
    // Headless fallback: no relay clients and no Telegram bridge → auto-approve first option
    if !has_relay_clients && !has_telegram_bridge {
        if let Some(opt) = req.options.first() {
            info!(
                "[HeadlessAutoApprove] No desktop/telegram; selecting {}",
                opt.option_id
            );
            send_synthetic_tool_call(&req.session_id, &req.tool_call, &cx);
            return responder.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    opt.option_id.clone(),
                )),
            ));
        } else {
            warn!("[HeadlessAutoApprove] No options; returning Cancelled");
            return responder.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ));
        }
    }

    let permission_id = uuid::Uuid::new_v4().to_string();
    let tool_kind = req
        .tool_call
        .fields
        .kind
        .as_ref()
        .map(|k| format!("{:?}", k))
        .unwrap_or_default();

    let options: Vec<ApprovalOption> = req
        .options
        .iter()
        .map(|opt| ApprovalOption {
            id: opt.option_id.to_string(),
            title: opt.name.clone(),
        })
        .collect();

    let mut description = if !tool_title.is_empty() {
        tool_title.to_string()
    } else if !tool_kind.is_empty() {
        format!("{} permission request", tool_kind)
    } else {
        "Permission request".to_string()
    };
    if description.is_empty() {
        description = "Permission request".to_string();
    }
    if description.len() > 240 {
        description.truncate(240);
        description.push_str("...");
    }

    let proxy_forward_epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(proxy_received_epoch_ms);
    if let Ok(notif) = UntypedMessage::new(
        crate::types::NOTIF_APPROVAL_TRACE_START,
        json!({
            "trace_id": trace_id,
            "phase": "proxy_forward_mirror",
            "session_id": acp_session_id_str.clone(),
            "tool_call_id": tool_call_id.clone(),
            "tool_title": tool_title.clone(),
            "proxy_received_epoch_ms": proxy_received_epoch_ms,
            "proxy_forward_epoch_ms": proxy_forward_epoch_ms,
        }),
    ) {
        let _ = cx.send_notification_to(Client, notif);
    }

    let cx_for_request = cx.clone();
    cx.spawn(async move {
        let mut desktop_fut = std::pin::pin!(cx_for_request.send_request_to(Client, req).block_task());
        let mut desktop_pending = true;
        let early_desktop = tokio::select! {
            desktop = &mut desktop_fut => Some(desktop),
            _ = tokio::time::sleep(std::time::Duration::from_millis(120)) => None,
        };

        if let Some(desktop_result) = early_desktop {
            desktop_pending = false;
            match desktop_result {
                Ok(resp) => {
                    return match resp.outcome {
                        RequestPermissionOutcome::Selected(selected) => {
                            let option_id = selected.option_id.to_string();
                            responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                    option_id,
                                )),
                            ))
                        }
                        _ => responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        )),
                    };
                }
                Err(e) => {
                    debug!(
                        "[Permission] Early desktop request failed for '{}': {}. Falling back to mirror lane...",
                        permission_id,
                        e
                    );
                }
            }
        }

        let approval_req = ApprovalRequest {
            permission_id: permission_id.clone(),
            session_id: acp_session_id_str.clone(),
            tool_title: tool_title.to_string(),
            tool_kind: tool_kind.clone(),
            description: description.clone(),
            options: options.clone(),
        };

        let rx = match approval_mgr
            .register(approval_req, std::time::Duration::from_secs(300))
            .await
        {
            Ok((_record, rx)) => rx,
            Err(e) => {
                warn!("[Permission] Failed to register approval: {}", e);
                if desktop_pending {
                    return match desktop_fut.await {
                        Ok(resp) => match resp.outcome {
                            RequestPermissionOutcome::Selected(selected) => {
                                let option_id = selected.option_id.to_string();
                                responder.respond(RequestPermissionResponse::new(
                                    RequestPermissionOutcome::Selected(
                                        SelectedPermissionOutcome::new(option_id),
                                    ),
                                ))
                            }
                            _ => responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Cancelled,
                            )),
                        },
                        Err(wait_err) => {
                            warn!(
                                "[Permission] Desktop lane also failed after register error for '{}': {}",
                                permission_id,
                                wait_err
                            );
                            responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Cancelled,
                            ))
                        }
                    };
                }
                return responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ));
            }
        };

        if track_autonomous_state {
            let loop_iteration = current_autonomous_iteration(&state, &acp_session_id_str)
                .await
                .max(1);
            set_autonomous_phase(
                &state,
                &acp_session_id_str,
                AutonomousPhase::WaitingForApproval,
                loop_iteration,
                Some(tool_title.to_string()),
                None,
            )
            .await;
        }

        let options_json: Vec<serde_json::Value> = options
            .iter()
            .map(|opt| {
                serde_json::json!({
                    "id": opt.id,
                    "title": opt.title,
                })
            })
            .collect();

        let event = RelayEvent::PermissionRequest {
            permission_id: permission_id.clone(),
            session_id: acp_session_id_str.clone(),
            tool_title: tool_title.to_string(),
            tool_kind: tool_kind.clone(),
            description,
            options: options_json,
        };

        relay_server::broadcast_event(&relay_tx_perm, event);

        let mut mirror_fut = std::pin::pin!(rx);

        enum ApprovalRace {
            Desktop(Result<RequestPermissionResponse, sacp::Error>),
            Mirror(Result<Option<String>, tokio::sync::oneshot::error::RecvError>),
        }

        let race_result = if desktop_pending {
            tokio::select! {
                desktop = &mut desktop_fut => ApprovalRace::Desktop(desktop),
                mirror = &mut mirror_fut => ApprovalRace::Mirror(mirror),
            }
        } else {
            ApprovalRace::Mirror(mirror_fut.as_mut().await)
        };

        match race_result {
            ApprovalRace::Desktop(Ok(resp)) => match resp.outcome {
                RequestPermissionOutcome::Selected(selected) => {
                    if track_autonomous_state {
                        transition_autonomous_phase(
                            &state,
                            &acp_session_id_str,
                            AutonomousPhase::ResumingAfterApproval,
                            Some("approval resolved by desktop".to_string()),
                            None,
                        )
                        .await;
                    }
                    let option_id = selected.option_id.to_string();
                    let approval_mgr_clone = approval_mgr.clone();
                    let permission_id_clone = permission_id.clone();
                    let option_id_clone = option_id.clone();
                    tokio::spawn(async move {
                        let _ = approval_mgr_clone
                            .resolve(&permission_id_clone, option_id_clone, Some("desktop".into()))
                            .await;
                    });
                    info!(
                        "[Permission] Desktop resolved '{}' → option='{}'",
                        permission_id, option_id
                    );
                    responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                            option_id,
                        )),
                    ))
                }
                _ => {
                    if track_autonomous_state {
                        transition_autonomous_phase(
                            &state,
                            &acp_session_id_str,
                            AutonomousPhase::Cancelled,
                            Some("approval cancelled by desktop".to_string()),
                            None,
                        )
                        .await;
                    }
                    let approval_mgr_clone = approval_mgr.clone();
                    let permission_id_clone = permission_id.clone();
                    tokio::spawn(async move {
                        let _ = approval_mgr_clone
                            .resolve(
                                &permission_id_clone,
                                DESKTOP_CANCEL_SENTINEL_OPTION_ID.to_string(),
                                Some("desktop".into()),
                            )
                            .await;
                    });
                    responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ))
                }
            },
            ApprovalRace::Desktop(Err(e)) => {
                debug!(
                    "[Permission] Desktop request failed for '{}': {}. Waiting mirror lane...",
                    permission_id,
                    e
                );
                match mirror_fut.as_mut().await {
                    Ok(Some(option_id)) if option_id != DESKTOP_CANCEL_SENTINEL_OPTION_ID => {
                        if track_autonomous_state {
                            transition_autonomous_phase(
                                &state,
                                &acp_session_id_str,
                                AutonomousPhase::ResumingAfterApproval,
                                Some("approval resolved by mirror".to_string()),
                                None,
                            )
                            .await;
                        }
                        info!(
                            "[Permission] Mirror resolved '{}' → option='{}'",
                            permission_id, option_id
                        );
                        responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                option_id,
                            )),
                        ))
                    }
                    Ok(Some(_)) | Ok(None) | Err(_) => {
                        if track_autonomous_state {
                            transition_autonomous_phase(
                                &state,
                                &acp_session_id_str,
                                AutonomousPhase::Cancelled,
                                Some("approval timed out or cancelled".to_string()),
                                None,
                            )
                            .await;
                        }
                        warn!("[Permission] Timeout/cancelled for '{}'", permission_id);
                        responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ))
                    }
                }
            }
            ApprovalRace::Mirror(Ok(Some(option_id))) => {
                if option_id == DESKTOP_CANCEL_SENTINEL_OPTION_ID {
                    if track_autonomous_state {
                        transition_autonomous_phase(
                            &state,
                            &acp_session_id_str,
                            AutonomousPhase::Cancelled,
                            Some("approval cancelled by mirror".to_string()),
                            None,
                        )
                        .await;
                    }
                    return responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ));
                }
                if track_autonomous_state {
                    transition_autonomous_phase(
                        &state,
                        &acp_session_id_str,
                        AutonomousPhase::ResumingAfterApproval,
                        Some("approval resolved by mirror".to_string()),
                        None,
                    )
                    .await;
                }
                info!(
                    "[Permission] Mirror-first resolved '{}' → option='{}'",
                    permission_id, option_id
                );
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id)),
                ))
            }
            ApprovalRace::Mirror(Ok(None) | Err(_)) => {
                if track_autonomous_state {
                    transition_autonomous_phase(
                        &state,
                        &acp_session_id_str,
                        AutonomousPhase::Cancelled,
                        Some("approval timed out".to_string()),
                        None,
                    )
                    .await;
                }
                warn!("[Permission] Timeout/cancelled for '{}'", permission_id);
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))
            }
        }
    })
}
