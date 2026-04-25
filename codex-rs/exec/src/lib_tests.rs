use super::*;
use codex_otel::set_parent_from_w3c_trace_context;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_utils_absolute_path::test_support::PathBufExt;
use codex_utils_absolute_path::test_support::test_path_buf;
use opentelemetry::trace::TraceContextExt;
use opentelemetry::trace::TraceId;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider;
use pretty_assertions::assert_eq;
use std::time::Duration;
use tempfile::tempdir;
use tracing_opentelemetry::OpenTelemetrySpanExt;

fn test_tracing_subscriber() -> impl tracing::Subscriber + Send + Sync {
    let provider = SdkTracerProvider::builder().build();
    let tracer = provider.tracer("codex-exec-tests");
    tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer))
}

#[test]
fn exec_defaults_analytics_to_enabled() {
    assert_eq!(DEFAULT_ANALYTICS_ENABLED, true);
}

#[test]
fn exec_root_span_can_be_parented_from_trace_context() {
    let subscriber = test_tracing_subscriber();
    let _guard = tracing::subscriber::set_default(subscriber);

    let parent = codex_protocol::protocol::W3cTraceContext {
        traceparent: Some("00-00000000000000000000000000000077-0000000000000088-01".into()),
        tracestate: Some("vendor=value".into()),
    };
    let exec_span = exec_root_span();
    assert!(set_parent_from_w3c_trace_context(&exec_span, &parent));

    let trace_id = exec_span.context().span().span_context().trace_id();
    assert_eq!(
        trace_id,
        TraceId::from_hex("00000000000000000000000000000077").expect("trace id")
    );
}

#[test]
fn builds_uncommitted_review_request() {
    let args = ReviewArgs {
        uncommitted: true,
        base: None,
        commit: None,
        commit_title: None,
        prompt: None,
    };
    let request = build_review_request(&args).expect("builds uncommitted review request");

    let expected = ReviewRequest {
        target: ReviewTarget::UncommittedChanges,
        user_facing_hint: None,
    };

    assert_eq!(request, expected);
}

#[test]
fn builds_commit_review_request_with_title() {
    let args = ReviewArgs {
        uncommitted: false,
        base: None,
        commit: Some("123456789".to_string()),
        commit_title: Some("Add review command".to_string()),
        prompt: None,
    };
    let request = build_review_request(&args).expect("builds commit review request");

    let expected = ReviewRequest {
        target: ReviewTarget::Commit {
            sha: "123456789".to_string(),
            title: Some("Add review command".to_string()),
        },
        user_facing_hint: None,
    };

    assert_eq!(request, expected);
}

#[test]
fn builds_custom_review_request_trims_prompt() {
    let args = ReviewArgs {
        uncommitted: false,
        base: None,
        commit: None,
        commit_title: None,
        prompt: Some("  custom review instructions  ".to_string()),
    };
    let request = build_review_request(&args).expect("builds custom review request");

    let expected = ReviewRequest {
        target: ReviewTarget::Custom {
            instructions: "custom review instructions".to_string(),
        },
        user_facing_hint: None,
    };

    assert_eq!(request, expected);
}

#[test]
fn decode_prompt_bytes_strips_utf8_bom() {
    let input = [0xEF, 0xBB, 0xBF, b'h', b'i', b'\n'];

    let out = decode_prompt_bytes(&input).expect("decode utf-8 with BOM");

    assert_eq!(out, "hi\n");
}

#[test]
fn decode_prompt_bytes_decodes_utf16le_bom() {
    // UTF-16LE BOM + "hi\n"
    let input = [0xFF, 0xFE, b'h', 0x00, b'i', 0x00, b'\n', 0x00];

    let out = decode_prompt_bytes(&input).expect("decode utf-16le with BOM");

    assert_eq!(out, "hi\n");
}

#[test]
fn decode_prompt_bytes_decodes_utf16be_bom() {
    // UTF-16BE BOM + "hi\n"
    let input = [0xFE, 0xFF, 0x00, b'h', 0x00, b'i', 0x00, b'\n'];

    let out = decode_prompt_bytes(&input).expect("decode utf-16be with BOM");

    assert_eq!(out, "hi\n");
}

