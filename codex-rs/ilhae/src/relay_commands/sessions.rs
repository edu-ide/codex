// commands

use crate::SharedState;
use crate::infer_agent_id_from_command;
use serde_json::json;
use uuid::Uuid;

fn payload_str<'a>(payload: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| payload.get(*key).and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn payload_i64(payload: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| payload.get(*key).and_then(|v| v.as_i64()))
}

pub async fn handle_session_list(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    match ctx.infra.brain.session_list(None) {
        Ok(sessions) => {
            let result = serde_json::to_value(&sessions).unwrap_or(json!([]));
            maybe_respond(cmd.request_id.as_deref(), result, None);
        }
        Err(e) => {
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some(e.to_string()),
            );
        }
    }
}

pub async fn handle_session_search(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let query = cmd
        .payload
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let limit = cmd
        .payload
        .get("limit")
        .and_then(|v| v.as_i64())
        .unwrap_or(50);
    match ctx.infra.brain.session_search(query, limit) {
        Ok(sessions) => {
            let result = serde_json::to_value(&sessions).unwrap_or(json!([]));
            maybe_respond(cmd.request_id.as_deref(), result, None);
        }
        Err(e) => {
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some(e.to_string()),
            );
        }
    }
}
pub async fn handle_session_load(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let session_id = payload_str(&cmd.payload, &["session_id", "sessionId"]).unwrap_or("");
    let limit = payload_i64(&cmd.payload, &["limit"]).and_then(|v| usize::try_from(v).ok());
    let before_id = payload_i64(&cmd.payload, &["before_id", "beforeId"]);
    match ctx
        .infra
        .brain
        .session_load_window(session_id, limit, before_id)
    {
        Ok(messages) => {
            let result = serde_json::to_value(&messages).unwrap_or(json!([]));
            maybe_respond(cmd.request_id.as_deref(), result, None);
        }
        Err(e) => {
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some(e.to_string()),
            );
        }
    }
}

pub async fn handle_session_timeline(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let session_id = payload_str(&cmd.payload, &["session_id", "sessionId"]).unwrap_or("");
    match crate::team_timeline::load_session_timeline(&ctx.infra.brain.sessions(), session_id) {
        Ok(events) => {
            let dto = events
                .into_iter()
                .map(|e| {
                    json!({
                        "id": e.message_id,
                        "message_id": e.message_id,
                        "session_id": e.session_id,
                        "timestamp": e.timestamp,
                        "kind": serde_json::to_value(e.kind).ok().and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_else(|| "system_notice".to_string()),
                        "role": e.role,
                        "agent_id": e.agent_id,
                        "content": e.content,
                        "thinking": e.thinking,
                        "tool_calls": e.tool_calls_json,
                        "content_blocks": e.content_blocks_json,
                        "channel_id": e.channel_id,
                        "input_tokens": e.input_tokens,
                        "output_tokens": e.output_tokens,
                        "total_tokens": e.total_tokens,
                        "duration_ms": e.duration_ms,
                        "priority": e.priority,
                        "metadata": e.metadata,
                    })
                })
                .collect::<Vec<_>>();
            let result = serde_json::to_value(dto).unwrap_or(json!([]));
            maybe_respond(cmd.request_id.as_deref(), result, None);
        }
        Err(e) => {
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some(e.to_string()),
            );
        }
    }
}

pub async fn handle_session_delete(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let session_id = payload_str(&cmd.payload, &["session_id", "sessionId"]).unwrap_or("");
    match ctx.infra.brain.session_delete(session_id) {
        Ok(()) => {
            maybe_respond(cmd.request_id.as_deref(), json!({"ok": true}), None);
        }
        Err(e) => {
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some(e.to_string()),
            );
        }
    }
}

pub async fn handle_session_create(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let payload = &cmd.payload;
    let cwd = payload_str(payload, &["cwd"]).unwrap_or("/");
    let title = payload_str(payload, &["title"]).unwrap_or("Untitled");

    let settings_snapshot = ctx.infra.settings_store.get();
    let inferred_agent_id = infer_agent_id_from_command(&settings_snapshot.agent.command);
    let agent_id = payload_str(payload, &["agent_id", "agentId"]).unwrap_or(&inferred_agent_id);

    let session_id = Uuid::new_v4().to_string();
    match ctx
        .infra
        .brain
        .session_create(&session_id, title, agent_id, cwd)
    {
        Ok(()) => match ctx.infra.brain.session_get(&session_id) {
            Ok(Some(session)) => {
                maybe_respond(cmd.request_id.as_deref(), session, None);
            }
            Ok(None) => {
                maybe_respond(
                    cmd.request_id.as_deref(),
                    serde_json::Value::Null,
                    Some("Session created but could not be loaded".to_string()),
                );
            }
            Err(e) => {
                maybe_respond(
                    cmd.request_id.as_deref(),
                    serde_json::Value::Null,
                    Some(e.to_string()),
                );
            }
        },
        Err(e) => {
            maybe_respond(
                cmd.request_id.as_deref(),
                serde_json::Value::Null,
                Some(e.to_string()),
            );
        }
    }
}
