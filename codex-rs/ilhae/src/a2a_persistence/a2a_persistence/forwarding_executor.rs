//! A2A Persistence Proxy — proper `AgentExecutor` chain pattern.
//!
//! Mirrors SACP's `PersistenceProxy` delegation pattern:
//! - `ForwardingExecutor` implements `AgentExecutor` by HTTP-forwarding to real agent
//! - `PersistenceScheduleStore` implements `ScheduleStore` with dual-write (InMemory + SessionStore)
//! - Each team role gets a proper `A2AServer<ForwardingExecutor, PersistenceScheduleStore>`
//!
//! ```text
//! Agent-A → A2AServer<ForwardingExecutor, PersistenceStore> → Agent-B
//!                      ↓ intercept (AgentExecutor.execute)
//!                    SessionStore DB
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use a2a_rs::client::{A2AClient, StreamEvent};
use a2a_rs::event::{EventBus, ExecutionEvent};
use a2a_rs::executor::{AgentExecutor, RequestContext};
use a2a_rs::proxy::extract_text_from_parts;
use a2a_rs::types::*;

use serde_json::{Value, json};
use tracing::{info, warn};

use crate::CxCache;
use crate::session_store::SessionStore;

// ═════════════════════════════════════════════════════════════════════
// ForwardingExecutor — AgentExecutor that forwards to real agent
// ═════════════════════════════════════════════════════════════════════

/// Shared cache: sub-agent role → last response text.
/// Sub-agent ForwardingExecutor writes here after execution;
/// Leader ForwardingExecutor reads when persisting delegation responses.
/// Backed by `<ilhae_dir>/delegation_cache.json` for crash recovery.
pub type DelegationResponseCache = Arc<std::sync::Mutex<HashMap<String, String>>>;

/// Write-through: update both in-memory cache and persistent file.
pub fn delegation_cache_write(
    cache: &DelegationResponseCache,
    role: &str,
    text: &str,
    ilhae_dir: &std::path::Path,
) {
    if let Ok(mut c) = cache.lock() {
        c.insert(role.to_string(), text.to_string());
        // Persist to file (best-effort, non-blocking)
        let path = delegation_cache_path(ilhae_dir);
        let snapshot = c.clone();
        std::thread::spawn(move || {
            let _ = std::fs::write(&path, serde_json::to_string(&snapshot).unwrap_or_default());
        });
    }
}

/// Read with file fallback: try in-memory first, then fall back to persistent file.
pub fn delegation_cache_read(
    cache: &DelegationResponseCache,
    role: &str,
    ilhae_dir: &std::path::Path,
) -> Option<String> {
    // 1. In-memory
    if let Ok(c) = cache.lock() {
        if let Some(val) = c.get(role) {
            return Some(val.clone());
        }
    }
    // 2. File fallback
    let path = delegation_cache_path(ilhae_dir);
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&content) {
            return map.get(role).cloned();
        }
    }
    None
}

fn delegation_cache_path(ilhae_dir: &std::path::Path) -> std::path::PathBuf {
    ilhae_dir.join("delegation_cache.json")
}

/// Implements `AgentExecutor` by forwarding requests to a real A2A agent
/// endpoint via HTTP, while persisting messages to the session database.
///
/// Decoupled from `SharedState` — takes only `SessionStore` and `CxCache`
/// for testability.
pub struct ForwardingExecutor {
    /// Real agent endpoint (e.g. `http://localhost:4322`)
    pub target_endpoint: String,
    /// Agent role name (e.g. "researcher")
    pub role: String,
    /// Whether this is the main (leader) agent
    pub is_main: bool,
    /// Session database for persistence
    pub store: Arc<SessionStore>,
    /// UI notification channel (optional — empty in tests)
    pub cx_cache: CxCache,
    /// Track delegation start times per (session_id, target_agent) for duration_ms calculation
    delegation_timers: std::sync::Mutex<HashMap<String, Instant>>,
    /// Shared delegation response cache — sub-agents write, leader reads
    pub delegation_cache: DelegationResponseCache,
    /// MCP server configurations to inject into outgoing A2A messages.
    /// Forwarded as `coderAgent.mcpServers` in message metadata so that
    /// gemini-cli's `mergeMcpServers()` can connect to SSE/HTTP MCP servers.
    pub mcp_servers: Option<Value>,
    /// Injected data directory (replaces crate::config:: calls)
    pub ilhae_dir: std::path::PathBuf,
}

impl ForwardingExecutor {
    pub fn new(
        target_endpoint: String,
        role: String,
        store: Arc<SessionStore>,
        cx_cache: CxCache,
    ) -> Self {
        Self::with_options(
            target_endpoint,
            role,
            false,
            store,
            cx_cache,
            Arc::new(std::sync::Mutex::new(HashMap::new())),
        )
    }

    pub fn with_main_flag(
        target_endpoint: String,
        role: String,
        is_main: bool,
        store: Arc<SessionStore>,
        cx_cache: CxCache,
    ) -> Self {
        Self::with_options(
            target_endpoint,
            role,
            is_main,
            store,
            cx_cache,
            Arc::new(std::sync::Mutex::new(HashMap::new())),
        )
    }