#[test]
fn decode_prompt_bytes_rejects_utf32le_bom() {
    // UTF-32LE BOM + "hi\n"
    let input = [
        0xFF, 0xFE, 0x00, 0x00, b'h', 0x00, 0x00, 0x00, b'i', 0x00, 0x00, 0x00, b'\n', 0x00, 0x00,
        0x00,
    ];

    let err = decode_prompt_bytes(&input).expect_err("utf-32le should be rejected");

    assert_eq!(
        err,
        PromptDecodeError::UnsupportedBom {
            encoding: "UTF-32LE"
        }
    );
}

#[test]
fn decode_prompt_bytes_rejects_utf32be_bom() {
    // UTF-32BE BOM + "hi\n"
    let input = [
        0x00, 0x00, 0xFE, 0xFF, 0x00, 0x00, 0x00, b'h', 0x00, 0x00, 0x00, b'i', 0x00, 0x00, 0x00,
        b'\n',
    ];

    let err = decode_prompt_bytes(&input).expect_err("utf-32be should be rejected");

    assert_eq!(
        err,
        PromptDecodeError::UnsupportedBom {
            encoding: "UTF-32BE"
        }
    );
}

#[test]
fn decode_prompt_bytes_rejects_invalid_utf8() {
    // Invalid UTF-8 sequence: 0xC3 0x28
    let input = [0xC3, 0x28];

    let err = decode_prompt_bytes(&input).expect_err("invalid utf-8 should fail");

    assert_eq!(err, PromptDecodeError::InvalidUtf8 { valid_up_to: 0 });
}

#[test]
fn prompt_with_stdin_context_wraps_stdin_block() {
    let combined = prompt_with_stdin_context("Summarize this concisely", "my output");

    assert_eq!(
        combined,
        "Summarize this concisely\n\n<stdin>\nmy output\n</stdin>"
    );
}

#[test]
fn prompt_with_stdin_context_preserves_trailing_newline() {
    let combined = prompt_with_stdin_context("Summarize this concisely", "my output\n");

    assert_eq!(
        combined,
        "Summarize this concisely\n\n<stdin>\nmy output\n</stdin>"
    );
}

#[test]
fn lagged_event_warning_message_is_explicit() {
    assert_eq!(
        lagged_event_warning_message(/*skipped*/ 7),
        "in-process app-server event stream lagged; dropped 7 events".to_string()
    );
}

#[tokio::test]
async fn resume_lookup_model_providers_filters_only_last_lookup() {
    let codex_home = tempdir().expect("create temp codex home");
    let cwd = tempdir().expect("create temp cwd");
    let mut config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .fallback_cwd(Some(cwd.path().to_path_buf()))
        .build()
        .await
        .expect("build default config");
    config.model_provider_id = "test-provider".to_string();

    let last_args = crate::cli::ResumeArgs {
        session_id: None,
        last: true,
        all: false,
        images: vec![],
        prompt: None,
    };
    let named_args = crate::cli::ResumeArgs {
        session_id: Some("named-session".to_string()),
        last: false,
        all: false,
        images: vec![],
        prompt: None,
    };

    assert_eq!(
        resume_lookup_model_providers(&config, &last_args),
        Some(vec!["test-provider".to_string()])
    );
    assert_eq!(resume_lookup_model_providers(&config, &named_args), None);
}

