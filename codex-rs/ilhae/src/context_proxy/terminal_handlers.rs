use std::collections::HashMap;
use std::sync::Arc;

use agent_client_protocol_schema::{
    CreateTerminalRequest, CreateTerminalResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse,
};
use sacp::{Agent, Conductor, ConnectionTo, Responder};
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

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
                async move |req: CreateTerminalRequest,
                            responder: Responder<CreateTerminalResponse>,
                            cx: ConnectionTo<Conductor>| {
                    handle_terminal_create(req, responder, cx, state.clone()).await
                }
            },
            sacp::on_receive_request!(),
        )
        .on_receive_request_from(
            Agent,
            {
                let state = state.clone();
                async move |req: TerminalOutputRequest,
                            responder: Responder<TerminalOutputResponse>,
                            cx: ConnectionTo<Conductor>| {
                    handle_terminal_output(req, responder, cx, state.clone()).await
                }
            },
            sacp::on_receive_request!(),
        )
        .on_receive_request_from(
            Agent,
            {
                let state = state.clone();
                async move |req: WaitForTerminalExitRequest,
                            responder: Responder<WaitForTerminalExitResponse>,
                            cx: ConnectionTo<Conductor>| {
                    handle_terminal_wait_for_exit(req, responder, cx, state.clone()).await
                }
            },
            sacp::on_receive_request!(),
        )
        .on_receive_request_from(
            Agent,
            {
                let state = state.clone();
                async move |req: ReleaseTerminalRequest,
                            responder: Responder<ReleaseTerminalResponse>,
                            cx: ConnectionTo<Conductor>| {
                    handle_terminal_release(req, responder, cx, state.clone()).await
                }
            },
            sacp::on_receive_request!(),
        )
}

/// Managed terminal state — tracks a spawned process and its output.
pub struct ManagedTerminal {
    pub child: Child,
    pub output: String,
    pub output_byte_limit: Option<usize>,
    pub exited: bool,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
}

/// TerminalManager — manages spawned terminal processes.
///
/// Thread-safe via Arc<Mutex<...>>. Lives on SharedState.
pub struct TerminalManager {
    pub terminals: Mutex<HashMap<String, ManagedTerminal>>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            terminals: Mutex::new(HashMap::new()),
        }
    }
}

/// Handle Agent → Client: terminal/create
pub async fn handle_terminal_create(
    req: CreateTerminalRequest,
    responder: Responder<CreateTerminalResponse>,
    _cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    // ── Tool Sandbox (Security Layer) ──
    // Prevent short-lived SubAgents from executing destructive OS commands
    let active_session = state.sessions.active_session_id.read().await.clone();
    if active_session.starts_with("subagent_") {
        warn!(
            "[Sandbox] SubAgent ({}) attempted to execute command: {}",
            active_session, req.command
        );
        return responder.respond_with_error(sacp::Error::new(
            -32001,
            format!("Sandbox Violation: SubAgents are read-only and cannot execute OS commands (blocked '{}')", req.command),
        ));
    }

    let terminal_id = format!(
        "term_{}",
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("x")
    );
    info!(
        "[terminal/create] id={} command={} args={:?} cwd={:?}",
        terminal_id, req.command, req.args, req.cwd
    );

    let mut cmd = tokio::process::Command::new(&req.command);

    if !req.args.is_empty() {
        cmd.args(&req.args);
    }
    if let Some(ref cwd) = req.cwd {
        cmd.current_dir(cwd);
    }
    if !req.env.is_empty() {
        for var in &req.env {
            cmd.env(&var.name, &var.value);
        }
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    match cmd.spawn() {
        Ok(child) => {
            let managed = ManagedTerminal {
                child,
                output: String::new(),
                output_byte_limit: req.output_byte_limit.map(|l| l as usize),
                exited: false,
                exit_code: None,
                signal: None,
            };

            state
                .infra
                .terminal_manager
                .terminals
                .lock()
                .await
                .insert(terminal_id.clone(), managed);

            let resp: CreateTerminalResponse =
                serde_json::from_value(json!({ "terminalId": terminal_id })).unwrap();
            responder.respond(resp)
        }
        Err(e) => {
            warn!("[terminal/create] Failed to spawn: {}", e);
            responder.respond_with_error(sacp::Error::new(
                -32603,
                format!("Failed to spawn command: {}", e),
            ))
        }
    }
}

/// Collect any available output from stdout/stderr into the terminal's buffer.
async fn collect_output(terminal: &mut ManagedTerminal) {
    // Read stdout
    if let Some(ref mut stdout) = terminal.child.stdout {
        let mut buf = vec![0u8; 8192];
        loop {
            match tokio::time::timeout(std::time::Duration::from_millis(10), stdout.read(&mut buf))
                .await
            {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    if let Ok(s) = String::from_utf8(buf[..n].to_vec()) {
                        terminal.output.push_str(&s);
                    }
                }
                _ => break,
            }
        }
    }

    // Read stderr (merge into output)
    if let Some(ref mut stderr) = terminal.child.stderr {
        let mut buf = vec![0u8; 8192];
        loop {
            match tokio::time::timeout(std::time::Duration::from_millis(10), stderr.read(&mut buf))
                .await
            {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    if let Ok(s) = String::from_utf8(buf[..n].to_vec()) {
                        terminal.output.push_str(&s);
                    }
                }
                _ => break,
            }
        }
    }

    // Apply byte limit (truncate from beginning)
    if let Some(limit) = terminal.output_byte_limit {
        if terminal.output.len() > limit {
            let excess = terminal.output.len() - limit;
            // Find next valid char boundary
            let mut boundary = excess;
            while !terminal.output.is_char_boundary(boundary) && boundary < terminal.output.len() {
                boundary += 1;
            }
            terminal.output = terminal.output[boundary..].to_string();
        }
    }
}

