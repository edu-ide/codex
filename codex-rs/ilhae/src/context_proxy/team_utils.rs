//! Team Utilities — Helper functions for team orchestration text analysis,
//! role inference, and event formatting.

use agent_client_protocol_schema::PromptResponse;
use regex::Regex;
use sacp::{Client, Conductor, ConnectionTo, UntypedMessage};
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

#[allow(unused_imports)]
use crate::context_proxy::role_parser::*;
#[allow(unused_imports)]
use crate::context_proxy::team_a2a::*;

/// Default team role names for pattern-matching against agent text.
/// These are used when no dynamic config is available.
pub const DEFAULT_TEAM_ROLES: &[&str] = &["Leader", "Researcher", "Verifier", "Creator"];

pub fn contains_abort_or_cancel_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("abort")
        || lower.contains("cancelled")
        || lower.contains("canceled")
        || lower.contains("user aborted")
}

pub fn should_suppress_prompt_error_after_cancel(
    err: &sacp::Error,
    prompt_start_cancel_ver: u64,
    latest_cancel_ver: u64,
) -> bool {
    if latest_cancel_ver <= prompt_start_cancel_ver {
        return false;
    }

    // JSON-RPC request_cancelled code (ACP unstable): -32800
    if i32::from(err.code) == -32800 {
        return true;
    }
    if contains_abort_or_cancel_text(&err.message) {
        return true;
    }

    if let Some(data) = &err.data {
        if let Some(s) = data.as_str() {
            if contains_abort_or_cancel_text(s) {
                return true;
            }
        } else if contains_abort_or_cancel_text(&data.to_string()) {
            return true;
        }
    }

    false
}

pub fn extract_a2a_text_from_prompt_response(response: &PromptResponse) -> Option<String> {
    let meta = response.meta.as_ref()?;
    let text = meta.get("a2a_text")?.as_str()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

pub fn extract_a2a_structured_from_prompt_response(
    response: &PromptResponse,
) -> Option<serde_json::Value> {
    let meta = response.meta.as_ref()?;
    let raw = meta.get("a2a_structured")?;
    if raw.is_null() {
        return None;
    }
    if let Some(s) = raw.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return None;
        }
        return serde_json::from_str(trimmed).ok();
    }
    Some(raw.clone())
}

pub fn looks_like_aggregated_team_payload(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    let mut role_hits = 0;
    for role in DEFAULT_TEAM_ROLES {
        if lower.contains(&role.to_ascii_lowercase()) {
            role_hits += 1;
        }
    }
    role_hits >= 3 && (message.contains('\n') || message.contains("**") || message.contains('['))
}

pub fn looks_like_verbose_role_event_payload(source_role: &str, message: &str) -> bool {
    if !message.contains('\n') {
        return false;
    }
    let lower = message.to_ascii_lowercase();
    let role = source_role.trim().to_ascii_lowercase();
    if role.is_empty() {
        return false;
    }

    lower.contains(&format!("**{} (", role))
        || lower.contains(&format!("[{}]", role))
        || lower.contains(&format!("\n{} (", role))
}

pub fn infer_team_role_from_text(text: &str) -> Option<String> {
    for role in DEFAULT_TEAM_ROLES {
        let pattern = format!(r"(?i)\b{}\b", role);
        if let Ok(re) = Regex::new(&pattern) {
            if re.is_match(text) {
                return normalize_team_role(role).map(|s| s.to_string());
            }
        }
    }
    None
}

pub fn infer_explicit_target_role_from_text(text: &str) -> Option<String> {
    // Build dynamic regex from DEFAULT_TEAM_ROLES
    let role_pattern = DEFAULT_TEAM_ROLES.join("|");
    let re = Regex::new(&format!(
        r"(?i)\b({roles})\b\s*->\s*\b({roles})\b",
        roles = role_pattern
    ))
    .ok()?;
    let caps = re.captures(text)?;
    let dst = caps.get(2).map(|m| m.as_str())?;
    normalize_team_role(dst).map(|s| s.to_string())
}

pub fn assign_event_team_role(source_role: &str, message: &str) -> String {
    if let Some(role) = infer_explicit_target_role_from_text(message) {
        return role;
    }
    if let Some(role) = infer_team_role_from_text(message) {
        return role;
    }
    if let Some(role) = normalize_team_role(source_role).map(|s| s.to_string()) {
        return role;
    }
    "Leader".to_string()
}

pub fn format_a2a_event_line(source_role: &str, assigned_role: &str, message: &str) -> String {
    let src = source_role.trim();
    let dst = normalize_team_role(assigned_role)
        .map(|s| s.to_string())
        .or_else(|| infer_explicit_target_role_from_text(message))
        .or_else(|| infer_team_role_from_text(message));
    if let Some(ref dst) = dst {
        if !src.eq_ignore_ascii_case(dst) {
            return format!("🛰️ A2A {} -> {}: {}", src, dst, message);
        }
    }
    format!("🛰️ A2A [{}] {}", src, message)
}

