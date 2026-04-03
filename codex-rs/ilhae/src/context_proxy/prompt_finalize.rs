use std::sync::Arc;

use agent_client_protocol_schema::{PromptResponse, StopReason};
use sacp::{Client, Conductor, ConnectionTo, Responder, UntypedMessage};
use serde_json::json;
use tracing::info;

use brain_knowledge_rs::memory_store;

use crate::SharedState;
use crate::context_proxy::autonomy::runner::spawn_autonomous_loop;
use crate::context_proxy::autonomy::should_continue_autonomous_on_stop_reason;
use crate::context_proxy::autonomy::state::{
    AutonomousPhase, clear_autonomous_state, current_autonomous_iteration, set_autonomous_phase,
};
use crate::context_proxy::{
    emit_a2a_role_event_notifications, extract_a2a_structured_from_prompt_response,
    extract_a2a_text_from_prompt_response, persist_team_split_messages,
    preferred_assistant_content_from_tool_calls, should_suppress_prompt_error_after_cancel,
    synthesize_assistant_from_a2a_structured,
};

pub struct PromptFinalizeInput {
    pub session_id: String,
    pub agent_id_for_save: String,
    pub user_text_for_save: String,
    pub team_mode_enabled: bool,
    pub autonomous_mode_enabled: bool,
    pub prompt_start_cancel_ver: u64,
    pub latest_cancel_ver: u64,
}

fn should_emit_assistant_turn_patch(
    input: &PromptFinalizeInput,
    has_a2a_structured_output: bool,
) -> bool {
    input.team_mode_enabled || input.autonomous_mode_enabled || has_a2a_structured_output
}

