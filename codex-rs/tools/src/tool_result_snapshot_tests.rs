use super::*;
use crate::JsonToolOutput;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn model_replacement_preserves_typed_results_success_and_provenance() {
    let value = json!({"structured": {"count": 3}});
    let original = JsonToolOutput::with_success(value.clone(), Some(false)).with_external_context();
    let preview = original.log_preview();
    let payload = ToolPayload::Function {
        arguments: "{}".to_string(),
    };
    let wrapped = ModelTextReplacement::new(Box::new(original), "compact".to_string());
    assert_eq!(
        wrapped.to_response_item("call", &payload),
        ResponseInputItem::FunctionCallOutput {
            call_id: "call".to_string(),
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::Text(
                    "Untrusted tool data (local result processor):\ncompact".to_string()
                ),
                success: Some(false)
            },
        }
    );
    assert_eq!(wrapped.code_mode_result(&payload), value);
    assert_eq!(wrapped.log_preview(), preview);
    assert!(!wrapped.success_for_logging());
    assert!(wrapped.contains_external_context());
}

#[test]
fn model_replacement_cannot_inject_unbounded_context() {
    let payload = ToolPayload::Custom {
        input: String::new(),
    };
    for replacement in [
        "large ".repeat(50_000),
        "한글🙂".repeat(50_000),
        "x".repeat(8_000),
    ] {
        let wrapped = ModelTextReplacement::new(
            Box::new(JsonToolOutput::new(json!("original"))),
            replacement,
        );
        let ResponseInputItem::CustomToolCallOutput { output, .. } =
            wrapped.to_response_item("call", &payload)
        else {
            panic!("custom output expected")
        };
        let text = output.body.to_text().unwrap();
        assert!(text.len() <= 8_000);
        assert!(text.starts_with(
            "Untrusted tool data (local result processor):\nWarning: truncated output"
        ));
        assert_eq!(wrapped.code_mode_result(&payload), json!("original"));
    }
}