pub fn emit_a2a_role_event_notifications(
    cx: &ConnectionTo<Conductor>,
    session_id: &str,
    _assistant_text: &str,
    structured: Option<&serde_json::Value>,
) {
    let events = structured
        .map(extract_a2a_role_events_from_structured)
        .unwrap_or_default();

    for (source_role, message) in events {
        let assigned_role = assign_event_team_role(&source_role, &message);
        let event_line = format_a2a_event_line(&source_role, &assigned_role, &message);
        if let Ok(notif) = UntypedMessage::new(
            crate::types::NOTIF_A2A_EVENT,
            json!({
                "session_id": session_id,
                "source_role": source_role,
                "assigned_role": assigned_role,
                "message": message,
                "event_line": event_line,
            }),
        ) {
            let _ = cx.send_notification_to(Client, notif);
        }
    }
}

fn map_tool_status(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "success" | "completed" | "complete" => "completed",
        "failed" | "error" => "failed",
        "validating" | "scheduled" | "executing" | "running" | "working" => "in_progress",
        _ => "pending",
    }
}

fn extract_string_output(value: &serde_json::Value) -> Option<String> {
    let obj = value.as_object()?;

    if let Some(result_display) = obj.get("resultDisplay").and_then(|v| v.as_str()) {
        let trimmed = result_display.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Some(output) = obj.get("output").and_then(|v| v.as_str()) {
        let trimmed = output.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Some(response) = obj.get("response").and_then(|v| v.as_str()) {
        let trimmed = response.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Some(subject) = obj.get("subject").and_then(|v| v.as_str()) {
        let description = obj
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let subject = subject.trim();
        if !subject.is_empty() || !description.is_empty() {
            return Some(if description.is_empty() {
                subject.to_string()
            } else {
                format!("{}\n{}", subject, description)
            });
        }
    }

    if let Some(parts) = obj.get("responseParts").and_then(|v| v.as_array()) {
        for part in parts {
            if let Some(output) = part
                .get("functionResponse")
                .and_then(|v| v.get("response"))
                .and_then(extract_string_output)
            {
                return Some(output);
            }
        }
    }

    if let Some(nested) = obj.get("response").and_then(extract_string_output) {
        return Some(nested);
    }

    None
}

fn collect_tool_calls_from_value(
    value: &serde_json::Value,
    tool_calls: &mut BTreeMap<String, serde_json::Value>,
    text_candidates: &mut Vec<String>,
    depth: usize,
) {
    if depth > 10 {
        return;
    }

    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                collect_tool_calls_from_value(item, tool_calls, text_candidates, depth + 1);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(request) = map.get("request").and_then(|v| v.as_object()) {
                if let Some(call_id) = request.get("callId").and_then(|v| v.as_str()) {
                    let title = map
                        .get("tool")
                        .and_then(|v| v.get("displayName").or_else(|| v.get("name")))
                        .and_then(|v| v.as_str())
                        .or_else(|| request.get("name").and_then(|v| v.as_str()))
                        .unwrap_or("tool")
                        .to_string();
                    let name = request
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&title)
                        .to_string();
                    let kind = map
                        .get("tool")
                        .and_then(|v| v.get("kind"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool")
                        .to_string();
                    let status_raw = map
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("pending");
                    let status = map_tool_status(status_raw).to_string();
                    let raw_input = request
                        .get("args")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let raw_output_text = map
                        .get("response")
                        .and_then(extract_string_output)
                        .or_else(|| extract_string_output(value));

                    if let Some(output) = raw_output_text.clone() {
                        if !output.trim().is_empty() && status == "completed" {
                            text_candidates.push(output.clone());
                        }
                    }

                    tool_calls.insert(
                        call_id.to_string(),
                        json!({
                            "toolCallId": call_id,
                            "title": title,
                            "name": name,
                            "kind": kind,
                            "status": status,
                            "rawInput": raw_input,
                            "rawOutput": raw_output_text.unwrap_or_default(),
                        }),
                    );
                }
            }

            if let Some(text) = extract_string_output(value) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    text_candidates.push(trimmed.to_string());
                }
            }

            for nested in map.values() {
                collect_tool_calls_from_value(nested, tool_calls, text_candidates, depth + 1);
            }
        }
        _ => {}
    }
}

