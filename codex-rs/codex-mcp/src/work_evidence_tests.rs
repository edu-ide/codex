use super::*;
use codex_rmcp_client::InProcessTransportFactory;
use futures::FutureExt;
use pretty_assertions::assert_eq;
use rmcp::ServerHandler;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::model::ClientCapabilities;
use rmcp::model::ContentBlock;
use rmcp::model::Implementation;
use rmcp::model::InitializeRequestParams;
use rmcp::model::ProtocolVersion;
use rmcp::model::ServerCapabilities;
use rmcp::model::ServerInfo;
use rmcp::service::RequestContext;
use rmcp::service::RoleServer;
use serde_json::json;
use std::sync::Mutex;
use tempfile::tempdir;

fn observed_result() -> CallToolResult {
    CallToolResult {
        content: vec![json!({"type":"text","text":"source result"})],
        structured_content: None,
        is_error: Some(false),
        meta: Some(json!({"ugot/work-evidence": [{
            "source_kind":"office", "resource_uri":"office://sample-book",
            "title":"Test workbook", "content":"A1: example", "locator":"sheet:Summary!A1",
            "account_id":"test-account", "profile_id":"test-profile",
            "observed_at":"2020-01-01T00:00:00Z", "status":"observed"
        }]})),
    }
}

fn request_meta() -> Value {
    json!({"threadId":"thread-test", "callId":"call-test"})
}

#[test]
fn observation_uses_exact_thread_and_provider_scope() {
    let events = events_from_result(
        "office",
        "get_sheet_data",
        Some(&request_meta()),
        &observed_result(),
    );
    let mut expected = observed_result().meta.unwrap()[ENVELOPE_KEY][0].clone();
    expected["work_key"] = json!("thread-test");
    expected["operation_id"] = json!("call-test");
    expected["tool_id"] = json!("office/get_sheet_data");
    expected["event_id"] = events[0]["event_id"].clone();
    assert_eq!(events, vec![expected]);
    assert_eq!(
        events,
        events_from_result(
            "office",
            "get_sheet_data",
            Some(&request_meta()),
            &observed_result()
        )
    );
}

#[test]
fn work_context_overrides_thread_but_unscoped_calls_are_ignored() {
    let meta = json!({"threadId":"thread-test", "ugot/work-context":{"work_key":"work-explicit"}});
    let events = events_from_result("office", "read_cell", Some(&meta), &observed_result());
    assert_eq!(events[0]["work_key"], json!("work-explicit"));
    for meta in [
        json!({}),
        json!({"ugot/work-context":{"work_key":"work-explicit"}}),
        json!({"threadId":"thread-test","ugot/work-context":{}}),
    ] {
        assert!(
            events_from_result("office", "read_cell", Some(&meta), &observed_result()).is_empty()
        );
    }
}

#[test]
fn invalid_scope_and_unconfirmed_success_are_not_indexed() {
    for (key, value) in [
        ("account_id", json!("*")),
        ("profile_id", json!("")),
        ("profile_id", json!(" padded ")),
        ("locator", json!({"cell":"A1"})),
        ("status", json!("success")),
        ("observed_at", json!("not-a-time")),
        ("observed_at", json!("2099-01-01T00:00:00Z")),
        ("content", json!("x".repeat(65537))),
    ] {
        let mut result = observed_result();
        result.meta.as_mut().unwrap()[ENVELOPE_KEY][0][key] = value;
        assert!(
            events_from_result("office", "read_cell", Some(&request_meta()), &result).is_empty(),
            "rejected {key}"
        );
    }
    let mut result = observed_result();
    result.is_error = Some(true);
    assert!(events_from_result("office", "read_cell", Some(&request_meta()), &result).is_empty());
    result.meta.as_mut().unwrap()[ENVELOPE_KEY][0]["status"] = json!("failed");
    assert_eq!(
        events_from_result("office", "read_cell", Some(&request_meta()), &result).len(),
        1
    );
}

