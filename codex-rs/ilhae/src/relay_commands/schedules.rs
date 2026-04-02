// commands

use crate::SharedState;
use serde_json::json;
pub async fn handle_task_list(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let schedules = ctx.infra.brain.schedule_list();
    let result = serde_json::to_value(&schedules).unwrap_or(json!([]));
    maybe_respond(cmd.request_id.as_deref(), result, None);
}

pub async fn handle_task_create(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let p = &cmd.payload;
    let title = p
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled");
    match ctx.infra.brain.schedule_create(
        title,
        p.get("description").and_then(|v| v.as_str()),
        p.get("schedule").and_then(|v| v.as_str()),
        p.get("category").and_then(|v| v.as_str()),
        p.get("days")
            .and_then(|v| serde_json::from_value::<Vec<u8>>(v.clone()).ok())
            .unwrap_or_default(),
        p.get("prompt").and_then(|v| v.as_str()),
        p.get("cron_expr").and_then(|v| v.as_str()),
        p.get("target_url").and_then(|v| v.as_str()),
        p.get("instructions").and_then(|v| v.as_str()),
        p.get("enabled").and_then(|v| v.as_bool()),
    ) {
        Ok(task) => {
            let result = serde_json::to_value(&task).unwrap_or_default();
            maybe_respond(cmd.request_id.as_deref(), result, None);
        }
        Err(e) => {
            maybe_respond(cmd.request_id.as_deref(), serde_json::Value::Null, Some(e));
        }
    }
}

pub async fn handle_task_update(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let p = &cmd.payload;
    let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("");
    match ctx.infra.brain.schedule_update_full(
        id,
        p.get("title").and_then(|v| v.as_str()),
        p.get("description").and_then(|v| v.as_str()),
        p.get("done").and_then(|v| v.as_bool()),
        p.get("status").and_then(|v| v.as_str()),
        p.get("schedule").and_then(|v| v.as_str()),
        p.get("category").and_then(|v| v.as_str()),
        p.get("days")
            .and_then(|v| serde_json::from_value::<Vec<u8>>(v.clone()).ok()),
        p.get("prompt").and_then(|v| v.as_str()),
        p.get("cron_expr").and_then(|v| v.as_str()),
        p.get("target_url").and_then(|v| v.as_str()),
        p.get("instructions").and_then(|v| v.as_str()),
        p.get("enabled").and_then(|v| v.as_bool()),
        p.get("assigned_agent").and_then(|v| v.as_str()),
        p.get("priority").and_then(|v| v.as_str()),
        p.get("retry_count")
            .and_then(|v| v.as_i64().map(|n| n as u32)),
        p.get("max_retries")
            .and_then(|v| v.as_i64().map(|n| n as u32)),
    ) {
        Ok(task) => {
            let result = serde_json::to_value(&task).unwrap_or_default();
            maybe_respond(cmd.request_id.as_deref(), result, None);
        }
        Err(e) => {
            maybe_respond(cmd.request_id.as_deref(), serde_json::Value::Null, Some(e));
        }
    }
}

pub async fn handle_task_delete(
    ctx: &SharedState,
    cmd: &crate::relay_server::RelayCommand,
    _client_id: u32,
    maybe_respond: impl Fn(Option<&str>, serde_json::Value, Option<String>),
) {
    let id = cmd.payload.get("id").and_then(|v| v.as_str()).unwrap_or("");
    match ctx.infra.brain.schedule_delete(id) {
        Ok(()) => {
            maybe_respond(cmd.request_id.as_deref(), json!({"ok": true}), None);
        }
        Err(e) => {
            maybe_respond(cmd.request_id.as_deref(), serde_json::Value::Null, Some(e));
        }
    }
}
