use crate::session_store::{SessionInfo, SessionMessage};

use super::metadata::{
    ToolMetaStage, derive_parent_role, extract_matching_tool_call_meta, extract_tool_call_meta,
    parse_json_value_or_null,
};

fn derive_effective_parent_role(
    parent_info: Option<&SessionInfo>,
    parent_messages: &[SessionMessage],
) -> String {
    let has_direct_target_user_prompt = parent_messages
        .iter()
        .any(|message| message.role == "user" && message.content.trim_start().starts_with('@'));

    if has_direct_target_user_prompt {
        "User".to_string()
    } else {
        derive_parent_role(parent_info)
    }
}

pub fn transform_descendant_message(
    mut message: SessionMessage,
    session_info: Option<&SessionInfo>,
    parent_info: Option<&SessionInfo>,
    sibling_messages: &[SessionMessage],
    parent_messages: &[SessionMessage],
) -> SessionMessage {
    let Some(info) = session_info else {
        return message;
    };
    if info.parent_session_id.is_empty() {
        return message;
    }

    let this_role = if !info.team_role.trim().is_empty() {
        info.team_role.clone()
    } else if !message.agent_id.trim().is_empty() {
        message.agent_id.clone()
    } else {
        "Agent".to_string()
    };

    let parent_role = derive_effective_parent_role(parent_info, parent_messages);

    if message.role == "user" {
        let request = sanitize_descendant_request(&message.content);
        let sibling_tool_meta = sibling_messages
            .iter()
            .find(|m| m.role == "assistant" && !m.tool_calls.trim().is_empty())
            .map(|m| {
                extract_matching_tool_call_meta(
                    std::slice::from_ref(m),
                    &this_role,
                    ToolMetaStage::Start,
                )
            })
            .unwrap_or_else(|| {
                extract_matching_tool_call_meta(parent_messages, &this_role, ToolMetaStage::Start)
            });
        message.role = "system".to_string();
        message.agent_id = parent_role.to_ascii_lowercase();
        message.channel_id = "a2a:delegation_start".to_string();
        message.content = format!("🛰️ {} → {}", parent_role, this_role);
        message.content_blocks = serde_json::to_string(&vec![serde_json::json!({
            "type": "a2a_delegation",
            "target_agent": this_role,
            "request": request,
            "mode": sibling_tool_meta.mode.clone(),
            "context_id": sibling_tool_meta.context_id.clone(),
            "task_id": sibling_tool_meta.task_id.clone(),
        })])
        .unwrap_or_default();
        message.tool_calls.clear();
        return message;
    }

    if message.role == "assistant" {
        message.content = sanitize_descendant_response(&message.content);
        message.thinking.clear();
        let tool_meta = {
            let local = extract_tool_call_meta(&parse_json_value_or_null(&message.tool_calls));
            if local.mode != serde_json::Value::Null
                || local.task_id != serde_json::Value::Null
                || local.context_id != serde_json::Value::Null
            {
                local
            } else {
                extract_matching_tool_call_meta(parent_messages, &this_role, ToolMetaStage::Result)
            }
        };
        let mut blocks = if message.content_blocks.trim().is_empty() {
            Vec::<serde_json::Value>::new()
        } else {
            serde_json::from_str::<Vec<serde_json::Value>>(&message.content_blocks)
                .unwrap_or_default()
        };
        blocks.retain(|b| b.get("type").and_then(|v| v.as_str()) != Some("thinking"));
        if !blocks
            .iter()
            .any(|b| b.get("type").and_then(|v| v.as_str()) == Some("a2a_delegation"))
        {
            blocks.push(serde_json::json!({
                "type": "a2a_delegation",
                "target_agent": parent_role,
                "mode": tool_meta.mode.clone(),
                "context_id": tool_meta.context_id.clone(),
            }));
        }
        if tool_meta.task_id != serde_json::Value::Null
            && !blocks
                .iter()
                .any(|b| b.get("type").and_then(|v| v.as_str()) == Some("a2a_task_result"))
        {
            let state = tool_meta
                .mode
                .as_str()
                .filter(|mode| mode.eq_ignore_ascii_case("background"))
                .map(|_| "submitted")
                .unwrap_or("completed");
            let block_type = if state == "submitted" {
                "a2a_task"
            } else {
                "a2a_task_result"
            };
            blocks.push(serde_json::json!({
                "type": block_type,
                "task_id": tool_meta.task_id,
                "task_state": state,
                "context_id": tool_meta.context_id,
                "response": message.content,
            }));
        }
        message.content_blocks = serde_json::to_string(&blocks).unwrap_or_default();
        return message;
    }

    message
}

fn sanitize_descendant_request(raw: &str) -> String {
    let mut stripped = raw.to_string();
    while let Some(start) = stripped.find("<system_directive") {
        if let Some(end_rel) = stripped[start..].find("</system_directive>") {
            let end = start + end_rel + "</system_directive>".len();
            stripped.replace_range(start..end, "");
        } else {
            stripped.truncate(start);
            break;
        }
    }

    let mut lines = Vec::new();
    for line in stripped.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
    }

    if let Some(last) = lines.last() {
        last.clone()
    } else {
        stripped.trim().to_string()
    }
}

fn sanitize_descendant_response(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && value.get("mode").and_then(|v| v.as_str()) == Some("background")
        && let Some(task_id) = value
            .get("next_action")
            .and_then(|v| v.get("task_id"))
            .and_then(|v| v.as_str())
    {
        return format!("background task queued ({})", task_id);
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(response) = value.get("response").and_then(|v| v.as_str())
        && !response.trim().is_empty()
    {
        return response.trim().to_string();
    }
    trimmed.to_string()
}
