use serde_json::json;
use tracing::warn;
use agent_client_protocol_schema::PromptResponse;
use regex::Regex;
use sacp::{Client, Conductor, ConnectionTo, UntypedMessage};

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
    if headers.len() < 2 {
        return Vec::new();
    }

    let mut sections: Vec<(String, String)> = Vec::new();
    for (i, (line_idx, role, inline)) in headers.iter().enumerate() {
        let next_line_idx = headers.get(i + 1).map(|h| h.0).unwrap_or(lines.len());
        let mut body_parts: Vec<String> = Vec::new();
        if !inline.trim().is_empty() {
            body_parts.push(inline.trim().to_string());
        }
        for line in lines.iter().take(next_line_idx).skip(line_idx + 1) {
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                body_parts.push(trimmed.to_string());
            }
        }
        let body = body_parts
            .join("\n")
            .trim()
            .trim_matches('*')
            .trim()
            .to_string();
        if body.is_empty() {
            continue;
        }
        if let Some((_, existing_body)) = sections.iter_mut().find(|(r, _)| r == role) {
            if !existing_body.is_empty() {
                existing_body.push_str("\n\n");
            }
            existing_body.push_str(&body);
        } else {
            sections.push((role.clone(), body));
        }
    }
    sections
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

    out.trim().to_string()
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
        if let Ok(re) = Regex::new(&pattern)
            && re.is_match(text) {
                return Some(role);
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
    if let Some(dst) = dst
        && !src.eq_ignore_ascii_case(dst) {
            return format!("🛰️ A2A {} -> {}: {}", src, dst, message);
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

}

// ─── ContextProxy state ─────────────────────────────────────────────────

