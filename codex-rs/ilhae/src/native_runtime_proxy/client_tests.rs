use super::*;

#[test]
fn stop_control_url_reuses_the_ensure_endpoint_namespace() {
    assert_eq!(
        control_action_url("http://127.0.0.1:8083/_ilhae/native-runtime/ensure", "stop")
            .expect("stop control URL")
            .as_str(),
        "http://127.0.0.1:8083/_ilhae/native-runtime/stop"
    );
}

#[test]
fn stop_control_url_rejects_an_ambiguous_control_path() {
    assert!(control_action_url("http://127.0.0.1:18083/control", "stop").is_err());
}

#[test]
fn proxy_token_is_trimmed_and_empty_values_are_ignored() {
    let config = IlhaeProfileNativeRuntimeConfig {
        proxy_token: Some("  secret  ".to_string()),
        ..Default::default()
    };
    assert_eq!(proxy_token(&config), Some("secret"));

    let empty = IlhaeProfileNativeRuntimeConfig {
        proxy_token: Some("  ".to_string()),
        ..Default::default()
    };
    assert_eq!(proxy_token(&empty), None);
}
