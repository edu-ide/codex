//! A2A HTTP client — mirrors `a2a-js/src/client/`.
//!
//! Supports the JSON-RPC v0.3 transport (the primary wire format) as well as
//! the RC v1 REST endpoints exposed by [`crate::server::A2AServer`].
//!
//! # Example
//! ```ignore
//! use a2a_rs::client::A2AClient;
//! use a2a_rs::{Part, Role, Message, SendMessageRequest};
//!
//! let client = A2AClient::from_base_url("http://localhost:5000").await?;
//! let response = client.send_message(SendMessageRequest {
//!     message: Message {
//!         message_id: "msg-1".into(),
//!         context_id: Some("ctx-1".into()),
//!         task_id: None,
//!         role: Role::User,
//!         parts: vec![Part::text("Hello")],
//!         metadata: None,
//!         extensions: vec![],
//!         reference_task_ids: None,
//!     },
//!     configuration: None,
//!     metadata: None,
//! }).await?;
//! ```

use crate::error::A2AError;
use crate::types::*;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

// ============================================================
// JSON-RPC envelope types (v0.3)
// ============================================================

#[derive(Debug, Serialize)]
struct JsonRpcRequest<P: Serialize> {
    jsonrpc: &'static str,
    method: &'static str,
    params: P,
    id: u64,
}

#[derive(Debug, Deserialize)]
struct JsonRpcSuccessResponse<R> {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: serde_json::Value,
    result: R,
}

#[derive(Debug, Deserialize)]
struct JsonRpcErrorDetail {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcErrorResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: serde_json::Value,
    error: JsonRpcErrorDetail,
}

/// Helper enum for deserializing a JSON-RPC response that could be success or error.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JsonRpcResponse<R> {
    Ok(JsonRpcSuccessResponse<R>),
    Err(JsonRpcErrorResponse),
}

// ============================================================
// Stream event types
// ============================================================

/// Events yielded during `send_message_stream`.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Task status update.
    StatusUpdate(TaskStatusUpdateEvent),
    /// Task artifact update.
    ArtifactUpdate(TaskArtifactUpdateEvent),
    /// Complete task (terminal).
    Task(Task),
    /// Direct message response (terminal).
    Message(Message),
}

// ============================================================
// Client error
// ============================================================

