use std::sync::Arc;

use agent_client_protocol_schema::{
    ReadTextFileRequest, ReadTextFileResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use sacp::{Agent, Conductor, ConnectionTo, Responder};
use serde_json::json;
use tracing::{debug, warn};

use crate::SharedState;

pub fn bind_routes<H>(
    builder: sacp::Builder<sacp::Proxy, H>,
    state: Arc<SharedState>,
) -> sacp::Builder<sacp::Proxy, impl sacp::HandleDispatchFrom<sacp::Conductor>>
where
    H: sacp::HandleDispatchFrom<sacp::Conductor> + 'static,
{
    builder
        .on_receive_request_from(
            Agent,
            {
                let state = state.clone();
                async move |req: ReadTextFileRequest,
                            responder: Responder<ReadTextFileResponse>,
                            cx: ConnectionTo<Conductor>| {
                    handle_read_text_file(req, responder, cx, state.clone()).await
                }
            },
            sacp::on_receive_request!(),
        )
        .on_receive_request_from(
            Agent,
            {
                let state = state.clone();
                async move |req: WriteTextFileRequest,
                            responder: Responder<WriteTextFileResponse>,
                            cx: ConnectionTo<Conductor>| {
                    handle_write_text_file(req, responder, cx, state.clone()).await
                }
            },
            sacp::on_receive_request!(),
        )
}

/// Handle Agent → Client: fs/read_text_file
///
/// Reads a file from the local filesystem and returns its content.
/// Supports optional `line` (1-based start) and `limit` (max lines).
pub async fn handle_read_text_file(
    req: ReadTextFileRequest,
    responder: Responder<ReadTextFileResponse>,
    _cx: ConnectionTo<Conductor>,
    _state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    let path = req.path.display().to_string();
    debug!("[fs/read_text_file] path={}", path);

    match std::fs::read_to_string(&req.path) {
        Ok(full_content) => {
            // Apply line/limit filtering
            let content = if req.line.is_some() || req.limit.is_some() {
                let lines: Vec<&str> = full_content.lines().collect();
                let start = req
                    .line
                    .map(|l| (l as usize).saturating_sub(1))
                    .unwrap_or(0);
                let limit = req.limit.map(|l| l as usize).unwrap_or(lines.len());
                let end = (start + limit).min(lines.len());
                if start < lines.len() {
                    lines[start..end].join("\n") + "\n"
                } else {
                    String::new()
                }
            } else {
                full_content
            };

            let resp: ReadTextFileResponse = serde_json::from_value(json!({
                "content": content
            }))
            .unwrap();
            responder.respond(resp)
        }
        Err(e) => {
            warn!("[fs/read_text_file] Failed to read {}: {}", path, e);
            responder.respond_with_error(sacp::Error::new(
                -32603,
                format!("Failed to read file: {}", e),
            ))
        }
    }
}

/// Handle Agent → Client: fs/write_text_file
///
/// Writes content to a file on the local filesystem.
/// Creates parent directories if they don't exist.
pub async fn handle_write_text_file(
    req: WriteTextFileRequest,
    responder: Responder<WriteTextFileResponse>,
    cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    let path_display = req.path.display().to_string();
    debug!("[fs/write_text_file] path={}", path_display);

    // ── Tool Sandbox (Security Layer) ──
    // Prevent short-lived SubAgents from executing destructive file writes
    let active_session = state
        .sessions
        .connection_sessions
        .get(&crate::shared_state::connection_key(&cx))
        .unwrap_or_default();
    if active_session.starts_with("subagent_") {
        warn!(
            "[Sandbox] SubAgent ({}) attempted to write file: {}",
            active_session, path_display
        );
        return responder.respond_with_error(sacp::Error::new(
            -32001,
            format!("Sandbox Violation: SubAgents are read-only and cannot write to files (blocked '{}')", path_display),
        ));
    }

    // Ensure parent directory exists
    if let Some(parent) = req.path.parent() {
        if !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warn!("[fs/write_text_file] Failed to create parent dir: {}", e);
                return responder.respond_with_error(sacp::Error::new(
                    -32603,
                    format!("Failed to create directory: {}", e),
                ));
            }
        }
    }

    match std::fs::write(&req.path, &req.content) {
        Ok(()) => {
            let resp: WriteTextFileResponse = serde_json::from_value(json!({})).unwrap();
            responder.respond(resp)
        }
        Err(e) => {
            warn!(
                "[fs/write_text_file] Failed to write {}: {}",
                path_display, e
            );
            responder.respond_with_error(sacp::Error::new(
                -32603,
                format!("Failed to write file: {}", e),
            ))
        }
    }
}
