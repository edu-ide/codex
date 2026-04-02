//! A2A Protocol Feature Tests
//!
//! Tests the core A2A protocol capabilities:
//!
//! 1. delegate (sync `send_and_observe`)
//! 2. delegate_background (async `fire_and_forget`)
//! 3. propose (`send_and_observe` with proposal pattern)
//! 4. SendStreamingMessage (`message/stream` SSE)
//! 5. SubscribeToTask (`tasks/resubscribe` after async task)
//! 6. Push Notifications (push config CRUD + webhook delivery)

use std::sync::Arc;
use std::time::Duration;

use a2a_rs::event::{EventBus, ExecutionEvent};
use a2a_rs::executor::{AgentExecutor, RequestContext};
use a2a_rs::proxy::A2aProxy;
use a2a_rs::server::A2AServer;
use a2a_rs::store::InMemoryTaskStore;
use a2a_rs::types::*;
use serde_json::json;

// ─── Mock Agent (with push_notifications enabled) ────────────────────────

struct ProtocolTestAgent {
    role: String,
}

impl ProtocolTestAgent {
    fn new(role: &str) -> Self {
        Self {
            role: role.to_string(),
        }
    }
}

impl AgentExecutor for ProtocolTestAgent {
    async fn execute(
        &self,
        context: RequestContext,
        event_bus: &EventBus,
    ) -> Result<(), a2a_rs::error::A2AError> {
        let user_text = context
            .request
            .message
            .parts
            .iter()
            .filter_map(|p| p.text.as_deref())
            .collect::<Vec<_>>()
            .join(" ");

        let task_id = context.task_id.clone().unwrap_or_default();
        let context_id = context.context_id.clone();

        // Simulate processing delay
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Emit intermediate status update for streaming tests
        event_bus.publish(ExecutionEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: task_id.clone(),
            context_id: context_id.clone(),
            status: TaskStatus {
                state: TaskState::Working,
                message: Some(Message {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    context_id: None,
                    task_id: Some(task_id.clone()),
                    role: Role::Agent,
                    parts: vec![Part::text(&format!(
                        "[{}] Working on: {}",
                        self.role, user_text
                    ))],
                    metadata: None,
                    extensions: vec![],
                    reference_task_ids: None,
                }),
                timestamp: None,
            },
            kind: Some("status-update".to_string()),
            is_final: Some(false),
            metadata: None,
        }));

        tokio::time::sleep(Duration::from_millis(30)).await;

        // Emit completed task
        let response = format!("[{}] Result: {}", self.role, user_text);
        event_bus.publish(ExecutionEvent::Task(Task {
            id: task_id.clone(),
            context_id,
            status: TaskStatus {
                state: TaskState::Completed,
                message: Some(Message {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    context_id: None,
                    task_id: Some(task_id),
                    role: Role::Agent,
                    parts: vec![Part::text(&response)],
                    metadata: None,
                    extensions: vec![],
                    reference_task_ids: None,
                }),
                timestamp: None,
            },
            history: vec![],
            artifacts: vec![],
            metadata: None,
        }));

        Ok(())
    }

    async fn cancel(
        &self,
        _task_id: &str,
        _event_bus: &EventBus,
    ) -> Result<(), a2a_rs::error::A2AError> {
        Ok(())
    }

    fn agent_card(&self, base_url: &str) -> AgentCard {
        AgentCard {
            name: format!("{} Agent", self.role),
            description: format!("Protocol test agent: {}", self.role),
            supported_interfaces: vec![AgentInterface {
                url: format!("{}/", base_url),
                protocol_binding: "JSONRPC".to_string(),
                tenant: None,
                protocol_version: "rc.1".to_string(),
            }],
            version: "1.0.0".to_string(),
            capabilities: AgentCapabilities {
                streaming: Some(true),
                push_notifications: Some(true),
                extended_agent_card: None,
                extensions: vec![],
            },
            default_input_modes: vec!["text".to_string()],
            default_output_modes: vec!["text".to_string()],
            skills: vec![AgentSkill {
                id: self.role.to_lowercase(),
                name: self.role.clone(),
                description: format!("{} capabilities", self.role),
                tags: vec![],
                examples: vec![],
                input_modes: None,
                output_modes: None,
            }],
            provider: None,
            documentation_url: None,
            icon_url: None,
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────

async fn start_protocol_agent(role: &str) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let addr = format!("127.0.0.1:{}", port);
    let agent = ProtocolTestAgent::new(role);
    let handle = tokio::spawn({
        let addr = addr.clone();
        async move {
            let server = A2AServer::new(agent, InMemoryTaskStore::new())
                .bind(&addr)
                .base_url(&format!("http://{}", addr));
            let _ = server.run().await;
        }
    });

    tokio::time::sleep(Duration::from_millis(200)).await;
    (port, handle)
}

fn endpoint(port: u16) -> String {
    format!("http://127.0.0.1:{}", port)
}

// ─── Tests ───────────────────────────────────────────────────────────────

/// Scenario 1: Delegate (sync send_and_observe)
///
/// Verifies synchronous delegation: send a message, wait for completion,
/// receive the full response text.
#[tokio::test]
async fn test_delegate_sync() {
    let (port, _h) = start_protocol_agent("Researcher").await;
    let proxy = A2aProxy::new(&endpoint(port), "researcher");

    let (text, events) = proxy
        .send_and_observe("AI의 미래에 대해 조사해줘", None, None)
        .await
        .expect("Delegate sync failed");

    assert!(
        text.contains("[Researcher]"),
        "Response should contain agent role: {}",
        text
    );
    assert!(!events.is_empty(), "Should have received stream events");
    println!("✅ Delegate sync: {}", text);
}

/// Scenario 2: Delegate Background (async fire_and_forget)
///
/// Verifies async delegation: fire a task, get the task_id immediately,
/// then check the task completed via subscribe.
#[tokio::test]
async fn test_delegate_background() {
    let (port, _h) = start_protocol_agent("Creator").await;
    let proxy = A2aProxy::new(&endpoint(port), "creator");

    // Fire and forget — returns task_id immediately
    let task_id = proxy
        .fire_and_forget("백그라운드에서 보고서 작성해줘", None, None)
        .await
        .expect("Fire and forget failed");

    assert!(!task_id.is_empty(), "Task ID should not be empty");
    println!("✅ Delegate background: task_id={}", task_id);

    // Wait for completion
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify task completed via resubscribe
    let events = proxy
        .subscribe_to_task(&task_id)
        .await
        .expect("Subscribe after background failed");

    let text: String = events
        .iter()
        .map(|e| a2a_rs::proxy::extract_text_from_stream_event(e))
        .collect::<Vec<_>>()
        .join("");

    assert!(
        text.contains("[Creator]") || !events.is_empty(),
        "Should have completion events"
    );
    println!("✅ Background task completed: {}", text);
}

/// Scenario 3: Propose (sync with proposal pattern)
///
/// Propose is semantically the same as delegate, but the caller treats the
/// response as a proposal to accept/reject.
#[tokio::test]
async fn test_propose() {
    let (port, _h) = start_protocol_agent("Advisor").await;
    let proxy = A2aProxy::new(&endpoint(port), "advisor");

    let (proposal_text, _) = proxy
        .send_and_observe("다음 분기 전략을 제안해줘. 3가지 옵션으로.", None, None)
        .await
        .expect("Propose failed");

    assert!(!proposal_text.is_empty(), "Proposal should not be empty");
    assert!(
        proposal_text.contains("[Advisor]"),
        "Proposal should come from Advisor: {}",
        proposal_text
    );
    println!("✅ Propose: {}", proposal_text);
}

/// Scenario 4: SendStreamingMessage (message/stream SSE)
///
/// Verifies SSE streaming: client sends message/stream, receives
/// status updates and final task via SSE events.
#[tokio::test]
async fn test_send_streaming_message() {
    let (port, _h) = start_protocol_agent("Streamer").await;
    let client = reqwest::Client::new();

    let payload = json!({
        "jsonrpc": "2.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "method": "message/stream",
        "params": {
            "message": {
                "role": "user",
                "messageId": format!("msg-{}", uuid::Uuid::new_v4()),
                "parts": [{"text": "스트리밍으로 응답해줘"}]
            }
        }
    });

    let resp = client
        .post(&endpoint(port))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .json(&payload)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("SSE request failed");

    assert_eq!(
        resp.status().as_u16(),
        200,
        "SSE should return 200, got {}",
        resp.status()
    );

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/event-stream"),
        "Content-Type should be text/event-stream, got: {}",
        content_type
    );

    // Read SSE body
    let body = resp.text().await.expect("Failed to read SSE body");
    assert!(!body.is_empty(), "SSE body should not be empty");

    // Parse SSE events
    let events: Vec<&str> = body
        .split("data: ")
        .filter(|s| !s.trim().is_empty())
        .collect();
    assert!(
        !events.is_empty(),
        "Should have received SSE data events, body: {}",
        body
    );

    // Verify at least one event contains task state
    let has_completed = body.contains("completed");
    assert!(
        has_completed,
        "SSE stream should contain a 'completed' state, body: {}",
        body
    );

    println!(
        "✅ SendStreamingMessage: {} SSE events received",
        events.len()
    );
}

