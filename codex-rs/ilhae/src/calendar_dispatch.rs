//! Dispatch durable calendar occurrences through the configured Browser MCP.

use brain_rs::schedule::BrowserBatchDispatch;
use brain_rs::schedule::BrowserBatchStatus;
use brain_rs::schedule::ScheduleStore;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::model::CallToolResult;
use rmcp::transport::TokioChildProcess;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct BrowserSnapshot {
    id: String,
    #[serde(rename = "runId")]
    run_id: String,
    #[serde(rename = "sourceBatchId")]
    source_batch_id: String,
    status: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    summary: String,
    error: Option<BrowserError>,
    #[serde(rename = "spaceSync")]
    space_sync: Option<Value>,
}

#[derive(Deserialize)]
struct BrowserError {
    code: String,
    message: String,
}

fn browser_command(config: &toml::Value) -> Result<Command, String> {
    let server = config
        .get("mcp_servers")
        .and_then(|servers| servers.get("browser"))
        .ok_or("Configure the canonical browser MCP entry before calendar dispatch")?;
    if server.get("enabled").and_then(toml::Value::as_bool) == Some(false) {
        return Err("The configured Browser MCP is disabled".into());
    }
    // The native stdio launcher discovers the active profile's authenticated
    // daemon/adapter; guessing a loopback port would select the wrong account.
    let executable = server
        .get("command")
        .and_then(toml::Value::as_str)
        .ok_or("Calendar dispatch requires a configured Browser stdio MCP transport")?;
    let mut command = Command::new(executable);
    if let Some(args) = server.get("args") {
        for argument in args.as_array().ok_or("Browser MCP args must be an array")? {
            command.arg(
                argument
                    .as_str()
                    .ok_or("Browser MCP arguments must be strings")?,
            );
        }
    }
    if let Some(env) = server.get("env") {
        for (name, value) in env.as_table().ok_or("Browser MCP env must be a table")? {
            command.env(
                name,
                value
                    .as_str()
                    .ok_or("Browser MCP environment must contain strings")?,
            );
        }
    }
    if let Some(cwd) = server.get("cwd") {
        command.current_dir(cwd.as_str().ok_or("Browser MCP cwd must be a string")?);
    }
    command.kill_on_drop(true).stderr(Stdio::null());
    Ok(command)
}

fn observation(
    mut dispatch: BrowserBatchDispatch,
    enabled: bool,
    result: CallToolResult,
) -> Result<BrowserBatchDispatch, String> {
    let structured = result
        .structured_content
        .ok_or("Browser MCP returned no structured result")?;
    if result.is_error == Some(true) {
        let error: BrowserError = serde_json::from_value(
            structured
                .get("error")
                .ok_or("Browser MCP returned an invalid error")?
                .clone(),
        )
        .map_err(|error| error.to_string())?;
        // Busy, unavailable and transport uncertainty keep the same durable UUID.
        let terminal_status = match error.code.as_str() {
            "stopped" if dispatch.status == BrowserBatchStatus::Dispatching => {
                Some(BrowserBatchStatus::Cancelled)
            }
            "run_not_found" if !enabled => Some(BrowserBatchStatus::Cancelled),
            "run_not_found" | "invalid_run" | "invalid_checkpoint" | "dispatch_conflict"
            | "invalid_definition" | "invalid_tool" => Some(BrowserBatchStatus::Failed),
            _ => None,
        };
        if let Some(status) = terminal_status {
            dispatch.status = status;
            dispatch.result_summary = Some(error.message.chars().take(1000).collect());
        }
        return Ok(dispatch);
    }
    let state: BrowserSnapshot =
        serde_json::from_value(structured).map_err(|error| error.to_string())?;
    if state.id != dispatch.browser_run_id
        || state.run_id != dispatch.browser_run_id
        || state.source_batch_id != dispatch.browser_batch_id
    {
        return Err("Browser MCP response does not match the claimed occurrence".into());
    }
    dispatch.status = match state.status.as_str() {
        "created" => BrowserBatchStatus::Dispatching,
        "running" => BrowserBatchStatus::Running,
        "waiting_agent" => BrowserBatchStatus::WaitingAgent,
        "paused" => BrowserBatchStatus::Paused,
        "needs_review" => BrowserBatchStatus::NeedsReview,
        "completed" => BrowserBatchStatus::Completed,
        "failed" => BrowserBatchStatus::Failed,
        "cancelled" => BrowserBatchStatus::Cancelled,
        status => return Err(format!("Unknown Browser batch status: {status}")),
    };
    dispatch.bridge_updated_at = Some(
        chrono::DateTime::parse_from_rfc3339(&state.updated_at)
            .map_err(|error| error.to_string())?
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    );
    let summary = if !state.summary.is_empty() {
        Some(state.summary)
    } else {
        state.error.map(|error| error.message)
    };
    dispatch.result_summary = summary.map(|text| text.chars().take(1000).collect());
    dispatch.space_sync_pending = state
        .space_sync
        .as_ref()
        .is_some_and(|sync| sync.get("retryable").and_then(Value::as_bool) == Some(true));
    Ok(dispatch)
}

