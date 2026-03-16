//! A2A RC v1 HTTP server using Axum.
//!
//! Mirrors `a2a-js/src/server/express/` and `a2a-js/src/server/request_handler/`.
//!
//! Uses [`AgentExecutor`] for agent logic, [`TaskStore`] for persistence,
//! and [`EventBus`] for streaming events.

use std::collections::HashMap;
use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::Arc;

use crate::error::A2AError;
use crate::event::{EventBus, ExecutionEvent};
use crate::executor::{AgentExecutor, RequestContext};
use crate::store::TaskStore;
use crate::types::*;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::sse::{Event, Sse},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

const HTTP_EXTENSION_HEADER: &str = "a2a-extensions";
const LEGACY_HTTP_EXTENSION_HEADER: &str = "x-a2a-extensions";

// ============================================================
// Server state
// ============================================================

/// Shared state for the A2A server.
pub struct A2AServerState<E: AgentExecutor, S: TaskStore> {
    pub executor: Arc<E>,
    pub store: Arc<S>,
    pub base_url: String,
    /// Active task cancellation tokens, keyed by task ID.
    pub cancel_tokens: Arc<Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>>>,
    /// In-memory task index for metadata listing endpoint.
    pub task_index: Arc<Mutex<HashSet<String>>>,
    /// Per-task push notification configurations.
    pub push_configs: Arc<Mutex<HashMap<String, Vec<PushNotificationConfig>>>>,
    /// Active execution event buses keyed by task ID for resubscription.
    pub event_buses: Arc<Mutex<HashMap<String, EventBus>>>,
    /// Global broadcaster for ACP/HTTP clients.
    pub acp_broadcaster: tokio::sync::broadcast::Sender<Value>,
}

impl<E: AgentExecutor, S: TaskStore> Clone for A2AServerState<E, S> {
    fn clone(&self) -> Self {
        Self {
            executor: Arc::clone(&self.executor),
            store: Arc::clone(&self.store),
            base_url: self.base_url.clone(),
            cancel_tokens: Arc::clone(&self.cancel_tokens),
            task_index: Arc::clone(&self.task_index),
            push_configs: Arc::clone(&self.push_configs),
            event_buses: Arc::clone(&self.event_buses),
            acp_broadcaster: self.acp_broadcaster.clone(),
        }
    }
}

// ============================================================
// A2AServer builder
// ============================================================

/// Builder for the A2A HTTP server.
pub struct A2AServer<E: AgentExecutor, S: TaskStore> {
    executor: Arc<E>,
    store: Arc<S>,
    addr: String,
    base_url: Option<String>,
    acp_broadcaster: Option<tokio::sync::broadcast::Sender<Value>>,
}

impl<E: AgentExecutor, S: TaskStore> A2AServer<E, S> {
    /// Create a new server with the given executor and store.
    pub fn new(executor: E, store: S) -> Self {
        Self {
            executor: Arc::new(executor),
            store: Arc::new(store),
            addr: "0.0.0.0:5000".to_string(),
            base_url: None,
            acp_broadcaster: None,
        }
    }

    /// Set an existing ACP broadcaster
    pub fn with_acp_broadcaster(mut self, tx: tokio::sync::broadcast::Sender<Value>) -> Self {
        self.acp_broadcaster = Some(tx);
        self
    }

    /// Set the bind address (default: `0.0.0.0:5000`).
    pub fn bind(mut self, addr: impl Into<String>) -> Self {
        self.addr = addr.into();
        self
    }

    /// Set the base URL for the agent card (default: derived from addr).
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    fn make_state(&self) -> A2AServerState<E, S> {
        let base_url = self
            .base_url
            .clone()
            .unwrap_or_else(|| format!("http://{}", self.addr));

        A2AServerState {
            executor: Arc::clone(&self.executor),
            store: Arc::clone(&self.store),
            base_url,
            cancel_tokens: Arc::new(Mutex::new(HashMap::new())),
            task_index: Arc::new(Mutex::new(HashSet::new())),
            push_configs: Arc::new(Mutex::new(HashMap::new())),
            event_buses: Arc::new(Mutex::new(HashMap::new())),
            acp_broadcaster: self.acp_broadcaster.clone().unwrap_or_else(|| tokio::sync::broadcast::channel(256).0),
        }
    }

    /// Build the Axum router without starting the server.
    pub fn router(&self) -> Router {
        let state = self.make_state();
        let a2a_routes = Self::a2a_routes();

        Router::new()
            .merge(a2a_routes.clone())
            .route("/acp", post(handle_acp_post::<E, S>))
            .route("/acp/stream", get(handle_acp_stream::<E, S>))
            // Multi-tenancy: all A2A routes under /{tenant}/
            .nest("/{tenant}", a2a_routes)
            .with_state(state)
    }

    /// Build the router WITHOUT /acp and /acp/stream routes.
    pub fn router_without_acp(&self) -> Router {
        let state = self.make_state();
        let a2a_routes = Self::a2a_routes();

        Router::new()
            .merge(a2a_routes.clone())
            .nest("/{tenant}", a2a_routes)
            .with_state(state)
    }

    /// Core A2A routes shared between tenant and non-tenant paths.
    fn a2a_routes() -> Router<A2AServerState<E, S>> {
        Router::new()
            .route("/", post(handle_jsonrpc::<E, S>))
            .route(
                "/.well-known/agent-card.json",
                get(handle_agent_card_v03::<E, S>),
            )
            .route("/.well-known/agent.json", get(handle_agent_card::<E, S>))
            .route("/message:send", post(handle_send_message_http::<E, S>))
            .route("/message:stream", post(handle_stream_message_http::<E, S>))
            .route("/tasks", get(handle_list_tasks::<E, S>))
            .route(
                "/tasks/{id}",
                get(handle_get_task::<E, S>).post(handle_cancel_task_compat::<E, S>),
            )
            .route("/tasks/metadata", get(handle_list_task_metadata::<E, S>))
            .route("/tasks/{id}/cancel", post(handle_cancel_task::<E, S>))
            .route(
                "/tasks/{id}:subscribe",
                get(handle_subscribe_to_task::<E, S>),
            )
            .route(
                "/extendedAgentCard",
                get(handle_get_extended_agent_card::<E, S>),
            )
    }