/// Handle Agent → Client: terminal/output
pub async fn handle_terminal_output(
    req: TerminalOutputRequest,
    responder: Responder<TerminalOutputResponse>,
    _cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    let tid = req.terminal_id.to_string();
    debug!("[terminal/output] terminalId={}", tid);

    let mut terminals = state.infra.terminal_manager.terminals.lock().await;
    if let Some(terminal) = terminals.get_mut(&tid) {
        collect_output(terminal).await;

        // Check if process has exited
        if !terminal.exited {
            if let Ok(Some(status)) = terminal.child.try_wait() {
                terminal.exited = true;
                terminal.exit_code = status.code();
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    terminal.signal = status.signal().map(|s| format!("SIG{}", s));
                }
            }
        }

        let truncated = terminal
            .output_byte_limit
            .map(|l| terminal.output.len() >= l)
            .unwrap_or(false);

        let exit_status = if terminal.exited {
            Some(json!({
                "exitCode": terminal.exit_code,
                "signal": terminal.signal,
            }))
        } else {
            None
        };

        let resp: TerminalOutputResponse = serde_json::from_value(json!({
            "output": terminal.output,
            "truncated": truncated,
            "exitStatus": exit_status,
        }))
        .unwrap();
        responder.respond(resp)
    } else {
        responder.respond_with_error(sacp::Error::new(
            -32602,
            format!("Terminal not found: {}", tid),
        ))
    }
}

/// Handle Agent → Client: terminal/wait_for_exit
pub async fn handle_terminal_wait_for_exit(
    req: WaitForTerminalExitRequest,
    responder: Responder<WaitForTerminalExitResponse>,
    _cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    let tid = req.terminal_id.to_string();
    debug!("[terminal/wait_for_exit] terminalId={}", tid);

    let mut terminals = state.infra.terminal_manager.terminals.lock().await;
    if let Some(terminal) = terminals.get_mut(&tid) {
        if terminal.exited {
            let resp: WaitForTerminalExitResponse = serde_json::from_value(json!({
                "exitCode": terminal.exit_code,
                "signal": terminal.signal,
            }))
            .unwrap();
            return responder.respond(resp);
        }

        let status = terminal.child.wait().await;
        collect_output(terminal).await;

        match status {
            Ok(s) => {
                terminal.exited = true;
                terminal.exit_code = s.code();
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    terminal.signal = s.signal().map(|sig| format!("SIG{}", sig));
                }
                let resp: WaitForTerminalExitResponse = serde_json::from_value(json!({
                    "exitCode": terminal.exit_code,
                    "signal": terminal.signal,
                }))
                .unwrap();
                responder.respond(resp)
            }
            Err(e) => responder
                .respond_with_error(sacp::Error::new(-32603, format!("Wait failed: {}", e))),
        }
    } else {
        responder.respond_with_error(sacp::Error::new(
            -32602,
            format!("Terminal not found: {}", tid),
        ))
    }
}

/// Handle Agent → Client: terminal/release
pub async fn handle_terminal_release(
    req: ReleaseTerminalRequest,
    responder: Responder<ReleaseTerminalResponse>,
    _cx: ConnectionTo<Conductor>,
    state: Arc<SharedState>,
) -> Result<(), sacp::Error> {
    let tid = req.terminal_id.to_string();
    info!("[terminal/release] terminalId={}", tid);

    let mut terminals = state.infra.terminal_manager.terminals.lock().await;
    if let Some(mut terminal) = terminals.remove(&tid) {
        if !terminal.exited {
            let _ = terminal.child.kill().await;
        }
    }
    // Always succeed (idempotent)
    let resp: ReleaseTerminalResponse = serde_json::from_value(json!({})).unwrap();
    responder.respond(resp)
}
