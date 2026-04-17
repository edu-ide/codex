use std::sync::Arc;
use std::{collections::BTreeMap, time::Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_client_protocol_schema::{ContentBlock, PromptRequest, PromptResponse, TextContent};
use codex_protocol::{
    items::LoopLifecycleItem,
    protocol::{LoopLifecycleKind, LoopLifecycleStatus},
};
use sacp::{Agent, Client, Conductor, ConnectionTo, Responder, UntypedMessage};
use serde_json::json;
use tracing::{debug, info, warn};

use crate::{
    SharedState, build_dynamic_instructions,
    session_context_service::{
        SessionPromptContextDeps, extract_user_text, prepare_session_prompt_context,
    },
    session_persistence_service::SessionRegistryService,
    session_recall_service::{SessionRecallDeps, prepare_prompt_recall_blocks},
};

use super::{
    PROMPT_PREFLIGHT_WARN_THRESHOLD_MS,
    execution_mode::decide_execution_mode,
    prompt_finalize::{PromptFinalizeInput, finalize_prompt_result},
    team_preflight::{TeamPromptPreparation, prepare_team_prompt, try_handle_direct_target_route},
};

fn context_loop_item_id(session_id: &str, epoch_ms: u64) -> String {
    format!("context:{session_id}:{epoch_ms}")
}

fn loop_counts(entries: impl IntoIterator<Item = (&'static str, i64)>) -> Option<BTreeMap<String, i64>> {
    let map: BTreeMap<String, i64> = entries
        .into_iter()
        .filter(|(_, value)| *value > 0)
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    (!map.is_empty()).then_some(map)
}

fn context_loop_item(
    item_id: String,
    summary: String,
    detail: Option<String>,
    status: LoopLifecycleStatus,
    counts: Option<BTreeMap<String, i64>>,
    duration_ms: Option<i64>,
    error: Option<String>,
) -> LoopLifecycleItem {
    LoopLifecycleItem {
        id: item_id,
        kind: LoopLifecycleKind::ContextInjection,
        title: "Injecting Context".to_string(),
        summary,
        detail,
        status,
        reason: Some("prompt_preflight".to_string()),
        counts,
        error,
        duration_ms,
        target_profile: None,
    }
}

pub fn bind_routes<H>(
    builder: sacp::Builder<sacp::Proxy, H>,
    state: Arc<SharedState>,
) -> sacp::Builder<sacp::Proxy, impl sacp::HandleDispatchFrom<sacp::Conductor>>
where
    H: sacp::HandleDispatchFrom<sacp::Conductor> + 'static,
{
    builder.on_receive_request_from(
        Client,
        {
            let state = state.clone();
            async move |req: PromptRequest,
                        responder: Responder<PromptResponse>,
                        cx: ConnectionTo<Conductor>| {
                handle_prompt_request(req, responder, cx, state.clone()).await
            }
        },
        sacp::on_receive_request!(),
    )
}

pub async fn handle_prompt_request(
    mut req: PromptRequest,
    responder: Responder<PromptResponse>,
    cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    let brain = state.infra.brain.clone();
    let settings = state.infra.settings_store.clone();
    let buffers = state.sessions.assistant_buffers.clone();
    let pending_history = state.sessions.pending_history.clone();
    let session_turn_seq = state.sessions.turn_seq.clone();
    let session_id_map = state.sessions.id_map.clone();
    let instr_version = state.sessions.instructions_version.clone();
    let session_instr_ver = state.sessions.instructions_ver.clone();
    let session_cancel_ver = state.sessions.cancel_ver.clone();
    let cx_cache = state.infra.relay_conductor_cx.clone();
    let ilhae_dir = state.infra.ilhae_dir.clone();

    cx_cache.try_add(cx.clone()).await;
    let session_id = req.session_id.0.to_string();
    let is_subagent = session_id.starts_with("subagent_");
    let prompt_start_cancel_ver = {
        let map = &session_cancel_ver;
        map.get(&session_id).unwrap_or(0)
    };
    let proxy_prompt_received_epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let context_loop_started_at = Instant::now();
    let context_loop_id = context_loop_item_id(&session_id, proxy_prompt_received_epoch_ms);
    if let Ok(notif) = UntypedMessage::new(
        crate::types::NOTIF_PROMPT_TRACE_START,
        json!({
            "phase": "proxy_prompt_received",
            "session_id": session_id.clone(),
            "proxy_prompt_received_epoch_ms": proxy_prompt_received_epoch_ms,
        }),
    ) {
        let _ = cx.send_notification_to(Client, notif);
    }
    let _ = cx.send_notification_to(
        Client,
        crate::types::IlhaeLoopLifecycleNotification::Started {
            session_id: session_id.clone(),
            item: context_loop_item(
                context_loop_id.clone(),
                "Loading session context".to_string(),
                None,
                LoopLifecycleStatus::InProgress,
                None,
                None,
                None,
            ),
        },
    );
    debug!("Received PromptRequest: {:?}", session_id);

    let sid_for_map = session_id.clone();
    let sid_for_buf = session_id.clone();

    let (remap_result, _) = tokio::join!(
        async {
            let map = &session_id_map;
            map.get(&sid_for_map)
        },
        async {
            let next_turn_seq = {
                let map = &session_turn_seq;
                let next = map.get(&sid_for_buf).unwrap_or(0).saturating_add(1);
                map.insert(sid_for_buf.clone(), next);
                next
            };
            let lock = buffers.clone();
            let sid_for_acc = sid_for_buf.clone();
            lock.insert(
                sid_for_buf,
                crate::turn_accumulator::TurnAccumulator::new(
                    sid_for_acc,
                    String::new(),
                    next_turn_seq,
                ),
            );
        }
    );

    if let Some(acp_session_id) = remap_result {
        info!(
            "Cross-agent: remapping PromptRequest session {} → {}",
            session_id, acp_session_id
        );
        match serde_json::from_value(json!(acp_session_id)) {
            Ok(sid) => req.session_id = sid,
            Err(e) => warn!("Failed to remap session ID: {}", e),
        }
    }

    let user_text = extract_user_text(&req.prompt);
    let context_deps = SessionPromptContextDeps {
        brain: state.infra.brain.clone(),
        settings_store: state.infra.settings_store.clone(),
        ilhae_dir: state.infra.ilhae_dir.clone(),
        reverse_session_map: Some(state.sessions.reverse_map.clone()),
        active_session_id: Some(state.sessions.active_session_id.clone()),
    };
    let prepared_context = prepare_session_prompt_context(&context_deps, &session_id, is_subagent)
        .await
        .map_err(|err| {
            let _ = cx.send_notification_to(
                Client,
                crate::types::IlhaeLoopLifecycleNotification::Failed {
                    session_id: session_id.clone(),
                    item: context_loop_item(
                        context_loop_id.clone(),
                        "Context injection failed".to_string(),
                        None,
                        LoopLifecycleStatus::Failed,
                        None,
                        Some(context_loop_started_at.elapsed().as_millis() as i64),
                        Some(err.to_string()),
                    ),
                },
            );
            sacp::util::internal_error(err.to_string())
        })?;
    let session_context_blocks = prepared_context.prompt_blocks.len() as i64;
    prepend_prompt_blocks(&mut req.prompt, prepared_context.prompt_blocks);
    let session_info = prepared_context.session_info;
    let effective_session_id = session_info
        .as_ref()
        .map(|info| info.id.clone())
        .or_else(|| state.sessions.reverse_map.get(&session_id))
        .unwrap_or_else(|| session_id.clone());
    state.sessions.connection_sessions.insert(
        crate::shared_state::connection_key(&cx),
        effective_session_id,
    );

    let sid_for_instr = session_id.clone();
    let sid_for_hist = session_id.clone();
    let settings_for_instr = settings.clone();

    let (dynamic_ctx_opt, history_opt) = tokio::join!(
        async {
            let current_ver = instr_version.load(std::sync::atomic::Ordering::Relaxed);
            let ver_map = &session_instr_ver;
            let last_ver = ver_map.get(&sid_for_instr).unwrap_or(0);
            if last_ver < current_ver {
                let dynamic_ctx = build_dynamic_instructions(&settings_for_instr.get());
                ver_map.insert(sid_for_instr, current_ver);
                if !dynamic_ctx.is_empty() {
                    Some((dynamic_ctx, last_ver, current_ver))
                } else {
                    None
                }
            } else {
                None
            }
        },
        async {
            let lock = &pending_history;
            let v = lock.get(&sid_for_hist);
            if v.is_some() {
                lock.invalidate(&sid_for_hist);
            }
            v
        }
    );

    if let Some((ref dynamic_ctx, last_ver, current_ver)) = dynamic_ctx_opt {
        info!(
            "Injecting dynamic instructions for session: {} (ver {} → {})",
            session_id, last_ver, current_ver
        );
        req.prompt
            .insert(0, ContentBlock::Text(TextContent::new(dynamic_ctx.clone())));
    }

    if let Some(ref history_text) = history_opt {
        info!("Injecting cross-agent history for session: {}", session_id);
        req.prompt
            .insert(0, ContentBlock::Text(TextContent::new(history_text.clone())));
    }

    let current_agent_id = prepared_context.current_agent_id;
    let recall_deps = SessionRecallDeps {
        brain: state.infra.brain.clone(),
    };
    let recall_blocks = prepare_prompt_recall_blocks(
        &recall_deps,
        &session_id,
        is_subagent,
        &current_agent_id,
        &user_text,
    )
    .await;
    let recall_block_count = recall_blocks.len() as i64;
    prepend_prompt_blocks(&mut req.prompt, recall_blocks);

    if let Some(info) = &session_info
        && info.title == "Untitled"
        && !user_text.trim().is_empty()
    {
        let truncated_title: String = user_text
            .trim()
            .lines()
            .next()
            .unwrap_or("New Chat")
            .chars()
            .take(40)
            .collect();
        info!("Updating session title to: {}", truncated_title);
        let _ = SessionRegistryService::update_session_title(
            &state.infra.brain,
            &session_id,
            &truncated_title,
        );
        let notif = UntypedMessage::new(
            crate::types::NOTIF_SESSION_INFO_UPDATE,
            json!({
                "sessionId": req.session_id.0,
                "update": { "sessionUpdate": "session_info_update", "title": truncated_title }
            }),
        )
        .unwrap();
        let _ = cx.send_notification_to(Client, notif);
    }

    brain
        .session_add_message_simple(&session_id, "user", &user_text, "")
        .ok();
    let dynamic_instruction_count = dynamic_ctx_opt.as_ref().map(|_| 1).unwrap_or(0);
    let cross_agent_history_count = history_opt.as_ref().map(|_| 1).unwrap_or(0);
    let mut loaded_sections = Vec::new();
    if session_context_blocks > 0 {
        loaded_sections.push(format!("session_context({session_context_blocks})"));
    }
    if dynamic_instruction_count > 0 {
        loaded_sections.push("dynamic_instructions".to_string());
    }
    if cross_agent_history_count > 0 {
        loaded_sections.push("cross_agent_history".to_string());
    }
    if recall_block_count > 0 {
        loaded_sections.push(format!("session_recall({recall_block_count})"));
    }
    let context_counts = loop_counts([
        ("session_blocks", session_context_blocks),
        ("dynamic_blocks", dynamic_instruction_count),
        ("history_blocks", cross_agent_history_count),
        ("recall_blocks", recall_block_count),
    ]);
    let context_detail = (!loaded_sections.is_empty()).then(|| loaded_sections.join(", "));
    let _ = cx.send_notification_to(
        Client,
        crate::types::IlhaeLoopLifecycleNotification::Progress {
            session_id: session_id.clone(),
            item_id: context_loop_id.clone(),
            kind: LoopLifecycleKind::ContextInjection,
            summary: if loaded_sections.is_empty() {
                "No extra session context was injected".to_string()
            } else {
                format!("Loaded {} context sources", loaded_sections.len())
            },
            detail: context_detail.clone(),
            counts: context_counts.clone(),
        },
    );
    let proxy_prompt_forward_epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(proxy_prompt_received_epoch_ms);
    let proxy_prompt_preflight_ms =
        proxy_prompt_forward_epoch_ms.saturating_sub(proxy_prompt_received_epoch_ms);
    if proxy_prompt_preflight_ms > PROMPT_PREFLIGHT_WARN_THRESHOLD_MS {
        warn!(
            "Prompt preflight slow: {}ms (session={})",
            proxy_prompt_preflight_ms, session_id
        );
    }
    if let Ok(notif) = UntypedMessage::new(
        crate::types::NOTIF_PROMPT_TRACE_FORWARDED,
        json!({
            "phase": "proxy_prompt_forwarded",
            "session_id": session_id.clone(),
            "proxy_prompt_received_epoch_ms": proxy_prompt_received_epoch_ms,
            "proxy_prompt_forward_epoch_ms": proxy_prompt_forward_epoch_ms,
            "proxy_prompt_preflight_ms": proxy_prompt_preflight_ms,
        }),
    ) {
        let _ = cx.send_notification_to(Client, notif);
    }
    let _ = cx.send_notification_to(
        Client,
        crate::types::IlhaeLoopLifecycleNotification::Completed {
            session_id: session_id.clone(),
            item: context_loop_item(
                context_loop_id,
                if loaded_sections.is_empty() {
                    "Context injection completed".to_string()
                } else {
                    format!("Injected {} context sources", loaded_sections.len())
                },
                context_detail,
                LoopLifecycleStatus::Completed,
                context_counts,
                Some(context_loop_started_at.elapsed().as_millis() as i64),
                None,
            ),
        },
    );

    let settings_snapshot = settings.get();
    let execution_mode = decide_execution_mode(&settings_snapshot);
    let team_mode_enabled = execution_mode.is_team();
    let autonomous_mode_enabled = execution_mode.is_autonomous();
    let mock_mode_enabled = execution_mode.is_mock();
    info!(
        "[TeamMode] execution_mode={:?}, team_mode={}, autonomous_mode={}, mock_mode={}",
        execution_mode, team_mode_enabled, autonomous_mode_enabled, mock_mode_enabled
    );

    if team_mode_enabled
        && let Some(response) = try_handle_direct_target_route(
            req.meta.as_ref(),
            &user_text,
            &session_id,
            &current_agent_id,
            &state,
            &cx,
            &ilhae_dir,
        )
        .await?
    {
        responder.respond(response)?;
        return Ok(());
    }

    match prepare_team_prompt(
        execution_mode,
        &mut req,
        &session_id,
        &current_agent_id,
        &user_text,
        prompt_start_cancel_ver,
        &state,
        &ilhae_dir,
    )
    .await?
    {
        TeamPromptPreparation::Cancelled(response) => {
            responder.respond(response)?;
            return Ok(());
        }
        TeamPromptPreparation::Prepared | TeamPromptPreparation::NotApplicable => {}
    }

    let sid = session_id.clone();
    let agent_id_for_save = current_agent_id.clone();
    let user_text_for_save = user_text.clone();

    // ── Real mode: forward to LLM via conductor chain ───────────────────
    cx.send_request_to(Agent, req)
        .on_receiving_result(async move |result| {
            let latest_cancel_ver = {
                let map = &session_cancel_ver;
                map.get(&sid).unwrap_or(0)
            };
            finalize_prompt_result(
                state.clone(),
                cx.clone(),
                responder,
                result,
                PromptFinalizeInput {
                    session_id: sid.clone(),
                    agent_id_for_save: agent_id_for_save.clone(),
                    user_text_for_save: user_text_for_save.clone(),
                    team_mode_enabled,
                    autonomous_mode_enabled,
                    prompt_start_cancel_ver,
                    latest_cancel_ver,
                },
            )
            .await
        })
}

fn prepend_prompt_blocks(prompt: &mut Vec<ContentBlock>, blocks: Vec<ContentBlock>) {
    for block in blocks.into_iter().rev() {
        prompt.insert(0, block);
    }
}