/// Polling continues when Kairos is disabled so cancellations and Space delivery settle.
pub(crate) async fn poll(store: Arc<ScheduleStore>) {
    let pending = match store.calendar_browser_dispatches() {
        Ok(pending) if !pending.is_empty() => pending,
        Ok(_) => return,
        Err(error) => {
            tracing::warn!("Calendar Browser receipts: {error}");
            return;
        }
    };
    let command = tokio::task::spawn_blocking(|| {
        let path = crate::config::calendar_browser_mcp_config_path()?;
        let config: toml::Value = std::fs::read_to_string(path)
            .map_err(|error| error.to_string())?
            .parse()
            .map_err(|error: toml::de::Error| error.to_string())?;
        browser_command(&config)
    })
    .await;
    let command = match command {
        Ok(Ok(command)) => command,
        error => {
            tracing::warn!("Calendar Browser transport unavailable: {error:?}");
            return;
        }
    };
    dispatch_pending(&store, command, pending).await;
}

async fn dispatch_pending(
    store: &ScheduleStore,
    command: Command,
    pending: Vec<(BrowserBatchDispatch, bool)>,
) {
    let transport = match TokioChildProcess::new(command) {
        Ok(transport) => transport,
        Err(error) => {
            tracing::warn!("Calendar Browser launch: {error}");
            return;
        }
    };
    let client = match timeout(REQUEST_TIMEOUT, ().serve(transport)).await {
        Ok(Ok(client)) => client,
        error => {
            tracing::warn!("Calendar Browser initialize: {error:?}");
            return;
        }
    };
    for (dispatch, enabled) in pending {
        let (tool, arguments) = if !enabled && !dispatch.status.is_terminal() {
            ("batch_cancel", json!({"id": dispatch.browser_run_id}))
        } else if dispatch.status == BrowserBatchStatus::Dispatching {
            (
                "batch_schedule_start",
                json!({"id": dispatch.browser_batch_id, "run_id": dispatch.browser_run_id}),
            )
        } else {
            ("batch_status", json!({"id": dispatch.browser_run_id}))
        };
        let mut request = CallToolRequestParams::new(tool);
        request.arguments = arguments.as_object().cloned();
        match timeout(REQUEST_TIMEOUT, client.call_tool(request)).await {
            Ok(Ok(result)) => match observation(dispatch, enabled, result) {
                Ok(receipt) => {
                    if let Err(error) = store.calendar_record_browser_dispatch(receipt) {
                        tracing::warn!("Calendar Browser persist: {error}");
                    }
                }
                Err(error) => tracing::warn!("Calendar Browser result: {error}"),
            },
            error => {
                tracing::warn!("Calendar Browser {tool}: {error:?}");
                break;
            }
        }
    }
    let _ = timeout(Duration::from_secs(1), client.cancel()).await;
}

#[cfg(test)]
#[path = "calendar_dispatch_tests.rs"]
mod tests;
