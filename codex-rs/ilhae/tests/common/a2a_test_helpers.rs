//! Shared A2A test helpers — session store + proxy server spawning.
//!
//! Used by `live_a2a_proxy.rs` and `live_a2a_e2e.rs`.

#![allow(dead_code)]

use std::time::Duration;

/// Create a temp SessionStore for testing.
pub fn make_test_session_store() -> (
    std::sync::Arc<ilhae_proxy::session_store::SessionStore>,
    tempfile::TempDir,
) {
    let tmp = tempfile::TempDir::new().expect("Failed to create temp dir");
    let store = ilhae_proxy::session_store::SessionStore::new(&tmp.path().to_path_buf())
        .expect("Failed to create SessionStore");
    (std::sync::Arc::new(store), tmp)
}

/// Start a ForwardingExecutor proxy A2A server for a single agent.
/// Returns the proxy URL (e.g. "http://127.0.0.1:PORT/a2a/researcher").
pub async fn start_test_proxy(
    role: &str,
    target_endpoint: &str,
    store: std::sync::Arc<ilhae_proxy::session_store::SessionStore>,
) -> (String, tokio::task::JoinHandle<()>) {
    use ilhae_proxy::CxCache;
    use ilhae_proxy::a2a_persistence::{ForwardingExecutor, PersistenceScheduleStore};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind");
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{}", port);
    let role_base = format!("{}/a2a/{}", base_url, role);

    let executor = ForwardingExecutor::new(
        target_endpoint.to_string(),
        role.to_string(),
        store.clone(),
        CxCache::new(),
    );
    let task_store = PersistenceScheduleStore::new(store, role.to_string());

    let server = a2a_rs::server::A2AServer::new(executor, task_store).base_url(&role_base);
    let mut app = axum::Router::new();
    app = app.nest(&format!("/a2a/{}", role), server.router());

    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    (role_base, handle)
}

/// Helper: send a message to an A2A agent and return the JSON-RPC result (async).
pub async fn send_a2a_message(
    client: &reqwest::Client,
    endpoint: &str,
    text: &str,
) -> serde_json::Value {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "messageId": format!("msg-{}", uuid::Uuid::new_v4()),
                "parts": [{"text": text}]
            }
        }
    });

    let resp = client
        .post(endpoint)
        .json(&payload)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .unwrap_or_else(|e| panic!("Failed to reach {}: {}", endpoint, e));

    assert!(
        resp.status().is_success(),
        "message/send to {} returned {}",
        endpoint,
        resp.status()
    );

    resp.json().await.unwrap()
}

/// Helper: send a JSON-RPC request and return the parsed response (blocking/sync).
pub fn a2a_rpc(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": format!("test-{}", uuid::Uuid::new_v4()),
        "method": method,
        "params": params
    });
    let resp = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(Duration::from_secs(120))
        .send()
        .unwrap_or_else(|e| panic!("{} {} failed: {}", method, endpoint, e));
    resp.json::<serde_json::Value>()
        .unwrap_or_else(|e| panic!("{} response parse error: {}", method, e))
}
