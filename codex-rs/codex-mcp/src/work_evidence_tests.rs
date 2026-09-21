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
