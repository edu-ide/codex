use super::*;

fn active_runtime() -> ActiveRuntime {
    ActiveRuntime {
        profile_id: "fable".to_string(),
        runtime_config_sha256: "abc".to_string(),
        upstream_base_url: "http://127.0.0.1:8081/v1".to_string(),
        upstream_health_url: "http://127.0.0.1:8081/health".to_string(),
    }
}

#[test]
fn upstream_url_preserves_v1_path_and_query_without_rewriting_runtime_args() {
    let uri = "/v1/responses?draft=mtp".parse().expect("request URI");

    assert_eq!(
        upstream_url_for_request(&active_runtime(), &uri).expect("upstream URL"),
        "http://127.0.0.1:8081/v1/responses?draft=mtp"
    );
}

#[test]
fn upstream_url_forwards_arbitrary_http_api_paths() {
    let uri = "/metrics?format=prometheus".parse().expect("request URI");

    assert_eq!(
        upstream_url_for_request(&active_runtime(), &uri).expect("upstream URL"),
        "http://127.0.0.1:8081/metrics?format=prometheus"
    );
}

#[test]
fn normalized_runtime_config_preserves_connection_only_mode() {
    let config = crate::config::IlhaeProfileNativeRuntimeConfig {
        enabled: false,
        proxy_base_url: Some("http://127.0.0.1:18083/v1".to_string()),
        proxy_control_url: Some("http://127.0.0.1:18083/_ilhae/native-runtime/ensure".to_string()),
        ..Default::default()
    };

    assert_eq!(
        normalized_runtime_config(config),
        crate::config::IlhaeProfileNativeRuntimeConfig::default()
    );
}

#[test]
fn remote_controller_rejects_non_loopback_upstream_targets() {
    let config = crate::config::IlhaeProfileNativeRuntimeConfig {
        base_url: "http://192.168.1.10:8081/v1".to_string(),
        health_url: "http://192.168.1.10:8081/health".to_string(),
        ..Default::default()
    };

    assert!(validate_runtime_request("fable", &config, UpstreamHostPolicy::LoopbackOnly).is_err());
}

#[test]
fn remote_controller_validates_upstream_instead_of_the_client_proxy_url() {
    let config = crate::config::IlhaeProfileNativeRuntimeConfig {
        base_url: "http://192.168.1.10:8081/v1".to_string(),
        health_url: "http://192.168.1.10:8081/health".to_string(),
        proxy_base_url: Some("http://127.0.0.1:18083/v1".to_string()),
        ..Default::default()
    };

    assert!(validate_runtime_request("fable", &config, UpstreamHostPolicy::LoopbackOnly).is_err());
}

#[test]
fn remote_controller_accepts_localhost_upstream_targets() {
    let config = crate::config::IlhaeProfileNativeRuntimeConfig {
        base_url: "http://localhost:8081/v1".to_string(),
        health_url: "http://localhost:8081/health".to_string(),
        ..Default::default()
    };

    assert!(validate_runtime_request("fable", &config, UpstreamHostPolicy::LoopbackOnly).is_ok());
}

#[test]
fn remote_controller_can_explicitly_allow_non_loopback_upstream_targets() {
    let config = crate::config::IlhaeProfileNativeRuntimeConfig {
        base_url: "http://192.168.1.10:8081/v1".to_string(),
        health_url: "http://192.168.1.10:8081/health".to_string(),
        ..Default::default()
    };

    assert!(validate_runtime_request("fable", &config, UpstreamHostPolicy::AllowAny).is_ok());
}

#[test]
fn remote_controller_rejects_non_http_upstream_schemes() {
    let config = crate::config::IlhaeProfileNativeRuntimeConfig {
        base_url: "file://localhost/tmp/v1".to_string(),
        health_url: "file://localhost/tmp/health".to_string(),
        ..Default::default()
    };

    assert!(validate_runtime_request("fable", &config, UpstreamHostPolicy::LoopbackOnly).is_err());
}

#[test]
fn connection_header_names_are_not_forwarded() {
    let headers = HeaderMap::from_iter([
        (
            "connection".parse().expect("connection name"),
            "keep-alive, X-Internal-Hop"
                .parse()
                .expect("connection value"),
        ),
        (
            "x-internal-hop".parse().expect("internal name"),
            "secret".parse().expect("internal value"),
        ),
    ]);

    assert_eq!(
        connection_header_names(&headers),
        HashSet::from([
            "keep-alive".parse().expect("keep-alive name"),
            "x-internal-hop".parse().expect("internal name"),
        ])
    );
}

