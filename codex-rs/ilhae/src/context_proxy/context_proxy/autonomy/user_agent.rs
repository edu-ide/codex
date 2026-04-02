use serde_json::json;
use tracing::{info, warn};

use super::resolve_user_agent_endpoint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserAgentDirective {
    Continue(String),
    Complete,
    Empty,
}

pub async fn request_next_directive(
    session_id: &str,
    progress: &str,
) -> Result<UserAgentDirective, String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        request_next_directive_inner(session_id, progress),
    )
    .await
    .map_err(|_| "User Agent timed out after 5s".to_string())?
}

async fn request_next_directive_inner(
    session_id: &str,
    progress: &str,
) -> Result<UserAgentDirective, String> {
    let ua_prompt = format!(
        "Leader agent has finished its turn.\n\n\
         [Leader's Latest Output]\n{}\n\n\
         [System Directive]\n\
         You are the Autonomous Tech Lead & Visionary Product Manager. Do NOT accept completion easily.\n\
         1. TECHNICAL: Scrutinize the output for unhandled edge cases, potential runtime errors, or missing tests.\n\
         2. CONCEPTUAL: If the basic task is done, suggest the 'Next Logical Feature' or UX improvement to expand the project's value.\n\
         If the system is not absolutely bulletproof AND functionally rich, provide a directive to improve or expand it.\n\
         Only if the project is 100% robust and you cannot think of ANY valuable new features, say '모든 작업이 완료되었습니다'.\n\
         CRITICAL: Return PLAIN TEXT ONLY. Do NOT call tools. Do NOT delegate. Do NOT use MCP. Do NOT emit tool calls.\n\
         Your output must be exactly one short next directive sentence in Korean, or exactly '모든 작업이 완료되었습니다'.\n\
         What is your next directive?",
        progress
    );

    let ua_request = json!({
        "jsonrpc": "2.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "parts": [{ "kind": "text", "text": ua_prompt }],
                "messageId": uuid::Uuid::new_v4().to_string(),
                "contextId": session_id.to_string()
            }
        }
    });

    let http_client = reqwest::Client::new();
    let endpoint = resolve_user_agent_endpoint();
    info!(
        "[AutoMode] Requesting next directive from user-agent endpoint={} session={}",
        endpoint, session_id
    );
    let response = http_client
        .post(&endpoint)
        .json(&ua_request)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("User Agent unreachable ({}): {}", endpoint, e))?;

    if !response.status().is_success() {
        return Err(format!("User Agent returned HTTP {}", response.status()));
    }
    info!(
        "[AutoMode] User-agent HTTP response received endpoint={} status={}",
        endpoint,
        response.status()
    );

    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("User Agent JSON parse failed: {}", e))?;

    let mut text = String::new();
    let result_node = body.get("result").unwrap_or(&body);
    if let Some(parts) = result_node
        .pointer("/status/message/parts")
        .and_then(|v| v.as_array())
    {
        for part in parts {
            if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                text.push_str(t);
            }
        }
    }
    if text.is_empty()
        && let Some(content) = result_node.get("content").and_then(|v| v.as_array())
    {
        for block in content {
            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                text.push_str(t);
            }
        }
    }
    if text.is_empty()
        && let Some(history) = result_node.get("history").and_then(|v| v.as_array())
    {
        for message in history.iter().rev() {
            let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if !role.eq_ignore_ascii_case("agent") {
                continue;
            }
            if let Some(parts) = message.get("parts").and_then(|v| v.as_array()) {
                for part in parts {
                    if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                        if !t.trim().is_empty() {
                            text = t.to_string();
                            break;
                        }
                    }
                }
            }
            if !text.is_empty() {
                break;
            }
        }
    }

    let trimmed = text.trim().to_string();
    info!(
        "[AutoMode] User-agent directive parsed endpoint={} len={} preview={}",
        endpoint,
        trimmed.len(),
        &trimmed[..trimmed.len().min(120)]
    );
    if trimmed.is_empty() {
        return Ok(UserAgentDirective::Empty);
    }
    if trimmed.contains("완료되었습니다") || trimmed.contains("프로젝트를 종료") {
        return Ok(UserAgentDirective::Complete);
    }
    Ok(UserAgentDirective::Continue(trimmed))
}

pub async fn request_next_directive_with_fallback(
    session_id: &str,
    progress: &str,
    fallback_builder: impl FnOnce() -> String,
) -> String {
    match request_next_directive(session_id, progress).await {
        Ok(UserAgentDirective::Continue(text)) => text,
        Ok(UserAgentDirective::Complete) => "모든 작업이 완료되었습니다".to_string(),
        Ok(UserAgentDirective::Empty) => {
            warn!("[AutoMode] User Agent returned empty response. Falling back to Ralph Loop.");
            fallback_builder()
        }
        Err(error) => {
            warn!("[AutoMode] {}. Falling back to Ralph Loop.", error);
            fallback_builder()
        }
    }
}
