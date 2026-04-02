use crate::session_store::{SessionInfo, SessionMessage};

use super::events::TeamTimelineKind;

pub struct ToolCallMeta {
    pub target_role: serde_json::Value,
    pub request: serde_json::Value,
    pub mode: serde_json::Value,
    pub task_id: serde_json::Value,
    pub context_id: serde_json::Value,
}

pub fn empty_tool_meta() -> ToolCallMeta {
    ToolCallMeta {
        target_role: serde_json::Value::Null,
        request: serde_json::Value::Null,
        mode: serde_json::Value::Null,
        task_id: serde_json::Value::Null,
        context_id: serde_json::Value::Null,
    }
}

pub enum ToolMetaStage {
    Start,
    Result,
}

pub fn parse_json_value_or_null(raw: &str) -> serde_json::Value {
    if raw.trim().is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

pub fn extract_delegation_fields(
    blocks: &serde_json::Value,
) -> (
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
) {
    let Some(items) = blocks.as_array() else {
        return (
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
        );
    };
    for item in items {
        if item.get("type").and_then(|v| v.as_str()) == Some("a2a_delegation") {
            return (
                item.get("target_agent")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                item.get("request")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                item.get("mode").cloned().unwrap_or(serde_json::Value::Null),
                item.get("task_id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                item.get("context_id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
        }
    }
    (
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::Value::Null,
    )
}

pub fn derive_parent_role(parent_info: Option<&SessionInfo>) -> String {
    parent_info
        .map(|parent| {
            if !parent.team_role.trim().is_empty() {
                parent.team_role.clone()
            } else if parent.title.trim_start().starts_with('@') {
                "User".to_string()
            } else {
                "Leader".to_string()
            }
        })
        .unwrap_or_else(|| "Leader".to_string())
}

pub fn build_metadata(
    message: &SessionMessage,
    kind: TeamTimelineKind,
    session_info: Option<&SessionInfo>,
    parent_info: Option<&SessionInfo>,
) -> serde_json::Value {
    let blocks = parse_json_value_or_null(&message.content_blocks);
    let tool_calls = parse_json_value_or_null(&message.tool_calls);
    let (target_role, request, block_mode, block_task_id, block_context_id) =
        extract_delegation_fields(&blocks);
    let parent_team_role = derive_parent_role(parent_info);
    let tool_meta = extract_tool_call_meta(&tool_calls);

    serde_json::json!({
        "kind": kind,
        "channel_id": message.channel_id,
        "agent_id": message.agent_id,
        "team_role": session_info.map(|s| s.team_role.clone()).unwrap_or_default(),
        "parent_session_id": session_info.map(|s| s.parent_session_id.clone()).unwrap_or_default(),
        "parent_team_role": parent_team_role,
        "target_role": if !target_role.is_null() { target_role } else { tool_meta.target_role },
        "request": if !request.is_null() { request } else { tool_meta.request },
        "mode": if !block_mode.is_null() { block_mode } else { tool_meta.mode },
        "task_id": if !block_task_id.is_null() { block_task_id } else { tool_meta.task_id },
        "context_id": if !block_context_id.is_null() { block_context_id } else { tool_meta.context_id },
        "content_blocks": blocks,
        "tool_calls": tool_calls,
    })
}

pub fn extract_matching_tool_call_meta(
    messages: &[SessionMessage],
    target_role: &str,
    stage: ToolMetaStage,
) -> ToolCallMeta {
    for message in messages.iter().rev() {
        if message.role != "assistant" || message.tool_calls.trim().is_empty() {
            continue;
        }
        let parsed = parse_json_value_or_null(&message.tool_calls);
        let Some(items) = parsed.as_array() else {
            continue;
        };
        let filtered = items
            .iter()
            .filter(|item| {
                let input = item.get("rawInput").and_then(|v| v.as_object());
                let output = item
                    .get("rawOutput")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .or_else(|| item.get("rawOutput").cloned());
                let candidate = input
                    .and_then(|v| {
                        v.get("role")
                            .or_else(|| v.get("agent"))
                            .and_then(|v| v.as_str())
                    })
                    .or_else(|| {
                        output
                            .as_ref()
                            .and_then(|v| v.get("role").and_then(|v| v.as_str()))
                    })
                    .unwrap_or("");
                candidate.eq_ignore_ascii_case(target_role)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !filtered.is_empty() {
            let preferred = match stage {
                ToolMetaStage::Start => filtered
                    .iter()
                    .find(|item| {
                        let name = item
                            .get("name")
                            .or_else(|| item.get("toolName"))
                            .or_else(|| item.get("tool_name"))
                            .or_else(|| item.get("title"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        name.contains("delegate_background")
                            || name == "delegate_background"
                            || name.contains("delegate")
                            || name == "delegate"
                    })
                    .cloned(),
                ToolMetaStage::Result => filtered
                    .iter()
                    .rev()
                    .find(|item| item.get("status").and_then(|v| v.as_str()) == Some("completed"))
                    .cloned()
                    .or_else(|| filtered.last().cloned()),
            };

            if let Some(item) = preferred {
                return extract_tool_call_meta(&serde_json::Value::Array(vec![item]));
            }
            return extract_tool_call_meta(&serde_json::Value::Array(filtered));
        }
    }
    empty_tool_meta()
}

pub fn extract_tool_call_meta(tool_calls: &serde_json::Value) -> ToolCallMeta {
    let mut meta = empty_tool_meta();

    let Some(items) = tool_calls.as_array() else {
        return meta;
    };

    let preferred = items
        .iter()
        .rev()
        .find(|item| item.get("status").and_then(|v| v.as_str()) == Some("completed"))
        .or_else(|| items.last());

    let Some(item) = preferred else {
        return meta;
    };

    let name = item
        .get("name")
        .or_else(|| item.get("toolName"))
        .or_else(|| item.get("tool_name"))
        .or_else(|| item.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let input = item.get("rawInput").and_then(|v| v.as_object());
    let output = item
        .get("rawOutput")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .or_else(|| item.get("rawOutput").cloned())
        .unwrap_or(serde_json::Value::Null);
    let output_obj = output.as_object();

    meta.target_role = input
        .and_then(|v| v.get("role").or_else(|| v.get("agent")))
        .cloned()
        .or_else(|| output_obj.and_then(|v| v.get("role").cloned()))
        .unwrap_or(serde_json::Value::Null);
    meta.request = input
        .and_then(|v| {
            v.get("query")
                .or_else(|| v.get("request"))
                .or_else(|| v.get("message"))
                .or_else(|| v.get("proposal"))
        })
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    meta.mode = output_obj
        .and_then(|v| v.get("mode").cloned())
        .or_else(|| input.and_then(|v| v.get("mode").cloned()))
        .or_else(|| {
            if name.contains("delegate_background") {
                Some(serde_json::Value::String("background".to_string()))
            } else if name.contains("subscribe_task")
                || name.contains("task_status")
                || name.contains("task_result")
            {
                Some(serde_json::Value::String("subscribe".to_string()))
            } else if name.contains("delegate") || name.contains("propose") {
                Some(serde_json::Value::String("sync".to_string()))
            } else {
                None
            }
        })
        .unwrap_or(serde_json::Value::Null);
    meta.task_id = input
        .and_then(|v| v.get("task_id").cloned())
        .or_else(|| {
            output_obj.and_then(|v| v.get("task_id").or_else(|| v.get("schedule_id")).cloned())
        })
        .unwrap_or(serde_json::Value::Null);
    meta.context_id = input
        .and_then(|v| v.get("context_id").cloned())
        .or_else(|| {
            output_obj.and_then(|v| v.get("context_id").or_else(|| v.get("contextId")).cloned())
        })
        .unwrap_or(serde_json::Value::Null);

    meta
}