    /// Run the server (blocks until shutdown).
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let router = self.router();
        let listener = tokio::net::TcpListener::bind(&self.addr).await?;
        tracing::info!("A2A server listening on {}", self.addr);
        axum::serve(listener, router).await?;
        Ok(())
    }
}

// ============================================================
// Route handlers
// ============================================================

/// `GET /.well-known/agent.json`
async fn handle_agent_card<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
) -> Json<AgentCard> {
    Json(state.executor.agent_card(&state.base_url))
}

/// Compatibility endpoint for A2A 0.3 JSON-RPC stacks.
///
/// Returns a v0.3-style card shape at `/.well-known/agent-card.json`.
async fn handle_agent_card_v03<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
) -> Json<Value> {
    let card = state.executor.agent_card(&state.base_url);
    let url = card
        .supported_interfaces
        .first()
        .map(|iface| iface.url.clone())
        .unwrap_or_else(|| format!("{}/", state.base_url.trim_end_matches('/')));
    Json(json!({
        "name": card.name,
        "description": card.description,
        "url": url,
        "provider": card.provider,
        "version": card.version,
        "protocolVersion": "0.3.0",
        "capabilities": {
            "streaming": card.capabilities.streaming.unwrap_or(false),
            "pushNotifications": card.capabilities.push_notifications.unwrap_or(false),
            "stateTransitionHistory": false,
            "extensions": card.capabilities.extensions
        },
        "defaultInputModes": card.default_input_modes,
        "defaultOutputModes": card.default_output_modes,
        "skills": card.skills,
        "supportsAuthenticatedExtendedCard": card.capabilities.extended_agent_card.unwrap_or(false)
    }))
}

#[derive(Debug, Deserialize)]
struct JsonRpcEnvelope {
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Value,
    #[serde(default = "jsonrpc_null_id")]
    id: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonRpcTaskQueryParams {
    id: String,
    #[allow(dead_code)]
    history_length: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcTaskIdParams {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonRpcTaskPushNotificationConfigSetParams {
    task_id: String,
    push_notification_config: PushNotificationConfig,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JsonRpcTaskPushNotificationConfigGetParams {
    TaskId(JsonRpcTaskIdParams),
    Config(GetTaskPushNotificationConfigParams),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonRpcTaskPushNotificationConfigDeleteParams {
    id: String,
    push_notification_config_id: String,
}

fn jsonrpc_null_id() -> Value {
    Value::Null
}

fn jsonrpc_ok<T: serde::Serialize>(id: Value, result: T) -> Response {
    Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    }))
    .into_response()
}

fn jsonrpc_err(id: Value, err: A2AError, status: StatusCode) -> Response {
    (
        status,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": err.to_jsonrpc_error()
        })),
    )
        .into_response()
}

fn parse_requested_extensions(headers: &HeaderMap) -> Vec<String> {
    let mut requested = Vec::new();
    let mut seen = HashSet::new();

    for name in [HTTP_EXTENSION_HEADER, LEGACY_HTTP_EXTENSION_HEADER] {
        for value in headers.get_all(name) {
            let Ok(value) = value.to_str() else {
                continue;
            };
            for candidate in value.split(',') {
                let uri = candidate.trim();
                if uri.is_empty() {
                    continue;
                }
                if seen.insert(uri.to_string()) {
                    requested.push(uri.to_string());
                }
            }
        }
    }

    requested
}

fn resolve_activated_extensions<E: AgentExecutor, S: TaskStore>(
    state: &A2AServerState<E, S>,
    headers: &HeaderMap,
    fallback_request_extensions: Option<&[String]>,
) -> Vec<String> {
    let mut requested = parse_requested_extensions(headers);
    if requested.is_empty() {
        if let Some(fallback_extensions) = fallback_request_extensions {
            for extension in fallback_extensions {
                let uri = extension.trim();
                if !uri.is_empty() {
                    requested.push(uri.to_string());
                }
            }
        }
    }
    if requested.is_empty() {
        return Vec::new();
    }

    let supported: HashSet<String> = state
        .executor
        .agent_card(&state.base_url)
        .capabilities
        .extensions
        .into_iter()
        .map(|extension| extension.uri)
        .collect();
    if supported.is_empty() {
        return Vec::new();
    }

    let mut activated = Vec::new();
    let mut seen = HashSet::new();
    for extension in requested {
        if supported.contains(&extension) && seen.insert(extension.clone()) {
            activated.push(extension);
        }
    }
    activated
}

fn with_activated_extensions(mut response: Response, activated_extensions: &[String]) -> Response {
    if activated_extensions.is_empty() {
        return response;
    }

    let header_value = activated_extensions.join(",");
    let Ok(value) = HeaderValue::from_str(&header_value) else {
        return response;
    };
    response.headers_mut().insert(
        HeaderName::from_static(HTTP_EXTENSION_HEADER),
        value.clone(),
    );
    response.headers_mut().insert(
        HeaderName::from_static(LEGACY_HTTP_EXTENSION_HEADER),
        value,
    );
    response
}

fn jsonrpc_sse_single(
    id: Value,
    result: Value,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
    .to_string();
    Sse::new(stream::once(
        async move { Ok(Event::default().data(payload)) },
    ))
}

fn is_terminal_state(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Completed
            | TaskState::Failed
            | TaskState::Canceled
            | TaskState::Rejected
            | TaskState::InputRequired
    )
}

fn supports_push_notifications<E: AgentExecutor, S: TaskStore>(state: &A2AServerState<E, S>) -> bool {
    state
        .executor
        .agent_card(&state.base_url)
        .capabilities
        .push_notifications
        .unwrap_or(false)
}

fn event_to_stream_response_payload(event: &ExecutionEvent) -> Value {
    match event {
        ExecutionEvent::Task(task) => json!({ "task": task }),
        ExecutionEvent::Message(message) => json!({ "message": message }),
        ExecutionEvent::StatusUpdate(update) => json!({ "statusUpdate": update }),
        ExecutionEvent::ArtifactUpdate(update) => json!({ "artifactUpdate": update }),
    }
}