pub async fn finalize_prompt_result(
    state: Arc<SharedState>,
    cx: ConnectionTo<Conductor>,
    responder: Responder<PromptResponse>,
    result: Result<PromptResponse, sacp::Error>,
    input: PromptFinalizeInput,
) -> Result<(), sacp::Error> {
    let store_for_save = state.infra.brain.sessions().clone();
    let brain_for_save = state.infra.brain.clone();
    let mem_brain_for_save = state.infra.brain.clone();
    let buffers_for_save = state.sessions.assistant_buffers.clone();
    let structured_meta = result
        .as_ref()
        .ok()
        .and_then(extract_a2a_structured_from_prompt_response);
    let a2a_text_meta = result
        .as_ref()
        .ok()
        .and_then(extract_a2a_text_from_prompt_response);
    let cancelled_after_start = input.latest_cancel_ver > input.prompt_start_cancel_ver;
    let should_emit_patch = should_emit_assistant_turn_patch(&input, structured_meta.is_some());
    let mut leader_content_for_auto = String::new();
    let mut saved_from_buffer = false;
    let lock = &buffers_for_save;

    let buf_opt = lock.get(&input.session_id);
    if buf_opt.is_some() {
        lock.invalidate(&input.session_id);
    }
    if let Some(buf) = buf_opt
        && buf.has_content()
    {
        saved_from_buffer = true;
        let db = buf.to_db_fields();
        let tc_count = buf.tool_calls.len();
        let label = if result.is_ok() {
            "Saving"
        } else {
            "Saving partial (on error)"
        };
        let first_tc_id = buf
            .tool_calls
            .first()
            .and_then(|tc| tc.get("toolCallId").or_else(|| tc.get("tool_call_id")))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        info!(
            "[PromptResponse] {} assistant message for session {} ({} tool calls, tc_bytes={}, cb_bytes={}, first_tc={})",
            label,
            input.session_id,
            tc_count,
            db.tool_calls.len(),
            db.content_blocks.len(),
            first_tc_id
        );
        if tc_count == 0 {
            tracing::debug!(
                "[PromptResponse] session {} has 0 tool calls in buffer at save time",
                input.session_id
            );
        }

        if result.is_ok() {
            persist_team_split_messages(
                store_for_save.as_ref(),
                &input.session_id,
                &input.user_text_for_save,
                &db.content,
                &input.agent_id_for_save,
                &db.tool_calls,
                structured_meta.as_ref(),
            );
        }

        if let Some(msg_id) = buf.db_message_id {
            let _ = store_for_save.update_full_message_with_blocks_by_id(
                msg_id,
                &db.content,
                &db.thinking,
                &db.tool_calls,
                &db.content_blocks,
                db.input_tokens,
                db.output_tokens,
                db.total_tokens,
                db.duration_ms,
            );
        } else {
            store_for_save
                .add_full_message_with_blocks(
                    &input.session_id,
                    "assistant",
                    &db.content,
                    &input.agent_id_for_save,
                    &db.thinking,
                    &db.tool_calls,
                    &db.content_blocks,
                    db.input_tokens,
                    db.output_tokens,
                    db.total_tokens,
                    db.duration_ms,
                )
                .ok();
        }
        {
            let vault_dir = brain_for_save.vault_dir();
            let artifact_dir = vault_dir.join("sessions").join(&input.session_id);
            if artifact_dir.exists()
                && let Ok(entries) = std::fs::read_dir(&artifact_dir)
            {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.extension().map(|e| e == "md").unwrap_or(false) {
                        continue;
                    }
                    let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    let Ok(content) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    let latest = store_for_save.list_artifact_versions(&input.session_id, filename);
                    let needs_save = match &latest {
                        Ok(versions) if !versions.is_empty() => {
                            let versions: Vec<serde_json::Value> = versions.clone();
                            let latest_ver = versions[0]["version"].as_i64().unwrap_or(0);
                            if let Ok(Some(v)) = store_for_save.get_artifact_version(
                                &input.session_id,
                                filename,
                                latest_ver,
                            ) {
                                v["content"].as_str() != Some(content.as_str())
                            } else {
                                true
                            }
                        }
                        _ => true,
                    };
                    if !needs_save || content.trim().is_empty() {
                        continue;
                    }
                    let artifact_type = match filename {
                        "task.md" => "task",
                        "implementation_plan.md" => "implementation_plan",
                        "walkthrough.md" => "walkthrough",
                        _ => "other",
                    };
                    if let Ok(ver) = store_for_save.save_artifact_version(
                        &input.session_id,
                        filename,
                        &content,
                        "",
                        artifact_type,
                    ) {
                        tracing::info!(
                            "[ArtifactVersion] Saved {} v{} for session {}",
                            filename,
                            ver,
                            input.session_id
                        );
                    }
                }
            }
        }

        emit_a2a_role_event_notifications(
            &cx,
            &input.session_id,
            &db.content,
            structured_meta.as_ref(),
        );

        if should_emit_patch {
            let mut final_patch = buf.to_patch();
            final_patch["final"] = json!(true);
            if let Ok(notif) =
                UntypedMessage::new(crate::types::NOTIF_ASSISTANT_TURN_PATCH, final_patch)
            {
                let _ = cx.send_notification_to(Client, notif);
            }
        }

        if input.autonomous_mode_enabled {
            leader_content_for_auto = db.content.clone();
        }

        if result.is_ok() && memory_store::should_capture(&db.content) {
            info!(
                "[Auto-capture] Storing significant response for session {}",
                input.session_id
            );
            mem_brain_for_save
                .memory_chunk_store(&db.content, "auto-capture")
                .ok();
        }
    }
    let _ = lock;

    if !saved_from_buffer
        && let Some(structured) = structured_meta.as_ref()
        && let Some((content, tool_calls)) = synthesize_assistant_from_a2a_structured(structured)
    {
        let content_trimmed = preferred_assistant_content_from_tool_calls(&tool_calls)
            .or_else(|| {
                a2a_text_meta
                    .as_ref()
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty())
            })
            .unwrap_or_else(|| content.trim().to_string());
        let tool_calls_json =
            serde_json::to_string(&tool_calls).unwrap_or_else(|_| "[]".to_string());
        let agent_id = if input.team_mode_enabled {
            "leader"
        } else {
            &input.agent_id_for_save
        };
        if input.autonomous_mode_enabled {
            leader_content_for_auto = content_trimmed.clone();
        }
        info!(
            "[PromptResponse] Synthesizing assistant fallback for session {} (content={}B, tool_calls={})",
            input.session_id,
            content_trimmed.len(),
            tool_calls.len()
        );

        persist_team_split_messages(
            store_for_save.as_ref(),
            &input.session_id,
            &input.user_text_for_save,
            &content_trimmed,
            agent_id,
            &tool_calls_json,
            Some(structured),
        );

        let _ = store_for_save.add_full_message_with_blocks(
            &input.session_id,
            "assistant",
            &content_trimmed,
            agent_id,
            "",
            &tool_calls_json,
            "",
            0,
            0,
            0,
            0,
        );

        emit_a2a_role_event_notifications(
            &cx,
            &input.session_id,
            &content_trimmed,
            Some(structured),
        );

        if should_emit_patch {
            if let Ok(notif) = UntypedMessage::new(
                crate::types::NOTIF_ASSISTANT_TURN_PATCH,
                json!({
                    "sessionId": input.session_id,
                    "agentId": agent_id,
                    "content": content_trimmed,
                    "thinking": "",
                    "toolCalls": tool_calls,
                    "contentBlocks": if content_trimmed.is_empty() {
                        Vec::<serde_json::Value>::new()
                    } else {
                        vec![json!({"type":"text","text": content_trimmed})]
                    },
                    "final": true,
                }),
            ) {
                let _ = cx.send_notification_to(Client, notif);
            }
        }
    }

    match result {
        Ok(response) => {
            if cancelled_after_start {
                info!(
                    "[PromptResponse] Suppressing late success as cancelled (session={}, cancel_ver {} -> {})",
                    input.session_id, input.prompt_start_cancel_ver, input.latest_cancel_ver
                );
                set_autonomous_phase(
                    &state,
                    &input.session_id,
                    AutonomousPhase::Cancelled,
                    0,
                    Some("cancelled after start".to_string()),
                    None,
                )
                .await;
                responder.respond(PromptResponse::new(StopReason::Cancelled))?;
                return Ok(());
            }

            let should_continue_autonomy =
                should_continue_autonomous_on_stop_reason(response.stop_reason);
            if input.autonomous_mode_enabled {
                info!(
                    "[AutoMode] Prompt finalized (session={}, stop_reason={:?}, continue={})",
                    input.session_id, response.stop_reason, should_continue_autonomy
                );
            }

            if input.autonomous_mode_enabled && should_continue_autonomy {
                let existing_iteration =
                    current_autonomous_iteration(&state, &input.session_id).await;
                if existing_iteration > 0 {
                    info!(
                        "[AutoMode] Continuation stop reason during active loop; returning to existing runner (session={}, iteration={}, stop_reason={:?})",
                        input.session_id, existing_iteration, response.stop_reason
                    );
                    responder.respond(response)?;
                    return Ok(());
                }
                info!(
                    "[AutoMode] Continuation stop reason detected with autonomous_mode=true. Spawning auto-loop (session={}, stop_reason={:?})",
                    input.session_id, response.stop_reason
                );
                set_autonomous_phase(
                    &state,
                    &input.session_id,
                    AutonomousPhase::Running,
                    1,
                    Some("autonomous loop starting".to_string()),
                    None,
                )
                .await;
                responder.respond(response)?;
                spawn_autonomous_loop(
                    state.clone(),
                    cx.clone(),
                    input.session_id.clone(),
                    leader_content_for_auto,
                );
                return Ok(());
            }

            if input.autonomous_mode_enabled {
                set_autonomous_phase(
                    &state,
                    &input.session_id,
                    AutonomousPhase::Completed,
                    0,
                    Some(format!("initial stop_reason={:?}", response.stop_reason)),
                    None,
                )
                .await;
            } else {
                clear_autonomous_state(&state, &input.session_id).await;
            }
            responder.respond(response)?;
            Ok(())
        }
        Err(e) => {
            if should_suppress_prompt_error_after_cancel(
                &e,
                input.prompt_start_cancel_ver,
                input.latest_cancel_ver,
            ) {
                info!(
                    "[PromptResponse] Suppressing abort error as cancelled (session={}, cancel_ver {} -> {})",
                    input.session_id, input.prompt_start_cancel_ver, input.latest_cancel_ver
                );
                set_autonomous_phase(
                    &state,
                    &input.session_id,
                    AutonomousPhase::Cancelled,
                    0,
                    Some("cancelled during prompt".to_string()),
                    None,
                )
                .await;
                responder.respond(PromptResponse::new(StopReason::Cancelled))?;
                Ok(())
            } else {
                set_autonomous_phase(
                    &state,
                    &input.session_id,
                    AutonomousPhase::Failed,
                    0,
                    Some(format!("{}", e)),
                    None,
                )
                .await;
                responder.respond_with_result(Err(e))?;
                Ok(())
            }
        }
    }
}
