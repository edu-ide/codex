use super::EnsureNativeRuntimeRequest;
use super::EnsureNativeRuntimeResponse;
use super::StopNativeRuntimeRequest;
use super::StopNativeRuntimeResponse;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::extract::Request;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::HeaderName;
use axum::http::StatusCode;
use axum::middleware;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use axum::routing::post;
use sha2::Digest;
use std::collections::HashSet;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

const DEFAULT_LISTEN_ADDRESS: &str = "127.0.0.1:8083";
const RUNTIME_TOKEN_HEADER: &str = "x-ilhae-runtime-token";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveRuntime {
    profile_id: String,
    runtime_config_sha256: String,
    upstream_base_url: String,
    upstream_health_url: String,
}

#[derive(Clone)]
struct ProxyState {
    client: reqwest::Client,
    active_runtime: Arc<RwLock<Option<ActiveRuntime>>>,
    control_lock: Arc<Mutex<()>>,
    token_sha256: Option<[u8; 32]>,
    upstream_host_policy: UpstreamHostPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpstreamHostPolicy {
    LoopbackOnly,
    AllowAny,
}

pub async fn run_native_runtime_proxy() -> anyhow::Result<()> {
    let listen_address = std::env::var("ILHAE_RUNTIME_PROXY_LISTEN")
        .unwrap_or_else(|_| DEFAULT_LISTEN_ADDRESS.to_string())
        .parse::<SocketAddr>()?;
    let token = std::env::var("ILHAE_RUNTIME_PROXY_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());
    if !listen_address.ip().is_loopback() && token.is_none() {
        anyhow::bail!(
            "ILHAE_RUNTIME_PROXY_TOKEN is required when binding the runtime proxy to a non-loopback address"
        );
    }
    let upstream_host_policy =
        if environment_flag_enabled("ILHAE_RUNTIME_PROXY_ALLOW_NON_LOOPBACK_UPSTREAM") {
            UpstreamHostPolicy::AllowAny
        } else {
            UpstreamHostPolicy::LoopbackOnly
        };
    let state = ProxyState {
        client: native_runtime_proxy_client()?,
        active_runtime: Arc::new(RwLock::new(None)),
        control_lock: Arc::new(Mutex::new(())),
        token_sha256: token.map(|token| sha2::Sha256::digest(token).into()),
        upstream_host_policy,
    };
    let app = native_runtime_proxy_router(state);
    let listener = tokio::net::TcpListener::bind(listen_address).await?;
    tracing::info!(address = %listen_address, "native runtime control proxy listening");
    axum::serve(listener, app).await?;
    Ok(())
}

fn native_runtime_proxy_router(state: ProxyState) -> Router {
    Router::new()
        .route("/_ilhae/health", get(proxy_health))
        .route("/_ilhae/native-runtime/ensure", post(ensure_native_runtime))
        .route("/_ilhae/native-runtime/stop", post(stop_native_runtime))
        .fallback(forward_request)
        .layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_authorization,
        ))
        .with_state(state)
}

fn native_runtime_proxy_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(Into::into)
}

async fn require_authorization(
    State(state): State<ProxyState>,
    request: Request,
    next: Next,
) -> Response {
    if !authorized(&state, request.headers()) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid runtime proxy token");
    }
    next.run(request).await
}

async fn proxy_health(State(state): State<ProxyState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid runtime proxy token");
    }
    let active = state.active_runtime.read().await;
    Json(serde_json::json!({
        "status": "ok",
        "active_profile": active.as_ref().map(|runtime| runtime.profile_id.as_str()),
        "runtime_config_sha256": active
            .as_ref()
            .map(|runtime| runtime.runtime_config_sha256.as_str()),
    }))
    .into_response()
}

async fn stop_native_runtime(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(request): Json<StopNativeRuntimeRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid runtime proxy token");
    }
    if let Err(error) = validate_runtime_request(
        &request.profile_id,
        &request.config,
        state.upstream_host_policy,
    ) {
        return error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }

    let _control_guard = state.control_lock.lock().await;
    if state
        .active_runtime
        .read()
        .await
        .as_ref()
        .is_some_and(|runtime| runtime.profile_id != request.profile_id)
    {
        return error_response(
            StatusCode::CONFLICT,
            "the requested profile is not the active managed runtime",
        );
    }
    let runtime_config = normalized_runtime_config(request.config);
    if runtime_config.enabled {
        if let Err(error) = crate::startup_main::stop_native_runtime_server_for_config(
            &request.profile_id,
            &runtime_config,
        )
        .await
        {
            return error_response(StatusCode::CONFLICT, &error.to_string());
        }
    }
    *state.active_runtime.write().await = None;
    Json(StopNativeRuntimeResponse {
        profile_id: request.profile_id,
    })
    .into_response()
}

