use crate::session_store::SessionMessage;

use super::events::TeamTimelineKind;
use super::metadata::parse_json_value_or_null;

pub fn kind_from_message(message: &SessionMessage) -> TeamTimelineKind {
    let blocks = parse_json_value_or_null(&message.content_blocks);
    let task_state = blocks
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find_map(|item| match item.get("type").and_then(|v| v.as_str()) {
                    Some("a2a_task") | Some("a2a_task_result") => {
                        item.get("task_state").and_then(|v| v.as_str())
                    }
                    _ => None,
                })
        })
        .unwrap_or("");

    match message.channel_id.as_str() {
        "a2a:delegation_start" => TeamTimelineKind::DelegationStarted,
        "a2a:task_submitted" => TeamTimelineKind::TaskSubmitted,
        "a2a:task_status" | "a2a:task_working" | "a2a:task_completed" | "a2a:task_failed" => {
            TeamTimelineKind::TaskStatus
        }
        "a2a:delegation_complete" => TeamTimelineKind::DelegationCompleted,
        _ if matches!(task_state, "submitted" | "queued") => TeamTimelineKind::TaskSubmitted,
        _ if !task_state.is_empty() => TeamTimelineKind::TaskStatus,
        "team"
            if message.role == "assistant" && message.agent_id.eq_ignore_ascii_case("leader") =>
        {
            TeamTimelineKind::LeaderFinal
        }
        "team" if message.role == "assistant" => TeamTimelineKind::AgentResponse,
        _ if message.role == "user" => TeamTimelineKind::UserPrompt,
        _ if message.role == "assistant" && !message.agent_id.is_empty() => {
            TeamTimelineKind::AgentResponse
        }
        _ => TeamTimelineKind::SystemNotice,
    }
}

pub fn filter_visible_tool_calls(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return String::new();
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return raw.to_string();
    };

    let filter_item = |item: &serde_json::Value| -> bool {
        let name = item
            .get("name")
            .or_else(|| item.get("toolName"))
            .or_else(|| item.get("tool_name"))
            .or_else(|| item.get("title"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        !(name.contains("mcp_ilhae-tools_delegate")
            || name.contains("mcp_ilhae-tools_delegate_background")
            || name.contains("mcp_ilhae-tools_subscribe_task")
            || name.contains("mcp_ilhae-tools_task_status")
            || name.contains("mcp_ilhae-tools_task_result")
            || name.contains("mcp_ilhae-tools_team_list")
            || name == "delegate"
            || name == "delegate_background"
            || name == "subscribe_task"
            || name == "task_status"
            || name == "task_result"
            || name == "team_list"
            || name == "propose"
            || name == "propose_to_leader")
    };

    match value {
        serde_json::Value::Array(items) => {
            let filtered: Vec<_> = items.into_iter().filter(filter_item).collect();
            if filtered.is_empty() {
                String::new()
            } else {
                serde_json::to_string(&filtered).unwrap_or_default()
            }
        }
        other => {
            if filter_item(&other) {
                serde_json::to_string(&other).unwrap_or_default()
            } else {
                String::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_message(role: &str, channel_id: &str, agent_id: &str) -> SessionMessage {
        SessionMessage {
            id: 1,
            session_id: "s1".to_string(),
            role: role.to_string(),
            content: String::new(),
            timestamp: "2026-03-12T00:00:00Z".to_string(),
            agent_id: agent_id.to_string(),
            thinking: String::new(),
            tool_calls: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            duration_ms: 0,
            content_blocks: String::new(),
            channel_id: channel_id.to_string(),
        }
    }

    #[test]
    fn detects_task_submitted_from_content_block() {
        let mut msg = test_message("assistant", "team", "researcher");
        msg.content_blocks = serde_json::to_string(&vec![serde_json::json!({
            "type": "a2a_task",
            "task_id": "task-1",
            "task_state": "submitted"
        })])
        .unwrap();

        assert_eq!(kind_from_message(&msg), TeamTimelineKind::TaskSubmitted);
    }

    #[test]
    fn detects_task_status_from_content_block() {
        let mut msg = test_message("assistant", "team", "researcher");
        msg.content_blocks = serde_json::to_string(&vec![serde_json::json!({
            "type": "a2a_task_result",
            "task_id": "task-1",
            "task_state": "completed"
        })])
        .unwrap();

        assert_eq!(kind_from_message(&msg), TeamTimelineKind::TaskStatus);
    }
}