#[tokio::test]
async fn control_requests_are_not_limited_to_axum_default_body_size() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy");
    let address = listener.local_addr().expect("proxy address");
    let state = ProxyState {
        client: reqwest::Client::new(),
        active_runtime: Arc::new(RwLock::new(None)),
        control_lock: Arc::new(Mutex::new(())),
        token_sha256: None,
        upstream_host_policy: UpstreamHostPolicy::LoopbackOnly,
    };
    let server = tokio::spawn(async move {
        axum::serve(listener, native_runtime_proxy_router(state))
            .await
            .expect("serve proxy");
    });
    let request = EnsureNativeRuntimeRequest {
        profile_id: String::new(),
        thinking_mode: "off".to_string(),
        config: crate::config::IlhaeProfileNativeRuntimeConfig {
            log_file: "x".repeat(2 * 1024 * 1024 + 1),
            ..Default::default()
        },
    };

    let response = reqwest::Client::new()
        .post(format!("http://{address}/_ilhae/native-runtime/ensure"))
        .json(&request)
        .send()
        .await
        .expect("send oversized control request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    server.abort();
}

#[tokio::test]
async fn authorization_rejects_large_control_body_before_json_extraction() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy");
    let address = listener.local_addr().expect("proxy address");
    let state = ProxyState {
        client: reqwest::Client::new(),
        active_runtime: Arc::new(RwLock::new(None)),
        control_lock: Arc::new(Mutex::new(())),
        token_sha256: Some(sha2::Sha256::digest("expected token").into()),
        upstream_host_policy: UpstreamHostPolicy::LoopbackOnly,
    };
    let server = tokio::spawn(async move {
        axum::serve(listener, native_runtime_proxy_router(state))
            .await
            .expect("serve proxy");
    });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/_ilhae/native-runtime/ensure"))
        .header(RUNTIME_TOKEN_HEADER, "wrong token")
        .body("x".repeat(2 * 1024 * 1024 + 1))
        .send()
        .await
        .expect("send unauthorized oversized control request");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    server.abort();
}

#[tokio::test]
async fn forward_request_streams_body_to_an_arbitrary_upstream_path() {
    async fn echo(request: Request) -> Response {
        let path_and_query = request
            .uri()
            .path_and_query()
            .expect("path and query")
            .as_str()
            .to_string();
        let body = axum::body::to_bytes(request.into_body(), usize::MAX)
            .await
            .expect("read streamed body");
        Response::new(Body::from(format!(
            "{path_and_query}:{}",
            String::from_utf8_lossy(&body)
        )))
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let address = listener.local_addr().expect("upstream address");
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(echo))
            .await
            .expect("serve upstream");
    });
    let state = ProxyState {
        client: reqwest::Client::new(),
        active_runtime: Arc::new(RwLock::new(Some(ActiveRuntime {
            profile_id: "fable".to_string(),
            runtime_config_sha256: "abc".to_string(),
            upstream_base_url: format!("http://{address}/v1"),
            upstream_health_url: format!("http://{address}/health"),
        }))),
        control_lock: Arc::new(Mutex::new(())),
        token_sha256: None,
        upstream_host_policy: UpstreamHostPolicy::LoopbackOnly,
    };
    let body = Body::from_stream(futures_util::stream::iter([
        Ok::<_, std::io::Error>("streamed "),
        Ok::<_, std::io::Error>("request"),
    ]));
    let request = Request::builder()
        .method("POST")
        .uri("/custom/inference?mode=full")
        .body(body)
        .expect("proxy request");

    let response = forward_request(State(state.clone()), request).await;
    let active_write = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        state.active_runtime.write(),
    )
    .await
    .expect("streamed response must not retain the active runtime read lock");
    drop(active_write);
    let response_body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read proxy response");

    assert_eq!(
        response_body,
        "/custom/inference?mode=full:streamed request"
    );
    server.abort();
}

#[tokio::test]
async fn forward_request_returns_redirect_without_following_it() {
    async fn redirect() -> Response {
        (StatusCode::TEMPORARY_REDIRECT, [("location", "/followed")]).into_response()
    }

    async fn followed() -> &'static str {
        "redirect was followed"
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let address = listener.local_addr().expect("upstream address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/redirect", get(redirect))
                .route("/followed", get(followed)),
        )
        .await
        .expect("serve upstream");
    });
    let state = ProxyState {
        client: native_runtime_proxy_client().expect("proxy client"),
        active_runtime: Arc::new(RwLock::new(Some(ActiveRuntime {
            profile_id: "fable".to_string(),
            runtime_config_sha256: "abc".to_string(),
            upstream_base_url: format!("http://{address}/v1"),
            upstream_health_url: format!("http://{address}/health"),
        }))),
        control_lock: Arc::new(Mutex::new(())),
        token_sha256: None,
        upstream_host_policy: UpstreamHostPolicy::LoopbackOnly,
    };
    let request = Request::builder()
        .uri("/redirect")
        .body(Body::empty())
        .expect("proxy request");

    let response = forward_request(State(state), request).await;

    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response.headers().get("location").expect("location header"),
        "/followed"
    );
    server.abort();
}