fn event_to_json_value(event: ExecutionEvent) -> Value {
    match event {
        ExecutionEvent::Task(task) => json!(task),
        ExecutionEvent::Message(msg) => json!(msg),
        ExecutionEvent::StatusUpdate(update) => json!(update),
        ExecutionEvent::ArtifactUpdate(update) => json!(update),
    }
}

async fn send_push_notification(
    client: &reqwest::Client,
    config: &PushNotificationConfig,
    event: &ExecutionEvent,
) -> Result<(), reqwest::Error> {
    let mut req = client
        .post(&config.url)
        .header("Content-Type", "application/json");
    if let Some(token) = &config.token {
        req = req.header("X-A2A-Notification-Token", token);
    }
    let payload = event_to_stream_response_payload(event);
    let _ = req.json(&payload).send().await?.error_for_status()?;
    Ok(())
}

fn spawn_push_notification_forwarder<E: AgentExecutor, S: TaskStore>(
    state: &A2AServerState<E, S>,
    task_id: &str,
    mut rx: tokio::sync::broadcast::Receiver<ExecutionEvent>,
) {
    let push_configs = Arc::clone(&state.push_configs);
    let task_id = task_id.to_string();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let configs = {
                        let guard = push_configs.lock().await;
                        guard.get(&task_id).cloned().unwrap_or_default()
                    };
                    for config in configs {
                        if let Err(err) = send_push_notification(&client, &config, &event).await {
                            tracing::warn!(
                                "failed to send push notification for task {} to {}: {}",
                                task_id,
                                config.url,
                                err
                            );
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

async fn upsert_push_config<E: AgentExecutor, S: TaskStore>(
    state: &A2AServerState<E, S>,
    params: TaskPushNotificationConfig,
) -> Result<TaskPushNotificationConfig, A2AError> {
    if !supports_push_notifications(state) {
        return Err(A2AError::push_notification_not_supported());
    }

    let task_id = params.task_id.clone();
    if state.store.load(&task_id).await?.is_none() {
        return Err(A2AError::task_not_found(&task_id));
    }

    let mut config = params.push_notification_config.clone();
    if config.id.as_ref().map_or(true, |id| id.is_empty()) {
        config.id = Some(task_id.clone());
    }
    let config_id = config.id.clone().unwrap_or_else(|| task_id.clone());

    {
        let mut guard = state.push_configs.lock().await;
        let entries = guard.entry(task_id.clone()).or_default();
        if let Some(existing_idx) = entries
            .iter()
            .position(|existing| existing.id.as_deref() == Some(config_id.as_str()))
        {
            entries[existing_idx] = config.clone();
        } else {
            entries.push(config.clone());
        }
    }

    Ok(TaskPushNotificationConfig {
        task_id,
        push_notification_config: config,
    })
}

async fn get_push_config<E: AgentExecutor, S: TaskStore>(
    state: &A2AServerState<E, S>,
    params: GetTaskPushNotificationConfigParams,
) -> Result<TaskPushNotificationConfig, A2AError> {
    if !supports_push_notifications(state) {
        return Err(A2AError::push_notification_not_supported());
    }

    let task_id = params.id.clone();
    if state.store.load(&task_id).await?.is_none() {
        return Err(A2AError::task_not_found(&task_id));
    }

    let config_id = params
        .push_notification_config_id
        .unwrap_or_else(|| task_id.clone());
    let maybe = {
        let guard = state.push_configs.lock().await;
        guard
            .get(&task_id)
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.id.as_deref() == Some(config_id.as_str()))
            })
            .cloned()
    };

    match maybe {
        Some(push_notification_config) => Ok(TaskPushNotificationConfig {
            task_id,
            push_notification_config,
        }),
        None => Err(A2AError::internal_error(format!(
            "Push notification config with id '{config_id}' not found for task {task_id}"
        ))),
    }
}

async fn list_push_configs<E: AgentExecutor, S: TaskStore>(
    state: &A2AServerState<E, S>,
    task_id: String,
) -> Result<Vec<TaskPushNotificationConfig>, A2AError> {
    if !supports_push_notifications(state) {
        return Err(A2AError::push_notification_not_supported());
    }
    if state.store.load(&task_id).await?.is_none() {
        return Err(A2AError::task_not_found(&task_id));
    }

    let configs = {
        let guard = state.push_configs.lock().await;
        guard.get(&task_id).cloned().unwrap_or_default()
    };
    Ok(configs
        .into_iter()
        .map(|push_notification_config| TaskPushNotificationConfig {
            task_id: task_id.clone(),
            push_notification_config,
        })
        .collect())
}

async fn delete_push_config<E: AgentExecutor, S: TaskStore>(
    state: &A2AServerState<E, S>,
    task_id: String,
    push_notification_config_id: String,
) -> Result<(), A2AError> {
    if !supports_push_notifications(state) {
        return Err(A2AError::push_notification_not_supported());
    }
    if state.store.load(&task_id).await?.is_none() {
        return Err(A2AError::task_not_found(&task_id));
    }

    let mut guard = state.push_configs.lock().await;
    if let Some(entries) = guard.get_mut(&task_id) {
        entries.retain(|entry| {
            entry.id.as_deref() != Some(push_notification_config_id.as_str())
        });
        if entries.is_empty() {
            guard.remove(&task_id);
        }
    }
    Ok(())
}

