use serde_json::json;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::SharedState;
use crate::a2a_persistence::events::DelegationEvent;
use crate::team_timeline::{
    agent_response_event, delegation_completed_event, delegation_started_event, persist_events,
};

pub async fn spawn_tracker_daemon(
    shared: Arc<SharedState>,
    mut rx: broadcast::Receiver<DelegationEvent>,
) {
    tokio::spawn(async move {
        info!("[DelegationTracker] Daemon started");
        let store = shared.infra_context().brain.sessions().clone();

        while let Ok(event) = rx.recv().await {
            match event {
                DelegationEvent::Started {
                    leader_session_id,
                    target_role,
                    leader_role,
                    mode,
                    request_text,
                    channel_id,
                } => {
                    let _ = store.ensure_team_sub_session(
                        &leader_session_id,
                        &target_role,
                        &target_role,
                        "/",
                    );
                    persist_events(
                        &store,
                        &leader_session_id,
                        [
                            delegation_started_event(
                                &target_role,
                                &mode,
                                &request_text,
                                None,
                                None,
                            )
                            .with_agent_id(leader_role.clone())
                            .with_channel_id(&channel_id),
                        ],
                    );
                    debug!(
                        "[DelegationTracker] Persisted DelegationStarted for {}",
                        target_role
                    );
                }
                DelegationEvent::ResultTapped {
                    leader_session_id,
                    target_role,
                    leader_role,
                    response_text,
                    schedule_id,
                    task_state,
                    duration_ms,
                    artifacts,
                    history,
                } => {
                    let resp_blocks = serde_json::to_string(&vec![
                        json!({"type": "text", "text": response_text}),
                        json!({
                            "type": "a2a_task",
                            "schedule_id": schedule_id,
                            "task_state": task_state,
                            "duration_ms": duration_ms,
                            "artifacts": artifacts,
                            "history": history,
                        }),
                    ])
                    .unwrap_or_default();

                    persist_events(
                        &store,
                        &leader_session_id,
                        [agent_response_event(
                            &target_role,
                            &response_text,
                            "[]",
                            "sync",
                            Some(&schedule_id),
                        )
                        .with_content_blocks_json(resp_blocks)
                        .with_channel_id("a2a:delegation_response")
                        .with_metadata(json!({
                            "schedule_id": schedule_id,
                            "task_state": task_state,
                            "duration_ms": duration_ms,
                        }))],
                    );

                    let complete_blocks = serde_json::to_string(&vec![
                        json!({"type": "text", "text": format!("✅ @{} completed", target_role)}),
                        json!({
                            "type": "a2a_task_result",
                            "agent": target_role,
                            "schedule_id": schedule_id,
                            "task_state": task_state,
                            "duration_ms": duration_ms,
                            "response_preview": &response_text[..response_text.len().min(200)],
                        }),
                    ])
                    .unwrap_or_default();

                    persist_events(
                        &store,
                        &leader_session_id,
                        [delegation_completed_event(
                            &target_role,
                            Some(&schedule_id),
                            &task_state,
                            &response_text,
                            "sync",
                        )
                        .with_agent_id(leader_role.clone())
                        .with_channel_id("a2a:delegation_complete")
                        .with_content_blocks_json(complete_blocks)],
                    );

                    debug!(
                        "[DelegationTracker] Persisted ResultTapped for {}, duration: {}ms",
                        target_role, duration_ms
                    );
                }
            }
        }
        warn!("[DelegationTracker] Daemon exiting (channel closed)");
    });
}
