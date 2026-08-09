use super::*;

#[test]
fn stop_control_url_reuses_the_ensure_endpoint_namespace() {
    assert_eq!(
        control_action_url(
            "http://127.0.0.1:18083/_ilhae/native-runtime/ensure",
            "stop"
        )
        .expect("stop control URL")
        .as_str(),
        "http://127.0.0.1:18083/_ilhae/native-runtime/stop"
    );
}

#[test]
fn stop_control_url_rejects_an_ambiguous_control_path() {
    assert!(control_action_url("http://127.0.0.1:18083/control", "stop").is_err());
}

#[tokio::test]
async fn ssh_host_reuses_an_existing_verified_proxy_tunnel() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy health listener");
    let address = listener.local_addr().expect("proxy health address");
    let app = axum::Router::new().route(
        "/_ilhae/health",
        axum::routing::get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve proxy health response");
    });
    let config = IlhaeProfileNativeRuntimeConfig {
        ssh_host: Some("unused.invalid".to_string()),
        ssh_local_port: Some(address.port()),
        ..Default::default()
    };

    ensure_ssh_tunnel(&config)
        .await
        .expect("verified proxy should be reused without opening another tunnel");

    server.abort();
}