    pub fn with_options(
        target_endpoint: String,
        role: String,
        is_main: bool,
        store: Arc<SessionStore>,
        cx_cache: CxCache,
        delegation_cache: DelegationResponseCache,
    ) -> Self {
        let ilhae_dir = crate::config::resolve_ilhae_data_dir();
        Self {
            target_endpoint,
            role,
            is_main,
            store,
            cx_cache,
            delegation_timers: std::sync::Mutex::new(HashMap::new()),
            delegation_cache,
            mcp_servers: None,
            ilhae_dir,
        }
    }

    /// Set MCP server configurations to inject into outgoing A2A messages.
    pub fn with_mcp_servers(mut self, servers: Value) -> Self {
        self.mcp_servers = Some(servers);
        self
    }

    /// Persist a message to the session database.
    fn persist_message(&self, session_id: &str, role_str: &str, content: &str, agent_id: &str) {
        if content.is_empty() {
            return;
        }
        let content_blocks = serde_json::to_string(&vec![json!({"type": "text", "text": content})])
            .unwrap_or_default();
        let _ = self.store.add_full_message_with_blocks(
            session_id,
            role_str,
            content,
            agent_id,
            "",   // thinking
            "[]", // tool_calls
            &content_blocks,
            0,
            0,
            0,
            0,
        );
    }

    /// Persist a message with tool_calls to the session DB.
    fn persist_message_with_tools(
        &self,
        session_id: &str,
        role_str: &str,
        content: &str,
        agent_id: &str,
        tool_calls: &str,
    ) {
        if content.is_empty() && tool_calls.is_empty() {
            return;
        }
        let content_blocks = serde_json::to_string(&vec![json!({"type": "text", "text": content})])
            .unwrap_or_default();
        let tc = if tool_calls.is_empty() {
            "[]"
        } else {
            tool_calls
        };
        let _ = self.store.add_full_message_with_blocks(
            session_id,
            role_str,
            content,
            agent_id,
            "", // thinking
            tc,
            &content_blocks,
            0,
            0,
            0,
            0,
        );
    }

    /// Record a delegation start time for the given session+target.
    fn start_delegation_timer(&self, session_id: &str, target_agent: &str) {
        let key = format!("{}:{}", session_id, target_agent);
        if let Ok(mut timers) = self.delegation_timers.lock() {
            timers.insert(key, Instant::now());
        }
    }

    /// Consume and return elapsed ms since delegation_start for the given session+target.
    /// Returns 0 if no start time was recorded.
    fn take_delegation_duration_ms(&self, session_id: &str, target_agent: &str) -> i64 {
        let key = format!("{}:{}", session_id, target_agent);
        if let Ok(mut timers) = self.delegation_timers.lock() {
            if let Some(start) = timers.remove(&key) {
                return start.elapsed().as_millis() as i64;
            }
        }
        0
    }

    /// Persist a delegation event as a system message with channel_id.
    fn persist_delegation_event(
        &self,
        session_id: &str,
        channel_id: &str,
        target_agent: &str,
        content: &str,
        duration_ms: i64,
    ) {
        // All delegation events stored as assistant messages so they show with agent avatars
        // delegation_response → delegated agent's avatar
        // delegation_start/complete/failed → leader's avatar (agent_id set at call site)
        let role = "assistant";
        let content_blocks = serde_json::to_string(&vec![json!({"type": "text", "text": content})])
            .unwrap_or_default();
        let _ = self.store.add_full_message_with_blocks_channel(
            session_id,
            role,
            content,
            target_agent,
            "",   // thinking
            "[]", // tool_calls
            &content_blocks,
            channel_id,
            0,
            0,
            0,
            duration_ms,
        );
        info!(
            "[A2aProxy:{}] Persisted delegation event: channel={}, target={}, role={}",
            self.role, channel_id, target_agent, role
        );
    }

    /// Ensure sub-session exists for this agent role and return its session id.
    fn ensure_sub_session(&self, session_id: &str) -> String {
        let parent_cwd = self
            .store
            .get_session(session_id)
            .ok()
            .and_then(|s| s.map(|v| v.cwd))
            .filter(|cwd| !cwd.trim().is_empty())
            .unwrap_or_else(|| "/".to_string());
        self.store
            .ensure_team_sub_session(session_id, &self.role, &self.role, &parent_cwd)
            .unwrap_or_else(|_| session_id.to_string())
    }

    /// Send UI patch via relay_conductor_cx.
    async fn notify_ui(&self, method: &str, params: Value) {
        use sacp::{Client, Conductor, ConnectionTo, UntypedMessage};
        let maybe_cx: Option<ConnectionTo<Conductor>> =
            self.cx_cache.inner.read().await.last().cloned();
        if let Some(cx) = maybe_cx {
            if let Ok(notif) = UntypedMessage::new(method, params) {
                let _ = cx.send_notification_to(Client, notif);
            }
        }
    }

