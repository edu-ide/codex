use rusqlite::Result as SqlResult;
use serde::{Deserialize, Serialize};

use crate::session_store::{SessionInfo, SessionMessage, SessionStore};

use super::classifier::{filter_visible_tool_calls, kind_from_message};
use super::descendant_transform::transform_descendant_message;
use super::events::TeamTimelineKind;
use super::loader::load_timeline_inputs;
use super::metadata::build_metadata;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTeamTimelineEvent {
    pub message_id: i64,
    pub session_id: String,
    pub timestamp: String,
    pub kind: TeamTimelineKind,
    pub role: String,
    pub agent_id: String,
    pub content: String,
    pub thinking: String,
    pub tool_calls_json: String,
    pub content_blocks_json: String,
    pub channel_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub duration_ms: i64,
    pub priority: i32,
    pub metadata: serde_json::Value,
}

pub fn load_session_timeline(
    store: &SessionStore,
    session_id: &str,
) -> SqlResult<Vec<PersistedTeamTimelineEvent>> {
    let (session_map, source_messages, messages_by_session) =
        load_timeline_inputs(store, session_id)?;

    let mut events: Vec<_> = source_messages
        .into_iter()
        .map(|message| project_message(message, &session_map, &messages_by_session))
        .collect();

    events.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| a.priority.cmp(&b.priority))
            .then_with(|| a.message_id.cmp(&b.message_id))
    });

    Ok(postprocess_timeline(events))
}

fn project_message(
    message: SessionMessage,
    session_map: &std::collections::HashMap<String, SessionInfo>,
    messages_by_session: &std::collections::HashMap<String, Vec<SessionMessage>>,
) -> PersistedTeamTimelineEvent {
    let session_info = session_map.get(&message.session_id);
    let parent_info = session_info.and_then(|info| {
        if info.parent_session_id.is_empty() {
            None
        } else {
            session_map.get(&info.parent_session_id)
        }
    });
    let sibling_messages = messages_by_session
        .get(&message.session_id)
        .map(|items| items.as_slice())
        .unwrap_or(&[]);
    let parent_messages = session_info
        .and_then(|info| {
            if info.parent_session_id.is_empty() {
                None
            } else {
                messages_by_session.get(&info.parent_session_id)
            }
        })
        .map(|items| items.as_slice())
        .unwrap_or(&[]);
    let transformed = transform_descendant_message(
        message,
        session_info,
        parent_info,
        sibling_messages,
        parent_messages,
    );
    let kind = kind_from_message(&transformed);
    let metadata = build_metadata(&transformed, kind, session_info, parent_info);
    let tool_calls_json = filter_visible_tool_calls(&transformed.tool_calls);

    PersistedTeamTimelineEvent {
        message_id: transformed.id,
        session_id: transformed.session_id,
        timestamp: transformed.timestamp,
        kind,
        role: transformed.role,
        agent_id: transformed.agent_id,
        content: transformed.content,
        thinking: transformed.thinking,
        tool_calls_json,
        content_blocks_json: transformed.content_blocks,
        channel_id: transformed.channel_id,
        input_tokens: transformed.input_tokens,
        output_tokens: transformed.output_tokens,
        total_tokens: transformed.total_tokens,
        duration_ms: transformed.duration_ms,
        priority: kind.priority(),
        metadata,
    }
}

fn postprocess_timeline(
    events: Vec<PersistedTeamTimelineEvent>,
) -> Vec<PersistedTeamTimelineEvent> {
    events
        .iter()
        .enumerate()
        .filter_map(|(idx, event)| {
            if should_hide_noise(event, idx, &events) {
                None
            } else {
                Some(event.clone())
            }
        })
        .collect()
}

fn should_hide_noise(
    event: &PersistedTeamTimelineEvent,
    idx: usize,
    events: &[PersistedTeamTimelineEvent],
) -> bool {
    if event.kind == TeamTimelineKind::UserPrompt {
        let parent_session_id = event
            .metadata
            .get("parent_session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let is_root_user = parent_session_id.is_empty() && event.role == "user";
        let is_direct_target = is_root_user && event.content.trim_start().starts_with('@');
        if is_direct_target {
            let has_descendant_user_delegate = events[idx + 1..].iter().any(|candidate| {
                candidate.kind == TeamTimelineKind::DelegationStarted
                    && candidate.agent_id.eq_ignore_ascii_case("user")
            });
            if has_descendant_user_delegate {
                return true;
            }
        }
        return false;
    }

    if event.kind != TeamTimelineKind::AgentResponse {
        return false;
    }

    let content = event.content.trim();
    let content_lower = content.to_ascii_lowercase();
    if matches!(content_lower.as_str(), "gemini" | "codex" | "claude") {
        return true;
    }

    let parent_session_id = event
        .metadata
        .get("parent_session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_descendant = !parent_session_id.is_empty();
    if !is_descendant {
        return false;
    }

    if content.contains("Error: MCP tool 'delegate' reported an error.") {
        let later_success = events[idx + 1..].iter().any(|candidate| {
            candidate.session_id == event.session_id
                && candidate.kind == TeamTimelineKind::AgentResponse
                && !candidate
                    .content
                    .contains("Error: MCP tool 'delegate' reported an error.")
        });
        if later_success {
            return true;
        }
    }

    let later_nested_delegate = events[idx + 1..].iter().any(|candidate| {
        candidate.session_id == event.session_id
            && matches!(
                candidate.kind,
                TeamTimelineKind::DelegationStarted
                    | TeamTimelineKind::TaskSubmitted
                    | TeamTimelineKind::TaskStatus
            )
    });
    let later_descendant_response = events[idx + 1..].iter().any(|candidate| {
        candidate.session_id == event.session_id
            && candidate.kind == TeamTimelineKind::AgentResponse
            && !candidate.content.trim().is_empty()
    });
    if later_nested_delegate && later_descendant_response {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn maps_delegation_start_kind() {
        let msg = test_message("system", "a2a:delegation_start", "leader");
        assert_eq!(kind_from_message(&msg), TeamTimelineKind::DelegationStarted);
    }

    #[test]
    fn maps_leader_final_kind() {
        let msg = test_message("assistant", "team", "leader");
        assert_eq!(kind_from_message(&msg), TeamTimelineKind::LeaderFinal);
    }

    fn timeline_event(
        kind: TeamTimelineKind,
        role: &str,
        agent_id: &str,
        content: &str,
        parent_session_id: &str,
    ) -> PersistedTeamTimelineEvent {
        PersistedTeamTimelineEvent {
            message_id: 1,
            session_id: if parent_session_id.is_empty() {
                "root".to_string()
            } else {
                "child".to_string()
            },
            timestamp: "2026-03-12T00:00:00Z".to_string(),
            kind,
            role: role.to_string(),
            agent_id: agent_id.to_string(),
            content: content.to_string(),
            thinking: String::new(),
            tool_calls_json: String::new(),
            content_blocks_json: String::new(),
            channel_id: kind.default_channel_id().to_string(),
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            duration_ms: 0,
            priority: kind.priority(),
            metadata: json!({
                "parent_session_id": parent_session_id,
            }),
        }
    }

    #[test]
    fn hides_root_direct_target_user_prompt_when_child_delegation_exists() {
        let root = timeline_event(
            TeamTimelineKind::UserPrompt,
            "user",
            "user",
            "@researcher check this",
            "",
        );
        let child = timeline_event(
            TeamTimelineKind::DelegationStarted,
            "system",
            "user",
            "🛰️ User → Researcher",
            "root",
        );
        let events = vec![root.clone(), child];
        assert!(should_hide_noise(&root, 0, &events));
    }
}
