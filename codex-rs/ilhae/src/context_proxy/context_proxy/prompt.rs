use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_client_protocol_schema::{ContentBlock, PromptRequest, PromptResponse, TextContent};
use sacp::{Agent, Client, Conductor, ConnectionTo, Responder, UntypedMessage};
use serde_json::json;
use tracing::{debug, info, warn};

use crate::{SharedState, build_dynamic_instructions, infer_agent_id_from_command};
use brain_knowledge_rs::memory_store;

use super::{
    MEMORY_SEARCH_TIMEOUT_MS, PINNED_FETCH_TIMEOUT_MS, PROMPT_PREFLIGHT_WARN_THRESHOLD_MS,
    execution_mode::decide_execution_mode,
    prompt_finalize::{PromptFinalizeInput, finalize_prompt_result},
    team_preflight::{TeamPromptPreparation, prepare_team_prompt, try_handle_direct_target_route},
};

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
    let context_prefix = state.infra.context_prefix.clone();
    let buffers = state.sessions.assistant_buffers.clone();
    let pending_history = state.sessions.pending_history.clone();
    let session_turn_seq = state.sessions.turn_seq.clone();
    let session_id_map = state.sessions.id_map.clone();
    let instr_version = state.sessions.instructions_version.clone();
    let session_instr_ver = state.sessions.instructions_ver.clone();
    let session_cancel_ver = state.sessions.cancel_ver.clone();
    let cx_cache = state.infra.relay_conductor_cx.clone();
    let mem_brain = state.infra.brain.clone();
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
    debug!("Received PromptRequest: {:?}", session_id);

    let sid_for_map = session_id.clone();
    let sid_for_buf = session_id.clone();
    let mem_brain_pinned = mem_brain.clone();

    let (remap_result, _, pinned_result) = tokio::join!(
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
        },
        async move {
            if is_subagent {
                return Ok(Ok(Ok(Vec::new())));
            }
            tokio::time::timeout(
                Duration::from_millis(PINNED_FETCH_TIMEOUT_MS),
                tokio::task::spawn_blocking(move || mem_brain_pinned.memory_list_pinned()),
            )
            .await
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

    // Update active session ID for MCP tools (e.g. artifact_save).
    // Use reverse_session_map to find the DB session ID if available.
    {
        let db_sid = {
            let rmap = &state.sessions.reverse_map;
            rmap.get(&session_id)
        };
        let effective_sid = db_sid.unwrap_or_else(|| session_id.clone());
        *state.sessions.active_session_id.write().await = effective_sid;
    }

    let user_text = req
        .prompt
        .iter()
        .filter_map(|b| {
            if let ContentBlock::Text(t) = b {
                if t.text.starts_with("__MCP_WIDGET_CTX__:") {
                    None
                } else {
                    Some(t.text.clone())
                }
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let session_info = if is_subagent {
        None
    } else {
        brain.session_get_raw(&session_id).unwrap_or(None)
    };
    let is_new = session_info.is_none();

    if is_new && !is_subagent {
        let is_team = settings.get().agent.team_mode;
        let agent_id = if is_team {
            "team".to_string()
        } else {
            infer_agent_id_from_command(&settings.get().agent.command)
        };
        info!("New session detected via PromptRequest: {}", session_id);
        let _ = brain.session_ensure(&session_id, &agent_id, &agent_id, "/");
    }

    let should_inject_context = !is_subagent
        && (is_new
            || session_info
                .as_ref()
                .is_some_and(|info| info.message_count == 0));
    if should_inject_context {
        info!("Injecting context for session: {}", session_id);
        let context_prefix = crate::config::build_context_prefix(&ilhae_dir);
        req.prompt.insert(
            0,
            ContentBlock::Text(TextContent::new(context_prefix)),
        );

        // Inject session-specific artifact directory path so the agent writes
        // artifacts (task.md, implementation_plan.md, walkthrough.md) to the
        // brain session folder. The artifact_save MCP tool handles path and
        // versioning automatically — the LLM does NOT need to know the path.
        let vault_dir = state.infra.brain.vault_dir();
        let session_artifact_dir = vault_dir.join("sessions").join(&session_id);
        let _ = std::fs::create_dir_all(&session_artifact_dir);
        let artifact_instruction = crate::ARTIFACT_INSTRUCTION.to_string();
        req.prompt.insert(
            0,
            ContentBlock::Text(TextContent::new(artifact_instruction)),
        );

        // ── Locale-based language instruction ──
        let locale_val = state.infra.settings_store.get_value("ui.locale");
        let locale_str = locale_val.as_str().unwrap_or("");
        if !locale_str.is_empty() {
            let lang_name = match locale_str {
                "ko" => "Korean (한국어)",
                "en" => "English",
                "ja" => "Japanese (日本語)",
                "zh" => "Chinese (中文)",
                _ => locale_str,
            };
            let locale_instruction = format!(
                "\n<system_directive priority=\"high\">\n\
                 RESPONSE LANGUAGE: You MUST respond in {}.\n\
                 All artifacts (task, plan, walkthrough) MUST also be written in {}.\n\
                 Use the user's preferred language consistently throughout all outputs.\n\
                 </system_directive>\n",
                lang_name, lang_name
            );
            req.prompt
                .insert(0, ContentBlock::Text(TextContent::new(locale_instruction)));
        }
    }

    match pinned_result {
        Ok(Ok(Ok(pinned))) => {
            let pinned_text = memory_store::format_pinned_for_prompt(&pinned);
            if !pinned_text.is_empty() {
                req.prompt
                    .insert(0, ContentBlock::Text(TextContent::new(pinned_text)));
            }
        }
        Ok(Ok(Err(e))) => {
            warn!(
                "Pinned memory fetch failed for session {}: {}",
                session_id, e
            );
        }
        Ok(Err(e)) => {
            warn!(
                "Pinned memory worker join failed for session {}: {}",
                session_id, e
            );
        }
        Err(_) => {
            warn!(
                "Pinned memory fetch timed out ({}ms) for session {}",
                PINNED_FETCH_TIMEOUT_MS, session_id
            );
        }
    }

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

    if let Some((dynamic_ctx, last_ver, current_ver)) = dynamic_ctx_opt {
        info!(
            "Injecting dynamic instructions for session: {} (ver {} → {})",
            session_id, last_ver, current_ver
        );
        req.prompt
            .insert(0, ContentBlock::Text(TextContent::new(dynamic_ctx)));
    }

    if let Some(history_text) = history_opt {
        info!("Injecting cross-agent history for session: {}", session_id);
        req.prompt
            .insert(0, ContentBlock::Text(TextContent::new(history_text)));
    }

    if !user_text.trim().is_empty() && !is_subagent {
        let mem_brain_search = mem_brain.clone();
        let query = user_text.clone();
        match tokio::time::timeout(
            Duration::from_millis(MEMORY_SEARCH_TIMEOUT_MS),
            tokio::task::spawn_blocking(move || mem_brain_search.memory_deep_search(&query, 5)),
        )
        .await
        {
            Ok(Ok(Ok(memories))) => {
                if !memories.is_empty() {
                    let recall_text = memory_store::format_memories_for_prompt(&memories);
                    info!(
                        "Auto-recall: injecting {} memory chunks for session {}",
                        memories.len(),
                        session_id
                    );
                    req.prompt
                        .insert(0, ContentBlock::Text(TextContent::new(recall_text)));
                }
            }
            Ok(Ok(Err(e))) => {
                warn!(
                    "Auto-recall search failed for session {}: {}",
                    session_id, e
                );
            }
            Ok(Err(e)) => {
                warn!(
                    "Auto-recall worker join failed for session {}: {}",
                    session_id, e
                );
            }
            Err(_) => {
                warn!(
                    "Auto-recall search timed out ({}ms) for session {}",
                    MEMORY_SEARCH_TIMEOUT_MS, session_id
                );
            }
        }
    }

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
        let _ = brain.session_update_title(&session_id, &truncated_title);
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

    let current_agent_id = infer_agent_id_from_command(&settings.get().agent.command);
    brain
        .session_add_message_simple(&session_id, "user", &user_text, "")
        .ok();
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
