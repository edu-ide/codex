use super::*;
use brain_rs::schedule::CalendarEventInput;

fn receipt() -> BrowserBatchDispatch {
    BrowserBatchDispatch {
        schedule_id: "calendar".into(),
        occurrence_id: "calendar:2026-10-02T12:00:00Z".into(),
        browser_batch_id: "72f62885-91b2-4500-8f86-51caf369c488".into(),
        browser_run_id: "59c6f313-9cce-52ed-809d-471c719e20ea".into(),
        status: BrowserBatchStatus::Dispatching,
        result_summary: None,
        bridge_updated_at: None,
        space_sync_pending: false,
    }
}

fn result(structured: Value, is_error: bool) -> CallToolResult {
    let mut result = CallToolResult::structured(structured);
    result.is_error = Some(is_error);
    result
}

#[test]
fn observations_validate_occurrence_identity_and_preserve_handoffs() {
    let claimed = receipt();
    let wire = json!({"id":claimed.browser_run_id,"runId":claimed.browser_run_id,
        "sourceBatchId":claimed.browser_batch_id,"status":"waiting_agent",
        "updatedAt":"2026-10-02T21:01:00+09:00","summary":"Agent judgment pending"});
    let mut expected = claimed.clone();
    expected.status = BrowserBatchStatus::WaitingAgent;
    expected.bridge_updated_at = Some("2026-10-02T12:01:00.000Z".into());
    expected.result_summary = Some("Agent judgment pending".into());
    assert_eq!(
        observation(
            claimed.clone(),
            /*enabled*/ true,
            result(wire.clone(), /*is_error*/ false)
        )
        .unwrap(),
        expected
    );
    for field in ["id", "runId", "sourceBatchId"] {
        let mut wrong = wire.clone();
        wrong[field] = json!("another-occurrence");
        assert!(
            observation(
                claimed.clone(),
                /*enabled*/ true,
                result(wrong, /*is_error*/ false)
            )
            .is_err()
        );
    }
}

#[test]
fn uncertain_dispatch_stays_pending_while_stop_and_invalid_sources_settle() {
    for (code, enabled, status) in [
        ("batch_busy", true, BrowserBatchStatus::Dispatching),
        ("disconnected", true, BrowserBatchStatus::Dispatching),
        ("stopped", true, BrowserBatchStatus::Cancelled),
        ("run_not_found", false, BrowserBatchStatus::Cancelled),
        ("run_not_found", true, BrowserBatchStatus::Failed),
        ("dispatch_conflict", true, BrowserBatchStatus::Failed),
    ] {
        let observed = observation(
            receipt(),
            enabled,
            result(
                json!({"error":{"code":code,"message":"Observed refusal"}}),
                /*is_error*/ true,
            ),
        )
        .unwrap();
        assert_eq!(observed.status, status);
        assert_eq!(observed.result_summary.is_some(), status.is_terminal());
    }
}

#[test]
fn browser_stdio_config_preserves_explicit_profile_environment_and_rejects_missing_transport() {
    let config: toml::Value = r#"
        [mcp_servers.browser]
        command = "ugot-browser"
        args = ["mcp"]
        [mcp_servers.browser.env]
        UGOT_BROWSER_PROFILE = "authorized-profile"
    "#
    .parse()
    .unwrap();
    let command = browser_command(&config).unwrap();
    let command = command.as_std();
    assert_eq!(command.get_program(), "ugot-browser");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![std::ffi::OsStr::new("mcp")]
    );
    assert_eq!(
        command.get_envs().collect::<Vec<_>>(),
        vec![(
            std::ffi::OsStr::new("UGOT_BROWSER_PROFILE"),
            Some(std::ffi::OsStr::new("authorized-profile"))
        )]
    );
    assert!(browser_command(&toml::Value::Table(Default::default())).is_err());
    let remote: toml::Value = "[mcp_servers.browser]\nurl = 'http://127.0.0.1:18701/mcp'"
        .parse()
        .unwrap();
    assert!(browser_command(&remote).is_err());
}