#[test]
fn turn_items_for_thread_returns_matching_turn_items() {
    let thread = AppServerThread {
        id: "thread-1".to_string(),
        forked_from_id: None,
        preview: String::new(),
        ephemeral: false,
        model_provider: "openai".to_string(),
        created_at: 0,
        updated_at: 0,
        status: codex_app_server_protocol::ThreadStatus::Idle,
        path: None,
        cwd: test_path_buf("/tmp/project").abs(),
        cli_version: "0.0.0-test".to_string(),
        source: codex_app_server_protocol::SessionSource::Exec,
        agent_nickname: None,
        agent_role: None,
        git_info: None,
        name: None,
        turns: vec![
            codex_app_server_protocol::Turn {
                id: "turn-1".to_string(),
                items: vec![AppServerThreadItem::AgentMessage {
                    id: "msg-1".to_string(),
                    text: "hello".to_string(),
                    phase: None,
                    memory_citation: None,
                }],
                status: codex_app_server_protocol::TurnStatus::Completed,
                error: None,
                started_at: None,
                completed_at: None,
                duration_ms: None,
            },
            codex_app_server_protocol::Turn {
                id: "turn-2".to_string(),
                items: vec![AppServerThreadItem::Plan {
                    id: "plan-1".to_string(),
                    text: "ship it".to_string(),
                }],
                status: codex_app_server_protocol::TurnStatus::Completed,
                error: None,
                started_at: None,
                completed_at: None,
                duration_ms: None,
            },
        ],
    };

    assert_eq!(
        turn_items_for_thread(&thread, "turn-1"),
        Some(vec![AppServerThreadItem::AgentMessage {
            id: "msg-1".to_string(),
            text: "hello".to_string(),
            phase: None,
            memory_citation: None,
        }])
    );
    assert_eq!(turn_items_for_thread(&thread, "missing-turn"), None);
}

#[test]
fn should_backfill_turn_completed_items_skips_ephemeral_threads() {
    let notification =
        ServerNotification::TurnCompleted(codex_app_server_protocol::TurnCompletedNotification {
            thread_id: "thread-1".to_string(),
            turn: codex_app_server_protocol::Turn {
                id: "turn-1".to_string(),
                items: Vec::new(),
                status: codex_app_server_protocol::TurnStatus::Completed,
                error: None,
                started_at: None,
                completed_at: None,
                duration_ms: None,
            },
        });

    assert!(!should_backfill_turn_completed_items(
        /*thread_ephemeral*/ true,
        &notification
    ));
}

#[test]
fn canceled_mcp_server_elicitation_response_uses_cancel_action() {
    let value = canceled_mcp_server_elicitation_response()
        .expect("mcp elicitation cancel response should serialize");
    let response: McpServerElicitationRequestResponse =
        serde_json::from_value(value).expect("cancel response should deserialize");

    assert_eq!(
        response,
        McpServerElicitationRequestResponse {
            action: McpServerElicitationAction::Cancel,
            content: None,
            meta: None,
        }
    );
}

#[tokio::test]
async fn thread_start_params_include_review_policy_when_review_policy_is_manual_only() {
    let codex_home = tempdir().expect("create temp codex home");
    let cwd = tempdir().expect("create temp cwd");
    let config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .harness_overrides(ConfigOverrides {
            approvals_reviewer: Some(ApprovalsReviewer::User),
            ..Default::default()
        })
        .fallback_cwd(Some(cwd.path().to_path_buf()))
        .build()
        .await
        .expect("build config with manual-only review policy");

    let params = thread_start_params_from_config(&config);

    assert_eq!(
        params.approvals_reviewer,
        Some(codex_app_server_protocol::ApprovalsReviewer::User)
    );
    assert_eq!(params.sandbox, None);
    assert_eq!(
        params.permission_profile,
        Some(config.permissions.permission_profile().into())
    );
}

#[tokio::test]
async fn thread_start_params_include_review_policy_when_auto_review_is_enabled() {
    let codex_home = tempdir().expect("create temp codex home");
    let cwd = tempdir().expect("create temp cwd");
    let config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .harness_overrides(ConfigOverrides {
            approvals_reviewer: Some(ApprovalsReviewer::AutoReview),
            ..Default::default()
        })
        .fallback_cwd(Some(cwd.path().to_path_buf()))
        .build()
        .await
        .expect("build config with guardian review policy");

    let params = thread_start_params_from_config(&config);

    assert_eq!(
        params.approvals_reviewer,
        Some(codex_app_server_protocol::ApprovalsReviewer::AutoReview)
    );
}

