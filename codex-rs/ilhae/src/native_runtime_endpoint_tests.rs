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
fn ssh_host_derives_proxy_urls_while_explicit_values_remain_overrides() {
    let mut config = IlhaeProfileNativeRuntimeConfig {
        ssh_host: Some("yth".to_string()),
        ssh_local_port: Some(28083),
        ssh_remote_port: Some(9083),
        args: vec!["--port".to_string(), "8082".to_string()],
        ..Default::default()
    };

    assert!(uses_remote_proxy(&config));
    assert_eq!(
        effective_proxy_base_url(&config).as_deref(),
        Some("http://127.0.0.1:28083/v1")
    );
    assert_eq!(
        effective_proxy_control_url(&config).as_deref(),
        Some("http://127.0.0.1:28083/_ilhae/native-runtime/ensure")
    );
    assert_eq!(ssh_remote_port(&config), 9083);

    config.proxy_base_url = Some("http://127.0.0.1:38083/v1".to_string());
    config.proxy_control_url =
        Some("http://127.0.0.1:38083/_ilhae/native-runtime/ensure".to_string());
    assert_eq!(
        effective_proxy_base_url(&config).as_deref(),
        Some("http://127.0.0.1:38083/v1")
    );
    assert_eq!(
        effective_proxy_control_url(&config).as_deref(),
        Some("http://127.0.0.1:38083/_ilhae/native-runtime/ensure")
    );
}
