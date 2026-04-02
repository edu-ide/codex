use serde_json::Value;

/// Events related to team delegation and orchestration.
/// These events are emitted by the A2A routing layer and consumed
/// by the delegation tracker daemon to persist history to the brain.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum DelegationEvent {
    /// Fired when an agent delegates to another agent.
    Started {
        leader_session_id: String,
        target_role: String,
        leader_role: String,
        mode: String,
        request_text: String,
        channel_id: String,
    },
    /// Fired when a sub-agent completes a delegation request and returns a response.
    ResultTapped {
        leader_session_id: String,
        target_role: String,
        leader_role: String,
        response_text: String,
        schedule_id: String,
        task_state: String,
        duration_ms: i64,
        artifacts: Value,
        history: Value,
    },
}