/// `POST /` JSON-RPC compatibility endpoint for A2A 0.3 clients.
async fn handle_jsonrpc<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let fallback_id = body.get("id").cloned().unwrap_or(Value::Null);
    let envelope: JsonRpcEnvelope = match serde_json::from_value(body) {
        Ok(envelope) => envelope,
        Err(e) => {
            return jsonrpc_err(
                fallback_id,
                A2AError::parse_error(format!("Failed to parse JSON-RPC request: {e}")),
                StatusCode::BAD_REQUEST,
            );
        }
    };

    if envelope.jsonrpc != "2.0" {
        return jsonrpc_err(
            envelope.id,
            A2AError::invalid_request("jsonrpc must be '2.0'"),
            StatusCode::BAD_REQUEST,
        );
    }

    match envelope.method.as_str() {
        // v1.0 PascalCase method aliases
        "SendMessage" | "message/send" => {
            let params: SendMessageRequest = match serde_json::from_value(envelope.params) {
                Ok(v) => v,
                Err(e) => {
                    return jsonrpc_err(
                        envelope.id,
                        A2AError::invalid_params(format!("Invalid message/send params: {e}")),
                        StatusCode::BAD_REQUEST,
                    );
                }
            };
            let activated_extensions =
                resolve_activated_extensions(&state, &headers, Some(&params.message.extensions));
            match handle_send_message::<E, S>(State(state), Json(params)).await {
                Ok(Json(result)) => {
                    with_activated_extensions(jsonrpc_ok(envelope.id, result), &activated_extensions)
                }
                Err(err) => with_activated_extensions(
                    jsonrpc_err(envelope.id, err, StatusCode::OK),
                    &activated_extensions,
                ),
            }
        }
        "GetTask" | "tasks/get" => {
            let params: JsonRpcTaskQueryParams = match serde_json::from_value(envelope.params) {
                Ok(v) => v,
                Err(e) => {
                    return jsonrpc_err(
                        envelope.id,
                        A2AError::invalid_params(format!("Invalid tasks/get params: {e}")),
                        StatusCode::BAD_REQUEST,
                    );
                }
            };
            match handle_get_task::<E, S>(State(state), Path(params.id)).await {
                Ok(Json(task)) => jsonrpc_ok(envelope.id, task),
                Err(err) => jsonrpc_err(envelope.id, err, StatusCode::OK),
            }
        }
        "CancelTask" | "tasks/cancel" => {
            let params: JsonRpcTaskIdParams = match serde_json::from_value(envelope.params) {
                Ok(v) => v,
                Err(e) => {
                    return jsonrpc_err(
                        envelope.id,
                        A2AError::invalid_params(format!("Invalid tasks/cancel params: {e}")),
                        StatusCode::BAD_REQUEST,
                    );
                }
            };
            match handle_cancel_task::<E, S>(State(state), Path(params.id)).await {
                Ok(Json(task)) => jsonrpc_ok(envelope.id, task),
                Err(err) => jsonrpc_err(envelope.id, err, StatusCode::OK),
            }
        }
        "SendStreamingMessage" | "message/stream" => {
            let params: SendMessageRequest = match serde_json::from_value(envelope.params) {
                Ok(v) => v,
                Err(e) => {
                    return jsonrpc_err(
                        envelope.id,
                        A2AError::invalid_params(format!("Invalid message/stream params: {e}")),
                        StatusCode::BAD_REQUEST,
                    );
                }
            };
            let activated_extensions =
                resolve_activated_extensions(&state, &headers, Some(&params.message.extensions));
            with_activated_extensions(
                handle_jsonrpc_stream_message::<E, S>(state, params, envelope.id)
                    .await
                    .into_response(),
                &activated_extensions,
            )
        }
        "SubscribeToTask" | "tasks/resubscribe" => {
            let params: JsonRpcTaskIdParams = match serde_json::from_value(envelope.params) {
                Ok(v) => v,
                Err(e) => {
                    return jsonrpc_err(
                        envelope.id,
                        A2AError::invalid_params(format!("Invalid tasks/resubscribe params: {e}")),
                        StatusCode::BAD_REQUEST,
                    );
                }
            };
            let request_id = envelope.id.clone();
            match handle_jsonrpc_resubscribe_task::<E, S>(state, params.id, request_id.clone()).await {
                Ok(response) => response,
                Err(err) => jsonrpc_err(request_id, err, StatusCode::OK),
            }
        }
        "CreateTaskPushNotificationConfig" | "tasks/pushNotificationConfig/set" => {
            let params: JsonRpcTaskPushNotificationConfigSetParams =
                match serde_json::from_value(envelope.params) {
                    Ok(v) => v,
                    Err(e) => {
                        return jsonrpc_err(
                            envelope.id,
                            A2AError::invalid_params(format!(
                                "Invalid tasks/pushNotificationConfig/set params: {e}"
                            )),
                            StatusCode::BAD_REQUEST,
                        );
                    }
                };
            let typed = TaskPushNotificationConfig {
                task_id: params.task_id,
                push_notification_config: params.push_notification_config,
            };
            match upsert_push_config(&state, typed).await {
                Ok(config) => jsonrpc_ok(envelope.id, config),
                Err(err) => jsonrpc_err(envelope.id, err, StatusCode::OK),
            }
        }
        "GetTaskPushNotificationConfig" | "tasks/pushNotificationConfig/get" => {
            let params: JsonRpcTaskPushNotificationConfigGetParams =
                match serde_json::from_value(envelope.params) {
                    Ok(v) => v,
                    Err(e) => {
                        return jsonrpc_err(
                            envelope.id,
                            A2AError::invalid_params(format!(
                                "Invalid tasks/pushNotificationConfig/get params: {e}"
                            )),
                            StatusCode::BAD_REQUEST,
                        );
                    }
                };
            let typed = match params {
                JsonRpcTaskPushNotificationConfigGetParams::TaskId(task) => {
                    GetTaskPushNotificationConfigParams {
                        id: task.id,
                        push_notification_config_id: None,
                        metadata: None,
                    }
                }
                JsonRpcTaskPushNotificationConfigGetParams::Config(config) => config,
            };
            match get_push_config(&state, typed).await {
                Ok(config) => jsonrpc_ok(envelope.id, config),
                Err(err) => jsonrpc_err(envelope.id, err, StatusCode::OK),
            }
        }
        "ListTaskPushNotificationConfigs" | "tasks/pushNotificationConfig/list" => {
            let params: ListTaskPushNotificationConfigParams =
                match serde_json::from_value(envelope.params) {
                    Ok(v) => v,
                    Err(e) => {
                        return jsonrpc_err(
                            envelope.id,
                            A2AError::invalid_params(format!(
                                "Invalid tasks/pushNotificationConfig/list params: {e}"
                            )),
                            StatusCode::BAD_REQUEST,
                        );
                    }
                };
            match list_push_configs(&state, params.id).await {
                Ok(configs) => jsonrpc_ok(envelope.id, configs),
                Err(err) => jsonrpc_err(envelope.id, err, StatusCode::OK),
            }
        }
        "DeleteTaskPushNotificationConfig" | "tasks/pushNotificationConfig/delete" => {
            let params: JsonRpcTaskPushNotificationConfigDeleteParams =
                match serde_json::from_value(envelope.params) {
                    Ok(v) => v,
                    Err(e) => {
                        return jsonrpc_err(
                            envelope.id,
                            A2AError::invalid_params(format!(
                                "Invalid tasks/pushNotificationConfig/delete params: {e}"
                            )),
                            StatusCode::BAD_REQUEST,
                        );
                    }
                };
            match delete_push_config(&state, params.id, params.push_notification_config_id).await {
                Ok(()) => jsonrpc_ok(envelope.id, Value::Null),
                Err(err) => jsonrpc_err(envelope.id, err, StatusCode::OK),
            }
        }
        "GetExtendedAgentCard" | "agent/getAuthenticatedExtendedCard" => {
            let card = state.executor.agent_card(&state.base_url);
            if card.capabilities.extended_agent_card.unwrap_or(false) {
                jsonrpc_ok(envelope.id, card)
            } else {
                jsonrpc_err(
                    envelope.id,
                    A2AError::unsupported_operation("agent/getAuthenticatedExtendedCard"),
                    StatusCode::NOT_IMPLEMENTED,
                )
            }
        }
        method => jsonrpc_err(
            envelope.id,
            A2AError::method_not_found(method),
            StatusCode::NOT_FOUND,
        ),
    }
}

