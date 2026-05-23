use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::body::Bytes;
use axum::extract::OriginalUri;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::Method;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::any;
use axum::routing::get;
use axum::routing::post;
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::api::ErrorResponse;
use super::api::LeaseMode;
use super::api::LeaseRequest;
use super::api::LeaseState;
use super::client::GpuQueueClient;

mod backend;
mod config;
mod files;

use backend::BackendResponse;
use backend::backend_response;
use backend::should_forward_header;
pub use config::ComfyProxyConfig;
pub use config::ComfyProxyConfigOverrides;
pub use files::ResolvedComfyViewPath;
pub use files::resolve_view_path;

#[derive(Clone)]
struct ComfyProxy {
    config: Arc<ComfyProxyConfig>,
    http: reqwest::Client,
    gpu_queue: GpuQueueClient,
    history_cache: Arc<Mutex<HashMap<String, Bytes>>>,
    pending_prompts: Arc<Mutex<HashMap<String, Arc<PromptCompletion>>>>,
}

struct PromptLease {
    lease_id: String,
    stop_heartbeat: oneshot::Sender<()>,
    heartbeat_task: JoinHandle<()>,
}

struct PromptCompletion {
    result: Mutex<Option<Result<Bytes, String>>>,
    notify: Notify,
}

impl PromptCompletion {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    async fn complete(&self, result: Result<Bytes, String>) {
        *self.result.lock().await = Some(result);
        self.notify.notify_waiters();
    }

    async fn wait(&self) -> anyhow::Result<Bytes> {
        loop {
            if let Some(result) = self.result.lock().await.clone() {
                return result.map_err(anyhow::Error::msg);
            }
            self.notify.notified().await;
        }
    }
}

