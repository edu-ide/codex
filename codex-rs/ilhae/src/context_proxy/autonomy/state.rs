use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::SharedState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AutonomousPhase {
    QueuedTurn,
    Running,
    WaitingForApproval,
    ResumingAfterApproval,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousSessionState {
    pub phase: AutonomousPhase,
    pub loop_iteration: u32,
    pub updated_at_ms: u64,
    pub note: Option<String>,
    pub queued_directive: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_observation: Option<String>,
    #[serde(default)]
    pub stalled_turns: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

impl AutonomousSessionState {
    pub fn new(
        phase: AutonomousPhase,
        loop_iteration: u32,
        note: Option<String>,
        queued_directive: Option<String>,
    ) -> Self {
        Self {
            phase,
            loop_iteration,
            updated_at_ms: now_millis(),
            note,
            queued_directive,
            goal: None,
            last_observation: None,
            stalled_turns: 0,
            stop_reason: None,
        }
    }
}

pub async fn set_autonomous_snapshot(
    state: &Arc<SharedState>,
    session_id: &str,
    snapshot: AutonomousSessionState,
) {
    let sessions = &state.sessions.autonomous_sessions;
    sessions.insert(session_id.to_string(), snapshot);
    let snapshot = sessions.get(session_id);
    if let Some(snapshot) = snapshot {
        state
            .infra
            .relay_conductor_cx
            .notify_desktop(
                crate::types::NOTIF_AUTONOMOUS_STATE,
                json!({
                    "sessionId": session_id,
                    "active": true,
                    "phase": snapshot.phase,
                    "loopIteration": snapshot.loop_iteration,
                    "updatedAtMs": snapshot.updated_at_ms,
                    "note": snapshot.note,
                    "queuedDirective": snapshot.queued_directive,
                    "goal": snapshot.goal,
                    "lastObservation": snapshot.last_observation,
                    "stalledTurns": snapshot.stalled_turns,
                    "stopReason": snapshot.stop_reason,
                }),
            )
            .await;
    }
}

pub async fn set_autonomous_phase(
    state: &Arc<SharedState>,
    session_id: &str,
    phase: AutonomousPhase,
    loop_iteration: u32,
    note: Option<String>,
    queued_directive: Option<String>,
) {
    let previous = state.sessions.autonomous_sessions.get(session_id);
    let mut snapshot = AutonomousSessionState::new(phase, loop_iteration, note, queued_directive);
    if let Some(previous) = previous {
        snapshot.goal = previous.goal.clone();
        snapshot.last_observation = previous.last_observation.clone();
        snapshot.stalled_turns = previous.stalled_turns;
        snapshot.stop_reason = previous.stop_reason.clone();
    }
    set_autonomous_snapshot(state, session_id, snapshot).await;
}

pub async fn current_autonomous_iteration(state: &Arc<SharedState>, session_id: &str) -> u32 {
    let sessions = &state.sessions.autonomous_sessions;
    sessions
        .get(session_id)
        .map(|entry| entry.loop_iteration)
        .unwrap_or(0)
}

pub async fn transition_autonomous_phase(
    state: &Arc<SharedState>,
    session_id: &str,
    phase: AutonomousPhase,
    note: Option<String>,
    queued_directive: Option<String>,
) {
    let iteration = current_autonomous_iteration(state, session_id).await;
    set_autonomous_phase(state, session_id, phase, iteration, note, queued_directive).await;
}

pub async fn clear_autonomous_state(state: &Arc<SharedState>, session_id: &str) {
    let sessions = &state.sessions.autonomous_sessions;
    sessions.remove(session_id);
    state
        .infra
        .relay_conductor_cx
        .notify_desktop(
            crate::types::NOTIF_AUTONOMOUS_STATE,
            json!({
                "sessionId": session_id,
                "active": false,
                "phase": "idle",
                "loopIteration": 0,
                "updatedAtMs": now_millis(),
                "note": null,
                "queuedDirective": null,
                "goal": null,
                "lastObservation": null,
                "stalledTurns": 0,
                "stopReason": null,
            }),
        )
        .await;
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