#[test]
fn browser_archive_is_forwarded_exactly_and_changes_event_identity() {
    let mut result = observed_result();
    let envelope = &mut result.meta.as_mut().unwrap()[ENVELOPE_KEY][0];
    envelope["source_kind"] = json!("browser");
    envelope["browser_archive"] = json!({
        "html": "<html><body>관측한 내용</body></html>",
        "sha256": "a".repeat(64),
        "capture_kind": "sanitized_dom_html",
    });
    let expected = envelope["browser_archive"].clone();
    let events = events_from_result("browser", "read_page", Some(&request_meta()), &result);
    assert_eq!(events[0]["browser_archive"], expected);
    result.meta.as_mut().unwrap()[ENVELOPE_KEY][0]["browser_archive"]["html"] =
        json!("<html><body>Changed observation</body></html>");
    let changed = events_from_result("browser", "read_page", Some(&request_meta()), &result);
    assert_ne!(events[0]["event_id"], changed[0]["event_id"]);
}

#[test]
fn malformed_or_oversize_archive_is_not_silently_downgraded() {
    for archive in [
        json!({"html":"x", "sha256":"A".repeat(64), "capture_kind":"sanitized_dom_html"}),
        json!({"html":"x", "sha256":"a".repeat(63), "capture_kind":"sanitized_dom_html"}),
        json!({"html":"x", "sha256":"a".repeat(64), "capture_kind":"raw_html"}),
        json!({"html":"한".repeat(90000), "sha256":"a".repeat(64), "capture_kind":"sanitized_dom_html"}),
        json!({"html":"\0", "sha256":"a".repeat(64), "capture_kind":"sanitized_dom_html"}),
    ] {
        let mut result = observed_result();
        let envelope = &mut result.meta.as_mut().unwrap()[ENVELOPE_KEY][0];
        envelope["source_kind"] = json!("browser");
        envelope["browser_archive"] = archive;
        assert!(
            events_from_result("browser", "read_page", Some(&request_meta()), &result).is_empty()
        );
    }
}

#[test]
fn json_escaped_archive_survives_durable_outbox_reopen() {
    let temporary = tempdir().unwrap();
    let outbox = WorkEvidenceOutbox::new(temporary.path());
    let mut result = observed_result();
    let envelope = &mut result.meta.as_mut().unwrap()[ENVELOPE_KEY][0];
    envelope["source_kind"] = json!("browser");
    envelope["browser_archive"] = json!({
        "html": format!("<p>{}</p>", "\u{1}".repeat(MAX_BROWSER_ARCHIVE_BYTES - 7)),
        "sha256": "a".repeat(64),
        "capture_kind": "sanitized_dom_html",
    });
    let events = events_from_result("browser", "read_page", Some(&request_meta()), &result);
    assert!(serde_json::to_vec(&events[0]).unwrap().len() > 512 * 1024);
    outbox.persist(&events).unwrap();
    let reopened = WorkEvidenceOutbox::new(temporary.path());
    assert_eq!(reopened.pending().unwrap()[0].1, events[0]);
    let receipt = receipt_for_events(&events);
    assert!(!receipt.contains("browser_archive"));
    assert!(receipt.len() < 2048);
}

#[test]
fn brain_recursion_is_skipped_and_profile_ids_do_not_collide() {
    assert!(
        events_from_result(
            "brain",
            RECORD_TOOL,
            Some(&request_meta()),
            &observed_result()
        )
        .is_empty()
    );
    let events = events_from_result(
        "office",
        "read_cell",
        Some(&request_meta()),
        &observed_result(),
    );
    let mut other_profile = observed_result();
    other_profile.meta.as_mut().unwrap()[ENVELOPE_KEY][0]["profile_id"] = json!("second-profile");
    let other_events =
        events_from_result("office", "read_cell", Some(&request_meta()), &other_profile);
    assert_ne!(events[0]["event_id"], other_events[0]["event_id"]);
}