async fn ensure_native_runtime(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(request): Json<EnsureNativeRuntimeRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid runtime proxy token");
    }
    if let Err(error) = validate_runtime_request(
        &request.profile_id,
        &request.config,
        state.upstream_host_policy,
    ) {
        return error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    let thinking_mode = crate::settings_types::normalize_thinking_mode(&request.thinking_mode);
    if request.thinking_mode != thinking_mode {
        return error_response(
            StatusCode::BAD_REQUEST,
            "native runtime thinking_mode must be `on` or `off`",
        );
    }

    let _control_guard = state.control_lock.lock().await;
    let runtime_config = normalized_runtime_config(request.config);
    let fingerprint = crate::startup_main::native_runtime_execution_fingerprint_with_thinking_mode(
        &runtime_config,
        &thinking_mode,
    );
    if let Err(error) = crate::startup_main::ensure_native_runtime_for_proxy_with_thinking_mode(
        &request.profile_id,
        &runtime_config,
        &thinking_mode,
    )
    .await
    {
        *state.active_runtime.write().await = None;
        return error_response(StatusCode::CONFLICT, &error.to_string());
    }

    let upstream_base_url = crate::config::native_runtime_effective_base_url(&runtime_config);
    let upstream_health_url = crate::config::native_runtime_effective_health_url(&runtime_config);
    *state.active_runtime.write().await = Some(ActiveRuntime {
        profile_id: request.profile_id.clone(),
        runtime_config_sha256: fingerprint.clone(),
        upstream_base_url: upstream_base_url.clone(),
        upstream_health_url,
    });
    Json(EnsureNativeRuntimeResponse {
        profile_id: request.profile_id,
        runtime_config_sha256: fingerprint,
        upstream_base_url,
    })
    .into_response()
}

async fn forward_request(State(state): State<ProxyState>, request: Request) -> Response {
    if !authorized(&state, request.headers()) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid runtime proxy token");
    }
    let Some(active_runtime) = state.active_runtime.read().await.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "no native runtime has been activated",
        );
    };
    let upstream_url = match upstream_url_for_request(&active_runtime, request.uri()) {
        Ok(url) => url,
        Err(error) => return error_response(StatusCode::NOT_FOUND, &error.to_string()),
    };
    let (parts, body) = request.into_parts();
    let connection_headers = connection_header_names(&parts.headers);
    let mut upstream_request = state
        .client
        .request(parts.method, upstream_url)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()));
    for (name, value) in &parts.headers {
        if !is_hop_by_hop_header(name)
            && !connection_headers.contains(name)
            && name.as_str() != "host"
            && name.as_str() != RUNTIME_TOKEN_HEADER
        {
            upstream_request = upstream_request.header(name, value);
        }
    }
    let upstream_response = match upstream_request.send().await {
        Ok(response) => response,
        Err(error) => return error_response(StatusCode::BAD_GATEWAY, &error.to_string()),
    };
    streaming_response(upstream_response)
}

fn streaming_response(upstream: reqwest::Response) -> Response {
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let connection_headers = connection_header_names(&headers);
    let mut response = Response::new(Body::from_stream(upstream.bytes_stream()));
    *response.status_mut() = status;
    for (name, value) in &headers {
        if !is_hop_by_hop_header(name) && !connection_headers.contains(name) {
            response.headers_mut().append(name, value.clone());
        }
    }
    response
}

fn normalized_runtime_config(
    mut config: crate::config::IlhaeProfileNativeRuntimeConfig,
) -> crate::config::IlhaeProfileNativeRuntimeConfig {
    config.proxy_url = None;
    config.proxy_allow_insecure_http = false;
    config.proxy_token = None;
    config.proxy_bypass = true;
    if let Some(headers) = config.http_headers.as_mut() {
        headers.retain(|name, _| !name.eq_ignore_ascii_case(RUNTIME_TOKEN_HEADER));
    }
    config
}

fn validate_runtime_request(
    profile_id: &str,
    config: &crate::config::IlhaeProfileNativeRuntimeConfig,
    upstream_host_policy: UpstreamHostPolicy,
) -> anyhow::Result<()> {
    if profile_id.trim().is_empty() || profile_id.len() > 128 {
        anyhow::bail!("profile_id must contain 1 to 128 bytes");
    }
    let runtime_config = normalized_runtime_config(config.clone());
    let base_url = crate::config::native_runtime_effective_base_url(&runtime_config);
    let health_url = crate::config::native_runtime_effective_health_url(&runtime_config);
    for (field, value) in [("base_url", base_url), ("health_url", health_url)] {
        let url = url::Url::parse(value.trim())?;
        if !matches!(url.scheme(), "http" | "https") {
            anyhow::bail!("{field} must use the http or https scheme");
        }
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("{field} must include a host"))?;
        let is_loopback = host.eq_ignore_ascii_case("localhost")
            || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
        if !is_loopback && upstream_host_policy == UpstreamHostPolicy::LoopbackOnly {
            anyhow::bail!("{field} must target a loopback address on the runtime host");
        }
    }
    Ok(())
}

fn upstream_url_for_request(
    active: &ActiveRuntime,
    uri: &axum::http::Uri,
) -> anyhow::Result<String> {
    let mut url = if uri.path() == "/health" {
        url::Url::parse(&active.upstream_health_url)?
    } else {
        let mut url = url::Url::parse(&active.upstream_base_url)?;
        url.set_path(uri.path());
        url.set_query(None);
        url.set_fragment(None);
        url
    };
    if let Some(query) = uri.query() {
        url.set_query(Some(query));
    }
    Ok(url.into())
}

fn environment_flag_enabled(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn authorized(state: &ProxyState, headers: &HeaderMap) -> bool {
    let Some(expected) = state.token_sha256 else {
        return true;
    };
    let Some(token) = headers
        .get(RUNTIME_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let actual: [u8; 32] = sha2::Sha256::digest(token).into();
    actual
        .iter()
        .zip(expected)
        .fold(0_u8, |difference, (actual, expected)| {
            difference | (actual ^ expected)
        })
        == 0
}

fn is_hop_by_hop_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn connection_header_names(headers: &HeaderMap) -> HashSet<HeaderName> {
    headers
        .get_all("connection")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect()
}

fn error_response(status: StatusCode, error: &str) -> Response {
    (status, Json(serde_json::json!({ "error": error }))).into_response()
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
