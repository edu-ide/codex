use agent_client_protocol_schema::ContentBlock;
use codex_app_server_client::AppServerEvent;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::LoopLifecycleProgressNotification;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadItem;
use codex_ilhae::IlhaeLoopLifecycleNotification;
use codex_protocol::protocol::LoopLifecycleKind;
use codex_protocol::protocol::LoopLifecycleStatus;
use std::collections::BTreeMap;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub(super) struct VerificationLoopTracker {
    pub(super) item_id: String,
    pub(super) detail: String,
}

pub(super) fn acp_tool_content_items(
    content: &[agent_client_protocol_schema::ToolCallContent],
) -> Option<Vec<DynamicToolCallOutputContentItem>> {
    let items = content
        .iter()
        .filter_map(|item| match item {
            agent_client_protocol_schema::ToolCallContent::Content(content) => {
                match &content.content {
                    ContentBlock::Text(text) => Some(DynamicToolCallOutputContentItem::InputText {
                        text: text.text.clone(),
                    }),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if items.is_empty() { None } else { Some(items) }
}

fn verification_command_from_tool_call(
    tool: &str,
    arguments: &serde_json::Value,
) -> Option<String> {
    let tool_name = tool.trim().to_ascii_lowercase();
    if !(tool_name.contains("exec") || tool_name.contains("shell") || tool_name.contains("command"))
    {
        return None;
    }

    let command = arguments
        .get("cmd")
        .or_else(|| arguments.get("command"))
        .or_else(|| arguments.get("text"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let command_lower = command.to_ascii_lowercase();
    let verification_tokens = [
        "cargo test",
        "cargo clippy",
        "cargo check",
        "pytest",
        "py.test",
        "npm test",
        "pnpm test",
        "yarn test",
        "vitest",
        "jest",
        "playwright",
        "bazel test",
        "ruff check",
        "mypy",
        "tsc --noemit",
        "eslint",
    ];
    verification_tokens
        .iter()
        .any(|token| command_lower.contains(token))
        .then(|| command.to_string())
}

pub(super) fn start_verification_loop(
    event_tx: &mpsc::UnboundedSender<AppServerEvent>,
    verification_loops: &mut HashMap<String, VerificationLoopTracker>,
    tool_call_id: &str,
    tool: &str,
    arguments: &serde_json::Value,
    thread_id: &str,
    turn_id: &str,
) {
    let Some(detail) = verification_command_from_tool_call(tool, arguments) else {
        return;
    };
    if verification_loops.contains_key(tool_call_id) {
        return;
    }

    let item_id = format!("{tool_call_id}:verification");
    verification_loops.insert(
        tool_call_id.to_string(),
        VerificationLoopTracker {
            item_id: item_id.clone(),
            detail: detail.clone(),
        },
    );
    let _ = event_tx.send(AppServerEvent::ServerNotification(
        ServerNotification::ItemStarted(ItemStartedNotification {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            item: ThreadItem::LoopLifecycle {
                id: item_id,
                kind: LoopLifecycleKind::VerificationLoop,
                title: "Running Verification Loop".to_string(),
                summary: "Starting explicit verification command".to_string(),
                detail: Some(detail),
                status: LoopLifecycleStatus::InProgress,
                reason: Some("verification_started".to_string()),
                counts: None,
                error: None,
                duration_ms: None,
                target_profile: None,
            },
        }),
    ));
}

pub(super) fn finish_verification_loop(
    event_tx: &mpsc::UnboundedSender<AppServerEvent>,
    verification_loops: &mut HashMap<String, VerificationLoopTracker>,
    tool_call_id: &str,
    thread_id: &str,
    turn_id: &str,
    success: bool,
) {
    let Some(loop_state) = verification_loops.remove(tool_call_id) else {
        return;
    };
    let _ = event_tx.send(AppServerEvent::ServerNotification(
        ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            item: ThreadItem::LoopLifecycle {
                id: loop_state.item_id,
                kind: LoopLifecycleKind::VerificationLoop,
                title: "Running Verification Loop".to_string(),
                summary: if success {
                    "Verification command completed".to_string()
                } else {
                    "Verification command failed".to_string()
                },
                detail: Some(loop_state.detail),
                status: if success {
                    LoopLifecycleStatus::Completed
                } else {
                    LoopLifecycleStatus::Failed
                },
                reason: Some(if success {
                    "verification_completed".to_string()
                } else {
                    "verification_failed".to_string()
                }),
                counts: Some(BTreeMap::from([("verification_steps".to_string(), 1)])),
                error: None,
                duration_ms: None,
                target_profile: None,
            },
        }),
    ));
}

pub(super) fn loop_lifecycle_server_notifications(
    thread_id: &str,
    turn_id: &str,
    notification: IlhaeLoopLifecycleNotification,
) -> Vec<ServerNotification> {
    match notification {
        IlhaeLoopLifecycleNotification::Started { item, .. } => {
            vec![ServerNotification::ItemStarted(ItemStartedNotification {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: ThreadItem::LoopLifecycle {
                    id: item.id,
                    kind: item.kind,
                    title: item.title,
                    summary: item.summary,
                    detail: item.detail,
                    status: item.status,
                    reason: item.reason,
                    counts: item.counts,
                    error: item.error,
                    duration_ms: item.duration_ms,
                    target_profile: item.target_profile,
                },
            })]
        }
        IlhaeLoopLifecycleNotification::Progress {
            item_id,
            kind,
            summary,
            detail,
            counts,
            ..
        } => {
            vec![ServerNotification::LoopLifecycleProgress(
                LoopLifecycleProgressNotification {
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id,
                    kind,
                    summary,
                    detail,
                    counts,
                },
            )]
        }
        IlhaeLoopLifecycleNotification::Completed { item, .. }
        | IlhaeLoopLifecycleNotification::Failed { item, .. } => {
            vec![ServerNotification::ItemCompleted(
                ItemCompletedNotification {
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item: ThreadItem::LoopLifecycle {
                        id: item.id,
                        kind: item.kind,
                        title: item.title,
                        summary: item.summary,
                        detail: item.detail,
                        status: item.status,
                        reason: item.reason,
                        counts: item.counts,
                        error: item.error,
                        duration_ms: item.duration_ms,
                        target_profile: item.target_profile,
                    },
                },
            )]
        }
    }
}