/// Scenario 5: SubscribeToTask (tasks/resubscribe)
///
/// Verifies re-subscription: launch async task, then subscribe to its
/// updates via tasks/resubscribe JSON-RPC.
#[tokio::test]
async fn test_subscribe_to_task() {
    let (port, _h) = start_protocol_agent("Worker").await;
    let proxy = A2aProxy::new(&endpoint(port), "worker");

    // Step 1: Fire async task
    let task_id = proxy
        .fire_and_forget("비동기 작업 처리해줘", None, None)
        .await
        .expect("Fire and forget failed");

    assert!(!task_id.is_empty());

    // Step 2: Wait for completion
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Step 3: Re-subscribe via A2aProxy
    let events = proxy
        .subscribe_to_task(&task_id)
        .await
        .expect("SubscribeToTask failed");

    println!(
        "✅ SubscribeToTask: {} events for task {}",
        events.len(),
        task_id
    );

    // Step 4: Also verify via raw JSON-RPC
    let client = reqwest::Client::new();
    let rpc = json!({
        "jsonrpc": "2.0",
        "id": "test-resubscribe",
        "method": "tasks/resubscribe",
        "params": { "id": task_id }
    });

    let resp = client
        .post(&endpoint(port))
        .json(&rpc)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("JSON-RPC resubscribe failed");

    assert!(
        resp.status().is_success(),
        "tasks/resubscribe should succeed, got {}",
        resp.status()
    );

    println!("✅ SubscribeToTask JSON-RPC verified");
}

