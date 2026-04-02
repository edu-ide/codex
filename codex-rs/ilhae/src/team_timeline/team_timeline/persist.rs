use crate::session_store::SessionStore;
use brain_rs::BrainService;
use std::sync::Arc;

use super::events::TeamTimelineEvent;

/// Trait to allow persist_events to accept either BrainService or SessionStore.
pub trait PersistTarget {
    fn add_message_with_blocks_channel(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        agent_id: &str,
        thinking: &str,
        tool_calls: &str,
        content_blocks: &str,
        channel_id: &str,
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
        duration_ms: i64,
    );
}

impl PersistTarget for BrainService {
    fn add_message_with_blocks_channel(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        agent_id: &str,
        thinking: &str,
        tool_calls: &str,
        content_blocks: &str,
        channel_id: &str,
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
        duration_ms: i64,
    ) {
        let _ = self.session_add_message_with_blocks_channel(
            session_id,
            role,
            content,
            agent_id,
            thinking,
            tool_calls,
            content_blocks,
            channel_id,
            input_tokens,
            output_tokens,
            total_tokens,
            duration_ms,
        );
    }
}

impl PersistTarget for SessionStore {
    fn add_message_with_blocks_channel(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        agent_id: &str,
        thinking: &str,
        tool_calls: &str,
        content_blocks: &str,
        channel_id: &str,
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
        duration_ms: i64,
    ) {
        let _ = self.add_full_message_with_blocks_channel(
            session_id,
            role,
            content,
            agent_id,
            thinking,
            tool_calls,
            content_blocks,
            channel_id,
            input_tokens,
            output_tokens,
            total_tokens,
            duration_ms,
        );
    }
}

impl PersistTarget for Arc<SessionStore> {
    fn add_message_with_blocks_channel(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        agent_id: &str,
        thinking: &str,
        tool_calls: &str,
        content_blocks: &str,
        channel_id: &str,
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
        duration_ms: i64,
    ) {
        let _ = self.add_full_message_with_blocks_channel(
            session_id,
            role,
            content,
            agent_id,
            thinking,
            tool_calls,
            content_blocks,
            channel_id,
            input_tokens,
            output_tokens,
            total_tokens,
            duration_ms,
        );
    }
}

impl PersistTarget for Arc<BrainService> {
    fn add_message_with_blocks_channel(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        agent_id: &str,
        thinking: &str,
        tool_calls: &str,
        content_blocks: &str,
        channel_id: &str,
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
        duration_ms: i64,
    ) {
        let _ = self.session_add_message_with_blocks_channel(
            session_id,
            role,
            content,
            agent_id,
            thinking,
            tool_calls,
            content_blocks,
            channel_id,
            input_tokens,
            output_tokens,
            total_tokens,
            duration_ms,
        );
    }
}

pub fn persist_events(
    target: &impl PersistTarget,
    session_id: &str,
    events: impl IntoIterator<Item = TeamTimelineEvent>,
) {
    for event in events {
        target.add_message_with_blocks_channel(
            session_id,
            &event.role,
            &event.content,
            &event.agent_id,
            &event.thinking,
            &event.tool_calls_json,
            &event.content_blocks_json,
            &event.channel_id,
            event.input_tokens,
            event.output_tokens,
            event.total_tokens,
            event.duration_ms,
        );
    }
}
