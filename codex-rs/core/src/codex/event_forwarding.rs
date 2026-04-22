use tracing::debug;

use super::Session;
use super::TurnContext;
use super::plan_mode_stream::realtime_text_for_event;
use crate::agent::AgentStatus;
use crate::agent::agent_status_from_event;
use crate::agent::status::is_final;
use crate::session_prefix::format_subagent_notification_message;
use codex_features::Feature;
use codex_protocol::ThreadId;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;

pub(super) async fn handle_post_send_side_effects(
    sess: &Session,
    turn_context: &TurnContext,
    msg: &EventMsg,
) {
    maybe_notify_parent_of_terminal_turn(sess, turn_context, msg).await;
    maybe_mirror_event_text_to_realtime(sess, msg).await;
    maybe_clear_realtime_handoff_for_event(sess, msg).await;
}

async fn maybe_notify_parent_of_terminal_turn(
    sess: &Session,
    turn_context: &TurnContext,
    msg: &EventMsg,
) {
    if !sess.enabled(Feature::MultiAgentV2) {
        return;
    }

    if !matches!(msg, EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_)) {
        return;
    }

    let SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        agent_path: Some(child_agent_path),
        ..
    }) = &turn_context.session_source
    else {
        return;
    };

    let Some(status) = agent_status_from_event(msg) else {
        return;
    };
    if !is_final(&status) {
        return;
    }

    forward_child_completion_to_parent(sess, *parent_thread_id, child_agent_path, status).await;
}

async fn forward_child_completion_to_parent(
    sess: &Session,
    parent_thread_id: ThreadId,
    child_agent_path: &codex_protocol::AgentPath,
    status: AgentStatus,
) {
    let Some(parent_agent_path) = child_agent_path
        .as_str()
        .rsplit_once('/')
        .and_then(|(parent, _)| codex_protocol::AgentPath::try_from(parent).ok())
    else {
        return;
    };

    let message = format_subagent_notification_message(child_agent_path.as_str(), &status);
    let communication = InterAgentCommunication::new(
        child_agent_path.clone(),
        parent_agent_path,
        Vec::new(),
        message,
        /*trigger_turn*/ false,
    );
    if let Err(err) = sess
        .services
        .agent_control
        .send_inter_agent_communication(parent_thread_id, communication)
        .await
    {
        debug!("failed to notify parent thread {parent_thread_id}: {err}");
    }
}

async fn maybe_mirror_event_text_to_realtime(sess: &Session, msg: &EventMsg) {
    let Some(text) = realtime_text_for_event(msg) else {
        return;
    };
    if sess.conversation.running_state().await.is_none()
        || sess.conversation.active_handoff_id().await.is_none()
    {
        return;
    }
    if let Err(err) = sess.conversation.handoff_out(text).await {
        debug!("failed to mirror event text to realtime conversation: {err}");
    }
}

async fn maybe_clear_realtime_handoff_for_event(sess: &Session, msg: &EventMsg) {
    if !matches!(msg, EventMsg::TurnComplete(_)) {
        return;
    }
    if let Err(err) = sess.conversation.handoff_complete().await {
        debug!("failed to finalize realtime handoff output: {err}");
    }
    sess.conversation.clear_active_handoff().await;
}
