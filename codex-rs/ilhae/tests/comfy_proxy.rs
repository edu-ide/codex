use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::Path;
use axum::routing::get;
use axum::routing::post;
use codex_ilhae::gpu_queue::api::LeaseInfo;
use codex_ilhae::gpu_queue::api::LeaseMode;
use codex_ilhae::gpu_queue::api::LeaseRequest;
use codex_ilhae::gpu_queue::api::LeaseResponse;
use codex_ilhae::gpu_queue::api::LeaseState;
use codex_ilhae::gpu_queue::api::ReleaseLeaseResponse;
use codex_ilhae::gpu_queue::comfy_proxy::ComfyProxyConfig;
use codex_ilhae::gpu_queue::comfy_proxy::ComfyProxyConfigOverrides;
use codex_ilhae::gpu_queue::comfy_proxy::infer_gpu_queue_kind;
use codex_ilhae::gpu_queue::comfy_proxy::resolve_view_path;
use codex_ilhae::gpu_queue::comfy_proxy::router;
use serde_json::json;
use tokio::sync::Mutex;
use tokio::sync::Notify;

#[test]
fn infer_gpu_queue_kind_uses_workflow_output_nodes() {
    assert_eq!(
        infer_gpu_queue_kind(&json!({
            "1": {"class_type": "SaveImage", "inputs": {}}
        })),
        "image"
    );
    assert_eq!(
        infer_gpu_queue_kind(&json!({
            "1": {"class_type": "VHS_VideoCombine", "inputs": {}}
        })),
        "video"
    );
    assert_eq!(
        infer_gpu_queue_kind(&json!({
            "1": {"class_type": "SaveAudio", "inputs": {}}
        })),
        "audio"
    );
    assert_eq!(infer_gpu_queue_kind(&json!({})), "video");
}

#[test]
fn resolve_view_path_rejects_paths_outside_comfy_file_roots() {
    let root = PathBuf::from("/tmp/comfy");
    let resolved = resolve_view_path(&root, "output", "scene-a", "clip.mp4")
        .expect("valid ComfyUI output path");

    assert_eq!(
        resolved.path,
        PathBuf::from("/tmp/comfy/output/scene-a/clip.mp4")
    );
    assert_eq!(resolved.content_type, "video/mp4");

    let err = resolve_view_path(&root, "output", "", "../secret.mp4")
        .expect_err("path traversal should fail");
    assert!(err.to_string().contains("invalid ComfyUI view path"));

    let err = resolve_view_path(&root, "models", "", "model.safetensors")
        .expect_err("unsupported file root should fail");
    assert!(err.to_string().contains("invalid ComfyUI file type"));
}

#[test]
fn config_reads_comfy_proxy_table_from_ilhae_config_toml() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(
        tmp.path().join("config.toml"),
        r#"
[comfy_proxy]
listen = "127.0.0.1:19189"
backend_url = "http://127.0.0.1:18188"
comfy_root = "/tmp/comfy-root"
gpu_queue_addr = "127.0.0.1:13290"
owner = "test-comfy-proxy"
start_command = "echo start"
stop_command = "echo stop"
ttl_seconds = 120
wait_timeout_seconds = 12
prompt_poll_interval_ms = 25
prompt_timeout_seconds = 90
stop_after_prompt = false
start_backend_for_passthrough = false
"#,
    )
    .expect("write config");

    let config =
        ComfyProxyConfig::from_sources(Some(tmp.path()), ComfyProxyConfigOverrides::default());

    assert_eq!(config.listen, "127.0.0.1:19189");
    assert_eq!(config.backend_url, "http://127.0.0.1:18188");
    assert_eq!(config.comfy_root, PathBuf::from("/tmp/comfy-root"));
    assert_eq!(config.gpu_queue_addr, "127.0.0.1:13290");
    assert_eq!(config.owner, "test-comfy-proxy");
    assert_eq!(config.start_command.as_deref(), Some("echo start"));
    assert_eq!(config.stop_command.as_deref(), Some("echo stop"));
    assert_eq!(config.ttl_seconds, 120);
    assert_eq!(config.wait_timeout_seconds, 12);
    assert_eq!(config.prompt_poll_interval_ms, 25);
    assert_eq!(config.prompt_timeout_seconds, 90);
    assert!(!config.stop_after_prompt);
    assert!(!config.start_backend_for_passthrough);
}

