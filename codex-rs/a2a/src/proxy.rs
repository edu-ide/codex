//! A2A Proxy — generic observation and routing layer for A2A agents.
//!
//! Mirrors the SACP `Proxy` role pattern:
//! - intercepts agent-to-agent communication
//! - provides typed observation of `StreamEvent`s
//! - supports synchronous streaming, async subscription, and fire-and-forget

use crate::client::{A2AClient, ClientError, StreamEvent};
use crate::types::*;
use tracing::{debug, info, warn};

// ─── A2aProxy ────────────────────────────────────────────────────────────

/// A2A Proxy — wraps [`A2AClient`] with observation and routing logic.
///
/// Follows the same pattern as SACP's `Proxy` role:
/// - intercepts and observes agent-to-agent communication
/// - provides typed stream consumption
///
/// # Example
/// ```ignore
/// use a2a_rs::proxy::A2aProxy;
///
/// let proxy = A2aProxy::new("http://localhost:4322", "researcher");
///
/// // Synchronous streaming
/// let (text, events) = proxy.send_and_observe("analyze this", None).await?;
///
/// // Fire-and-forget
/// let task_id = proxy.fire_and_forget("background job", None).await?;
///
/// // Later: subscribe to that task
/// let events = proxy.subscribe_to_task(&task_id).await?;
/// ```
#[derive(Clone, Debug)]
pub struct A2aProxy {
    endpoint: String,
    role: String,
}

