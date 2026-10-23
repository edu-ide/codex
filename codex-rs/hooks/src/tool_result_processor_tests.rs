use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::future::pending;
use std::time::Instant;

fn config(script: &str) -> ToolResultProcessorConfig {
    serde_json::from_value(json!({"command": ["sh", "-c", script, "processor"]})).unwrap()
}

#[tokio::test]
async fn processor_exchanges_one_json_document_without_interpreting_argv() {
    let dir = tempfile::tempdir().unwrap();
    let literal = "$(touch should-not-exist)";
    let mut config = config(
        "cat > input.json; printf '%s' \"$1\" > argv; printf '%s' '{\"version\":1,\"action\":\"replace_text\",\"text\":\"compact\"}'",
    );
    config.command.push(literal.to_string());
    let request = json!({"output": {"text": "raw\noutput"}});
    assert_eq!(
        process_tool_result(&config, &request, dir.path(), pending()).await,
        Some("compact".to_string())
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(dir.path().join("input.json")).unwrap()
        )
        .unwrap(),
        request
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("argv")).unwrap(),
        literal
    );
    assert!(!dir.path().join("should-not-exist").exists());
}

#[tokio::test]
async fn processor_keeps_original_for_invalid_response_without_retry() {
    let dir = tempfile::tempdir().unwrap();
    for response in [
        "not-json",
        "{\"version\":2,\"action\":\"replace_text\",\"text\":\"wrong\"}",
        "{\"version\":1,\"action\":\"replace_text\",\"text\":\"wrong\",\"extra\":true}",
        "{\"version\":1,\"action\":\"unchanged\"}",
    ] {
        let mut config = config("cat >/dev/null; printf x >> calls; printf '%s' \"$1\"");
        config.command.push(response.to_string());
        assert_eq!(
            process_tool_result(&config, &json!({}), dir.path(), pending()).await,
            None
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("calls")).unwrap(),
        "xxxx"
    );
    assert_eq!(
        process_tool_result(
            &config("cat >/dev/null; exit 1"),
            &json!({}),
            dir.path(),
            pending()
        )
        .await,
        None
    );
}

#[tokio::test]
async fn processor_enforces_input_and_output_caps() {
    let dir = tempfile::tempdir().unwrap();
    let mut input_config = config("touch spawned");
    input_config.max_input_bytes = 1;
    assert_eq!(
        process_tool_result(
            &input_config,
            &json!({"text": "too large"}),
            dir.path(),
            pending()
        )
        .await,
        None
    );
    assert!(!dir.path().join("spawned").exists());
    let mut output_config = config("cat >/dev/null; printf '0123456789'; sleep 30");
    output_config.max_output_bytes = 4;
    let started = Instant::now();
    assert_eq!(
        process_tool_result(&output_config, &json!({}), dir.path(), pending()).await,
        None
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn processor_timeout_and_cancellation_reap_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config("echo $$ > pid; cat >/dev/null; sleep 30");
    config.timeout_ms = 100;
    let started = Instant::now();
    assert_eq!(
        process_tool_result(&config, &json!({}), dir.path(), pending()).await,
        None
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    #[cfg(target_os = "linux")]
    {
        let pid = std::fs::read_to_string(dir.path().join("pid")).unwrap();
        assert!(!Path::new("/proc").join(pid.trim()).exists());
    }
    config.timeout_ms = 30_000;
    let started = Instant::now();
    assert_eq!(
        process_tool_result(
            &config,
            &json!({}),
            dir.path(),
            tokio::time::sleep(Duration::from_millis(100))
        )
        .await,
        None
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}