/// Errors from the A2A client.
#[derive(Debug)]
pub enum ClientError {
    /// HTTP transport error.
    Http(reqwest::Error),
    /// JSON serialization/deserialization error.
    Json(serde_json::Error),
    /// JSON-RPC error returned by the server.
    Rpc {
        code: i32,
        message: String,
        data: Option<serde_json::Value>,
    },
    /// Agent card missing required fields.
    InvalidAgentCard(String),
    /// SSE stream error.
    Stream(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::Json(e) => write!(f, "JSON error: {e}"),
            Self::Rpc {
                code,
                message,
                data,
            } => {
                write!(f, "JSON-RPC error ({code}): {message}")?;
                if let Some(d) = data {
                    write!(f, " data={d}")?;
                }
                Ok(())
            }
            Self::InvalidAgentCard(msg) => write!(f, "Invalid agent card: {msg}"),
            Self::Stream(msg) => write!(f, "Stream error: {msg}"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<ClientError> for A2AError {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::Rpc {
                code,
                message,
                data,
            } => {
                let mut err = A2AError::new(code, message);
                err.data = data;
                err
            }
            other => A2AError::internal_error(other.to_string()),
        }
    }
}

// ============================================================
// A2AClient
// ============================================================

/// A2A client for communicating with A2A-compliant agents.
///
/// Supports both JSON-RPC v0.3 and RC v1 REST endpoints.
/// By default uses JSON-RPC v0.3 (the `POST /` endpoint).
pub struct A2AClient {
    http: HttpClient,
    /// JSON-RPC endpoint (e.g. `http://localhost:5000/`)
    endpoint: String,
    /// Cached agent card
    agent_card: Option<AgentCard>,
    /// Request ID counter
    next_id: AtomicU64,
}

impl A2AClient {
    /// Create a client from a known endpoint URL.
    ///
    /// The endpoint should be the JSON-RPC URL (typically the base URL ending
    /// with `/`).
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            http: HttpClient::new(),
            endpoint: endpoint.into(),
            agent_card: None,
            next_id: AtomicU64::new(1),
        }
    }

    /// Create a client with a custom `reqwest::Client`.
    pub fn with_http_client(endpoint: impl Into<String>, http: HttpClient) -> Self {
        Self {
            http,
            endpoint: endpoint.into(),
            agent_card: None,
            next_id: AtomicU64::new(1),
        }
    }

    /// Create a client by fetching the agent card from the base URL.
    ///
    /// Fetches `{base_url}/.well-known/agent-card.json` (v0.3) and extracts
    /// the service endpoint URL.
    pub async fn from_base_url(base_url: &str) -> Result<Self, ClientError> {
        let base = base_url.trim_end_matches('/');
        let card_url = format!("{base}/.well-known/agent-card.json");

        let http = HttpClient::new();
        let resp = http
            .get(&card_url)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(ClientError::InvalidAgentCard(format!(
                "Failed to fetch agent card from {card_url}: {}",
                resp.status()
            )));
        }

        let card: serde_json::Value = resp.json().await?;

        // v0.3 card has `url` field; RC v1 card has `supportedInterfaces[].url`
        let endpoint = card
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                card.get("supportedInterfaces")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|iface| iface.get("url"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .ok_or_else(|| {
                ClientError::InvalidAgentCard("Agent card missing 'url' field".to_string())
            })?;

        // Try to parse as typed AgentCard for caching
        let agent_card: Option<AgentCard> = serde_json::from_value(card).ok();

        Ok(Self {
            http,
            endpoint,
            agent_card,
            next_id: AtomicU64::new(1),
        })
    }

    /// Get the cached agent card (if available from `from_base_url`).
    pub fn agent_card(&self) -> Option<&AgentCard> {
        self.agent_card.as_ref()
    }

    /// Fetch the agent card from `/.well-known/agent-card.json` (v0.3 format).
    pub async fn fetch_agent_card(&self) -> Result<serde_json::Value, ClientError> {
        // Derive base URL from endpoint
        let base = self.endpoint.trim_end_matches('/');
        let card_url = format!("{base}/.well-known/agent-card.json");
        let resp = self
            .http
            .get(&card_url)
            .header("Accept", "application/json")
            .send()
            .await?;
        Ok(resp.json().await?)
    }

    // ────────────────────────────────────────────────────────────
    // Core API methods (JSON-RPC v0.3)
    // ────────────────────────────────────────────────────────────

    /// Send a message to the agent (blocking mode).
    ///
    /// Maps to JSON-RPC method `message/send`.
    pub async fn send_message(
        &self,
        request: SendMessageRequest,
    ) -> Result<SendMessageResponse, ClientError> {
        self.rpc_call("message/send", request).await
    }

    /// Send a message and receive streaming SSE events.
    ///
    /// Maps to JSON-RPC method `message/stream`.
    /// Returns a receiver that yields `StreamEvent` items.
    pub async fn send_message_stream(
        &self,
        request: SendMessageRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<Result<StreamEvent, ClientError>>, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let rpc_request = JsonRpcRequest {
            jsonrpc: "2.0",
            method: "message/stream",
            params: request,
            id,
        };

        let resp = self
            .http
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&rpc_request)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(self.parse_http_error("message/stream", status, &body));
        }

        let (tx, rx) = tokio::sync::mpsc::channel(64);

        // Spawn SSE reader task
        tokio::spawn(async move {
            let mut buffer = String::new();
            let bytes_stream = resp;

            // Read response body as text and parse SSE events
            let body = match bytes_stream.text().await {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx.send(Err(ClientError::Http(e))).await;
                    return;
                }
            };

            for line in body.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    buffer = data.to_string();
                } else if line.starts_with("data:") {
                    buffer = line[5..].to_string();
                } else if line.is_empty() && !buffer.is_empty() {
                    // End of SSE event — parse the accumulated data
                    let event = parse_sse_event_data(&buffer);
                    let is_terminal = matches!(
                        &event,
                        Ok(StreamEvent::Task(t))
                            if matches!(
                                t.status.state,
                                TaskState::Completed | TaskState::Failed | TaskState::Canceled
                            )
                    );
                    if tx.send(event).await.is_err() {
                        return;
                    }
                    if is_terminal {
                        return;
                    }
                    buffer.clear();
                } else if !line.is_empty() && !line.starts_with(':') {
                    // Continuation line
                    if !buffer.is_empty() {
                        buffer.push('\n');
                    }
                    buffer.push_str(line);
                }
            }

            // Handle trailing event without final newline
            if !buffer.is_empty() {
                let _ = tx.send(parse_sse_event_data(&buffer)).await;
            }
        });

        Ok(rx)
    }

    /// Get a task by ID.
    ///
    /// Maps to JSON-RPC method `tasks/get`.
    pub async fn get_task(&self, task_id: &str) -> Result<Task, ClientError> {
        #[derive(Serialize)]
        struct Params {
            id: String,
        }
        self.rpc_call("tasks/get", Params { id: task_id.to_string() })
            .await
    }

    /// Cancel a task by ID.
    ///
    /// Maps to JSON-RPC method `tasks/cancel`.
    pub async fn cancel_task(&self, task_id: &str) -> Result<Task, ClientError> {
        #[derive(Serialize)]
        struct Params {
            id: String,
        }
        self.rpc_call("tasks/cancel", Params { id: task_id.to_string() })
            .await
    }

    /// Resubscribe to task events (streaming).
    ///
    /// Maps to JSON-RPC method `tasks/resubscribe`.
    pub async fn resubscribe_task(
        &self,
        task_id: &str,
    ) -> Result<Task, ClientError> {
        #[derive(Serialize)]
        struct Params {
            id: String,
        }
        self.rpc_call("tasks/resubscribe", Params { id: task_id.to_string() })
            .await
    }

    // ────────────────────────────────────────────────────────────
    // Internal helpers
    // ────────────────────────────────────────────────────────────

    /// Make a JSON-RPC call and return the typed result.
    async fn rpc_call<P: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<R, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let rpc_request = JsonRpcRequest {
            jsonrpc: "2.0",
            method,
            params,
            id,
        };

        let resp = self
            .http
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(&rpc_request)
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;

        let rpc_resp: JsonRpcResponse<R> =
            serde_json::from_str(&body).map_err(|e| {
                if !status.is_success() {
                    self.parse_http_error(method, status, &body)
                } else {
                    ClientError::Json(e)
                }
            })?;

        match rpc_resp {
            JsonRpcResponse::Ok(ok) => Ok(ok.result),
            JsonRpcResponse::Err(err) => Err(ClientError::Rpc {
                code: err.error.code,
                message: err.error.message,
                data: err.error.data,
            }),
        }
    }

    fn parse_http_error(
        &self,
        method: &str,
        status: reqwest::StatusCode,
        body: &str,
    ) -> ClientError {
        // Try to parse as JSON-RPC error first
        if let Ok(rpc_err) = serde_json::from_str::<JsonRpcErrorResponse>(body) {
            return ClientError::Rpc {
                code: rpc_err.error.code,
                message: rpc_err.error.message,
                data: rpc_err.error.data,
            };
        }
        ClientError::Stream(format!(
            "HTTP error for {method}: {status}. Response: {body}"
        ))
    }
}

