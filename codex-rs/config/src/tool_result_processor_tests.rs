use super::*;
use pretty_assertions::assert_eq;

#[test]
fn defaults_and_invalid_limits() {
    let config: ToolResultProcessorConfig =
        serde_json::from_value(serde_json::json!({"command":["processor"]})).unwrap();
    assert_eq!(
        config,
        ToolResultProcessorConfig {
            command: vec!["processor".to_string()],
            timeout_ms: 2000,
            max_input_bytes: 32 * 1024 * 1024,
            max_output_bytes: 64 * 1024,
        }
    );
    for value in [
        serde_json::json!({"command":[]}),
        serde_json::json!({"command":["processor"],"timeout_ms":0}),
        serde_json::json!({"command":["processor"],"max_input_bytes":67108865}),
        serde_json::json!({"command":["processor"],"max_output_bytes":262145}),
    ] {
        assert!(serde_json::from_value::<ToolResultProcessorConfig>(value).is_err());
    }
}