#[tokio::test]
async fn prompt_route_holds_gpu_lease_until_comfy_history_completes() {
    #[derive(Clone)]
    struct Calls {
        entries: Arc<Mutex<Vec<String>>>,
        history_hits: Arc<Mutex<usize>>,
        released: Arc<Notify>,
    }

    let calls = Calls {
        entries: Arc::new(Mutex::new(Vec::new())),
        history_hits: Arc::new(Mutex::new(0)),
        released: Arc::new(Notify::new()),
    };
    let gpu_queue_url = spawn_router(
        Router::new()
            .route(
                "/leases",
                post({
                    let calls = calls.clone();
                    move |axum::Json(request): axum::Json<LeaseRequest>| {
                        let calls = calls.clone();
                        async move {
                            calls
                                .entries
                                .lock()
                                .await
                                .push(format!("lease:{}", request.kind));
                            axum::Json(LeaseResponse {
                                lease_id: "lease-1".to_string(),
                                state: LeaseState::Granted,
                                llm_was_preempted: true,
                            })
                        }
                    }
                }),
            )
            .route("/leases/:lease_id/heartbeat", post(heartbeat_lease))
            .route(
                "/leases/:lease_id/release",
                post({
                    let calls = calls.clone();
                    move |Path(lease_id): Path<String>| {
                        let calls = calls.clone();
                        async move {
                            calls
                                .entries
                                .lock()
                                .await
                                .push(format!("release:{lease_id}"));
                            calls.released.notify_one();
                            axum::Json(ReleaseLeaseResponse {
                                released: lease_info(&lease_id),
                                promoted: None,
                                llm_restarted: true,
                            })
                        }
                    }
                }),
            ),
    )
    .await;

    let backend_url = spawn_router(
        Router::new()
            .route(
                "/system_stats",
                get(|| async { axum::Json(json!({ "ok": true })) }),
            )
            .route(
                "/free",
                post({
                    let calls = calls.clone();
                    move || {
                        let calls = calls.clone();
                        async move {
                            calls.entries.lock().await.push("free".to_string());
                            axum::Json(json!({}))
                        }
                    }
                }),
            )
            .route(
                "/prompt",
                post({
                    let calls = calls.clone();
                    move || {
                        let calls = calls.clone();
                        async move {
                            calls.entries.lock().await.push("prompt".to_string());
                            axum::Json(json!({ "prompt_id": "prompt-1" }))
                        }
                    }
                }),
            )
            .route(
                "/history/:prompt_id",
                get({
                    let calls = calls.clone();
                    move |Path(prompt_id): Path<String>| {
                        let calls = calls.clone();
                        async move {
                            *calls.history_hits.lock().await += 1;
                            let mut body = serde_json::Map::new();
                            body.insert(
                                prompt_id,
                                json!({
                                    "status": { "completed": true, "status_str": "success" },
                                    "outputs": {}
                                }),
                            );
                            axum::Json(serde_json::Value::Object(body))
                        }
                    }
                }),
            ),
    )
    .await;

    let gateway_url = spawn_router(router(ComfyProxyConfig {
        backend_url,
        gpu_queue_addr: gpu_queue_url.trim_start_matches("http://").to_string(),
        prompt_poll_interval_ms: 1,
        prompt_timeout_seconds: 5,
        stop_after_prompt: false,
        start_backend_for_passthrough: false,
        ..ComfyProxyConfig::default()
    }))
    .await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/prompt"))
        .json(&json!({
            "prompt": {
                "1": { "class_type": "VHS_VideoCombine", "inputs": {}}
            }
        }))
        .send()
        .await
        .expect("gateway prompt response");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.expect("json"),
        json!({
            "prompt_id": "prompt-1"
        })
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), calls.released.notified())
        .await
        .expect("lease should be released after prompt history appears");

    let cached_history = reqwest::get(format!("{gateway_url}/history/prompt-1"))
        .await
        .expect("cached history response");
    assert_eq!(cached_history.status(), reqwest::StatusCode::OK);
    assert_eq!(
        cached_history
            .json::<serde_json::Value>()
            .await
            .expect("cached history json"),
        json!({
            "prompt-1": {
                "status": { "completed": true, "status_str": "success" },
                "outputs": {}
            }
        })
    );
    assert_eq!(*calls.history_hits.lock().await, 1);

    assert_eq!(
        *calls.entries.lock().await,
        vec![
            "lease:video".to_string(),
            "free".to_string(),
            "prompt".to_string(),
            "free".to_string(),
            "release:lease-1".to_string(),
        ]
    );
}

async fn heartbeat_lease(Path(lease_id): Path<String>) -> axum::Json<LeaseResponse> {
    axum::Json(LeaseResponse {
        lease_id,
        state: LeaseState::Granted,
        llm_was_preempted: true,
    })
}

fn lease_info(lease_id: &str) -> LeaseInfo {
    LeaseInfo {
        lease_id: lease_id.to_string(),
        owner: "test".to_string(),
        kind: "video".to_string(),
        mode: LeaseMode::Exclusive,
        state: LeaseState::Granted,
        preempt_llm: true,
        llm_was_preempted: true,
        ttl_seconds: 60,
        queued_at: 1,
        granted_at: Some(1),
        expires_at: Some(61),
    }
}

async fn spawn_router(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("test server addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("test server");
    });
    format!("http://{addr}")
}