// ============================================================
// SSE parsing helper
// ============================================================

/// Parse a single SSE `data:` payload as a JSON-RPC wrapped stream event.
fn parse_sse_event_data(data: &str) -> Result<StreamEvent, ClientError> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Err(ClientError::Stream("Empty SSE event data".to_string()));
    }

    // First try to parse as JSON-RPC envelope (server wraps events in
    // `{ jsonrpc, id, result }` for the `message/stream` method).
    if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(trimmed) {
        // If it has a `result` field, unwrap the JSON-RPC envelope
        let inner = if envelope.get("result").is_some() {
            envelope
                .get("result")
                .cloned()
                .unwrap_or(envelope.clone())
        } else if envelope.get("error").is_some() {
            // JSON-RPC error in SSE stream
            if let Ok(err) = serde_json::from_value::<JsonRpcErrorDetail>(
                envelope.get("error").cloned().unwrap_or_default(),
            ) {
                return Err(ClientError::Rpc {
                    code: err.code,
                    message: err.message,
                    data: err.data,
                });
            }
            return Err(ClientError::Stream(format!(
                "SSE event contained error: {trimmed}"
            )));
        } else {
            // Bare event (RC v1 REST style)
            envelope
        };

        // Try to identify the event type from its shape
        // Task has `id`, `contextId`, `status`
        if inner.get("id").is_some()
            && inner.get("contextId").is_some()
            && inner.get("status").is_some()
            && inner.get("artifacts").is_some()
        {
            if let Ok(task) = serde_json::from_value::<Task>(inner.clone()) {
                return Ok(StreamEvent::Task(task));
            }
        }

        // TaskStatusUpdateEvent has `taskId`, `contextId`, `status`
        if inner.get("taskId").is_some() && inner.get("status").is_some() {
            // Could be StatusUpdate or ArtifactUpdate — check for `artifact`
            if inner.get("artifact").is_some() {
                if let Ok(art) = serde_json::from_value::<TaskArtifactUpdateEvent>(inner.clone()) {
                    return Ok(StreamEvent::ArtifactUpdate(art));
                }
            }
            if let Ok(su) = serde_json::from_value::<TaskStatusUpdateEvent>(inner.clone()) {
                return Ok(StreamEvent::StatusUpdate(su));
            }
        }

        // Message has `messageId`, `role`, `parts`
        if inner.get("messageId").is_some() && inner.get("role").is_some() {
            if let Ok(msg) = serde_json::from_value::<Message>(inner.clone()) {
                return Ok(StreamEvent::Message(msg));
            }
        }

        // Fallback: try each type in order
        if let Ok(task) = serde_json::from_value::<Task>(inner.clone()) {
            return Ok(StreamEvent::Task(task));
        }
        if let Ok(msg) = serde_json::from_value::<Message>(inner.clone()) {
            return Ok(StreamEvent::Message(msg));
        }
        if let Ok(su) = serde_json::from_value::<TaskStatusUpdateEvent>(inner.clone()) {
            return Ok(StreamEvent::StatusUpdate(su));
        }
        if let Ok(art) = serde_json::from_value::<TaskArtifactUpdateEvent>(inner) {
            return Ok(StreamEvent::ArtifactUpdate(art));
        }
    }

    Err(ClientError::Stream(format!(
        "Failed to parse SSE event: {trimmed}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sse_task_event() {
        let data = r#"{"jsonrpc":"2.0","id":1,"result":{"id":"task-1","contextId":"ctx-1","status":{"state":"completed","message":{"messageId":"m1","role":"agent","parts":[{"text":"done"}]}},"artifacts":[]}}"#;
        let event = parse_sse_event_data(data).unwrap();
        assert!(matches!(event, StreamEvent::Task(t) if t.id == "task-1"));
    }

    #[test]
    fn parse_sse_status_update() {
        let data = r#"{"jsonrpc":"2.0","id":1,"result":{"taskId":"t1","contextId":"c1","status":{"state":"working","message":{"messageId":"m1","role":"agent","parts":[{"text":"working"}]}}}}"#;
        let event = parse_sse_event_data(data).unwrap();
        assert!(matches!(event, StreamEvent::StatusUpdate(_)));
    }

    #[test]
    fn parse_sse_rpc_error() {
        let data = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32001,"message":"Task not found"}}"#;
        let event = parse_sse_event_data(data);
        assert!(event.is_err());
        if let Err(ClientError::Rpc { code, .. }) = event {
            assert_eq!(code, -32001);
        }
    }
}