pub fn synthesize_assistant_from_a2a_structured(
    structured: &serde_json::Value,
) -> Option<(String, Vec<serde_json::Value>)> {
    let mut text_candidates: Vec<String> = Vec::new();

    if let Some(role_sections) = structured.get("role_sections").and_then(|v| v.as_array()) {
        for item in role_sections {
            let role = item
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let content = item
                .get("content")
                .or_else(|| item.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if !role.is_empty() && !content.is_empty() {
                text_candidates.push(format!("[{}]\n{}", role, content));
            }
        }
    }

    if let Some(events) = structured.get("events").and_then(|v| v.as_array()) {
        for item in events {
            let msg = item
                .get("message")
                .or_else(|| item.get("content"))
                .or_else(|| item.get("text"))
                .or_else(|| item.get("body"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if !msg.is_empty() {
                text_candidates.push(msg.to_string());
            }
        }
    }

    let mut tool_calls: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    collect_tool_calls_from_value(structured, &mut tool_calls, &mut text_candidates, 0);
    if let Some(raw_events) = structured.get("raw_events") {
        collect_tool_calls_from_value(raw_events, &mut tool_calls, &mut text_candidates, 0);
    }
    if let Some(stream_attempt) = structured.get("stream_attempt") {
        collect_tool_calls_from_value(stream_attempt, &mut tool_calls, &mut text_candidates, 0);
    }

    let content = text_candidates
        .into_iter()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .fold(Vec::<String>::new(), |mut acc, item| {
            if !acc.iter().any(|existing| existing == &item) {
                acc.push(item);
            }
            acc
        })
        .join("\n\n");

    if content.trim().is_empty() && tool_calls.is_empty() {
        return None;
    }

    Some((content, tool_calls.into_values().collect()))
}

fn parse_tool_output_response(raw_output: &str) -> Option<String> {
    let trimmed = raw_output.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(response) = value.get("response").and_then(|v| v.as_str()) {
            let response = response.trim();
            if !response.is_empty() {
                return Some(response.to_string());
            }
        }
    }

    Some(trimmed.to_string())
}

fn tool_call_role(tool_call: &serde_json::Value) -> Option<String> {
    tool_call
        .get("rawInput")
        .and_then(|v| v.get("role"))
        .and_then(|v| v.as_str())
        .map(|role| role.trim().to_string())
        .filter(|role| !role.is_empty())
        .or_else(|| {
            tool_call
                .get("rawOutput")
                .and_then(|v| v.as_str())
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                .and_then(|value| {
                    value
                        .get("role")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
        })
}

pub fn preferred_assistant_content_from_tool_calls(
    tool_calls: &[serde_json::Value],
) -> Option<String> {
    let mut last_non_empty: Option<String> = None;

    for tool_call in tool_calls {
        let raw_output = tool_call
            .get("rawOutput")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(response) = parse_tool_output_response(raw_output) else {
            continue;
        };
        let cleaned = response.trim();
        if cleaned.is_empty() {
            continue;
        }

        if tool_call_role(tool_call)
            .as_deref()
            .is_some_and(|role| role.eq_ignore_ascii_case("Creator"))
        {
            return Some(cleaned.to_string());
        }

        last_non_empty = Some(cleaned.to_string());
    }

    last_non_empty
}

#[derive(Debug, Clone)]
pub struct TeamSplitMessage {
    pub role: String,
    pub content: String,
    pub tool_call: serde_json::Value,
}

pub fn extract_team_split_messages_from_tool_calls(
    tool_calls: &[serde_json::Value],
) -> Vec<TeamSplitMessage> {
    let mut seen_ids = BTreeSet::new();
    let mut split_messages = Vec::new();

    for tool_call in tool_calls {
        let Some(role) = tool_call_role(tool_call) else {
            continue;
        };
        let raw_output = tool_call
            .get("rawOutput")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(content) = parse_tool_output_response(raw_output) else {
            continue;
        };
        let content = content.trim();
        if content.is_empty() {
            continue;
        }

        let tool_call_id = tool_call
            .get("toolCallId")
            .or_else(|| tool_call.get("tool_call_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !tool_call_id.is_empty() && !seen_ids.insert(tool_call_id.to_string()) {
            continue;
        }

        split_messages.push(TeamSplitMessage {
            role,
            content: content.to_string(),
            tool_call: tool_call.clone(),
        });
    }

    split_messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_abort_text() {
        assert!(contains_abort_or_cancel_text("Request was cancelled"));
        assert!(contains_abort_or_cancel_text("user aborted the task"));
        assert!(!contains_abort_or_cancel_text("everything is fine"));
    }

    #[test]
    fn infer_role_from_text_leader() {
        assert_eq!(
            infer_team_role_from_text("The Leader will coordinate"),
            Some("Leader".to_string())
        );
    }

    #[test]
    fn infer_role_from_text_none() {
        assert_eq!(infer_team_role_from_text("No role mentioned here"), None);
    }

    #[test]
    fn explicit_target_role() {
        let text = "Leader -> Researcher please look into this";
        let result = infer_explicit_target_role_from_text(text);
        assert_eq!(result, Some("Researcher".to_string()));
    }

    #[test]
    fn assign_role_fallback_to_source() {
        let role = assign_event_team_role("researcher", "Some text");
        assert_eq!(role, "Researcher");
    }

    #[test]
    fn assign_role_fallback_to_leader() {
        let role = assign_event_team_role("unknown_role", "Some text");
        assert_eq!(role, "Leader");
    }

    #[test]
    fn looks_like_aggregated_payload_requires_3_roles() {
        let text = "Leader\nResearcher\nVerifier\n**some bold**";
        assert!(looks_like_aggregated_team_payload(text));

        let text2 = "Leader only";
        assert!(!looks_like_aggregated_team_payload(text2));
    }

    #[test]
    fn format_event_line_same_role() {
        let line = format_a2a_event_line("Leader", "leader", "hello");
        assert!(line.contains("A2A [Leader]"));
    }

    #[test]
    fn format_event_line_different_roles() {
        let line = format_a2a_event_line("Leader", "researcher", "check this");
        assert!(line.contains("->"));
    }
}
