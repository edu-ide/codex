use regex::Regex;
use sacp::{Client, Conductor, ConnectionTo, UntypedMessage};
use serde_json::json;
use tracing::warn;

/// Extract role-based sections from the Leader's aggregated response.
/// The Leader may structure its response with role markers like:
///   **Leader (계획):** ...
///   **Researcher (자료 조사):** ...
/// If no such markers are found, the entire response is attributed to Leader.
pub fn extract_role_sections(text: &str) -> Vec<serde_json::Value> {
    let role_pattern = regex::Regex::new(r"(?m)^\*\*(\w+)\s*(?:\([^)]*\))?\s*:\*\*\s*(.*)$")
        .unwrap_or_else(|_| regex::Regex::new(r"^$").unwrap());

    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_role: Option<String> = None;
    let mut current_content = String::new();

    for line in text.lines() {
        if let Some(caps) = role_pattern.captures(line) {
            // Save previous section
            if let Some(role) = current_role.take() {
                if !current_content.trim().is_empty() {
                    sections.push((role, current_content.trim().to_string()));
                }
            }
            let role_name = caps
                .get(1)
                .map(|m| m.as_str())
                .unwrap_or("Leader")
                .to_string();
            let first_line = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            current_role = Some(role_name);
            current_content = first_line;
        } else if current_role.is_some() {
            current_content.push('\n');
            current_content.push_str(line);
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    // Flush last section
    if let Some(role) = current_role {
        if !current_content.trim().is_empty() {
            sections.push((role, current_content.trim().to_string()));
        }
    } else if !current_content.trim().is_empty() {
        // No role markers found — attribute everything to Leader
        sections.push(("Leader".to_string(), current_content.trim().to_string()));
    }

    sections
        .iter()
        .map(|(role, content)| json!({ "role": role, "content": content }))
        .collect()
}

pub fn normalize_team_role(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "leader" => Some("Leader"),
        "researcher" => Some("Researcher"),
        "verifier" => Some("Verifier"),
        "creator" => Some("Creator"),
        _ => None,
    }
}

pub fn extract_team_role_sections(text: &str) -> Vec<(String, String)> {
    let heading_re = match Regex::new(
        r"(?i)^\s*[\p{P}\p{S}\d\s]*(leader|researcher|verifier|creator)\b(?:\s*\([^\n)]*\))?\s*:?\s*\*{0,2}\s*(.*)$",
    ) {
        Ok(v) => v,
        Err(e) => {
            warn!("[TeamSplit] failed to compile heading regex: {}", e);
            return Vec::new();
        }
    };

    let lines: Vec<&str> = text.lines().collect();
    let mut headers: Vec<(usize, String, String)> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let Some(cap) = heading_re.captures(line) else {
            continue;
        };
        let Some(role_raw) = cap.get(1).map(|m| m.as_str()) else {
            continue;
        };
        let Some(role) = normalize_team_role(role_raw) else {
            continue;
        };
        let mut inline = cap
            .get(2)
            .map(|m| {
                m.as_str()
                    .trim()
                    .trim_start_matches(|ch: char| {
                        matches!(
                            ch,
                            ']' | ')' | '>' | '}' | ':' | '-' | '–' | '—' | '·' | '•' | '|'
                        )
                    })
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();
        if let Some(idx) = inline.find(']') {
            let before = inline[..idx].trim();
            if idx == 0 || (!before.contains(' ') && before.chars().count() <= 12) {
                inline = inline[idx + 1..].trim().to_string();
            }
        }
        headers.push((idx, role.to_string(), inline));
    }

    let mut result = Vec::new();
    for i in 0..headers.len() {
        let (start, ref role, ref inline) = headers[i];
        let end = if i + 1 < headers.len() {
            headers[i + 1].0
        } else {
            lines.len()
        };
        let mut body_lines: Vec<&str> = Vec::new();
        if !inline.is_empty() {
            body_lines.push(inline);
        }
        for line_idx in (start + 1)..end {
            body_lines.push(lines[line_idx]);
        }
        let body = body_lines.join("\n").trim().to_string();
        if !body.is_empty() {
            result.push((role.clone(), body));
        }
    }
    result
}

pub fn extract_a2a_role_events(text: &str) -> Vec<(String, String)> {
    let line_re = match Regex::new(r"^\s*\[([A-Za-z][A-Za-z0-9_-]{1,31})\]\s*(.+?)\s*$") {
        Ok(v) => v,
        Err(e) => {
            warn!("[TeamSplit] failed to compile a2a event regex: {}", e);
            return Vec::new();
        }
    };

    text.lines()
        .filter_map(|line| {
            let cap = line_re.captures(line)?;
            let role = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            let msg = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            if role.is_empty() || msg.is_empty() {
                return None;
            }
            Some((role.to_string(), msg.to_string()))
        })
        .collect()
}

pub fn upsert_role_section(sections: &mut Vec<(String, String)>, role: String, content: String) {
    if let Some((_, existing)) = sections.iter_mut().find(|(r, _)| *r == role) {
        if !existing.is_empty() {
            existing.push_str("\n\n");
        }
        existing.push_str(content.trim());
    } else {
        sections.push((role, content.trim().to_string()));
    }
}

pub fn sanitize_role_section_content(role: &str, content: &str) -> String {
    let mut out = content.replace("\r\n", "\n").trim().to_string();
    if out.is_empty() {
        return out;
    }

    let role_escaped = regex::escape(role);
    let mut patterns = Vec::with_capacity(3);
    for raw in [
        format!(r"(?is)^\s*\[\s*{}\s*\]\s*", role_escaped),
        format!(r"(?is)^\s*{}\s*\]\s*", role_escaped),
        format!(
            r"(?is)^\s*\*{{0,2}}\s*{}(?:\s*\([^\n)]*\))?\s*:?\s*\*{{0,2}}\s*",
            role_escaped
        ),
    ] {
        if let Ok(re) = Regex::new(&raw) {
            patterns.push(re);
        }
    }

    for _ in 0..6 {
        let mut changed = false;
        for re in &patterns {
            if re.is_match(&out) {
                out = re.replace(&out, "").to_string().trim_start().to_string();
                changed = true;
            }
        }
        let trimmed = out
            .trim_start_matches(|ch: char| {
                matches!(
                    ch,
                    ']' | ')' | '}' | '>' | ':' | '-' | '–' | '—' | '•' | '·' | '|'
                )
            })
            .trim_start()
            .to_string();
        if trimmed != out {
            out = trimmed;
            changed = true;
        }
        if !changed {
            break;
        }
    }
    out
}

pub fn normalize_role_sections(sections: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (role, content) in sections {
        let cleaned = sanitize_role_section_content(&role, &content);
        if cleaned.is_empty() {
            continue;
        }
        upsert_role_section(&mut out, role, cleaned);
    }
    out
}

pub fn has_malformed_leading_role_markers(sections: &[(String, String)]) -> bool {
    let heading_like = Regex::new(r"(?i)^\s*\[(leader|researcher|verifier|creator)\b").ok();
    sections.iter().any(|(_, body)| {
        let trimmed = body.trim_start();
        trimmed.starts_with(']')
            || heading_like
                .as_ref()
                .map(|re| re.is_match(trimmed))
                .unwrap_or(false)
    })
}

pub fn extract_team_role_sections_from_structured(
    _structured: Option<&serde_json::Value>,
    _role_names: &[String],
    events: &mut Vec<serde_json::Value>,
) {
    let items = _structured
        .and_then(|s| s.get("role_sections"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let role_raw = obj
            .get("role")
            .or_else(|| obj.get("team_role"))
            .or_else(|| obj.get("agent_role"))
            .or_else(|| obj.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let Some(role) = normalize_team_role(role_raw).map(str::to_string) else {
            continue;
        };
        let content = obj
            .get("content")
            .or_else(|| obj.get("text"))
            .or_else(|| obj.get("body"))
            .or_else(|| obj.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if content.is_empty() {
            continue;
        }
        events.push(serde_json::json!({
            "source_role": role,
            "message": content
        }));
    }
}

pub fn extract_a2a_role_events_from_structured(
    structured: &serde_json::Value,
) -> Vec<(String, String)> {
    let mut events = Vec::new();
    let items = structured
        .get("events")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let source_role = obj
            .get("source_role")
            .or_else(|| obj.get("role"))
            .or_else(|| obj.get("agent"))
            .or_else(|| obj.get("from"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let message = obj
            .get("message")
            .or_else(|| obj.get("content"))
            .or_else(|| obj.get("text"))
            .or_else(|| obj.get("body"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let message = sanitize_role_section_content(&source_role, &message);
        if source_role.eq_ignore_ascii_case("agent") && looks_like_aggregated_team_payload(&message)
        {
            continue;
        }
        if looks_like_verbose_role_event_payload(&source_role, &message) {
            continue;
        }
        if source_role.is_empty() || message.is_empty() {
            continue;
        }
        events.push((source_role, message));
    }
    events
}

pub fn looks_like_aggregated_team_payload(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    let mut role_hits = 0;
    for role in ["leader", "researcher", "verifier", "creator"] {
        if lower.contains(role) {
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

pub fn infer_team_role_from_text(text: &str) -> Option<&'static str> {
    for role in ["Leader", "Researcher", "Verifier", "Creator"] {
        let pattern = format!(r"(?i)\b{}\b", role);
        if let Ok(re) = Regex::new(&pattern) {
            if re.is_match(text) {
                return Some(role);
            }
        }
    }
    None
}

pub fn infer_explicit_target_role_from_text(text: &str) -> Option<&'static str> {
    // Prefer explicit transfer syntax (A -> B) to avoid ambiguous role inference.
    let re = Regex::new(
        r"(?i)\b(leader|researcher|verifier|creator)\b\s*->\s*\b(leader|researcher|verifier|creator)\b",
    )
    .ok()?;
    let caps = re.captures(text)?;
    let dst = caps.get(2).map(|m| m.as_str())?;
    normalize_team_role(dst)
}

pub fn assign_event_team_role(source_role: &str, message: &str) -> &'static str {
    if let Some(role) = infer_explicit_target_role_from_text(message) {
        return role;
    }
    if let Some(role) = infer_team_role_from_text(message) {
        return role;
    }
    if let Some(role) = normalize_team_role(source_role) {
        return role;
    }
    "Leader"
}

pub fn format_a2a_event_line(source_role: &str, assigned_role: &str, message: &str) -> String {
    let src = source_role.trim();
    let dst = normalize_team_role(assigned_role)
        .or_else(|| infer_explicit_target_role_from_text(message))
        .or_else(|| infer_team_role_from_text(message));
    if let Some(dst) = dst {
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
        let event_line = format_a2a_event_line(&source_role, assigned_role, &message);
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

pub fn persist_team_split_messages(
    _store: &crate::session_store::SessionStore,
    _parent_session_id: &str,
    _user_text: &str,
    _assistant_text: &str,
    _parent_agent_id: &str,
    _structured: Option<&serde_json::Value>,
) {
    // True A2A: Disable backend legacy text splitting.
    // The actual agents will stream their own turns into the DB via ilhae/assistant_turn_patch.
    return;
}

pub fn normalize_a2a_state(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace('_', "-")
}

pub fn is_terminal_a2a_state(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "canceled" | "cancelled")
}

pub fn is_input_required_a2a_state(state: &str) -> bool {
    matches!(state, "input-required" | "inputrequired")
}

pub fn extract_text_from_a2a_part(part: &serde_json::Value) -> Option<String> {
    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let data = part.get("data")?;
    if let Some(text) = data.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let obj = data.as_object()?;
    for key in ["text", "description", "content", "summary", "message"] {
        if let Some(text) = obj.get(key).and_then(|v| v.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub fn collect_text_from_a2a_parts(parts: &[serde_json::Value], delimiter: &str) -> String {
    parts
        .iter()
        .filter_map(extract_text_from_a2a_part)
        .collect::<Vec<_>>()
        .join(delimiter)
}

pub fn parse_a2a_message_text(result: &serde_json::Value) -> String {
    let parts_text = result
        .get("parts")
        .and_then(|v| v.as_array())
        .map(|parts| collect_text_from_a2a_parts(parts, ""))
        .unwrap_or_default();
    if !parts_text.trim().is_empty() {
        return parts_text.trim().to_string();
    }

    let status_text = result
        .get("status")
        .and_then(|v| v.get("message"))
        .and_then(|v| v.get("parts"))
        .and_then(|v| v.as_array())
        .map(|parts| collect_text_from_a2a_parts(parts, ""))
        .unwrap_or_default();
    if !status_text.trim().is_empty() {
        return status_text.trim().to_string();
    }

    let history_text = result
        .get("history")
        .and_then(|v| v.as_array())
        .map(|history| {
            history
                .iter()
                .filter(|msg| msg.get("role").and_then(|v| v.as_str()) == Some("agent"))
                .flat_map(|msg| {
                    msg.get("parts")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|part| extract_text_from_a2a_part(&part))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if !history_text.trim().is_empty() {
        return history_text.trim().to_string();
    }

    let artifact_text = result
        .get("artifacts")
        .and_then(|v| v.as_array())
        .map(|artifacts| {
            artifacts
                .iter()
                .flat_map(|artifact| {
                    artifact
                        .get("parts")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|part| extract_text_from_a2a_part(&part))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    artifact_text.trim().to_string()
}

/// Parsed result from an A2A response.
#[derive(Debug, Clone, Default)]
pub struct A2AResponseParsed {
    pub text: String,
    pub state: Option<String>,
    pub schedule_id: Option<String>,
    pub context_id: Option<String>,
}

pub fn parse_a2a_result(result: &serde_json::Value) -> A2AResponseParsed {
    let state = result
        .get("status")
        .and_then(|v| v.get("state"))
        .and_then(|v| v.as_str())
        .map(normalize_a2a_state)
        .or_else(|| {
            result
                .get("state")
                .and_then(|v| v.as_str())
                .map(normalize_a2a_state)
        });

    let schedule_id = result
        .get("taskId")
        .and_then(|v| v.as_str())
        .or_else(|| result.get("schedule_id").and_then(|v| v.as_str()))
        .or_else(|| result.get("id").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    let context_id = result
        .get("contextId")
        .and_then(|v| v.as_str())
        .or_else(|| result.get("context_id").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    A2AResponseParsed {
        text: parse_a2a_message_text(result),
        state,
        schedule_id,
        context_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── normalize_team_role ──────────────────────────────────────

    #[test]
    fn normalize_team_role_known_roles() {
        assert_eq!(normalize_team_role("leader"), Some("Leader"));
        assert_eq!(normalize_team_role("RESEARCHER"), Some("Researcher"));
        assert_eq!(normalize_team_role("  Verifier  "), Some("Verifier"));
        assert_eq!(normalize_team_role("Creator"), Some("Creator"));
    }

    #[test]
    fn normalize_team_role_unknown() {
        assert_eq!(normalize_team_role("unknown"), None);
        assert_eq!(normalize_team_role(""), None);
        assert_eq!(normalize_team_role("codex"), None);
    }

    // ── extract_role_sections ────────────────────────────────────

    #[test]
    fn extract_role_sections_with_markers() {
        let text = "**Leader (계획):** Plan the task\n**Researcher (조사):** Find data";
        let sections = extract_role_sections(text);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0]["role"], "Leader");
        assert_eq!(sections[1]["role"], "Researcher");
    }

    #[test]
    fn extract_role_sections_no_markers() {
        let text = "Just a normal response with no role markers.";
        let sections = extract_role_sections(text);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0]["role"], "Leader");
        assert_eq!(sections[0]["content"], text);
    }

    #[test]
    fn extract_role_sections_empty() {
        let sections = extract_role_sections("");
        assert!(sections.is_empty());
    }

    // ── parse_a2a_result ────────────────────────────────────────

    #[test]
    fn parse_a2a_result_with_status() {
        let val = serde_json::json!({
            "status": {
                "state": "COMPLETED",
                "message": {
                    "role": "agent",
                    "parts": [{ "text": "Hello from agent" }]
                }
            },
            "taskId": "task-123"
        });
        let parsed = parse_a2a_result(&val);
        assert_eq!(parsed.state, Some("completed".to_string()));
        assert_eq!(parsed.schedule_id, Some("task-123".to_string()));
        assert!(parsed.text.contains("Hello from agent"));
    }

    #[test]
    fn parse_a2a_result_empty() {
        let val = serde_json::json!({});
        let parsed = parse_a2a_result(&val);
        assert_eq!(parsed.state, None);
        assert_eq!(parsed.schedule_id, None);
        assert!(parsed.text.is_empty());
    }

    // ── extract_team_role_sections ──────────────────────────────

    #[test]
    fn extract_team_role_sections_basic() {
        let text = "**Leader:** Here is the plan\n\n**Researcher:** Findings below";
        let sections = extract_team_role_sections(text);
        assert!(sections.len() >= 2);
        assert_eq!(sections[0].0, "Leader");
        assert_eq!(sections[1].0, "Researcher");
    }
}
