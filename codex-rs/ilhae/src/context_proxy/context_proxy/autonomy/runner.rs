use a2a_rs::A2aProxy;
use agent_client_protocol_schema::{
    ContentBlock, PromptRequest, SessionId, StopReason, TextContent,
};
use sacp::{Agent, Client, Conductor, ConnectionTo, UntypedMessage};
use serde_json::json;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::SharedState;
use crate::context_proxy::autonomy::state::{AutonomousPhase, set_autonomous_phase};

use crate::context_proxy::load_team_runtime_config;

use super::build_ralph_loop_prompt;

fn resolve_leader_a2a_proxy(state: &std::sync::Arc<SharedState>) -> Option<A2aProxy> {
    if !state.infra.settings_store.get().agent.team_mode {
        return None;
    }
    let ilhae_dir = state.infra.brain.ilhae_data_dir().to_path_buf();
    let team_cfg = load_team_runtime_config(&ilhae_dir)?;
    let leader = team_cfg
        .agents
        .iter()
        .find(|agent| agent.is_main)
        .or_else(|| team_cfg.agents.first())?;
    Some(A2aProxy::new(&leader.endpoint, &leader.role))
}

#[allow(dead_code)]
fn extract_text_from_a2a_parts(parts: &[a2a_rs::Part]) -> String {
    parts
        .iter()
        .filter_map(|part| part.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(dead_code)]
fn upsert_tool_call(tool_calls: &mut Vec<serde_json::Value>, tool_call: serde_json::Value) {
    let tool_call_id = tool_call
        .get("toolCallId")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();

    if tool_call_id.is_empty() {
        tool_calls.push(tool_call);
        return;
    }

    if let Some(existing) = tool_calls.iter_mut().find(|existing| {
        existing.get("toolCallId").and_then(|value| value.as_str()) == Some(tool_call_id.as_str())
    }) {
        if let (Some(existing_obj), Some(new_obj)) =
            (existing.as_object_mut(), tool_call.as_object())
        {
            for (key, value) in new_obj {
                existing_obj.insert(key.clone(), value.clone());
            }
            return;
        }
    }

    tool_calls.push(tool_call);
}

pub fn spawn_autonomous_loop(
    state: std::sync::Arc<SharedState>,
    cx: ConnectionTo<Conductor>,
    session_id: String,
    leader_text: String,
) {
    tokio::spawn(async move {
        let settings_snapshot = state.infra.settings_store.get();
        let max_turns = settings_snapshot.agent.auto_max_turns.max(1);
        let timebox = Duration::from_secs(
            u64::from(settings_snapshot.agent.auto_timebox_minutes.max(1)) * 60,
        );
        let pause_on_error = settings_snapshot.agent.auto_pause_on_error;
        let started_at = Instant::now();
        let mut context_so_far = leader_text;
        let _leader_a2a_proxy = resolve_leader_a2a_proxy(&state);

        for turn in 1..=max_turns {
            if started_at.elapsed() >= timebox {
                info!(
                    "[AutoMode] Timebox reached after {:?} (session={})",
                    started_at.elapsed(),
                    session_id
                );
                set_autonomous_phase(
                    &state,
                    &session_id,
                    AutonomousPhase::Completed,
                    turn.saturating_sub(1),
                    Some("autonomous timebox reached".to_string()),
                    None,
                )
                .await;
                break;
            }
            set_autonomous_phase(
                &state,
                &session_id,
                AutonomousPhase::Running,
                turn,
                Some("autonomous loop executing".to_string()),
                None,
            )
            .await;
            info!(
                "[AutoMode] Turn {}/{} (session={})",
                turn, max_turns, session_id
            );

            let ua_response = match super::user_agent::request_next_directive(
                &session_id,
                &context_so_far,
            )
            .await
            {
                Ok(super::user_agent::UserAgentDirective::Continue(text)) => text,
                Ok(super::user_agent::UserAgentDirective::Complete) => {
                    info!("[AutoMode] User Agent signaled completion. Ending auto-loop.");
                    set_autonomous_phase(
                        &state,
                        &session_id,
                        AutonomousPhase::Completed,
                        turn,
                        Some("user-agent signaled completion".to_string()),
                        None,
                    )
                    .await;
                    break;
                }
                Ok(super::user_agent::UserAgentDirective::Empty) => {
                    warn!(
                        "[AutoMode] User Agent returned empty response. Falling back to Ralph Loop."
                    );
                    build_ralph_loop_prompt(&context_so_far)
                }
                Err(error) => {
                    warn!("[AutoMode] {}. Falling back to Ralph Loop.", error);
                    build_ralph_loop_prompt(&context_so_far)
                }
            };

            info!(
                "[AutoMode] User Agent directive ({}chars): {}",
                ua_response.len(),
                &ua_response[..ua_response.len().min(100)]
            );

            set_autonomous_phase(
                &state,
                &session_id,
                AutonomousPhase::QueuedTurn,
                turn,
                Some("next directive queued".to_string()),
                Some(ua_response.clone()),
            )
            .await;

            set_autonomous_phase(
                &state,
                &session_id,
                AutonomousPhase::Running,
                turn,
                Some("queued turn dispatched".to_string()),
                None,
            )
            .await;

            let directive_blocks = serde_json::to_string(&vec![json!({
                "type": "text",
                "text": ua_response.clone()
            })])
            .unwrap_or_else(|_| "[]".to_string());
            let _ = state.infra.brain.session_add_message_with_blocks(
                &session_id,
                "system",
                &ua_response,
                "leader",
                "",
                "[]",
                &directive_blocks,
                0,
                0,
                0,
                0,
            );

            let new_req = PromptRequest::new(
                SessionId::new(session_id.clone()),
                vec![ContentBlock::Text(TextContent::new(ua_response.clone()))],
            );

            match cx.send_request_to(Agent, new_req).block_task().await {
                Ok(resp) => {
                    info!(
                        "[AutoMode] Leader turn completed, stop_reason={:?}",
                        resp.stop_reason
                    );
                    if resp.stop_reason != StopReason::EndTurn {
                        info!("[AutoMode] Leader stop_reason != EndTurn. Ending auto-loop.");
                        set_autonomous_phase(
                            &state,
                            &session_id,
                            AutonomousPhase::Completed,
                            turn,
                            Some(format!("leader stop_reason={:?}", resp.stop_reason)),
                            None,
                        )
                        .await;
                        break;
                    }
                    context_so_far = state
                        .infra
                        .brain
                        .sessions()
                        .get_latest_agent_content(&session_id, "leader")
                        .ok()
                        .flatten()
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or_else(|| ua_response.clone());
                }
                Err(error) => {
                    warn!("[AutoMode] Autonomous turn failed: {}", error);
                    if pause_on_error {
                        set_autonomous_phase(
                            &state,
                            &session_id,
                            AutonomousPhase::Failed,
                            turn,
                            Some(format!("{}", error)),
                            None,
                        )
                        .await;
                        break;
                    }
                    set_autonomous_phase(
                        &state,
                        &session_id,
                        AutonomousPhase::Running,
                        turn,
                        Some(format!("turn failed but continuing: {}", error)),
                        None,
                    )
                    .await;
                    continue;
                }
            }

            if let Ok(notif) = UntypedMessage::new(
                crate::types::NOTIF_ASSISTANT_TURN_PATCH,
                serde_json::json!({
                    "sessionId": session_id,
                    "agentId": "leader",
                    "thinking": "",
                    "content": build_ralph_loop_prompt(&context_so_far),
                    "final": false,
                }),
            ) {
                let _ = cx.send_notification_to(Client, notif);
            }

            let turn_id = format!("autonomy-leader-turn-{turn}");
            let item_id = format!("{turn_id}:leader");
            let preview_text = build_ralph_loop_prompt(&context_so_far);
            if let Ok(notif) = UntypedMessage::new(
                crate::types::NOTIF_APP_SESSION_EVENT,
                crate::types::IlhaeAppSessionEventNotification {
                    engine: "leader".to_string(),
                    event: crate::types::IlhaeAppSessionEventDto::MessageDelta {
                        thread_id: session_id.clone(),
                        turn_id: turn_id.clone(),
                        item_id,
                        channel: "assistant".to_string(),
                        delta: preview_text,
                    },
                },
            ) {
                let _ = cx.send_notification_to(Client, notif);
            }
            if let Ok(notif) = UntypedMessage::new(
                crate::types::NOTIF_APP_SESSION_EVENT,
                crate::types::IlhaeAppSessionEventNotification {
                    engine: "leader".to_string(),
                    event: crate::types::IlhaeAppSessionEventDto::TurnCompleted {
                        thread_id: session_id.clone(),
                        turn_id,
                        status: "completed".to_string(),
                    },
                },
            ) {
                let _ = cx.send_notification_to(Client, notif);
            }
        }
    });
}