#[tokio::test]
async fn thread_start_params_include_developer_instructions_from_config() {
    let codex_home = tempdir().expect("create temp codex home");
    let cwd = tempdir().expect("create temp cwd");
    let config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .harness_overrides(ConfigOverrides {
            developer_instructions: Some("ILHAE RUNTIME LOOP STATE".to_string()),
            ..Default::default()
        })
        .fallback_cwd(Some(cwd.path().to_path_buf()))
        .build()
        .await
        .expect("build config with developer instructions");

    let params = thread_start_params_from_config(&config);

    assert_eq!(
        params.developer_instructions.as_deref(),
        Some("ILHAE RUNTIME LOOP STATE")
    );
}

#[test]
fn session_configured_from_thread_response_uses_review_policy_from_response() {
    let response = ThreadStartResponse {
        thread: codex_app_server_protocol::Thread {
            id: "67e55044-10b1-426f-9247-bb680e5fe0c8".to_string(),
            forked_from_id: None,
            preview: String::new(),
            ephemeral: false,
            model_provider: "openai".to_string(),
            created_at: 0,
            updated_at: 0,
            status: codex_app_server_protocol::ThreadStatus::Idle,
            path: Some(PathBuf::from("/tmp/rollout.jsonl")),
            cwd: test_path_buf("/tmp").abs(),
            cli_version: "0.0.0".to_string(),
            source: codex_app_server_protocol::SessionSource::Cli,
            agent_nickname: None,
            agent_role: None,
            git_info: None,
            name: Some("thread".to_string()),
            turns: vec![],
        },
        model: "gpt-5.4".to_string(),
        model_provider: "openai".to_string(),
        service_tier: None,
        cwd: test_path_buf("/tmp").abs(),
        instruction_sources: Vec::new(),
        approval_policy: codex_app_server_protocol::AskForApproval::OnRequest,
        approvals_reviewer: codex_app_server_protocol::ApprovalsReviewer::AutoReview,
        sandbox: codex_app_server_protocol::SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            read_only_access: codex_app_server_protocol::ReadOnlyAccess::FullAccess,
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        },
        permission_profile: Some(
            codex_protocol::models::PermissionProfile::from_legacy_sandbox_policy(
                &codex_protocol::protocol::SandboxPolicy::new_workspace_write_policy(),
                &test_path_buf("/tmp"),
            )
            .into(),
        ),
        reasoning_effort: None,
    };

    let event = session_configured_from_thread_start_response(&response)
        .expect("build bootstrap session configured event");

    assert_eq!(event.approvals_reviewer, ApprovalsReviewer::AutoReview);
}

// Tests from HEAD

#[test]
fn should_process_notification_ignores_followup_turns_without_autonomous_follow() {
    let notification = ServerNotification::TurnStarted(TurnStartedNotification {
        thread_id: "thread-1".to_string(),
        turn: codex_app_server_protocol::Turn {
            id: "turn-2".to_string(),
            items: Vec::new(),
            status: codex_app_server_protocol::TurnStatus::InProgress,
            error: None,
            started_at: None,
            completed_at: None,
            duration_ms: None,
        },
    });

    assert!(!should_process_notification(
        &notification,
        "thread-1",
        "turn-1",
        false,
    ));
}

#[test]
fn should_process_notification_accepts_followup_turns_when_autonomous_follow_is_enabled() {
    let notification =
        ServerNotification::ItemCompleted(codex_app_server_protocol::ItemCompletedNotification {
            item: AppServerThreadItem::AgentMessage {
                id: "msg-2".to_string(),
                text: "next turn".to_string(),
                phase: None,
                memory_citation: None,
            },
            thread_id: "thread-1".to_string(),
            turn_id: "turn-2".to_string(),
        });

    assert!(should_process_notification(
        &notification,
        "thread-1",
        "turn-1",
        true,
    ));
}

#[test]
fn should_process_notification_accepts_follow_on_turn_for_same_thread() {
    let notification = ServerNotification::TurnStarted(TurnStartedNotification {
        thread_id: "thread-1".to_string(),
        turn: codex_app_server_protocol::Turn {
            id: "turn-2".to_string(),
            items: Vec::new(),
            status: codex_app_server_protocol::TurnStatus::InProgress,
            error: None,
            started_at: None,
            completed_at: None,
            duration_ms: None,
        },
    });

    assert!(
        should_process_notification(&notification, "thread-1", "turn-1", true),
        "exec should keep tracking same-thread follow-on turns so autonomous/headless loops can continue"
    );
}