#[tokio::test]
async fn office_and_mail_archives_survive_outbox_without_model_payload_growth() {
    for (server, source_kind, capture_kind) in [
        ("office", "office", "office_document"),
        ("email", "mail", "mail_message"),
    ] {
        let temporary = tempdir().unwrap();
        let outbox = Arc::new(WorkEvidenceOutbox::new(temporary.path()));
        let mut result = observed_result();
        let envelope = &mut result.meta.as_mut().unwrap()[ENVELOPE_KEY][0];
        envelope["source_kind"] = json!(source_kind);
        let body = serde_json::to_string(&json!({
            "version": 1, "source_kind": source_kind,
            "original": { "data_base64": "a".repeat(3 * 1024 * 1024) },
            "view": {"body": "exact \"quote\"\n"},
        }))
        .unwrap();
        let archive = json!({"json": body, "sha256": "a".repeat(64), "capture_kind": capture_kind});
        envelope["source_archive"] = archive.clone();
        outbox
            .observe(server, "read", Some(&request_meta()), &mut result)
            .await;
        let reopened = WorkEvidenceOutbox::new(temporary.path());
        let records = reopened.pending().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].1["source_archive"], archive);
        assert!(serde_json::to_vec(&result.content).unwrap().len() < 4096);
        let original_id = records[0].1["event_id"].clone();
        result.meta.as_mut().unwrap()[ENVELOPE_KEY][0]["source_archive"]["json"] = json!(
            serde_json::to_string(
                &json!({"version":1,"source_kind":source_kind,"view":{"body":"changed"}})
            )
            .unwrap()
        );
        let changed = events_from_result(server, "read", Some(&request_meta()), &result);
        assert_ne!(original_id, changed[0]["event_id"]);
    }
}

#[test]
fn source_archive_rejects_mismatched_source_invalid_json_and_oversize_without_downgrade() {
    let body = serde_json::to_string(&json!({"version":1,"source_kind":"office"})).unwrap();
    for archive in [
        json!({"json":body,"sha256":"a".repeat(64),"capture_kind":"mail_message"}),
        json!({"json":body,"sha256":"bad","capture_kind":"office_document"}),
        json!({"json":"not-json","sha256":"a".repeat(64),"capture_kind":"office_document"}),
        json!({"json": "x".repeat(MAX_SOURCE_ARCHIVE_BYTES + 1),"sha256":"a".repeat(64),"capture_kind":"office_document"}),
        json!({"json":"{\"version\":1,\"source_kind\":\"mail\"}","sha256":"a".repeat(64),"capture_kind":"office_document"}),
    ] {
        let mut result = observed_result();
        result.meta.as_mut().unwrap()[ENVELOPE_KEY][0]["source_archive"] = archive;
        assert!(events_from_result("office", "read", Some(&request_meta()), &result).is_empty());
    }
}