/// `POST /message:send` with extension negotiation headers.
async fn handle_send_message_http<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
    headers: HeaderMap,
    Json(request): Json<SendMessageRequest>,
) -> Result<Response, A2AError> {
    let activated_extensions =
        resolve_activated_extensions(&state, &headers, Some(&request.message.extensions));
    let Json(result) = handle_send_message::<E, S>(State(state), Json(request)).await?;
    Ok(with_activated_extensions(
        Json(result).into_response(),
        &activated_extensions,
    ))
}

/// `POST /message:send`
async fn handle_send_message<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
    Json(request): Json<SendMessageRequest>,
) -> Result<Json<SendMessageResponse>, A2AError> {
    let context_id = request
        .message
        .context_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let task_id = request
        .message
        .task_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let blocking = request
        .configuration
        .as_ref()
        .and_then(|cfg| cfg.blocking)
        .unwrap_or(true);
    let incoming_message = request.message.clone();
    let request_metadata = request.metadata.clone();
    let event_bus = EventBus::new(64);
    let mut rx = event_bus.subscribe();
    let push_rx = event_bus.subscribe();

    let context = RequestContext {
        request,
        task_id: Some(task_id.clone()),
        context_id: context_id.clone(),
    };

    // Save inline push notification config if provided.
    if let Some(config) = context
        .request
        .configuration
        .as_ref()
        .and_then(|cfg| cfg.push_notification_config.clone())
    {
        let _ = upsert_push_config(
            &state,
            TaskPushNotificationConfig {
                task_id: task_id.clone(),
                push_notification_config: config,
            },
        )
        .await;
    }

    // Register cancel token
    let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
    state
        .cancel_tokens
        .lock()
        .await
        .insert(task_id.clone(), cancel_tx);
    state
        .event_buses
        .lock()
        .await
        .insert(task_id.clone(), event_bus.clone_sender());

    if supports_push_notifications(&state) {
        spawn_push_notification_forwarder(&state, &task_id, push_rx);
    }

    // Execute in background.
    let executor = Arc::clone(&state.executor);
    let cancel_tokens = Arc::clone(&state.cancel_tokens);
    let event_buses = Arc::clone(&state.event_buses);
    let task_id_clone = task_id.clone();
    let task_id_for_cancel = task_id.clone();
    tokio::spawn(async move {
        if let Err(e) = executor.execute(context, &event_bus).await {
            tracing::error!("AgentExecutor error: {e}");
        }
        // Cleanup cancel token
        cancel_tokens.lock().await.remove(&task_id_clone);
        event_buses.lock().await.remove(&task_id_for_cancel);
    });

    // Non-blocking mode: return immediately with a submitted task,
    // while persisting terminal updates in the background.
    if !blocking {
        let now = now_iso8601();
        let submitted_task = Task {
            id: task_id.clone(),
            context_id: context_id.clone(),
            status: TaskStatus {
                state: TaskState::Submitted,
                message: None,
                timestamp: Some(now.clone()),
            },
            artifacts: vec![],
            history: vec![incoming_message],
            metadata: request_metadata,
            created_at: Some(now.clone()),
            last_modified: Some(now),
        };
        state.store.save(submitted_task.clone()).await?;
        state.task_index.lock().await.insert(submitted_task.id.clone());

        // Persist task index for ListTasks
        let store = Arc::clone(&state.store);
        let task_index = Arc::clone(&state.task_index);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ExecutionEvent::Task(task)) => {
                        if let Err(err) = store.save(task.clone()).await {
                            tracing::warn!("failed to persist task {}: {}", task.id, err);
                        } else {
                            task_index.lock().await.insert(task.id.clone());
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        return Ok(Json(SendMessageResponse::Task(submitted_task)));
    }

    // Non-streaming mode still may receive intermediate streaming events first.
    // Keep consuming until a terminal Task or Message is received.
    loop {
        match rx.recv().await {
            Ok(ExecutionEvent::Task(task)) => {
                // Save to store.
                state.store.save(task.clone()).await?;
                state.task_index.lock().await.insert(task.id.clone());
                return Ok(Json(SendMessageResponse::Task(task)));
            }
            Ok(ExecutionEvent::Message(message)) => {
                return Ok(Json(SendMessageResponse::Message(message)));
            }
            Ok(ExecutionEvent::StatusUpdate(_) | ExecutionEvent::ArtifactUpdate(_)) => {
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return Err(A2AError::internal_error(
                    "Executor finished without terminal event",
                ));
            }
        }
    }
}

/// `POST /message:stream` with extension negotiation headers.
async fn handle_stream_message_http<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
    headers: HeaderMap,
    Json(request): Json<SendMessageRequest>,
) -> Response {
    let activated_extensions =
        resolve_activated_extensions(&state, &headers, Some(&request.message.extensions));
    let sse = handle_stream_message::<E, S>(State(state), Json(request)).await;
    with_activated_extensions(sse.into_response(), &activated_extensions)
}