#[test]
fn turn_items_need_backfill_when_completion_payload_is_empty() {
    assert!(turn_items_need_backfill(&[]));
}

#[test]
fn turn_items_need_backfill_when_completion_payload_contains_in_progress_item() {
    let items = vec![AppServerThreadItem::CommandExecution {
        id: "cmd-1".to_string(),
        command: "find /tmp -name thing".to_string(),
        aggregated_output: Some(String::new()),
        exit_code: None,
        status: codex_app_server_protocol::CommandExecutionStatus::InProgress,
        duration_ms: None,
        cwd: test_path_buf("/tmp").abs(),
        process_id: None,
        source: codex_app_server_protocol::CommandExecutionSource::UserShell,
        command_actions: vec![codex_app_server_protocol::CommandAction::Unknown {
            command: "find /tmp -name thing".to_string(),
        }],
    }];

    assert!(turn_items_need_backfill(items.as_slice()));
}

#[test]
fn turn_items_need_backfill_is_false_when_payload_already_has_completed_items() {
    let items = vec![
        AppServerThreadItem::CommandExecution {
            id: "cmd-1".to_string(),
            command: "echo done".to_string(),
            aggregated_output: Some("done".to_string()),
            exit_code: Some(0),
            status: codex_app_server_protocol::CommandExecutionStatus::Completed,
            duration_ms: Some(5),
            cwd: test_path_buf("/tmp").abs(),
            process_id: None,
            source: codex_app_server_protocol::CommandExecutionSource::UserShell,
            command_actions: vec![codex_app_server_protocol::CommandAction::Unknown {
                command: "echo done".to_string(),
            }],
        },
        AppServerThreadItem::AgentMessage {
            id: "msg-1".to_string(),
            text: "완료했습니다.".to_string(),
            phase: None,
            memory_citation: None,
        },
    ];

    assert!(!turn_items_need_backfill(items.as_slice()));
}

#[test]
fn decide_exec_autonomy_followup_stops_when_max_turns_reached() {
    let decision = decide_exec_autonomy_followup(
        ExecAutonomySettings {
            max_turns: 2,
            timebox: Duration::from_secs(600),
        },
        Duration::from_secs(5),
        2,
        None,
        0,
        "원래 작업",
        &[AppServerThreadItem::AgentMessage {
            id: "msg-1".to_string(),
            text: "다음 단계로 진행하겠습니다.".to_string(),
            phase: None,
            memory_citation: None,
        }],
    );

    assert_eq!(
        decision,
        ExecAutonomyDecision::Stop {
            reason: "max_turns"
        }
    );
}

#[test]
fn decide_exec_autonomy_followup_stops_when_timebox_exceeded() {
    let decision = decide_exec_autonomy_followup(
        ExecAutonomySettings {
            max_turns: 5,
            timebox: Duration::from_secs(60),
        },
        Duration::from_secs(61),
        1,
        None,
        0,
        "원래 작업",
        &[AppServerThreadItem::AgentMessage {
            id: "msg-1".to_string(),
            text: "계속 진행하겠습니다.".to_string(),
            phase: None,
            memory_citation: None,
        }],
    );

    assert_eq!(decision, ExecAutonomyDecision::Stop { reason: "timebox" });
}

#[test]
fn decide_exec_autonomy_followup_stops_when_agent_reports_completion() {
    let decision = decide_exec_autonomy_followup(
        ExecAutonomySettings {
            max_turns: 5,
            timebox: Duration::from_secs(600),
        },
        Duration::from_secs(5),
        1,
        None,
        0,
        "원래 작업",
        &[AppServerThreadItem::AgentMessage {
            id: "msg-1".to_string(),
            text: "모든 작업이 완료되었습니다".to_string(),
            phase: None,
            memory_citation: None,
        }],
    );

    assert_eq!(
        decision,
        ExecAutonomyDecision::Stop {
            reason: "completed"
        }
    );
}

