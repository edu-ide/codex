use super::*;
use codex_tools::ModelTextReplacement;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn processor_exec_snapshot_is_raw_and_model_replacement_keeps_native_headers() {
    let payload = ToolPayload::Function {
        arguments: "{}".to_string(),
    };
    let original = ExecCommandToolOutput {
        event_call_id: "native-event".to_string(),
        chunk_id: "chunk".to_string(),
        wall_time: Duration::from_millis(1250),
        raw_output: b"raw-before-truncation".to_vec(),
        truncation_policy: TruncationPolicy::Tokens(1000),
        max_output_tokens: Some(64),
        process_id: Some(123),
        exit_code: None,
        original_token_count: Some(42),
        output_omitted_bytes: NonZeroUsize::new(100),
        hook_command: Some("command".to_string()),
    };
    assert_eq!(
        serde_json::to_value(original.raw_text_snapshot().unwrap()).unwrap(),
        json!({
            "text":"raw-before-truncation", "is_error":false,
            "completeness":{"state":"partial","omitted_bytes":100},
            "process_id":123,"chunk_id":"chunk"
        })
    );
    let typed = original.code_mode_result(&payload);
    let hook = original.post_tool_use_response("call", &payload);
    let wrapped = ModelTextReplacement::new(Box::new(original), "short".to_string());
    let ResponseInputItem::FunctionCallOutput { output, .. } =
        wrapped.to_response_item("call", &payload)
    else {
        panic!("function output expected")
    };
    assert_eq!(output, FunctionCallOutputPayload {
        body: FunctionCallOutputBody::Text("Chunk ID: chunk\nWall time: 1.2500 seconds\nProcess running with session ID 123\nOriginal token count: 42\nOutput:\nUntrusted tool data (local result processor):\nshort".to_string()),
        success: Some(true),
    });
    assert_eq!(wrapped.code_mode_result(&payload), typed);
    assert_eq!(wrapped.post_tool_use_id("call"), "native-event");
    assert_eq!(wrapped.post_tool_use_response("call", &payload), hook);
}

#[test]
fn processor_mcp_replacement_preserves_media_typed_json_and_error_status() {
    let payload = ToolPayload::Function {
        arguments: "{}".to_string(),
    };
    let original = McpToolOutput {
        result: CallToolResult {
            content: vec![
                json!({"type":"text","text":"original"}),
                json!({"type":"image","mimeType":"image/png","data":"AAA"}),
                json!({"type":"text","text":"more text"}),
                json!({"type":"audio","mimeType":"audio/wav","data":"BBB"}),
                json!({"type":"text","text":"opaque","_meta":{"codex/encryptedContent":true}}),
            ],
            structured_content: Some(json!({"structured":"original"})),
            is_error: Some(true),
            meta: Some(json!({"private":"metadata"})),
        },
        tool_input: json!({"input":true}),
        wall_time: Duration::from_millis(500),
        original_image_detail_supported: false,
        truncation_policy: TruncationPolicy::Bytes(4096),
    };
    assert_eq!(
        original.raw_text_snapshot().unwrap(),
        ToolResultSnapshot::complete("original\nmore text".to_string(), true)
    );
    let typed = original.code_mode_result(&payload);
    let hook = original.post_tool_use_response("call", &payload);
    let external_context = original.contains_external_context();
    let wrapped = ModelTextReplacement::new(Box::new(original), "compact".to_string());
    let ResponseInputItem::FunctionCallOutput { output, .. } =
        wrapped.to_response_item("call", &payload)
    else {
        panic!("function output expected")
    };
    assert_eq!(
        output,
        FunctionCallOutputPayload {
            body: FunctionCallOutputBody::ContentItems(vec![
                FunctionCallOutputContentItem::InputText {
                    text: "Wall time: 0.5000 seconds\nOutput:".to_string()
                },
                FunctionCallOutputContentItem::InputText {
                    text: "Untrusted tool data (local result processor):\ncompact".to_string()
                },
                FunctionCallOutputContentItem::InputImage {
                    image_url: "data:image/png;base64,AAA".to_string(),
                    detail: Some(codex_protocol::models::DEFAULT_IMAGE_DETAIL)
                },
                FunctionCallOutputContentItem::InputAudio {
                    audio_url: "data:audio/wav;base64,BBB".to_string(),
                },
                FunctionCallOutputContentItem::EncryptedContent {
                    encrypted_content: "opaque".to_string(),
                },
            ]),
            success: Some(false),
        }
    );
    assert_eq!(wrapped.code_mode_result(&payload), typed);
    assert_eq!(wrapped.post_tool_use_response("call", &payload), hook);
    assert!(!wrapped.success_for_logging());
    assert_eq!(wrapped.contains_external_context(), external_context);
}