#[tokio::test]
async fn real_stdio_dispatch_records_fresh_run_and_disabled_calendar_cancellation() {
    let directory = tempfile::tempdir().unwrap();
    let store = ScheduleStore::new(directory.path());
    let start = (chrono::Utc::now() + chrono::Duration::seconds(1)).to_rfc3339();
    let input: CalendarEventInput = serde_json::from_value(json!({
        "title":"Isolated calendar transport test","kind":"automation",
        "start_at":start,"end_at":start,
        "timezone":"UTC","all_day":false,"reminder_minutes":[],"enabled":true,
        "recurrence":{"frequency":"none","interval":1,"weekdays":[],"exceptions":[]},
        "browser_batch_id":receipt().browser_batch_id
    }))
    .unwrap();
    let saved = store.calendar_save(/*id*/ None, input).unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        store
            .try_run_with_scope(Some("all"), Some(&saved.id))
            .unwrap()
            .len(),
        1
    );
    let claimed = store.calendar_browser_dispatches().unwrap()[0].0.clone();
    let log = directory.path().join("calls.jsonl");
    let script = r#"
        const fs = require('node:fs');
        const readline = require('node:readline');
        const [log, source] = process.argv.slice(1);
        readline.createInterface({input:process.stdin}).on('line', line => {
          const request = JSON.parse(line);
          if (request.id === undefined) return;
          let result;
          if (request.method === 'initialize') result = {protocolVersion:request.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'isolated-browser',version:'1'}};
          else if (request.method === 'tools/call') {
            fs.appendFileSync(log, JSON.stringify({ name: request.params.name, arguments: request.params.arguments }) + '\n');
            const id = request.params.arguments.run_id || request.params.arguments.id;
            result = {content:[],structuredContent:{id,runId:id,sourceBatchId:source,status:request.params.name==='batch_cancel'?'cancelled':'running',updatedAt:request.params.name==='batch_cancel'?'2026-10-02T12:02:00.000Z':'2026-10-02T12:01:00.000Z',summary:'Actual fixture result',spaceSync:{retryable:false}}};
          } else return;
          process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result})+'\n');
        });
    "#;
    let make_command = || {
        let mut command = Command::new("node");
        command
            .args(["-e", script])
            .arg(&log)
            .arg(&claimed.browser_batch_id)
            .kill_on_drop(true);
        command
    };
    dispatch_pending(
        &store,
        make_command(),
        store.calendar_browser_dispatches().unwrap(),
    )
    .await;
    let mut running = claimed.clone();
    running.status = BrowserBatchStatus::Running;
    running.bridge_updated_at = Some("2026-10-02T12:01:00.000Z".into());
    running.result_summary = Some("Actual fixture result".into());
    assert_eq!(
        store.calendar_browser_dispatches().unwrap(),
        vec![(running, true)]
    );
    let mut disabled = store.calendar_get(&saved.id).unwrap().event;
    disabled.enabled = false;
    store.calendar_save(Some(&saved.id), disabled).unwrap();
    // The same claimed UUID is cancelled, never a source batch or a fresh UUID.
    dispatch_pending(
        &store,
        make_command(),
        store.calendar_browser_dispatches().unwrap(),
    )
    .await;
    assert!(store.calendar_browser_dispatches().unwrap().is_empty());
    let cancelled = store.calendar_get(&saved.id).unwrap().execution;
    assert_eq!(cancelled.status, "cancelled");
    assert_eq!(
        cancelled.browser_run_id,
        Some(claimed.browser_run_id.clone())
    );
    let requests = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        requests,
        vec![
            json!({"name":"batch_schedule_start","arguments":{"id":claimed.browser_batch_id,"run_id":claimed.browser_run_id}}),
            json!({"name":"batch_cancel","arguments":{"id":claimed.browser_run_id}}),
        ]
    );
}