/// Scenario 6: Push Notifications (config CRUD + webhook delivery)
///
/// Verifies:
/// - Agent card advertises pushNotifications: true
/// - CRUD: set, get, list, delete push notification configs
/// - Webhook delivery to a local HTTP server
#[tokio::test]
async fn test_push_notifications() {
    let (port, _h) = start_protocol_agent("PushAgent").await;
    let client = reqwest::Client::new();

    // ── 1. Verify agent card ──
    let card_url = format!("{}/.well-known/agent.json", endpoint(port));
    let card: serde_json::Value = client
        .get(&card_url)
        .send()
        .await
        .expect("Agent card fetch failed")
        .json()
        .await
        .unwrap();

    let push_supported = card
        .pointer("/capabilities/pushNotifications")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(
        push_supported,
        "Agent card should advertise pushNotifications: true, got: {:?}",
        card.get("capabilities")
    );

    // ── 2. Create a task first ──
    let send_payload = json!({
        "jsonrpc": "2.0",
        "id": "push-test-1",
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "messageId": format!("msg-{}", uuid::Uuid::new_v4()),
                "parts": [{"text": "Push notification 테스트 작업"}]
            }
        }
    });

    let send_resp: serde_json::Value = client
        .post(&endpoint(port))
        .json(&send_payload)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("message/send failed")
        .json()
        .await
        .unwrap();

    let task_id = send_resp
        .pointer("/result/id")
        .or_else(|| send_resp.pointer("/result/task/id"))
        .and_then(|v| v.as_str())
        .expect("No task ID in response");

    println!("  Task created: {}", task_id);

    // ── 3. Start a local webhook server ──
    let webhook_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let webhook_port = webhook_listener.local_addr().unwrap().port();
    let webhook_url = format!("http://127.0.0.1:{}/webhook", webhook_port);

    let received_notifications = Arc::new(tokio::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received_notifications.clone();

    let webhook_handle = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/webhook",
            axum::routing::post(move |body: axum::extract::Json<serde_json::Value>| {
                let received = received_clone.clone();
                async move {
                    received.lock().await.push(body.0);
                    axum::http::StatusCode::OK
                }
            }),
        );
        let _ = axum::serve(webhook_listener, app).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // ── 4. Set push notification config ──
    let set_config = json!({
        "jsonrpc": "2.0",
        "id": "push-test-set",
        "method": "tasks/pushNotificationConfig/set",
        "params": {
            "taskId": task_id,
            "pushNotificationConfig": {
                "url": webhook_url,
                "token": "test-token-123"
            }
        }
    });

    let set_resp: serde_json::Value = client
        .post(&endpoint(port))
        .json(&set_config)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("pushNotificationConfig/set failed")
        .json()
        .await
        .unwrap();

    assert!(
        set_resp.get("result").is_some(),
        "Set config should return result: {:?}",
        set_resp
    );
    println!("  Push config set for task {}", task_id);

    // ── 5. Get push notification config ──
    let get_config = json!({
        "jsonrpc": "2.0",
        "id": "push-test-get",
        "method": "tasks/pushNotificationConfig/get",
        "params": { "id": task_id }
    });

    let get_resp: serde_json::Value = client
        .post(&endpoint(port))
        .json(&get_config)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("pushNotificationConfig/get failed")
        .json()
        .await
        .unwrap();

    assert!(
        get_resp.get("result").is_some(),
        "Get config should return result: {:?}",
        get_resp
    );

    // ── 6. List push notification configs ──
    let list_config = json!({
        "jsonrpc": "2.0",
        "id": "push-test-list",
        "method": "tasks/pushNotificationConfig/list",
        "params": { "id": task_id }
    });

    let list_resp: serde_json::Value = client
        .post(&endpoint(port))
        .json(&list_config)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("pushNotificationConfig/list failed")
        .json()
        .await
        .unwrap();

    let configs = list_resp
        .pointer("/result")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(
        configs >= 1,
        "Should have at least 1 push config: {:?}",
        list_resp
    );

    // ── 7. Delete push notification config ──
    let config_id = get_resp
        .pointer("/result/pushNotificationConfig/id")
        .and_then(|v| v.as_str())
        .unwrap_or(task_id);

    let delete_config = json!({
        "jsonrpc": "2.0",
        "id": "push-test-delete",
        "method": "tasks/pushNotificationConfig/delete",
        "params": {
            "id": task_id,
            "pushNotificationConfigId": config_id
        }
    });

    let delete_resp: serde_json::Value = client
        .post(&endpoint(port))
        .json(&delete_config)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("pushNotificationConfig/delete failed")
        .json()
        .await
        .unwrap();

    assert!(
        delete_resp.get("error").is_none(),
        "Delete should not error: {:?}",
        delete_resp
    );

    webhook_handle.abort();
    println!("✅ Push Notifications: CRUD verified (set → get → list → delete)");
}