impl A2aProxy {
    /// Create a new proxy for the given agent endpoint and role name.
    pub fn new(endpoint: &str, role: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            role: role.to_lowercase(),
        }
    }

    /// The agent's endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The agent's role name (lowercase).
    pub fn role(&self) -> &str {
        &self.role
    }

    /// Create an [`A2AClient`] for this agent endpoint.
    pub fn client(&self) -> A2AClient {
        A2AClient::new(&self.endpoint)
    }

    /// Send a message and observe the streaming response (synchronous).
    ///
    /// Returns `(accumulated_text, events)`.
    pub async fn send_and_observe(
        &self,
        prompt: &str,
        context_id: Option<String>,
    ) -> Result<(String, Vec<StreamEvent>), ClientError> {
        let client = self.client();

        let message = Message {
            message_id: uuid::Uuid::new_v4().to_string(),
            context_id,
            task_id: None,
            role: Role::User,
            parts: vec![Part::text(prompt)],
            metadata: None,
            extensions: vec![],
            reference_task_ids: None,
        };

        let request = SendMessageRequest {
            message,
            configuration: None,
            metadata: None,
        };

        info!("[A2A-Proxy] {} send_and_observe", self.role);

        let mut rx = client.send_message_stream(request).await?;

        let mut events = Vec::new();
        let mut accumulated_text = String::new();

        while let Some(result) = rx.recv().await {
            match result {
                Ok(event) => {
                    let text = extract_text_from_stream_event(&event);
                    if !text.is_empty() {
                        accumulated_text.push_str(&text);
                    }
                    debug!("[A2A-Proxy] {} event: {:?}", self.role, event);
                    events.push(event);
                }
                Err(e) => {
                    warn!("[A2A-Proxy] {} stream error: {}", self.role, e);
                    return Err(e);
                }
            }
        }

        info!(
            "[A2A-Proxy] {} completed: {}B text, {} events",
            self.role,
            accumulated_text.len(),
            events.len()
        );
        Ok((accumulated_text, events))
    }

    /// Subscribe to an existing task's updates (async subscription).
    ///
    /// Uses the A2A `tasks/resubscribe` JSON-RPC method.
    pub async fn subscribe_to_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<StreamEvent>, ClientError> {
        let client = self.client();

        info!("[A2A-Proxy] {} subscribe_to_task: {}", self.role, task_id);

        let mut rx = client.resubscribe_task(task_id).await?;

        let mut events = Vec::new();
        while let Some(result) = rx.recv().await {
            match result {
                Ok(event) => events.push(event),
                Err(e) => {
                    debug!("[A2A-Proxy] {} resubscribe error: {}", self.role, e);
                    return Err(e);
                }
            }
        }

        Ok(events)
    }

    /// Send a message and get a receiver for streaming events.
    ///
    /// Unlike [`send_and_observe`], this returns the channel immediately
    /// so the caller can process events as they arrive.
    pub async fn send_streaming(
        &self,
        prompt: &str,
        context_id: Option<String>,
    ) -> Result<tokio::sync::mpsc::Receiver<Result<StreamEvent, ClientError>>, ClientError> {
        let client = self.client();

        let message = Message {
            message_id: uuid::Uuid::new_v4().to_string(),
            context_id,
            task_id: None,
            role: Role::User,
            parts: vec![Part::text(prompt)],
            metadata: None,
            extensions: vec![],
            reference_task_ids: None,
        };

        let request = SendMessageRequest {
            message,
            configuration: None,
            metadata: None,
        };

        info!("[A2A-Proxy] {} send_streaming", self.role);
        client.send_message_stream(request).await
    }

    /// Fire-and-forget: send a message without waiting for completion.
    ///
    /// Returns the task ID for later subscription via [`subscribe_to_task`].
    pub async fn fire_and_forget(
        &self,
        prompt: &str,
        context_id: Option<String>,
    ) -> Result<String, ClientError> {
        let client = self.client();

        let message = Message {
            message_id: uuid::Uuid::new_v4().to_string(),
            context_id,
            task_id: None,
            role: Role::User,
            parts: vec![Part::text(prompt)],
            metadata: None,
            extensions: vec![],
            reference_task_ids: None,
        };

        let request = SendMessageRequest {
            message,
            configuration: Some(SendMessageConfiguration {
                blocking: Some(false),
                ..Default::default()
            }),
            metadata: None,
        };

        info!("[A2A-Proxy] {} fire_and_forget", self.role);

        let response = client.send_message(request).await?;

        let task_id = match response {
            SendMessageResponse::Task(task) => task.id,
            SendMessageResponse::Message(msg) => {
                msg.task_id.unwrap_or_else(|| "unknown".to_string())
            }
        };

        info!("[A2A-Proxy] {} fire_and_forget task_id: {}", self.role, task_id);
        Ok(task_id)
    }

    /// Get the current state of a task.
    pub async fn get_task(&self, task_id: &str) -> Result<Task, ClientError> {
        self.client().get_task(task_id).await
    }

    /// Cancel a running task.
    pub async fn cancel_task(&self, task_id: &str) -> Result<Task, ClientError> {
        self.client().cancel_task(task_id).await
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────

/// Extract text content from a typed [`StreamEvent`].
pub fn extract_text_from_stream_event(event: &StreamEvent) -> String {
    match event {
        StreamEvent::StatusUpdate(su) => {
            if let Some(msg) = &su.status.message {
                extract_text_from_parts(&msg.parts)
            } else {
                String::new()
            }
        }
        StreamEvent::ArtifactUpdate(au) => extract_text_from_parts(&au.artifact.parts),
        StreamEvent::Task(task) => {
            if let Some(msg) = &task.status.message {
                extract_text_from_parts(&msg.parts)
            } else {
                String::new()
            }
        }
        StreamEvent::Message(msg) => extract_text_from_parts(&msg.parts),
    }
}

/// Extract text from A2A [`Part`]s.
pub fn extract_text_from_parts(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| p.text.as_deref())
        .collect::<Vec<_>>()
        .join("")
}

/// Check if a [`TaskState`] is terminal (no more updates expected).
pub fn is_terminal_state(state: &TaskState) -> bool {
    matches!(
        state,
        TaskState::Completed | TaskState::Failed | TaskState::Canceled | TaskState::Rejected
    )
}
