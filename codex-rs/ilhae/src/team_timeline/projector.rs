use serde_json::json;

use crate::context_proxy::team_utils::TeamSplitMessage;

use super::events::{TeamTimelineEvent, TeamTimelineKind};

pub fn delegation_started_event(
    target_role: &str,
    mode: &str,
    request: &str,
    task_id: Option<&str>,
    tool_call_id: Option<&serde_json::Value>,
) -> TeamTimelineEvent {
    let start_blocks = serde_json::to_string(&vec![json!({
        "type": "a2a_delegation",
        "target_agent": target_role,
        "request": request,
        "mode": mode,
        "tool_call_id": tool_call_id.cloned().unwrap_or(serde_json::Value::Null),
    })])
    .unwrap_or_else(|_| "[]".to_string());

    TeamTimelineEvent::new(
        TeamTimelineKind::DelegationStarted,
        "system",
        format!("🛰️ Leader → {} [{}]", target_role, mode),
    )
    .with_agent_id("leader")
    .with_content_blocks_json(start_blocks)
    .with_metadata(json!({
        "target_role": target_role,
        "request": request,
        "mode": mode,
        "task_id": task_id,
    }))
}

pub fn task_submitted_event(
    role: &str,
    task_id: &str,
    preview: &str,
    state: &str,
) -> TeamTimelineEvent {
    let task_text = json!({
        "taskId": task_id,
        "state": state,
        "preview": preview,
    })
    .to_string();
    let task_blocks = serde_json::to_string(&vec![json!({
        "type": "a2a_task",
        "task_id": task_id,
        "task_state": state,
        "preview": preview,
    })])
    .unwrap_or_else(|_| "[]".to_string());

    TeamTimelineEvent::new(TeamTimelineKind::TaskSubmitted, "system", task_text)
        .with_agent_id(role.to_string())
        .with_content_blocks_json(task_blocks)
        .with_metadata(json!({
            "target_role": role,
            "task_id": task_id,
            "state": state,
            "preview": preview,
        }))
}

pub fn task_status_event(
    role: &str,
    task_id: &str,
    preview: &str,
    state: &str,
    response: Option<&str>,
) -> TeamTimelineEvent {
    let task_text = json!({
        "taskId": task_id,
        "state": state,
        "preview": preview,
        "response": response,
    })
    .to_string();
    let task_blocks = serde_json::to_string(&vec![json!({
        "type": "a2a_task",
        "task_id": task_id,
        "task_state": state,
        "preview": preview,
        "response": response,
    })])
    .unwrap_or_else(|_| "[]".to_string());

    TeamTimelineEvent::new(TeamTimelineKind::TaskStatus, "system", task_text)
        .with_agent_id(role.to_string())
        .with_channel_id("a2a:task_status")
        .with_content_blocks_json(task_blocks)
        .with_metadata(json!({
            "target_role": role,
            "task_id": task_id,
            "state": state,
            "preview": preview,
            "response": response,
        }))
}

pub fn agent_response_event(
    role: &str,
    text: &str,
    tool_calls_json: &str,
    mode: &str,
    task_id: Option<&str>,
) -> TeamTimelineEvent {
    TeamTimelineEvent::new(
        TeamTimelineKind::AgentResponse,
        "assistant",
        text.to_string(),
    )
    .with_agent_id(role.to_string())
    .with_tool_calls_json(tool_calls_json.to_string())
    .with_channel_id("team")
    .with_metadata(json!({
        "source_role": role,
        "mode": mode,
        "task_id": task_id,
    }))
}

pub fn delegation_completed_event(
    role: &str,
    task_id: Option<&str>,
    state: &str,
    response: &str,
    mode: &str,
) -> TeamTimelineEvent {
    let complete_blocks = serde_json::to_string(&vec![json!({
        "type": "a2a_task_result",
        "task_id": task_id,
        "task_state": state,
        "response": response,
    })])
    .unwrap_or_else(|_| "[]".to_string());

    TeamTimelineEvent::new(
        TeamTimelineKind::DelegationCompleted,
        "system",
        format!("✅ {} completed delegation", role),
    )
    .with_agent_id(role.to_string())
    .with_content_blocks_json(complete_blocks)
    .with_metadata(json!({
        "source_role": role,
        "task_id": task_id,
        "state": state,
        "response": response,
        "mode": mode,
    }))
}

pub fn project_split_message_events(
    split: &TeamSplitMessage,
    tool_name: &str,
    request: &str,
    task_id: &str,
    mode: &str,
    output: &serde_json::Value,
    single_tool_call_json: &str,
) -> Vec<TeamTimelineEvent> {
    let mut events = Vec::new();

    if matches!(
        tool_name,
        "delegate" | "delegate_background" | "propose" | "propose_to_leader"
    ) {
        events.push(delegation_started_event(
            &split.role,
            mode,
            request,
            if task_id.is_empty() {
                None
            } else {
                Some(task_id)
            },
            split
                .tool_call
                .get("toolCallId")
                .or_else(|| split.tool_call.get("tool_call_id")),
        ));
    }

    if tool_name == "delegate_background" && !task_id.is_empty() {
        events.push(task_submitted_event(
            &split.role,
            task_id,
            request,
            "submitted",
        ));
    }

    let skip_assistant_for_control_envelope = tool_name == "delegate_background"
        && output.get("mode").and_then(|v| v.as_str()) == Some("background")
        && output.get("task_id").is_some();

    if !skip_assistant_for_control_envelope {
        events.push(agent_response_event(
            &split.role,
            &split.content,
            single_tool_call_json,
            mode,
            if task_id.is_empty() {
                None
            } else {
                Some(task_id)
            },
        ));
    }

    if matches!(tool_name, "delegate" | "subscribe_task" | "task_result") {
        events.push(delegation_completed_event(
            &split.role,
            if task_id.is_empty() {
                None
            } else {
                Some(task_id)
            },
            output
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("completed"),
            &split.content,
            mode,
        ));
    }

    events
}
