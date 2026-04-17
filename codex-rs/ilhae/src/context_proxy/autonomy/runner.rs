use a2a_rs::A2aProxy;
use agent_client_protocol_schema::{
    ContentBlock, PromptRequest, SessionId, StopReason, TextContent,
};
use sacp::{Agent, Client, Conductor, ConnectionTo, UntypedMessage};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::SharedState;
use crate::context_proxy::autonomy::should_continue_autonomous_on_stop_reason;
use crate::context_proxy::autonomy::state::{
    AutonomousPhase, AutonomousSessionState, set_autonomous_snapshot,
};

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

fn compact_loop_text(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(max_chars).collect()
}

fn progress_signature(text: &str) -> u64 {
    let normalized = compact_loop_text(text, 1200);
    let mut hasher = DefaultHasher::new();
    normalized.hash(&mut hasher);
    hasher.finish()
}

fn build_loop_snapshot(
    phase: AutonomousPhase,
    loop_iteration: u32,
    note: Option<String>,
    queued_directive: Option<String>,
    goal: &str,
    last_observation: &str,
    stalled_turns: u32,
    stop_reason: Option<String>,
) -> AutonomousSessionState {
    let mut snapshot = AutonomousSessionState::new(phase, loop_iteration, note, queued_directive);
    if !goal.trim().is_empty() {
        snapshot.goal = Some(compact_loop_text(goal, 240));
    }
    if !last_observation.trim().is_empty() {
        snapshot.last_observation = Some(compact_loop_text(last_observation, 400));
    }
    snapshot.stalled_turns = stalled_turns;
    snapshot.stop_reason = stop_reason;
    snapshot
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
        let goal_summary = compact_loop_text(&leader_text, 240);
        let mut context_so_far = leader_text;
        let mut last_directive: Option<String> = None;
        let mut stalled_turns = 0u32;
        let mut last_progress_signature = Some(progress_signature(&context_so_far));
        let _leader_a2a_proxy = resolve_leader_a2a_proxy(&state);

        for turn in 1..=max_turns {
            if started_at.elapsed() >= timebox {
                info!(
                    "[AutoMode] Timebox reached after {:?} (session={})",
                    started_at.elapsed(),
                    session_id
                );
                set_autonomous_snapshot(
                    &state,
                    &session_id,
                    build_loop_snapshot(
                        AutonomousPhase::Completed,
                        turn.saturating_sub(1),
                        Some("autonomous timebox reached".to_string()),
                        None,
                        &goal_summary,
                        &context_so_far,
                        stalled_turns,
                        Some("timebox_reached".to_string()),
                    ),
                )
                .await;
                break;
            }
            set_autonomous_snapshot(
                &state,
                &session_id,
                build_loop_snapshot(
                    AutonomousPhase::Running,
                    turn,
                    Some("autonomous loop executing".to_string()),
                    None,
                    &goal_summary,
                    &context_so_far,
                    stalled_turns,
                    None,
                ),
            )
            .await;
            info!(
                "[AutoMode] Turn {}/{} (session={})",
                turn, max_turns, session_id
            );

            let remaining_turns = max_turns.saturating_sub(turn).saturating_add(1);
            let remaining_time_secs = timebox.saturating_sub(started_at.elapsed()).as_secs();
            let ua_response = match super::user_agent::request_next_directive_with_context(
                &session_id,
                &context_so_far,
                &super::user_agent::UserAgentLoopContext {
                    goal: &goal_summary,
                    last_directive: last_directive.as_deref(),
                    stalled_turns,
                    remaining_turns,
                    remaining_time_secs,
                },
            )
            .await
            {
                Ok(super::user_agent::UserAgentDirective::Continue(text)) => text,
                Ok(super::user_agent::UserAgentDirective::Complete) => {
                    let is_retro = state
                        .sessions
                        .autonomous_sessions
                        .get(&session_id)
                        .map(|s| s.phase == AutonomousPhase::Retro)
                        .unwrap_or(false);
                    if is_retro {
                        info!("[AutoMode] Retro complete. Ending auto-loop.");
                        set_autonomous_snapshot(
                            &state,
                            &session_id,
                            build_loop_snapshot(
                                AutonomousPhase::Completed,
                                turn,
                                Some("user-agent signaled completion after retro".to_string()),
                                None,
                                &goal_summary,
                                &context_so_far,
                                stalled_turns,
                                Some("retro_complete".to_string()),
                            ),
                        )
                        .await;
                        break;
                    } else {
                        info!("[AutoMode] User Agent signaled completion. Initiating Retro phase.");
                        let retro_msg = "작업이 완료되었습니다. 이제 지금까지의 작업 내역을 바탕으로 `brain_artifact_ops`를 사용해 `index.md`와 관련 프로젝트 위키를 갱신(Compile)하고 세션을 완벽히 종료하세요.".to_string();

                        set_autonomous_snapshot(
                            &state,
                            &session_id,
                            build_loop_snapshot(
                                AutonomousPhase::Retro,
                                turn,
                                Some("entering retro phase".to_string()),
                                Some(retro_msg.clone()),
                                &goal_summary,
                                &context_so_far,
                                stalled_turns,
                                None,
                            ),
                        )
                        .await;

                        retro_msg
                    }
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

            last_directive = Some(ua_response.clone());
            set_autonomous_snapshot(
                &state,
                &session_id,
                build_loop_snapshot(
                    AutonomousPhase::QueuedTurn,
                    turn,
                    Some("next directive queued".to_string()),
                    Some(ua_response.clone()),
                    &goal_summary,
                    &context_so_far,
                    stalled_turns,
                    None,
                ),
            )
            .await;

            set_autonomous_snapshot(
                &state,
                &session_id,
                build_loop_snapshot(
                    AutonomousPhase::Running,
                    turn,
                    Some("queued turn dispatched".to_string()),
                    None,
                    &goal_summary,
                    &context_so_far,
                    stalled_turns,
                    None,
                ),
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
                    if !should_continue_autonomous_on_stop_reason(resp.stop_reason) {
                        info!(
                            "[AutoMode] Leader stop_reason is not continuation-worthy. Ending auto-loop."
                        );
                        set_autonomous_snapshot(
                            &state,
                            &session_id,
                            build_loop_snapshot(
                                AutonomousPhase::Completed,
                                turn,
                                Some(format!("leader stop_reason={:?}", resp.stop_reason)),
                                None,
                                &goal_summary,
                                &context_so_far,
                                stalled_turns,
                                Some(format!("leader_{:?}", resp.stop_reason)),
                            ),
                        )
                        .await;
                        break;
                    }
                    if resp.stop_reason == StopReason::MaxTokens {
                        info!(
                            "[AutoMode] Leader hit MaxTokens; keeping auto-loop alive for replanning."
                        );
                    }
                    let latest_observation = state
                        .infra
                        .brain
                        .sessions()
                        .get_latest_agent_content(&session_id, "leader")
                        .ok()
                        .flatten()
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or_else(|| ua_response.clone());
                    let observation_signature = progress_signature(&latest_observation);
                    if last_progress_signature == Some(observation_signature) {
                        stalled_turns = stalled_turns.saturating_add(1);
                    } else {
                        stalled_turns = 0;
                    }
                    last_progress_signature = Some(observation_signature);
                    context_so_far = latest_observation;

                    if stalled_turns >= 2 {
                        warn!(
                            "[AutoMode] No material progress detected across consecutive turns. Stopping autonomous loop (session={})",
                            session_id
                        );
                        set_autonomous_snapshot(
                            &state,
                            &session_id,
                            build_loop_snapshot(
                                AutonomousPhase::Completed,
                                turn,
                                Some("no material progress across consecutive turns".to_string()),
                                None,
                                &goal_summary,
                                &context_so_far,
                                stalled_turns,
                                Some("stalled_no_progress".to_string()),
                            ),
                        )
                        .await;
                        break;
                    }

                    set_autonomous_snapshot(
                        &state,
                        &session_id,
                        build_loop_snapshot(
                            AutonomousPhase::Running,
                            turn,
                            Some(if resp.stop_reason == StopReason::MaxTokens {
                                "observation recorded after max_tokens; replanning".to_string()
                            } else {
                                "observation recorded; replanning".to_string()
                            }),
                            None,
                            &goal_summary,
                            &context_so_far,
                            stalled_turns,
                            None,
                        ),
                    )
                    .await;
                }
                Err(error) => {
                    warn!("[AutoMode] Autonomous turn failed: {}", error);
                    if pause_on_error {
                        set_autonomous_snapshot(
                            &state,
                            &session_id,
                            build_loop_snapshot(
                                AutonomousPhase::Failed,
                                turn,
                                Some(format!("{}", error)),
                                None,
                                &goal_summary,
                                &context_so_far,
                                stalled_turns,
                                Some("leader_turn_failed".to_string()),
                            ),
                        )
                        .await;
                        break;
                    }
                    set_autonomous_snapshot(
                        &state,
                        &session_id,
                        build_loop_snapshot(
                            AutonomousPhase::Running,
                            turn,
                            Some(format!("turn failed but continuing: {}", error)),
                            None,
                            &goal_summary,
                            &context_so_far,
                            stalled_turns,
                            None,
                        ),
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
