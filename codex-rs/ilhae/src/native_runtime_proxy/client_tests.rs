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