#[test]
fn durable_outbox_reopens_without_changing_observation_or_duplicating_files() {
    let temporary = tempdir().unwrap();
    let outbox = WorkEvidenceOutbox::new(temporary.path());
    let events = events_from_result(
        "office",
        "read_cell",
        Some(&request_meta()),
        &observed_result(),
    );
    outbox.persist(&events).unwrap();
    outbox.persist(&events).unwrap();
    let reopened = WorkEvidenceOutbox::new(temporary.path());
    assert_eq!(
        reopened
            .pending()
            .unwrap()
            .into_iter()
            .map(|(_, event)| event)
            .collect::<Vec<_>>(),
        events
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = &reopened.pending().unwrap()[0].0;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&outbox.directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[tokio::test]
async fn receipt_is_added_only_after_durable_persist_and_remains_bounded() {
    let temporary = tempdir().unwrap();
    let outbox = Arc::new(WorkEvidenceOutbox::new(temporary.path()));
    let mut result = observed_result();
    outbox
        .observe("office", "read_cell", Some(&request_meta()), &mut result)
        .await;
    assert_eq!(result.content[0], observed_result().content[0]);
    let receipt: Value = serde_json::from_str(result.content[1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        receipt["ugot_work_evidence"]["items"][0]["brain_index_status"],
        json!("pending")
    );
    assert_eq!(outbox.pending().unwrap().len(), 1);

    let events = events_from_result(
        "office",
        "read_cell",
        Some(&request_meta()),
        &observed_result(),
    );
    let many_events = vec![events[0].clone(); 32];
    let receipt = receipt_for_events(&many_events);
    assert!(receipt.len() <= 2048);
    let payload: Value = serde_json::from_str(&receipt).unwrap();
    assert!(
        payload["ugot_work_evidence"]["remaining_count"]
            .as_u64()
            .unwrap()
            > 0
    );

    let failed_home = temporary.path().join("not-a-directory");
    fs::write(&failed_home, "existing file").unwrap();
    let failed_outbox = Arc::new(WorkEvidenceOutbox::new(&failed_home));
    let mut result = observed_result();
    failed_outbox
        .observe("office", "read_cell", Some(&request_meta()), &mut result)
        .await;
    assert_eq!(result.content, observed_result().content);
}

#[test]
fn delivery_requires_matching_record_ack() {
    let mut result = observed_result();
    result.structured_content =
        Some(json!({"event_id":"event-one", "inserted":false, "is_latest":true}));
    assert!(acknowledged(&result, "event-one"));
    assert!(!acknowledged(&result, "event-other"));
    result.is_error = Some(true);
    assert!(!acknowledged(&result, "event-one"));
    result.is_error = Some(false);
    result.structured_content = Some(json!({"success":true}));
    assert!(!acknowledged(&result, "event-one"));
}

#[derive(Clone)]
struct RecordingBrain {
    calls: Arc<Mutex<Vec<Value>>>,
    fail_first: Arc<AtomicBool>,
}

impl ServerHandler for RecordingBrain {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
        assert_eq!(request.name.as_ref(), RECORD_TOOL);
        let arguments = Value::Object(request.arguments.unwrap());
        self.calls.lock().unwrap().push(arguments.clone());
        if self.fail_first.swap(false, Ordering::AcqRel) {
            return Ok(rmcp::model::CallToolResult::error(vec![ContentBlock::text(
                "temporarily unavailable",
            )])
            .into());
        }
        Ok(
            rmcp::model::CallToolResult::success(vec![ContentBlock::text(
                json!({
                    "event_id":arguments["event_id"], "inserted":false, "is_latest":true
                })
                .to_string(),
            )])
            .into(),
        )
    }
}

impl InProcessTransportFactory for RecordingBrain {
    fn open(
        &self,
    ) -> futures::future::BoxFuture<'static, std::io::Result<tokio::io::DuplexStream>> {
        let server = self.clone();
        async move {
            let (client_stream, server_stream) = tokio::io::duplex(4096);
            tokio::spawn(async move {
                server
                    .serve(server_stream)
                    .await
                    .unwrap()
                    .waiting()
                    .await
                    .unwrap();
            });
            Ok(client_stream)
        }
        .boxed()
    }
}

#[tokio::test]
async fn failed_brain_delivery_retries_identical_record_and_removes_only_after_ack() {
    let temporary = tempdir().unwrap();
    let outbox = Arc::new(WorkEvidenceOutbox::new(temporary.path()));
    let events = events_from_result(
        "office",
        "read_cell",
        Some(&request_meta()),
        &observed_result(),
    );
    outbox.persist(&events).unwrap();
    let server = RecordingBrain {
        calls: Arc::new(Mutex::new(Vec::new())),
        fail_first: Arc::new(AtomicBool::new(true)),
    };
    let client = Arc::new(
        RmcpClient::new_in_process_client(Arc::new(server.clone()))
            .await
            .unwrap(),
    );
    client
        .initialize(
            InitializeRequestParams::new(
                ClientCapabilities::default(),
                Implementation::new("evidence-test", "0.0.0"),
            )
            .with_protocol_version(ProtocolVersion::V_2025_06_18),
            Some(Duration::from_secs(5)),
            Box::new(|_, _| async { Err(anyhow::anyhow!("unexpected elicitation")) }.boxed()),
        )
        .await
        .unwrap();
    assert!(outbox.claim_delivery());
    tokio::time::timeout(Duration::from_secs(8), outbox.deliver(client))
        .await
        .unwrap();
    assert_eq!(
        *server.calls.lock().unwrap(),
        vec![events[0].clone(), events[0].clone()]
    );
    assert!(outbox.pending().unwrap().is_empty());
}

#[test]
fn untrusted_servers_and_mismatched_source_kinds_are_ignored() {
    for server in ["untrusted", "browser", "mail", "brain"] {
        assert!(
            events_from_result(server, "read", Some(&request_meta()), &observed_result())
                .is_empty()
        );
    }
    for server in ["office", "doc-mcp", "excel-mcp"] {
        assert_eq!(
            events_from_result(server, "read", Some(&request_meta()), &observed_result()).len(),
            1
        );
    }
}