/// `POST /message:stream` — SSE streaming of task events.
async fn handle_stream_message<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
    Json(request): Json<SendMessageRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let context_id = request
        .message
        .context_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let task_id = request
        .message
        .task_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let event_bus = EventBus::new(64);
    let rx = event_bus.subscribe();
    let push_rx = event_bus.subscribe();

    let context = RequestContext {
        request,
        task_id: Some(task_id.clone()),
        context_id,
    };

    if let Some(config) = context
        .request
        .configuration
        .as_ref()
        .and_then(|cfg| cfg.push_notification_config.clone())
    {
        let _ = upsert_push_config(
            &state,
            TaskPushNotificationConfig {
                task_id: task_id.clone(),
                push_notification_config: config,
            },
        )
        .await;
    }

    // Register cancel token
    let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
    state
        .cancel_tokens
        .lock()
        .await
        .insert(task_id.clone(), cancel_tx);
    state
        .event_buses
        .lock()
        .await
        .insert(task_id.clone(), event_bus.clone_sender());

    if supports_push_notifications(&state) {
        spawn_push_notification_forwarder(&state, &task_id, push_rx);
    }

    // Execute in background.
    let executor = Arc::clone(&state.executor);
    let _store = Arc::clone(&state.store);
    let cancel_tokens = Arc::clone(&state.cancel_tokens);
    let event_buses = Arc::clone(&state.event_buses);
    let task_id_clone = task_id.clone();
    let task_id_for_events = task_id.clone();
    tokio::spawn(async move {
        if let Err(e) = executor.execute(context, &event_bus).await {
            tracing::error!("AgentExecutor error: {e}");
        }
        cancel_tokens.lock().await.remove(&task_id_clone);
        event_buses.lock().await.remove(&task_id_for_events);
    });

    // Convert broadcast receiver into SSE stream.
    let store = Arc::clone(&state.store);
    let task_index = Arc::clone(&state.task_index);
    let stream = BroadcastStream::new(rx).filter_map(move |result| {
        let store = Arc::clone(&store);
        let task_index = Arc::clone(&task_index);
        match result {
            Ok(event) => {
                if let ExecutionEvent::Task(task) = &event {
                    let store = Arc::clone(&store);
                    let task_index = Arc::clone(&task_index);
                    let task_for_save = task.clone();
                    tokio::spawn(async move {
                        if let Err(e) = store.save(task_for_save.clone()).await {
                            tracing::warn!(
                                "failed to persist task {} from stream: {e}",
                                task_for_save.id
                            );
                        } else {
                            task_index.lock().await.insert(task_for_save.id.clone());
                        }
                    });
                }
                match &event {
                    ExecutionEvent::Task(task) => {
                        Event::default().event("task").json_data(task).ok().map(Ok)
                    }
                    ExecutionEvent::Message(msg) => Event::default()
                        .event("message")
                        .json_data(msg)
                        .ok()
                        .map(Ok),
                    ExecutionEvent::StatusUpdate(update) => Event::default()
                        .event("status")
                        .json_data(update)
                        .ok()
                        .map(Ok),
                    ExecutionEvent::ArtifactUpdate(update) => Event::default()
                        .event("artifact")
                        .json_data(update)
                        .ok()
                        .map(Ok),
                }
            }
            Err(_) => None, // Stream ended
        }
    });

    Sse::new(stream)
}

/// JSON-RPC variant of `message/stream` that wraps each SSE chunk into
/// `{ jsonrpc, id, result }`.
async fn handle_jsonrpc_stream_message<E: AgentExecutor, S: TaskStore>(
    state: A2AServerState<E, S>,
    request: SendMessageRequest,
    request_id: Value,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let context_id = request
        .message
        .context_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let task_id = request
        .message
        .task_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let event_bus = EventBus::new(64);
    let rx = event_bus.subscribe();
    let push_rx = event_bus.subscribe();

    let context = RequestContext {
        request,
        task_id: Some(task_id.clone()),
        context_id,
    };

    if let Some(config) = context
        .request
        .configuration
        .as_ref()
        .and_then(|cfg| cfg.push_notification_config.clone())
    {
        let _ = upsert_push_config(
            &state,
            TaskPushNotificationConfig {
                task_id: task_id.clone(),
                push_notification_config: config,
            },
        )
        .await;
    }

    // Register cancel token
    let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
    state
        .cancel_tokens
        .lock()
        .await
        .insert(task_id.clone(), cancel_tx);
    state
        .event_buses
        .lock()
        .await
        .insert(task_id.clone(), event_bus.clone_sender());

    if supports_push_notifications(&state) {
        spawn_push_notification_forwarder(&state, &task_id, push_rx);
    }

    // Execute in background.
    let executor = Arc::clone(&state.executor);
    let cancel_tokens = Arc::clone(&state.cancel_tokens);
    let event_buses = Arc::clone(&state.event_buses);
    let task_id_clone = task_id.clone();
    let task_id_for_events = task_id.clone();
    tokio::spawn(async move {
        if let Err(e) = executor.execute(context, &event_bus).await {
            tracing::error!("AgentExecutor error: {e}");
        }
        cancel_tokens.lock().await.remove(&task_id_clone);
        event_buses.lock().await.remove(&task_id_for_events);
    });

    let store = Arc::clone(&state.store);
    let task_index = Arc::clone(&state.task_index);
    let stream = BroadcastStream::new(rx).filter_map(move |result| {
        let request_id = request_id.clone();
        let store = Arc::clone(&store);
        let task_index = Arc::clone(&task_index);
        match result {
            Ok(event) => {
                if let ExecutionEvent::Task(task) = &event {
                    let store = Arc::clone(&store);
                    let task_index = Arc::clone(&task_index);
                    let task_for_save = task.clone();
                    tokio::spawn(async move {
                        if let Err(e) = store.save(task_for_save.clone()).await {
                            tracing::warn!(
                                "failed to persist task {} from jsonrpc stream: {e}",
                                task_for_save.id
                            );
                        } else {
                            task_index.lock().await.insert(task_for_save.id.clone());
                        }
                    });
                }
                let result_value = event_to_json_value(event);
                let payload = json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": result_value
                })
                .to_string();
                Some(Ok(Event::default().data(payload)))
            }
            Err(_) => None,
        }
    });

    Sse::new(stream)
}

