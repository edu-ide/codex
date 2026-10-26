use codex_ilhae::types::MemoryToolDreamPromoteInput;
use serde_json::json;

#[test]
fn dream_promote_preserves_reviewed_candidate() {
    let request = json!({
        "candidate": {
            "id": "reviewed-memory",
            "title": "Reviewed memory",
            "content": "An observation supported by the source chunk.",
            "tags": ["reviewed"],
            "evidence": [{
                "chunk_id": 7,
                "hash": "source-chunk-hash",
                "quote": "An observation supported by the source chunk."
            }],
            "review": {
                "confidence": 0.9,
                "helpful_count": 1,
                "harmful_count": 0,
                "reusable": true,
                "rationale": "The literal source supports this reusable observation."
            }
        }
    });

    let input: MemoryToolDreamPromoteInput = serde_json::from_value(request.clone()).unwrap();

    pretty_assertions::assert_eq!(serde_json::to_value(input).unwrap(), request);
}

#[test]
fn dream_promote_rejects_unreviewed_legacy_request() {
    let request = json!({
        "chunk_ids": [7],
        "ki_id": "unreviewed-memory",
        "title": "Unreviewed memory",
        "content": "An unreviewed observation.",
        "tags": []
    });

    let error = serde_json::from_value::<MemoryToolDreamPromoteInput>(request).unwrap_err();

    assert!(error.to_string().contains("missing field `candidate`"));
}