    /// Extract response text from a Task.
    /// Tries: status message → artifacts → history (agent messages).
    fn extract_task_text(task: &Task) -> String {
        // 1. Status message
        if let Some(msg) = &task.status.message {
            let text = extract_text_from_parts(&msg.parts);
            if !text.is_empty() {
                return text;
            }
        }
        // 2. Artifacts
        for artifact in &task.artifacts {
            let text = extract_text_from_parts(&artifact.parts);
            if !text.is_empty() {
                return text;
            }
        }
        // 3. History — last agent message
        for msg in task.history.iter().rev() {
            if matches!(msg.role, Role::Agent) {
                let text = extract_text_from_parts(&msg.parts);
                if !text.is_empty() {
                    return text;
                }
            }
        }
        String::new()
    }

    /// Inject artifact directory path into the request as a system text part.
    fn inject_artifact_dir(
        &self,
        request: &mut SendMessageRequest,
        artifact_dir: &Option<std::path::PathBuf>,
    ) {
        if let Some(dir) = artifact_dir {
            let inject_text = format!(
                "\n\n[System] Artifact Directory Path: {}\n{}",
                dir.display(),
                crate::ARTIFACT_INSTRUCTION
            );
            request.message.parts.push(Part {
                kind: Some("text".to_string()),
                text: Some(inject_text),
                raw: None,
                url: None,
                data: None,
                metadata: None,
                filename: None,
                media_type: None,
            });
            info!("[A2aProxy:{}] Injected artifact_dir: {:?}", self.role, dir);
        }
    }

    /// Inject artifact MCP server config into message metadata.
    ///
    /// Builds a stdio MCP config dynamically with session-specific env vars.
    /// gemini-cli's `mergeMcpServers()` handles stdio type natively.
    fn inject_mcp_metadata(&self, request: &mut SendMessageRequest, effective_session_id: &str) {
        let ilhae_dir = &self.ilhae_dir;

        // Resolve brain CLI binary (co-located target dirs → PATH fallback)
        let mcp_bin = {
            let mut found = String::from("brain");
            if let Ok(cwd) = std::env::current_dir() {
                let mut cur: Option<&std::path::Path> = Some(cwd.as_path());
                'outer: while let Some(dir) = cur {
                    for sub in [
                        "brain-cli/target/debug/brain",
                        "brain-cli/target/release/brain",
                        "target/debug/brain",
                        "target/release/brain",
                    ] {
                        let c = dir.join(sub);
                        if c.exists() {
                            found = c.to_string_lossy().to_string();
                            break 'outer;
                        }
                    }
                    cur = dir.parent();
                }
            }
            found
        };

        let mcp_servers = json!([{
            "type": "stdio",
            "name": "ilhae-tools",
            "command": mcp_bin,
            "args": ["mcp"],
            "env": [
                { "name": "ILHAE_DIR", "value": ilhae_dir.to_string_lossy() },
                { "name": "ILHAE_SESSION_ID", "value": effective_session_id },
            ]
        }]);

        let mut agent_settings = json!({
            "kind": "agent-settings",
            "workspacePath": std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            "mcpServers": mcp_servers,
        });
        // Merge any additional MCP servers from ForwardingExecutor config
        if let Some(ref extra) = self.mcp_servers {
            if let (Some(base), Some(extra_arr)) = (
                agent_settings["mcpServers"].as_array_mut(),
                extra.as_array(),
            ) {
                base.extend(extra_arr.iter().cloned());
            }
        }
        let metadata = request.message.metadata.get_or_insert_with(|| json!({}));
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert("coderAgent".to_string(), agent_settings);
        }
        info!(
            "[A2aProxy:{}] Injected stdio brain artifact MCP (session={})",
            self.role, effective_session_id
        );
    }

    /// Resolve the artifact directory for a given session.
    /// Returns the brain session directory where artifact files should be written.
    pub fn resolve_artifact_dir(&self, session_id: &str) -> Option<std::path::PathBuf> {
        let brain_dir = self.ilhae_dir.join("brain");

        // Get session info to determine path
        let session_info = self.store.get_session(session_id).ok().flatten()?;

        // Use BrainSessionWriter to compute the correct session directory
        let writer = brain_session_rs::brain_session_writer::BrainSessionWriter::new(&brain_dir);

        // Compute path — for team sessions this is a folder, for solo it's a file
        // We need the parent directory of the session file
        let session_path = writer.session_path(&session_info);
        let artifact_dir = if session_path.ends_with("index.md") {
            // Team session: artifact dir = the folder containing index.md
            session_path.parent()?.to_path_buf()
        } else {
            // Solo session: create a sibling artifacts folder
            let stem = session_path.file_stem()?.to_string_lossy().to_string();
            session_path.parent()?.join(format!("{}_artifacts", stem))
        };

        let _ = std::fs::create_dir_all(&artifact_dir);
        Some(artifact_dir)
    }
}