async fn handle_jsonrpc_resubscribe_task<E: AgentExecutor, S: TaskStore>(
    state: A2AServerState<E, S>,
    task_id: String,
    request_id: Value,
) -> Result<Response, A2AError> {
    let task = state
        .store
        .load(&task_id)
        .await?
        .ok_or_else(|| A2AError::task_not_found(&task_id))?;

    let first_payload = json!({
        "jsonrpc": "2.0",
        "id": request_id.clone(),
        "result": task
    })
    .to_string();
    let initial = stream::once(async move {
        Ok::<Event, Infallible>(Event::default().data(first_payload))
    });

    if is_terminal_state(task.status.state) {
        return Ok(Sse::new(initial).into_response());
    }

    let maybe_bus = {
        let guard = state.event_buses.lock().await;
        guard.get(&task_id).map(|bus| bus.clone_sender())
    };

    if let Some(event_bus) = maybe_bus {
        let stream_request_id = request_id.clone();
        let stream = BroadcastStream::new(event_bus.subscribe()).filter_map(move |result| {
            let request_id = stream_request_id.clone();
            match result {
                Ok(event) => {
                    let payload = json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": event_to_json_value(event),
                    })
                    .to_string();
                    Some(Ok(Event::default().data(payload)))
                }
                Err(_) => None,
            }
        });
        Ok(Sse::new(initial.chain(stream)).into_response())
    } else {
        Ok(Sse::new(initial).into_response())
    }
}

/// `POST /acp`
async fn handle_acp_post<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
    Json(body): Json<Value>,
) -> Response {
    match state.executor.acp_call(body).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(e.to_jsonrpc_error())).into_response(),
    }
}

/// `GET /acp/stream`
async fn handle_acp_stream<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.acp_broadcaster.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|res| match res {
        Ok(val) => {
            let payload = val.to_string();
            Some(Ok(Event::default().data(payload)))
        }
        Err(_) => None,
    });
    Sse::new(stream)
}

/// `GET /tasks/{id}`
async fn handle_get_task<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
    Path(task_id): Path<String>,
) -> Result<Json<Task>, A2AError> {
    match state.store.load(&task_id).await? {
        Some(task) => Ok(Json(task)),
        None => Err(A2AError::task_not_found(&task_id)),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskMetadataItem {
    id: String,
    context_id: String,
    state: TaskState,
    updated_at: Option<String>,
}

/// `GET /tasks/metadata`
///
/// Returns task metadata snapshots for all tasks seen by this server process.
async fn handle_list_task_metadata<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
) -> Result<Json<Vec<TaskMetadataItem>>, A2AError> {
    let mut ids: Vec<String> = state.task_index.lock().await.iter().cloned().collect();
    ids.sort();
    let mut out = Vec::new();
    for id in ids {
        if let Some(task) = state.store.load(&id).await? {
            out.push(TaskMetadataItem {
                id: task.id,
                context_id: task.context_id,
                state: task.status.state,
                updated_at: task.status.timestamp,
            });
        }
    }
    Ok(Json(out))
}

/// `GET /tasks` — v1.0 ListTasks with filtering and pagination.
async fn handle_list_tasks<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
    Query(request): Query<ListTasksRequest>,
) -> Result<Json<ListTasksResponse>, A2AError> {
    // Ensure all tracked tasks are in the store for listing.
    let response = state.store.list(&request).await?;
    Ok(Json(response))
}

/// `GET /tasks/{id}:subscribe` — v1.0 SubscribeToTask.
async fn handle_subscribe_to_task<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
    Path(task_id): Path<String>,
) -> Result<Response, A2AError> {
    let task = state
        .store
        .load(&task_id)
        .await?
        .ok_or_else(|| A2AError::task_not_found(&task_id))?;

    // If task is already in terminal state, return UnsupportedOperationError.
    if is_terminal_state(task.status.state) {
        return Err(A2AError::unsupported_operation(
            "Task is already in a terminal state",
        ));
    }

    // Send current task state first, then stream updates.
    let initial = stream::once({
        let task_json = serde_json::to_string(&task).unwrap_or_default();
        async move { Ok::<Event, Infallible>(Event::default().event("task").data(task_json)) }
    });

    let maybe_bus = {
        let guard = state.event_buses.lock().await;
        guard.get(&task_id).map(|bus| bus.clone_sender())
    };

    if let Some(event_bus) = maybe_bus {
        let stream = BroadcastStream::new(event_bus.subscribe()).filter_map(|result| match result {
            Ok(event) => match &event {
                ExecutionEvent::Task(task) => {
                    Event::default().event("task").json_data(task).ok().map(Ok)
                }
                ExecutionEvent::Message(msg) => {
                    Event::default().event("message").json_data(msg).ok().map(Ok)
                }
                ExecutionEvent::StatusUpdate(update) => {
                    Event::default().event("status").json_data(update).ok().map(Ok)
                }
                ExecutionEvent::ArtifactUpdate(update) => {
                    Event::default().event("artifact").json_data(update).ok().map(Ok)
                }
            },
            Err(_) => None,
        });
        Ok(Sse::new(initial.chain(stream)).into_response())
    } else {
        Ok(Sse::new(initial).into_response())
    }
}

/// `GET /extendedAgentCard` — v1.0 GetExtendedAgentCard.
async fn handle_get_extended_agent_card<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
) -> Result<Json<AgentCard>, A2AError> {
    let card = state.executor.agent_card(&state.base_url);
    if card.capabilities.extended_agent_card.unwrap_or(false) {
        Ok(Json(card))
    } else {
        Err(A2AError::unsupported_operation("GetExtendedAgentCard"))
    }
}