#[test]
fn decide_exec_autonomy_followup_stops_when_progress_stalls() {
    let signature = exec_autonomy_progress_signature("같은 상태");
    let decision = decide_exec_autonomy_followup(
        ExecAutonomySettings {
            max_turns: 5,
            timebox: Duration::from_secs(600),
        },
        Duration::from_secs(5),
        2,
        Some(signature),
        EXEC_AUTONOMY_STALLED_TURN_LIMIT - 1,
        "원래 작업",
        &[AppServerThreadItem::AgentMessage {
            id: "msg-1".to_string(),
            text: "같은 상태".to_string(),
            phase: None,
            memory_citation: None,
        }],
    );

    assert_eq!(decision, ExecAutonomyDecision::Stop { reason: "stalled" });
}

#[test]
fn decide_exec_autonomy_followup_continues_with_followup_prompt_for_new_progress() {
    let decision = decide_exec_autonomy_followup(
        ExecAutonomySettings {
            max_turns: 5,
            timebox: Duration::from_secs(600),
        },
        Duration::from_secs(5),
        1,
        None,
        0,
        "STEP1, STEP2, summary를 모두 완료해야 한다.",
        &[AppServerThreadItem::AgentMessage {
            id: "msg-1".to_string(),
            text: "STEP1 파일을 만들었고 이제 STEP2를 해야 합니다.".to_string(),
            phase: None,
            memory_citation: None,
        }],
    );

    match decision {
        ExecAutonomyDecision::Continue {
            reason,
            progress_signature,
            stalled_turns,
            followup_prompt,
        } => {
            assert_eq!(reason, "progressed");
            assert_eq!(stalled_turns, 0);
            assert_eq!(
                progress_signature,
                exec_autonomy_progress_signature("STEP1 파일을 만들었고 이제 STEP2를 해야 합니다.")
            );
            assert!(followup_prompt.contains("STEP1 파일을 만들었고 이제 STEP2를 해야 합니다."));
            assert!(followup_prompt.contains("STEP1, STEP2, summary를 모두 완료해야 한다."));
            assert!(followup_prompt.contains("실제 tool을 호출"));
        }
        other => panic!("expected continue decision, got {other:?}"),
    }
}

#[test]
fn decide_exec_autonomy_followup_continues_with_stalled_retry_reason_before_limit() {
    let signature = exec_autonomy_progress_signature("같은 상태");
    let decision = decide_exec_autonomy_followup(
        ExecAutonomySettings {
            max_turns: 5,
            timebox: Duration::from_secs(600),
        },
        Duration::from_secs(5),
        2,
        Some(signature),
        0,
        "원래 작업",
        &[AppServerThreadItem::AgentMessage {
            id: "msg-1".to_string(),
            text: "같은 상태".to_string(),
            phase: None,
            memory_citation: None,
        }],
    );

    match decision {
        ExecAutonomyDecision::Continue {
            reason,
            stalled_turns,
            ..
        } => {
            assert_eq!(reason, "stalled_retry");
            assert_eq!(stalled_turns, 1);
        }
        other => panic!("expected continue decision, got {other:?}"),
    }
}

#[test]
fn extract_pseudo_exec_command_reads_json_tool_stub() {
    let progress = r#"{
  "tool": "functions.exec_command",
  "arguments": {
    "cmd": "echo STEP1 > /tmp/file"
  }
}"#;

    assert_eq!(
        extract_pseudo_exec_command(progress).as_deref(),
        Some("echo STEP1 > /tmp/file")
    );
}

#[test]
fn build_exec_autonomy_followup_prompt_prioritizes_stubbed_exec_command() {
    let progress = r#"{
  "tool": "functions.exec_command",
  "arguments": {
    "cmd": "echo STEP1 > /tmp/file"
  }
}"#;

    let prompt = build_exec_autonomy_followup_prompt_with_root(progress, "원래 작업 조건");

    assert!(prompt.contains("실제 실행 대신 exec_command tool 호출 JSON만 출력"));
    assert!(prompt.contains("echo STEP1 > /tmp/file"));
    assert!(prompt.contains("원래 작업 조건"));
    assert!(prompt.contains("아직 실행하지 않은 다음 단계로 넘어가지 마세요"));
}