impl AgentExecutor for ForwardingExecutor {
    /// Execute by forwarding to the real agent via SSE streaming (`message/stream`).
    /// Each SSE event is immediately relayed to the EventBus for real-time fanout.
    async fn execute(
        &self,
        context: RequestContext,
        event_bus: &EventBus,
    ) -> Result<(), a2a_rs::error::A2AError> {
        println!(
            "🚀 [ForwardingExecutor::execute] role: {}, is_main: {}, target: {}",
            self.role, self.is_main, self.target_endpoint
        );
        let session_id = context.context_id.clone();
        let effective_session_id = if self.is_main {
            session_id.clone()
        } else {
            self.ensure_sub_session(&session_id)
        };
        let request_text = extract_text_from_parts(&context.request.message.parts);

        info!(
            "[A2aProxy:{}] execute: session={}, text={}B",
            self.role,
            session_id,
            request_text.len()
        );

        // ── Persist the incoming user request ──
        let from_role = match context.request.message.role {
            Role::User => "user",
            Role::Agent => "assistant",
        };
        // Only sub-agents persist user messages here.
        // Leader's user message is already persisted by the upstream (desktop/test).
        if !self.is_main {
            self.persist_message(&effective_session_id, from_role, &request_text, &self.role);
        }

        // ── SSE streaming to real agent ──
        let client = A2AClient::new(&self.target_endpoint);
        info!(
            "[A2aProxy:{}] streaming to {}",
            self.role, self.target_endpoint
        );

        // ── Inject per-session artifact directory path into request ──
        let mut request = context.request.clone();
        let artifact_dir = self.resolve_artifact_dir(&effective_session_id);
        self.inject_artifact_dir(&mut request, &artifact_dir);

        // ── Inject ilhae-tools stdio MCP server into message metadata ──
        self.inject_mcp_metadata(&mut request, &effective_session_id);

        // ── A2A Spec: Request extensions activation ──────────────────────
        // Per A2A spec, the client MUST list desired extensions in
        // message.extensions so the server activates them and sends
        // extension-specific metadata in status updates.
        let requested_extensions = vec![
            "urn:google:a2a:ext:tool-call-reporting".to_string(),
            "urn:ilhae:ext:file-output".to_string(),
            "urn:ilhae:ext:thought-stream".to_string(),
            "urn:ilhae:ext:progress".to_string(),
        ];
        request.message.extensions = requested_extensions;

        // ── A2A Spec: Push notification config ───────────────────────────
        // For async/fire-and-forget delegations, the server can POST
        // task completion events to this webhook instead of requiring
        // the client to keep an SSE connection open.
        let push_webhook_url = format!(
            "http://localhost:{}/a2a/push/{}",
            crate::port_config::health_port(),
            effective_session_id
        );
        let config = request
            .configuration
            .get_or_insert_with(|| a2a_rs::types::SendMessageConfiguration::default());
        config.push_notification_config = Some(a2a_rs::types::PushNotificationConfig {
            id: Some(format!("{}:{}", self.role, effective_session_id)),
            url: push_webhook_url,
            token: None,
            authentication: None,
        });

        let mut rx = client.send_message_stream(request).await.map_err(|e| {
            warn!("[A2aProxy:{}] stream connect error: {}", self.role, e);
            a2a_rs::error::A2AError::internal_error(format!(
                "Stream to {} failed: {}",
                self.role, e
            ))
        })?;

        // ── Relay SSE events → EventBus in real-time ──
        let mut accumulated_text = String::new();
        let _seen_tool_events = std::collections::HashSet::<String>::new();
        let mut last_task: Option<Task> = None;
        let mut event_count = 0u32;
        // A2A → ACP adapter: canonical proxy-side mapping owner.
        // This is the preferred place to normalize raw A2A status/artifact/metadata
        // into ACP-like tool calls, UI patches, and DB persistence structures.
        let mut acp_mapper =
            a2a_acp_adapter::AcpEventMapper::new(&["researcher", "verifier", "creator"]);

        while let Some(result) = rx.recv().await {
            event_count += 1;
            match result {
                Ok(event) => {
                    // Convert StreamEvent → ExecutionEvent and publish immediately
                    match &event {
                        StreamEvent::Task(task) => {
                            info!(
                                "[A2aProxy:{}] SSE task: id={}, state={:?}",
                                self.role, task.id, task.status.state
                            );
                            last_task = Some(task.clone());
                            // Extract text from task
                            let text = Self::extract_task_text(task);
                            if !text.is_empty() {
                                accumulated_text = text.clone();
                                if !self.is_main {
                                    delegation_cache_write(
                                        &self.delegation_cache,
                                        &self.role,
                                        &accumulated_text,
                                        &self.ilhae_dir,
                                    );
                                }
                            }
                            // Sub-agent: strip status.message to prevent text duplication
                            // through proxy SSE re-emission. Text is preserved in accumulated_text.
                            if self.is_main {
                                event_bus.publish(ExecutionEvent::Task(task.clone()));
                            } else {
                                let mut stripped = task.clone();
                                stripped.status.message = None;
                                event_bus.publish(ExecutionEvent::Task(stripped));
                            }

                            // ── Persist task to DB (schedules table) ──
                            let state_str = format!("{:?}", task.status.state).to_lowercase();
                            let preview = task
                                .status
                                .message
                                .as_ref()
                                .map(|m| extract_text_from_parts(&m.parts))
                                .unwrap_or_default();

                            // First event for this task → upsert; subsequent → update status
                            let description = if preview.is_empty() {
                                format!("Delegation → {}", self.role)
                            } else {
                                format!(
                                    "Delegation → {} → {}",
                                    self.role,
                                    &preview[..preview.len().min(100)]
                                )
                            };
                            let _ = self.store.upsert_task(
                                &task.id,
                                &session_id,
                                &self.role,
                                &description,
                            );
                            if state_str != "submitted" {
                                let result_text = if preview.is_empty() {
                                    None
                                } else {
                                    Some(preview.as_str())
                                };
                                let _ = self.store.update_task_status(
                                    &task.id,
                                    &state_str,
                                    result_text,
                                );
                            }

                            // ── Real-time push: A2A task update to UI ──
                            self.notify_ui(
                                "ilhae/a2a_task_update",
                                json!({
                                    "sessionId": session_id,
                                    "agentRole": self.role,
                                    "taskId": task.id,
                                    "state": state_str,
                                    "preview": preview.char_indices().nth(200).map_or(&preview[..], |(i, _)| &preview[..i]),
                                    "eventCount": event_count,
                                }),
                            ).await;
                        }
                        StreamEvent::Message(msg) => {
                            let text = extract_text_from_parts(&msg.parts);
                            info!("[A2aProxy:{}] SSE message: {}B", self.role, text.len());
                            if !text.is_empty() {
                                accumulated_text = text;
                                if !self.is_main {
                                    delegation_cache_write(
                                        &self.delegation_cache,
                                        &self.role,
                                        &accumulated_text,
                                        &self.ilhae_dir,
                                    );
                                }
                            }
                            // Sub-agent: strip message parts to prevent text duplication
                            if self.is_main {
                                event_bus.publish(ExecutionEvent::Message(msg.clone()));
                            } else {
                                let mut stripped = msg.clone();
                                stripped.parts = vec![];
                                event_bus.publish(ExecutionEvent::Message(stripped));
                            }
                        }
                        StreamEvent::StatusUpdate(su) => {
                            info!(
                                "[A2aProxy:{}] SSE status-update: {:?}",
                                self.role, su.status.state
                            );
                            // Extract text from status message
                            if let Some(msg) = &su.status.message {
                                let text = extract_text_from_parts(&msg.parts);
                                if !text.is_empty() {
                                    // Append: gemini-cli sends deltas, not full snapshots
                                    accumulated_text.push_str(&text);
                                    if !self.is_main {
                                        delegation_cache_write(
                                            &self.delegation_cache,
                                            &self.role,
                                            &accumulated_text,
                                            &self.ilhae_dir,
                                        );
                                    }
                                }
                            }

                            // ── Real-time push: status update to UI ──
                            let state_str = format!("{:?}", su.status.state).to_lowercase();
                            let preview = su
                                .status
                                .message
                                .as_ref()
                                .map(|m| extract_text_from_parts(&m.parts))
                                .unwrap_or_default();
                            self.notify_ui(
                                "ilhae/a2a_task_update",
                                json!({
                                    "sessionId": session_id,
                                    "agentRole": self.role,
                                    "taskId": su.task_id,
                                    "state": state_str,
                                    "preview": preview.char_indices().nth(200).map_or(&preview[..], |(i, _)| &preview[..i]),
                                    "eventCount": event_count,
                                }),
                            ).await;

                            // ── Detect tool calls via a2a-acp-adapter ──
                            // Process coderAgent metadata (gemini-cli specific extension)
                            if let Some(_meta) = &su.metadata {
                                let mapped_event = acp_mapper.map_status_update(su);
                                match mapped_event {
                                    a2a_acp_adapter::AcpMappedEvent::ToolCallUpdate {
                                        tool_call: _,
                                        fields,
                                    } => {
                                        // Non-delegation tool call (write_to_file, run_command, etc.)
                                        // Already accumulated in acp_mapper.tool_calls
                                        info!(
                                            "[A2aProxy:{}] ACP tool_call: {} ({})",
                                            self.role, fields.tool_name, fields.tool_status
                                        );

                                        // ── Artifact Versioning Interceptor ──
                                        // If gemini-cli uses write_file instead of artifact_save,
                                        // detect writes to the artifact directory and auto-version in DB.
                                        if fields.tool_status == "completed" {
                                            let is_write = matches!(
                                                fields.tool_name.as_str(),
                                                "write_file" | "write_to_file" | "create_file"
                                            );
                                            if is_write {
                                                if let Some(ref artifact_dir) = artifact_dir {
                                                    // Extract file path from rawInput or responseText
                                                    let file_path = fields
                                                        .raw_input
                                                        .as_ref()
                                                        .and_then(|v| {
                                                            v.get("file_path")
                                                                .or(v.get("path"))
                                                                .or(v.get("filePath"))
                                                        })
                                                        .and_then(|v| v.as_str())
                                                        .map(|s| std::path::PathBuf::from(s));

                                                    if let Some(ref fp) = file_path {
                                                        if fp.starts_with(artifact_dir) {
                                                            // Read the file content that was just written
                                                            if let Ok(content) =
                                                                std::fs::read_to_string(fp)
                                                            {
                                                                let filename = fp
                                                                    .file_name()
                                                                    .and_then(|n| n.to_str())
                                                                    .unwrap_or("artifact.md");
                                                                let artifact_type = match filename {
                                                                    "task.md" => "task",
                                                                    "implementation_plan.md" => {
                                                                        "plan"
                                                                    }
                                                                    "walkthrough.md" => {
                                                                        "walkthrough"
                                                                    }
                                                                    _ => "other",
                                                                };
                                                                match self
                                                                    .store
                                                                    .save_artifact_version(
                                                                        &session_id,
                                                                        filename,
                                                                        &content,
                                                                        "",
                                                                        artifact_type,
                                                                    ) {
                                                                    Ok(ver) => info!(
                                                                        "[A2aProxy:{}] Auto-versioned artifact {} v{} (intercepted write_file)",
                                                                        self.role, filename, ver
                                                                    ),
                                                                    Err(e) => warn!(
                                                                        "[A2aProxy:{}] Failed to auto-version artifact {}: {}",
                                                                        self.role, filename, e
                                                                    ),
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    a2a_acp_adapter::AcpMappedEvent::DelegationEvent { fields } => {
                                        // Delegation tool call — handle with existing delegation logic
                                        let tool_name = &fields.tool_name;
                                        let tool_status = &fields.tool_status;
                                        let tool_call_id = &fields.tool_call_id;
                                        let name_lower = tool_name.to_lowercase();
                                        let known_agents = ["researcher", "verifier", "creator"];
                                        if let Some(&target) =
                                            known_agents.iter().find(|&&a| name_lower.contains(a))
                                        {
                                            // A2A spec: get clean response from sub-agent's
                                            // ForwardingExecutor via shared DelegationResponseCache.
                                            let mut response_text = delegation_cache_read(
                                                &self.delegation_cache,
                                                target,
                                                &self.ilhae_dir,
                                            )
                                            .unwrap_or_default();

                                            if response_text.is_empty() {
                                                // Brief retry for sub-agent stream to finish
                                                for i in 0..5 {
                                                    tokio::time::sleep(
                                                        std::time::Duration::from_millis(200),
                                                    )
                                                    .await;
                                                    response_text = delegation_cache_read(
                                                        &self.delegation_cache,
                                                        target,
                                                        &self.ilhae_dir,
                                                    )
                                                    .unwrap_or_default();
                                                    println!(
                                                        "📖 [ForwardingExecutor::cache_read] retry: {}, target: {}, got: {:?}",
                                                        i,
                                                        target,
                                                        &response_text
                                                            [..response_text.len().min(50)]
                                                    );
                                                    if !response_text.is_empty() {
                                                        break;
                                                    }
                                                }
                                            }

                                            let response_text = response_text.trim();

                                            // Extract delegation mode from message data (rawInput.async)
                                            let delegation_mode = su
                                                .status
                                                .message
                                                .as_ref()
                                                .and_then(|msg| {
                                                    msg.parts.iter().find_map(|part| {
                                                        let data = part.data.as_ref()?;
                                                        let raw_input = data.get("rawInput")?;
                                                        let is_async = raw_input
                                                            .get("async")
                                                            .and_then(|v| v.as_bool())
                                                            .unwrap_or(false);
                                                        let is_fire_forget = raw_input
                                                            .get("fireAndForget")
                                                            .and_then(|v| v.as_bool())
                                                            .unwrap_or(false);
                                                        if is_fire_forget {
                                                            Some("fire & forget")
                                                        } else if is_async {
                                                            Some("async subscribe")
                                                        } else {
                                                            Some("sync")
                                                        }
                                                    })
                                                })
                                                .unwrap_or("sync");

                                            let (channel, msg_content) = match tool_status.as_str()
                                            {
                                                "started" => {
                                                    // Start delegation timer
                                                    self.start_delegation_timer(
                                                        &effective_session_id,
                                                        target,
                                                    );
                                                    (
                                                        "a2a:delegation_start",
                                                        format!(
                                                            "🛰️ Delegating to {} ({})",
                                                            target, delegation_mode
                                                        ),
                                                    )
                                                }
                                                "completed" => {
                                                    // Persist delegated agent's actual response as separate message
                                                    if !response_text.is_empty() {
                                                        info!(
                                                            "[A2aProxy:{}] Persisting delegation response from {} ({}B)",
                                                            self.role,
                                                            target,
                                                            response_text.len()
                                                        );
                                                        self.persist_delegation_event(
                                                            &effective_session_id,
                                                            "a2a:delegation_response",
                                                            target,
                                                            response_text,
                                                            0, // duration tracked on delegation_complete
                                                        );
                                                        let turn_id = if tool_call_id.is_empty() {
                                                            format!("delegation-response-{}", event_count)
                                                        } else {
                                                        format!("delegation-response-{}", tool_call_id)
                                                    };
                                                    self.notify_ui(
                                                        crate::types::NOTIF_APP_SESSION_EVENT,
                                                        json!({
                                                            "engine": target,
                                                            "event": {
                                                                "event": "message_delta",
                                                                "thread_id": effective_session_id,
                                                                "turn_id": turn_id,
                                                                "item_id": format!("{}:{}", turn_id, target),
                                                                "channel": "assistant",
                                                                "delta": response_text,
                                                            }
                                                        }),
                                                    ).await;
                                                    self.notify_ui(
                                                        crate::types::NOTIF_APP_SESSION_EVENT,
                                                        json!({
                                                            "engine": target,
                                                            "event": {
                                                                "event": "turn_completed",
                                                                "thread_id": effective_session_id,
                                                                "turn_id": turn_id,
                                                                "status": "completed",
                                                            }
                                                        }),
                                                    ).await;
                                                    }
                                                    // Clean up any prior delegation_failed for this target
                                                    // (gemini-cli may report intermediate "failed" before finally completing)
                                                    // delegation_failed is not persisted to DB, only logged.
                                                    (
                                                        "a2a:delegation_complete",
                                                        format!("✅ {} completed", target),
                                                    )
                                                }
                                                "failed" => {
                                                    // gemini-cli reports intermediate "failed" during retries.
                                                    // Don't persist to DB — only log and track duration.
                                                    info!(
                                                        "[A2aProxy:{}] Delegation to {} reported 'failed' (intermediate, not persisted)",
                                                        self.role, target
                                                    );
                                                    let _duration_ms = self
                                                        .take_delegation_duration_ms(
                                                            &effective_session_id,
                                                            target,
                                                        );
                                                    continue; // skip persist_delegation_event
                                                }
                                                _ => (
                                                    "a2a:delegation_update",
                                                    format!(
                                                        "🔄 {} status: {}",
                                                        target, tool_status
                                                    ),
                                                ),
                                            };
                                            // delegation_start/complete/failed are leader's actions → agent_id = self.role
                                            // delegation_response is the delegated agent's answer → agent_id = target (already persisted above)
                                            let duration_ms = match channel {
                                                "a2a:delegation_complete"
                                                | "a2a:delegation_failed" => self
                                                    .take_delegation_duration_ms(
                                                        &effective_session_id,
                                                        target,
                                                    ),
                                                _ => 0, // delegation_start has no duration
                                            };
                                            self.persist_delegation_event(
                                                &effective_session_id,
                                                channel,
                                                &self.role,
                                                &msg_content,
                                                duration_ms,
                                            );
                                        }
                                    }
                                    a2a_acp_adapter::AcpMappedEvent::TextUpdate(_)
                                    | a2a_acp_adapter::AcpMappedEvent::None => {}
                                }
                            }

                            // A2A spec: For sub-agents (not main/leader),
                            // strip status.message before publishing to EventBus.
                            // This prevents gemini-cli (the caller/Leader) from
                            // accumulating text from both StatusUpdate AND the
                            // final Task event, causing duplication ("ParisParis").
                            // The canonical response text lives only in the final
                            // Task.status.message.
                            // For main agent: keep messages intact for UI streaming.
                            if self.is_main {
                                event_bus.publish(ExecutionEvent::StatusUpdate(su.clone()));
                            } else {
                                let mut stripped = su.clone();
                                stripped.status.message = None;
                                event_bus.publish(ExecutionEvent::StatusUpdate(stripped));
                            }
                        }
                        StreamEvent::ArtifactUpdate(au) => {
                            info!("[A2aProxy:{}] SSE artifact-update", self.role);
                            let text = extract_text_from_parts(&au.artifact.parts);
                            if !text.is_empty() {
                                // Replace, not append: SSE sends full snapshots
                                accumulated_text = text;
                            }
                            // Sub-agent: strip artifact parts to prevent text duplication
                            if self.is_main {
                                event_bus.publish(ExecutionEvent::ArtifactUpdate(au.clone()));
                            } else {
                                let mut stripped = au.clone();
                                stripped.artifact.parts = vec![];
                                event_bus.publish(ExecutionEvent::ArtifactUpdate(stripped));
                            }
                        }
                    }

                    let turn_id = format!("a2a-stream-{}-{}", self.role, event_count);
                    self.notify_ui(
                        crate::types::NOTIF_APP_SESSION_EVENT,
                        json!({
                            "engine": self.role,
                            "event": {
                                "event": "message_delta",
                                "thread_id": session_id,
                                "turn_id": turn_id,
                                "item_id": format!("{}:{}", turn_id, self.role),
                                "channel": "assistant",
                                "delta": accumulated_text,
                            }
                        }),
                    )
                    .await;
                }
                Err(e) => {
                    warn!("[A2aProxy:{}] SSE error: {}", self.role, e);
                    break;
                }
            }
        }

        info!(
            "[A2aProxy:{}] stream ended: {} events, {}B text",
            self.role,
            event_count,
            accumulated_text.len()
        );

        // Sub-agents: cache their response for Leader to read
        if !self.is_main && !accumulated_text.is_empty() {
            delegation_cache_write(
                &self.delegation_cache,
                &self.role,
                &accumulated_text,
                &self.ilhae_dir,
            );
        }

        // ── Persist agent response ──
        // Both main and sub-agents persist their response with tool_calls.
        if !accumulated_text.is_empty() || !acp_mapper.tool_calls_json().is_empty() {
            self.persist_message_with_tools(
                &effective_session_id,
                "assistant",
                &accumulated_text,
                &self.role,
                &acp_mapper.tool_calls_json(),
            );
        }

        if !self.is_main && !accumulated_text.is_empty() {
            let turn_id = format!("a2a-final-{}-{}", self.role, event_count + 1);
            self.notify_ui(
                crate::types::NOTIF_APP_SESSION_EVENT,
                json!({
                    "engine": self.role,
                    "event": {
                        "event": "message_delta",
                        "thread_id": effective_session_id,
                        "turn_id": turn_id,
                        "item_id": format!("{}:{}", turn_id, self.role),
                        "channel": "assistant",
                        "delta": accumulated_text,
                    }
                }),
            )
            .await;
            self.notify_ui(
                crate::types::NOTIF_APP_SESSION_EVENT,
                json!({
                    "engine": self.role,
                    "event": {
                        "event": "turn_completed",
                        "thread_id": effective_session_id,
                        "turn_id": turn_id,
                        "status": "completed",
                    }
                }),
            )
            .await;
        }

        // Update task status in DB
        if let Some(task) = &last_task {
            let state_str = format!("{:?}", task.status.state).to_lowercase();
            let _ = self.store.update_task_status(
                &task.id,
                &state_str,
                if accumulated_text.is_empty() {
                    None
                } else {
                    Some(&accumulated_text)
                },
            );
        }

        Ok(())
    }

    /// Cancel by forwarding to real agent.
    async fn cancel(
        &self,
        schedule_id: &str,
        event_bus: &EventBus,
    ) -> Result<(), a2a_rs::error::A2AError> {
        info!("[A2aProxy:{}] cancel task: {}", self.role, schedule_id);
        let client = A2AClient::new(&self.target_endpoint);
        match client.cancel_task(schedule_id).await {
            Ok(task) => {
                event_bus.publish(ExecutionEvent::Task(task));
                Ok(())
            }
            Err(e) => Err(a2a_rs::error::A2AError::internal_error(format!(
                "Cancel forward failed: {}",
                e
            ))),
        }
    }

    /// Return the agent card (proxy card with correct URL).
    fn agent_card(&self, base_url: &str) -> AgentCard {
        AgentCard {
            name: self.role.clone(),
            description: format!("A2A proxy for {}", self.role),
            provider: Some(AgentProvider {
                organization: "ilhae".to_string(),
                url: base_url.to_string(),
            }),
            version: "1.0.0".to_string(),
            documentation_url: None,
            icon_url: None,
            supported_interfaces: vec![AgentInterface {
                url: format!("{}/", base_url.trim_end_matches('/')),
                protocol_binding: "JSONRPC".to_string(),
                protocol_version: "rc.1".to_string(),
                tenant: None,
            }],
            capabilities: AgentCapabilities {
                streaming: Some(true),
                push_notifications: Some(true),
                extended_agent_card: Some(false),
                extensions: vec![
                    a2a_rs::types::AgentExtension {
                        uri: "urn:google:a2a:ext:tool-call-reporting".to_string(),
                        description: Some("Reports tool call events (confirmation requests, status updates) as extension metadata".to_string()),
                        required: None,
                        params: None,
                    },
                    a2a_rs::types::AgentExtension {
                        uri: "urn:ilhae:ext:file-output".to_string(),
                        description: Some("Streams file creation and modification events during code generation".to_string()),
                        required: None,
                        params: None,
                    },
                    a2a_rs::types::AgentExtension {
                        uri: "urn:ilhae:ext:thought-stream".to_string(),
                        description: Some("Streams intermediate reasoning and thinking process".to_string()),
                        required: None,
                        params: None,
                    },
                    a2a_rs::types::AgentExtension {
                        uri: "urn:ilhae:ext:progress".to_string(),
                        description: Some("Reports task progress percentage (0-100)".to_string()),
                        required: None,
                        params: None,
                    },
                ],
            },
            default_input_modes: vec!["text".to_string()],
            default_output_modes: vec!["text".to_string()],
            skills: vec![],
        }
    }
}
