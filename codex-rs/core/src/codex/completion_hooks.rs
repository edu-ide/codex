use std::sync::Arc;

use super::Session;
use super::TurnContext;
use codex_hooks::HookEvent;
use codex_hooks::HookEventAfterAgent;
use codex_hooks::HookPayload;
use codex_hooks::HookResult;
use codex_protocol::items::build_hook_prompt_message;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::HookStartedEvent;
use codex_protocol::protocol::WarningEvent;
use tracing::warn;

pub(super) enum CompletionHooksAction {
    ContinueWithStopHook,
    Break,
    AbortTurn,
}

pub(super) async fn handle_completed_sampling_turn(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    stop_hook_active: bool,
    last_agent_message: Option<String>,
    input_messages: Vec<String>,
) -> CompletionHooksAction {
    let stop_hook_permission_mode = match turn_context.approval_policy.value() {
        AskForApproval::Never => "bypassPermissions",
        AskForApproval::UnlessTrusted
        | AskForApproval::OnFailure
        | AskForApproval::OnRequest
        | AskForApproval::Granular(_) => "default",
    }
    .to_string();
    let stop_request = codex_hooks::StopRequest {
        session_id: sess.conversation_id,
        turn_id: turn_context.sub_id.clone(),
        cwd: turn_context.cwd.clone(),
        transcript_path: sess.hook_transcript_path().await,
        model: turn_context.model_info.slug.clone(),
        permission_mode: stop_hook_permission_mode,
        stop_hook_active,
        last_assistant_message: last_agent_message.clone(),
    };
    for run in sess.hooks().preview_stop(&stop_request) {
        sess.send_event(
            turn_context,
            EventMsg::HookStarted(HookStartedEvent {
                turn_id: Some(turn_context.sub_id.clone()),
                run,
            }),
        )
        .await;
    }
    let stop_outcome = sess.hooks().run_stop(stop_request).await;
    for completed in stop_outcome.hook_events {
        sess.send_event(turn_context, EventMsg::HookCompleted(completed))
            .await;
    }
    if stop_outcome.should_block {
        if let Some(hook_prompt_message) =
            build_hook_prompt_message(&stop_outcome.continuation_fragments)
        {
            sess.record_conversation_items(turn_context, std::slice::from_ref(&hook_prompt_message))
                .await;
            return CompletionHooksAction::ContinueWithStopHook;
        }
        sess.send_event(
            turn_context,
            EventMsg::Warning(WarningEvent {
                message:
                    "Stop hook requested continuation without a prompt; ignoring the block."
                        .to_string(),
            }),
        )
        .await;
    }
    if stop_outcome.should_stop {
        return CompletionHooksAction::Break;
    }

    let hook_outcomes = sess
        .hooks()
        .dispatch(HookPayload {
            session_id: sess.conversation_id,
            cwd: turn_context.cwd.clone(),
            client: turn_context.app_server_client_name.clone(),
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: HookEventAfterAgent {
                    thread_id: sess.conversation_id,
                    turn_id: turn_context.sub_id.clone(),
                    input_messages,
                    last_assistant_message: last_agent_message,
                },
            },
        })
        .await;

    let mut abort_message = None;
    for hook_outcome in hook_outcomes {
        let hook_name = hook_outcome.hook_name;
        match hook_outcome.result {
            HookResult::Success => {}
            HookResult::FailedContinue(error) => {
                warn!(
                    turn_id = %turn_context.sub_id,
                    hook_name = %hook_name,
                    error = %error,
                    "after_agent hook failed; continuing"
                );
            }
            HookResult::FailedAbort(error) => {
                let message = format!(
                    "after_agent hook '{hook_name}' failed and aborted turn completion: {error}"
                );
                warn!(
                    turn_id = %turn_context.sub_id,
                    hook_name = %hook_name,
                    error = %error,
                    "after_agent hook failed; aborting operation"
                );
                if abort_message.is_none() {
                    abort_message = Some(message);
                }
            }
        }
    }
    if let Some(message) = abort_message {
        sess.send_event(
            turn_context,
            EventMsg::Error(ErrorEvent {
                message,
                codex_error_info: None,
            }),
        )
        .await;
        return CompletionHooksAction::AbortTurn;
    }

    CompletionHooksAction::Break
}