impl ComfyProxy {
    fn new(config: ComfyProxyConfig) -> Self {
        let gpu_queue = GpuQueueClient::from_addr(&config.gpu_queue_addr);
        Self {
            config: Arc::new(config),
            http: reqwest::Client::new(),
            gpu_queue,
            history_cache: Arc::new(Mutex::new(HashMap::new())),
            pending_prompts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn ensure_backend_started(&self) -> anyhow::Result<()> {
        if let Some(command) = self.config.start_command.as_ref() {
            run_shell_command(command)
                .await
                .with_context(|| format!("failed to run ComfyUI start command `{command}`"))?;
            self.wait_for_backend_reachable(Duration::from_secs(180))
                .await?;
        }
        Ok(())
    }

    async fn wait_for_backend_reachable(&self, timeout: Duration) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self
                .http
                .get(format!("{}/system_stats", self.config.backend_url))
                .send()
                .await
                .map(|response| response.status().is_success())
                .unwrap_or(false)
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "ComfyUI backend did not become reachable at {} within {}s",
                    self.config.backend_url,
                    timeout.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn stop_backend_after_prompt(&self) -> anyhow::Result<()> {
        if !self.config.stop_after_prompt {
            return Ok(());
        }
        if let Some(command) = self.config.stop_command.as_ref() {
            run_shell_command(command)
                .await
                .with_context(|| format!("failed to run ComfyUI stop command `{command}`"))?;
        }
        Ok(())
    }

    async fn acquire_prompt_lease(&self, kind: String) -> anyhow::Result<PromptLease> {
        let response = self
            .gpu_queue
            .acquire_lease(&LeaseRequest {
                owner: self.config.owner.clone(),
                kind,
                mode: LeaseMode::Exclusive,
                preempt_llm: self.config.preempt_llm,
                ttl_seconds: self.config.ttl_seconds,
                wait_timeout_seconds: Some(self.config.wait_timeout_seconds),
            })
            .await?;
        if response.state != LeaseState::Granted {
            anyhow::bail!("GPU lease `{}` was not granted", response.lease_id);
        }
        let (stop_heartbeat, heartbeat_stopped) = oneshot::channel();
        let lease_id = response.lease_id.clone();
        let client = self.gpu_queue.clone();
        let interval = Duration::from_secs((self.config.ttl_seconds / 3).clamp(5, 30));
        let heartbeat_task = tokio::spawn(async move {
            tokio::pin!(heartbeat_stopped);
            loop {
                tokio::select! {
                    _ = &mut heartbeat_stopped => break,
                    _ = tokio::time::sleep(interval) => {
                        let _ = client.heartbeat_lease(&lease_id).await;
                    }
                }
            }
        });

        Ok(PromptLease {
            lease_id: response.lease_id,
            stop_heartbeat,
            heartbeat_task,
        })
    }

    async fn cleanup_prompt_lease(&self, lease: PromptLease) -> anyhow::Result<()> {
        let _ = lease.stop_heartbeat.send(());
        lease.heartbeat_task.abort();
        let _ = self.free_backend_memory().await;
        if let Err(err) = self.stop_backend_after_prompt().await {
            eprintln!("Failed to stop ComfyUI before releasing GPU lease: {err:#}");
        }
        self.gpu_queue
            .release_lease(&lease.lease_id)
            .await
            .map(|_| ())
            .inspect_err(|err| {
                eprintln!("Failed to release GPU lease `{}`: {err:#}", lease.lease_id);
            })
    }

    async fn free_backend_memory(&self) -> anyhow::Result<()> {
        let response = self
            .http
            .post(format!("{}/free", self.config.backend_url))
            .json(&serde_json::json!({
                "unload_models": true,
                "free_memory": true,
            }))
            .send()
            .await?;
        if !response.status().is_success() {
            anyhow::bail!("ComfyUI /free returned {}", response.status());
        }
        Ok(())
    }

    async fn wait_for_prompt_completion(&self, prompt_id: &str) -> anyhow::Result<Bytes> {
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.config.prompt_timeout_seconds);
        let poll_interval = Duration::from_millis(self.config.prompt_poll_interval_ms);
        loop {
            let response = self.fetch_history(prompt_id).await?;
            if response.completed_history_for(prompt_id) {
                return Ok(response.body_bytes());
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("timed out waiting for ComfyUI prompt `{prompt_id}`");
            }
            tokio::time::sleep(poll_interval).await;
        }
    }

    async fn fetch_history(&self, prompt_id: &str) -> anyhow::Result<BackendResponse> {
        let response = self
            .http
            .get(format!("{}/history/{prompt_id}", self.config.backend_url))
            .send()
            .await?;
        backend_response(response).await
    }

    async fn cache_history(&self, prompt_id: &str, body: Bytes) {
        let mut cache = self.history_cache.lock().await;
        if cache.len() >= 128
            && let Some(oldest_key) = cache.keys().next().cloned()
        {
            cache.remove(&oldest_key);
        }
        cache.insert(prompt_id.to_string(), body);
    }

    async fn cached_history(&self, prompt_id: &str) -> Option<Bytes> {
        self.history_cache.lock().await.get(prompt_id).cloned()
    }

    async fn register_pending_prompt(&self, prompt_id: &str) -> Arc<PromptCompletion> {
        let completion = Arc::new(PromptCompletion::new());
        self.pending_prompts
            .lock()
            .await
            .insert(prompt_id.to_string(), completion.clone());
        completion
    }

    async fn pending_prompt(&self, prompt_id: &str) -> Option<Arc<PromptCompletion>> {
        self.pending_prompts.lock().await.get(prompt_id).cloned()
    }

    async fn finish_pending_prompt(
        &self,
        prompt_id: &str,
        completion: Arc<PromptCompletion>,
        result: anyhow::Result<Bytes>,
    ) {
        if let Ok(body) = result.as_ref() {
            self.cache_history(prompt_id, body.clone()).await;
        }
        self.pending_prompts.lock().await.remove(prompt_id);
        completion
            .complete(result.map_err(|error| error.to_string()))
            .await;
    }

    async fn forward_request(
        &self,
        method: Method,
        uri: OriginalUri,
        headers: HeaderMap,
        body: Bytes,
    ) -> anyhow::Result<BackendResponse> {
        let url = self.backend_url_for_uri(&uri)?;
        let reqwest_method = reqwest::Method::from_bytes(method.as_str().as_bytes())?;
        let mut request = self.http.request(reqwest_method, url);
        for (name, value) in headers.iter() {
            let name_text = name.as_str();
            if should_forward_header(name_text) {
                request = request.header(name_text, value);
            }
        }
        if method != Method::GET && method != Method::HEAD {
            request = request.body(body.to_vec());
        }

        let response = request.send().await?;
        backend_response(response).await
    }

    fn backend_url_for_uri(&self, uri: &OriginalUri) -> anyhow::Result<String> {
        let mut url = reqwest::Url::parse(&self.config.backend_url)?;
        let path_and_query = uri
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/");
        url.set_path("");
        Ok(format!(
            "{}{}",
            url.as_str().trim_end_matches('/'),
            path_and_query
        ))
    }
}

pub async fn run(config: ComfyProxyConfig) -> anyhow::Result<()> {
    let addr: SocketAddr = config
        .listen
        .parse()
        .with_context(|| format!("invalid ComfyUI proxy listen address `{}`", config.listen))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind ComfyUI proxy to {addr}"))?;
    println!(
        "ComfyUI GPU proxy listening on http://{} -> {}",
        addr, config.backend_url
    );
    axum::serve(listener, router(config)).await?;
    Ok(())
}

pub fn router(config: ComfyProxyConfig) -> Router {
    let proxy = ComfyProxy::new(config);
    Router::new()
        .route("/health", get(health))
        .route("/view", get(view))
        .route("/history/:prompt_id", get(history))
        .route("/prompt", post(prompt))
        .fallback(any(proxy_passthrough))
        .with_state(Arc::new(proxy))
}

pub fn infer_gpu_queue_kind(workflow: &Value) -> String {
    let class_types = workflow
        .as_object()
        .map(|nodes| {
            nodes
                .values()
                .filter_map(|node| node.get("class_type").and_then(Value::as_str))
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if class_types
        .iter()
        .any(|class_type| class_type.contains("video") || class_type.contains("vhs_"))
    {
        return "video".to_string();
    }
    if class_types
        .iter()
        .any(|class_type| class_type.contains("audio") || class_type.contains("tts"))
    {
        return "audio".to_string();
    }
    if class_types
        .iter()
        .any(|class_type| class_type.contains("image") || class_type.contains("sampler"))
    {
        return "image".to_string();
    }
    "video".to_string()
}

async fn health(State(proxy): State<Arc<ComfyProxy>>) -> Json<Value> {
    let backend_reachable = proxy
        .http
        .get(format!("{}/system_stats", proxy.config.backend_url))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false);
    Json(serde_json::json!({
        "ok": true,
        "backendUrl": proxy.config.backend_url,
        "backendReachable": backend_reachable,
    }))
}

async fn view(
    State(proxy): State<Arc<ComfyProxy>>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let file_type = query.get("type").map(String::as_str).unwrap_or("output");
    let subfolder = query.get("subfolder").map(String::as_str).unwrap_or("");
    let filename = query.get("filename").map(String::as_str).unwrap_or("");
    let resolved = resolve_view_path(&proxy.config.comfy_root, file_type, subfolder, filename)
        .map_err(bad_request)?;
    if let Ok(bytes) = tokio::fs::read(&resolved.path).await {
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", resolved.content_type)
            .body(Body::from(bytes))
            .map_err(internal_error)?;
        return Ok(response);
    }

    proxy
        .forward_request(Method::GET, OriginalUri(uri), headers, Bytes::new())
        .await
        .and_then(BackendResponse::into_axum)
        .map_err(proxy_error)
}

async fn history(
    State(proxy): State<Arc<ComfyProxy>>,
    Path(prompt_id): Path<String>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> ApiResult<Response> {
    if let Some(body) = proxy.cached_history(&prompt_id).await {
        return cached_json_response(body);
    }

    let response = proxy
        .forward_request(Method::GET, OriginalUri(uri), headers, Bytes::new())
        .await
        .map_err(proxy_error)?;
    if response.completed_history_for(&prompt_id) {
        if let Some(completion) = proxy.pending_prompt(&prompt_id).await {
            return cached_json_response(completion.wait().await.map_err(proxy_error)?);
        }
        proxy.cache_history(&prompt_id, response.body_bytes()).await;
    }
    response.into_axum().map_err(proxy_error)
}

async fn prompt(
    State(proxy): State<Arc<ComfyProxy>>,
    method: Method,
    uri: OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let payload: Value = serde_json::from_slice(&body).map_err(bad_request)?;
    let kind = headers
        .get("x-ilhae-gpu-kind")
        .and_then(|value| value.to_str().ok())
        .filter(|value| matches!(*value, "image" | "video" | "audio" | "text"))
        .map(ToString::to_string)
        .unwrap_or_else(|| infer_gpu_queue_kind(payload.get("prompt").unwrap_or(&Value::Null)));
    let lease = proxy
        .acquire_prompt_lease(kind)
        .await
        .map_err(proxy_error)?;
    let mut lease = Some(lease);

    let result = async {
        proxy.ensure_backend_started().await?;
        let _ = proxy.free_backend_memory().await;
        let response = proxy.forward_request(method, uri, headers, body).await?;
        let prompt_id = response.prompt_id();
        let response = response.into_axum()?;
        Ok::<_, anyhow::Error>((response, prompt_id))
    }
    .await;

    match result {
        Ok((response, Some(prompt_id))) => {
            let proxy_for_task = proxy.clone();
            let completion = proxy.register_pending_prompt(&prompt_id).await;
            let lease_for_task = lease.take().expect("lease should still exist");
            tokio::spawn(async move {
                let prompt_result = proxy_for_task.wait_for_prompt_completion(&prompt_id).await;
                let cleanup_result = proxy_for_task.cleanup_prompt_lease(lease_for_task).await;
                let result = match (prompt_result, cleanup_result) {
                    (Ok(body), Ok(())) => Ok(body),
                    (Err(err), _) => Err(err),
                    (Ok(_), Err(err)) => Err(err.context(format!(
                        "failed to release GPU lease after ComfyUI prompt `{prompt_id}` completed"
                    ))),
                };
                if let Err(err) = result.as_ref() {
                    eprintln!("ComfyUI prompt `{prompt_id}` watcher failed: {err:#}");
                }
                proxy_for_task
                    .finish_pending_prompt(&prompt_id, completion, result)
                    .await;
            });
            Ok(response)
        }
        Ok((response, None)) => {
            if let Some(lease) = lease.take() {
                proxy
                    .cleanup_prompt_lease(lease)
                    .await
                    .map_err(proxy_error)?;
            }
            Ok(response)
        }
        Err(err) => {
            if let Some(lease) = lease.take() {
                let _ = proxy.cleanup_prompt_lease(lease).await;
            }
            Err(proxy_error(err))
        }
    }
}

async fn proxy_passthrough(
    State(proxy): State<Arc<ComfyProxy>>,
    method: Method,
    uri: OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    if proxy.config.start_backend_for_passthrough {
        proxy.ensure_backend_started().await.map_err(proxy_error)?;
    }
    proxy
        .forward_request(method, uri, headers, body)
        .await
        .and_then(BackendResponse::into_axum)
        .map_err(proxy_error)
}

async fn run_shell_command(command: &str) -> anyhow::Result<()> {
    let status = Command::new("sh").arg("-lc").arg(command).status().await?;
    if !status.success() {
        anyhow::bail!("command exited with {status}");
    }
    Ok(())
}

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

fn bad_request(error: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}

fn proxy_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}

fn cached_json_response(body: Bytes) -> ApiResult<Response> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .map_err(internal_error)
}