/// Compatibility handler for `POST /tasks/{id}:cancel`.
///
/// Axum path params cannot include both a param and literal text in one segment,
/// so `/tasks/{id}:cancel` is matched as `/tasks/{id}` and parsed here.
async fn handle_cancel_task_compat<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
    Path(task_segment): Path<String>,
) -> Result<Json<Task>, A2AError> {
    let Some(task_id) = task_segment.strip_suffix(":cancel") else {
        return Err(A2AError::invalid_params(
            "Expected path format /tasks/{id}:cancel",
        ));
    };
    cancel_task_by_id(state, task_id).await
}

/// `POST /tasks/{id}:cancel`
async fn handle_cancel_task<E: AgentExecutor, S: TaskStore>(
    State(state): State<A2AServerState<E, S>>,
    Path(task_id): Path<String>,
) -> Result<Json<Task>, A2AError> {
    cancel_task_by_id(state, &task_id).await
}

async fn cancel_task_by_id<E: AgentExecutor, S: TaskStore>(
    state: A2AServerState<E, S>,
    task_id: &str,
) -> Result<Json<Task>, A2AError> {
    // Signal cancellation via the watch channel.
    let cancelled = {
        let tokens = state.cancel_tokens.lock().await;
        if let Some(tx) = tokens.get(task_id) {
            let _ = tx.send(true);
            true
        } else {
            false
        }
    };

    if !cancelled {
        return Err(A2AError::task_not_found(&task_id));
    }

    // Also call executor cancel for cleanup.
    let event_bus = {
        let guard = state.event_buses.lock().await;
        guard
            .get(task_id)
            .map(|bus| bus.clone_sender())
            .unwrap_or_else(|| EventBus::new(16))
    };
    let _ = state.executor.cancel(task_id, &event_bus).await;

    match state.store.load(&task_id).await? {
        Some(task) => Ok(Json(task)),
        None => Err(A2AError::task_not_found(&task_id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryTaskStore;

    struct StreamingFirstExecutor;

    impl AgentExecutor for StreamingFirstExecutor {
        fn execute(
            &self,
            context: RequestContext,
            event_bus: &EventBus,
        ) -> impl std::future::Future<Output = Result<(), A2AError>> + Send {
            async move {
                let task_id = context.task_id.unwrap_or_else(|| "task-1".to_string());
                let context_id = context.context_id;
                event_bus.publish_status_update(TaskStatusUpdateEvent {
                    task_id: task_id.clone(),
                    context_id: context_id.clone(),
                    status: TaskStatus {
                        state: TaskState::Working,
                        message: Some(Message {
                            message_id: "status-1".to_string(),
                            context_id: Some(context_id.clone()),
                            task_id: Some(task_id.clone()),
                            role: Role::Agent,
                            parts: vec![Part::text("working")],
                            metadata: None,
                            extensions: vec![],
                            reference_task_ids: None,
                        }),
                        timestamp: None,
                    },
                    metadata: None,
                });
                event_bus.publish(ExecutionEvent::Task(completed_task(
                    task_id, context_id, "done",
                )));
                Ok(())
            }
        }

        fn cancel(
            &self,
            _task_id: &str,
            _event_bus: &EventBus,
        ) -> impl std::future::Future<Output = Result<(), A2AError>> + Send {
            async move { Ok(()) }
        }

        fn agent_card(&self, base_url: &str) -> AgentCard {
            AgentCard {
                name: "test-agent".to_string(),
                description: "test".to_string(),
                supported_interfaces: vec![AgentInterface {
                    url: base_url.to_string(),
                    protocol_binding: "HTTP+JSON".to_string(),
                    tenant: None,
                    protocol_version: "1.0".to_string(),
                }],
                provider: None,
                version: "0.0.0".to_string(),
                documentation_url: None,
                capabilities: AgentCapabilities {
                    streaming: Some(true),
                    push_notifications: Some(false),
                    extended_agent_card: None,
                    extensions: vec![],
                },
                default_input_modes: vec!["text/plain".to_string()],
                default_output_modes: vec!["text/plain".to_string()],
                skills: vec![],
                icon_url: None,
            }
        }
    }

    #[tokio::test]
    async fn send_message_ignores_intermediate_status_and_returns_task() {
        let state: A2AServerState<StreamingFirstExecutor, InMemoryTaskStore> = A2AServerState {
            executor: Arc::new(StreamingFirstExecutor),
            store: Arc::new(InMemoryTaskStore::new()),
            base_url: "http://localhost".to_string(),
            cancel_tokens: Arc::new(Mutex::new(HashMap::new())),
            task_index: Arc::new(Mutex::new(HashSet::new())),
            push_configs: Arc::new(Mutex::new(HashMap::new())),
            event_buses: Arc::new(Mutex::new(HashMap::new())),
            acp_broadcaster: tokio::sync::broadcast::channel(16).0,
        };
        let state_for_assert = state.clone();

        let request = SendMessageRequest {
            message: Message {
                message_id: "msg-1".to_string(),
                context_id: Some("ctx-1".to_string()),
                task_id: None,
                role: Role::User,
                parts: vec![Part::text("hello")],
                metadata: None,
                extensions: vec![],
                reference_task_ids: None,
            },
            configuration: None,
            metadata: None,
        };

        let Json(response) = handle_send_message::<StreamingFirstExecutor, InMemoryTaskStore>(
            State(state),
            Json(request),
        )
        .await
        .expect("send should succeed");

        let SendMessageResponse::Task(task) = response else {
            panic!("expected task response");
        };
        assert_eq!(task.status.state, TaskState::Completed);

        let saved = state_for_assert
            .store
            .load(&task.id)
            .await
            .expect("store load should succeed");
        assert!(saved.is_some());

        let Json(metadata) = handle_list_task_metadata::<StreamingFirstExecutor, InMemoryTaskStore>(
            State(state_for_assert),
        )
        .await
        .expect("metadata should list saved task");
        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata[0].id, task.id);
        assert_eq!(metadata[0].context_id, task.context_id);
    }
}
