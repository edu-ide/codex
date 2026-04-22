use std::sync::Arc;

use super::Session;
use super::TurnContext;
use crate::hook_runtime::PendingInputHookDisposition;
use crate::hook_runtime::inspect_pending_input;
use crate::hook_runtime::record_additional_contexts;
use crate::hook_runtime::record_pending_input;
use crate::hook_runtime::run_pending_session_start_hooks;

pub(super) enum PendingInputLoopAction {
    Proceed,
    Continue,
    Break,
}

pub(super) async fn process_pending_input_for_sampling(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    can_drain_pending_input: bool,
) -> PendingInputLoopAction {
    if run_pending_session_start_hooks(sess, turn_context).await {
        return PendingInputLoopAction::Break;
    }

    let pending_input = if can_drain_pending_input {
        sess.get_pending_input().await
    } else {
        Vec::new()
    };

    let mut blocked_pending_input = false;
    let mut blocked_pending_input_contexts = Vec::new();
    let mut requeued_pending_input = false;
    let mut accepted_pending_input = Vec::new();
    if !pending_input.is_empty() {
        let mut pending_input_iter = pending_input.into_iter();
        while let Some(pending_input_item) = pending_input_iter.next() {
            match inspect_pending_input(sess, turn_context, pending_input_item).await {
                PendingInputHookDisposition::Accepted(pending_input) => {
                    accepted_pending_input.push(*pending_input);
                }
                PendingInputHookDisposition::Blocked {
                    additional_contexts,
                } => {
                    let remaining_pending_input = pending_input_iter.collect::<Vec<_>>();
                    if !remaining_pending_input.is_empty() {
                        let _ = sess.prepend_pending_input(remaining_pending_input).await;
                        requeued_pending_input = true;
                    }
                    blocked_pending_input_contexts = additional_contexts;
                    blocked_pending_input = true;
                    break;
                }
            }
        }
    }

    let has_accepted_pending_input = !accepted_pending_input.is_empty();
    for pending_input in accepted_pending_input {
        record_pending_input(sess, turn_context, pending_input).await;
    }
    record_additional_contexts(sess, turn_context, blocked_pending_input_contexts).await;

    if blocked_pending_input && !has_accepted_pending_input {
        if requeued_pending_input {
            return PendingInputLoopAction::Continue;
        }
        return PendingInputLoopAction::Break;
    }

    PendingInputLoopAction::Proceed
}
