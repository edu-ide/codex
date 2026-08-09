use super::*;

#[test]
fn local_runtime_urls_are_derived_from_separate_host_and_port_args() {
    let config = IlhaeProfileNativeRuntimeConfig {
        args: vec![
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "--port".to_string(),
            "8081".to_string(),
        ],
        ..Default::default()
    };

    assert_eq!(
        crate::config::native_runtime_effective_base_url(&config),
        "http://127.0.0.1:8081/v1"
    );
    assert_eq!(
        crate::config::native_runtime_effective_health_url(&config),
        "http://127.0.0.1:8081/health"
    );
}

#[test]
fn local_runtime_url_uses_the_last_inline_host_and_port_values() {
    let config = IlhaeProfileNativeRuntimeConfig {
        args: vec![
            "--host=wrong.example".to_string(),
            "--port=1".to_string(),
            "--host=::1".to_string(),
            "--port=8082".to_string(),
        ],
        ..Default::default()
    };

    assert_eq!(
        crate::config::native_runtime_effective_base_url(&config),
        "http://[::1]:8082/v1"
    );
}

#[test]
fn llama_server_uses_local_proxy_by_default_and_remote_origin_when_configured() {
    let mut config = IlhaeProfileNativeRuntimeConfig {
        provider: Some("llama-server".to_string()),
        args: vec!["--port".to_string(), "8082".to_string()],
        ..Default::default()
    };

    assert!(uses_runtime_proxy(&config));
    assert_eq!(
        effective_proxy_base_url(&config).as_deref(),
        Some("http://127.0.0.1:8083/v1")
    );
    assert_eq!(
        effective_proxy_control_url(&config).as_deref(),
        Some("http://127.0.0.1:8083/_ilhae/native-runtime/ensure")
    );

    config.proxy_url = Some("https://yth-runtime.example.com/".to_string());
    assert_eq!(
        effective_proxy_base_url(&config).as_deref(),
        Some("https://yth-runtime.example.com/v1")
    );
    assert_eq!(
        effective_proxy_control_url(&config).as_deref(),
        Some("https://yth-runtime.example.com/_ilhae/native-runtime/ensure")
    );
}

#[test]
fn proxy_host_bypasses_proxy_resolution_for_the_received_runtime_spec() {
    let config = IlhaeProfileNativeRuntimeConfig {
        provider: Some("llama-server".to_string()),
        proxy_url: Some("https://yth-runtime.example.com".to_string()),
        proxy_bypass: true,
        args: vec!["--port".to_string(), "8082".to_string()],
        ..Default::default()
    };

    assert!(!uses_runtime_proxy(&config));
    assert_eq!(effective_proxy_base_url(&config), None);
    assert_eq!(
        runtime_upstream_base_url(&config).as_deref(),
        Some("http://127.0.0.1:8082/v1")
    );
}

#[test]
fn proxy_transport_requires_https_and_a_token_off_loopback() {
    let mut config = IlhaeProfileNativeRuntimeConfig {
        provider: Some("llama-server".to_string()),
        proxy_url: Some("http://runtime.example.com".to_string()),
        ..Default::default()
    };

    assert_eq!(
        validate_proxy_transport(&config)
            .expect_err("plain HTTP must be rejected")
            .to_string(),
        "remote native runtime proxy_url must use HTTPS unless proxy_allow_insecure_http = true"
    );

    config.proxy_allow_insecure_http = true;
    assert_eq!(
        validate_proxy_transport(&config)
            .expect_err("insecure remote HTTP still requires authentication")
            .to_string(),
        "proxy_token is required for a remote native runtime proxy"
    );

    config.proxy_allow_insecure_http = false;
    config.proxy_url = Some("https://runtime.example.com".to_string());
    assert_eq!(
        validate_proxy_transport(&config)
            .expect_err("missing token must be rejected")
            .to_string(),
        "proxy_token is required for a remote native runtime proxy"
    );

    config.proxy_token = Some("secret".to_string());
    validate_proxy_transport(&config).expect("authenticated HTTPS proxy should be valid");

    config.proxy_url = Some("http://runtime.example.com".to_string());
    config.proxy_allow_insecure_http = true;
    validate_proxy_transport(&config).expect("explicit authenticated HTTP proxy should be valid");
}

#[test]
fn local_proxy_transport_does_not_require_a_token() {
    let config = IlhaeProfileNativeRuntimeConfig {
        provider: Some("llama-server".to_string()),
        ..Default::default()
    };

    validate_proxy_transport(&config).expect("loopback proxy should be valid without a token");
}
