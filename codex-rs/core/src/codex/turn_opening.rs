use std::collections::HashSet;
use std::sync::Arc;

use super::PreviousTurnSettings;
use super::Session;
use super::TurnContext;
use codex_analytics::AppInvocation;
use codex_analytics::TrackEventsContext;
use codex_plugin::PluginTelemetryMetadata;
use codex_protocol::models::ResponseItem;
use codex_protocol::user_input::UserInput;

pub(super) enum TurnOpeningAction {
    Proceed,
    AbortTurn,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn perform_turn_opening(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    input: &[UserInput],
    tracking: &TrackEventsContext,
    mentioned_app_invocations: Vec<AppInvocation>,
    mentioned_plugin_metadata: Vec<PluginTelemetryMetadata>,
    explicitly_enabled_connectors: &HashSet<String>,
    skill_items: &[ResponseItem],
    plugin_items: &[ResponseItem],
) -> TurnOpeningAction {
    if crate::hook_runtime::run_pending_session_start_hooks(sess, turn_context).await {
        return TurnOpeningAction::AbortTurn;
    }
    let additional_contexts = if input.is_empty() {
        Vec::new()
    } else {
        let initial_input_for_turn =
            codex_protocol::models::ResponseInputItem::from(input.to_vec());
        let response_item: ResponseItem = initial_input_for_turn.clone().into();
        let user_prompt_submit_outcome = crate::hook_runtime::run_user_prompt_submit_hooks(
            sess,
            turn_context,
            codex_protocol::items::UserMessageItem::new(input).message(),
        )
        .await;
        if user_prompt_submit_outcome.should_stop {
            crate::hook_runtime::record_additional_contexts(
                sess,
                turn_context,
                user_prompt_submit_outcome.additional_contexts,
            )
            .await;
            return TurnOpeningAction::AbortTurn;
        }
        sess.record_user_prompt_and_emit_turn_item(turn_context.as_ref(), input, response_item)
            .await;
        user_prompt_submit_outcome.additional_contexts
    };
    sess.services
        .analytics_events_client
        .track_app_mentioned(tracking.clone(), mentioned_app_invocations);
    for plugin in mentioned_plugin_metadata {
        sess.services
            .analytics_events_client
            .track_plugin_used(tracking.clone(), plugin);
    }
    sess.merge_connector_selection(explicitly_enabled_connectors.clone())
        .await;
    crate::hook_runtime::record_additional_contexts(sess, turn_context, additional_contexts).await;
    if !input.is_empty() {
        sess.set_previous_turn_settings(Some(PreviousTurnSettings {
            model: turn_context.model_info.slug.clone(),
            realtime_active: Some(turn_context.realtime_active),
        }))
        .await;
    }

    if !skill_items.is_empty() {
        sess.record_conversation_items(turn_context, skill_items).await;
    }
    if !plugin_items.is_empty() {
        sess.record_conversation_items(turn_context, plugin_items).await;
    }

    TurnOpeningAction::Proceed
}
