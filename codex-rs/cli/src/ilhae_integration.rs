//! Product-specific startup and lifecycle adapters for the shared Codex CLI.
//! Keep environment ownership and Ilhae-to-Codex event conversion here so
//! upstream command dispatch does not need to understand product internals.

use std::sync::Arc;

pub(super) fn is_invoked_as_ilhae_cli() -> bool {
    if crate::IS_ILHAE_BINARY
        || std::env::var("ILHAE_APP_SERVER").ok().as_deref() == Some("1")
        || std::env::var("ILHAE_RUNTIME").ok().as_deref() == Some("1")
    {
        return true;
    }

    std::env::args_os()
        .next()
        .and_then(|arg0| {
            std::path::Path::new(&arg0)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .is_some_and(|name| is_ilhae_cli_binary_name(&name))
}

fn is_ilhae_cli_binary_name(name: &str) -> bool {
    let name = name.strip_suffix(".exe").unwrap_or(name);
    matches!(name, "ilhae" | "codex-ilhae" | "codex-ilhae-cli")
}

fn thread_goal_loop_event_from_ilhae_lifecycle(
    notification: codex_ilhae::IlhaeLoopLifecycleNotification,
) -> codex_state::ThreadGoalLoopEvent {
    match notification {
        codex_ilhae::IlhaeLoopLifecycleNotification::Started { item, .. }
        | codex_ilhae::IlhaeLoopLifecycleNotification::Completed { item, .. }
        | codex_ilhae::IlhaeLoopLifecycleNotification::Failed { item, .. } => {
            thread_goal_loop_event_from_ilhae_item(item)
        }
        codex_ilhae::IlhaeLoopLifecycleNotification::Progress {
            item_id,
            kind,
            summary,
            detail,
            ..
        } => codex_state::ThreadGoalLoopEvent {
            id: item_id.clone(),
            phase: thread_goal_loop_phase_from_ilhae_parts(&item_id, "", kind),
            status: codex_state::ThreadGoalLoopStatus::InProgress,
            title: "Loop progress".to_string(),
            summary,
            detail,
            error: None,
        },
    }
}

fn thread_goal_loop_event_from_ilhae_item(
    item: codex_ilhae::LoopLifecycleItem,
) -> codex_state::ThreadGoalLoopEvent {
    codex_state::ThreadGoalLoopEvent {
        id: item.id.clone(),
        phase: thread_goal_loop_phase_from_ilhae_parts(&item.id, &item.title, item.kind),
        status: thread_goal_loop_status_from_ilhae(item.status),
        title: item.title,
        summary: item.summary,
        detail: item.detail,
        error: item.error,
    }
}

fn thread_goal_loop_phase_from_ilhae_parts(
    id: &str,
    title: &str,
    kind: codex_ilhae::LoopLifecycleKind,
) -> codex_state::ThreadGoalLoopPhase {
    let id = id.to_ascii_lowercase();
    let title = title.to_ascii_lowercase();
    if id.contains("kairos") || title.contains("kairos") {
        return codex_state::ThreadGoalLoopPhase::KairosLoop;
    }
    if id.contains("knowledge_loop") || title.contains("knowledge") {
        return codex_state::ThreadGoalLoopPhase::KnowledgeLoop;
    }
    if id.contains("cleanup") || title.contains("cleanup") || title.contains("hygiene") {
        return codex_state::ThreadGoalLoopPhase::CleanupLoop;
    }
    if id.contains("verification") || title.contains("verification") || title.contains("verify") {
        return codex_state::ThreadGoalLoopPhase::VerificationLoop;
    }
    match kind {
        codex_ilhae::LoopLifecycleKind::SuperLoop => codex_state::ThreadGoalLoopPhase::SuperLoop,
        codex_ilhae::LoopLifecycleKind::ExecutionLoop => {
            codex_state::ThreadGoalLoopPhase::ExecutionLoop
        }
        codex_ilhae::LoopLifecycleKind::ImprovementLoop => {
            codex_state::ThreadGoalLoopPhase::ImprovementLoop
        }
        codex_ilhae::LoopLifecycleKind::CleanupLoop => {
            codex_state::ThreadGoalLoopPhase::CleanupLoop
        }
        codex_ilhae::LoopLifecycleKind::ContextInjection => {
            codex_state::ThreadGoalLoopPhase::ContextInjection
        }
    }
}

fn thread_goal_loop_status_from_ilhae(
    status: codex_ilhae::LoopLifecycleStatus,
) -> codex_state::ThreadGoalLoopStatus {
    match status {
        codex_ilhae::LoopLifecycleStatus::InProgress => {
            codex_state::ThreadGoalLoopStatus::InProgress
        }
        codex_ilhae::LoopLifecycleStatus::Completed => codex_state::ThreadGoalLoopStatus::Completed,
        codex_ilhae::LoopLifecycleStatus::Failed => codex_state::ThreadGoalLoopStatus::Failed,
    }
}

async fn collect_ilhae_foreground_loop_events(
    goal_continuation: bool,
) -> Vec<codex_state::ThreadGoalLoopEvent> {
    let result = if goal_continuation {
        codex_ilhae::run_active_goal_foreground_loop_cycle_collecting_lifecycle().await
    } else {
        codex_ilhae::run_active_foreground_loop_cycle_collecting_lifecycle().await
    };
    match result {
        Ok(notifications) => notifications
            .into_iter()
            .map(thread_goal_loop_event_from_ilhae_lifecycle)
            .collect(),
        Err(err) => {
            let stage = if goal_continuation {
                "goal continuation"
            } else {
                "app-server turn"
            };
            tracing::warn!(
                error = ?err,
                "ilhae foreground loop cycle failed before {stage}"
            );
            Vec::new()
        }
    }
}

fn ilhae_foreground_loop_hook(goal_continuation: bool) -> codex_app_server::AppServerTurnStartHook {
    Arc::new(move || {
        Box::pin(async move {
            let thread_goal_loop_events =
                collect_ilhae_foreground_loop_events(goal_continuation).await;
            codex_app_server::AppServerTurnStartHookResult {
                thread_goal_loop_events,
            }
        })
    })
}

pub(super) fn ilhae_app_server_runtime_hooks() -> codex_app_server::AppServerRuntimeHooks {
    codex_app_server::AppServerRuntimeHooks {
        before_turn_start: Some(ilhae_foreground_loop_hook(/*goal_continuation*/ false)),
        before_goal_continuation: Some(ilhae_foreground_loop_hook(/*goal_continuation*/ true)),
    }
}

pub(super) fn prepare_ilhae_cli_environment_if_needed() -> anyhow::Result<Option<std::path::PathBuf>>
{
    if !is_invoked_as_ilhae_cli() {
        return Ok(None);
    }

    let codex_home = codex_ilhae::config::prepare_ilhae_codex_home().map_err(anyhow::Error::msg)?;
    Ok(Some(codex_home))
}

#[cfg(test)]
#[path = "ilhae_integration_tests.rs"]
mod tests;
