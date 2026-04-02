use std::sync::Arc;

use brain_rs::BrainService;
use brain_session_rs::session_store::SessionInfo;

use crate::{
    DeleteSessionResponse, ListSessionsResponse, LoadSessionMessagesResponse,
    LoadTeamTimelineResponse, SearchSessionsResponse, SessionInfoDto, SessionMessageDto,
    TeamTimelineEventDto, UpdateSessionTitleResponse,
};

/// Session registry/metadata access that is safe to share with native Codex paths.
///
/// This service intentionally avoids transcript writes/reads beyond metadata fields so
/// native Codex flows can reuse session continuity without mirroring full transcripts
/// into brain storage.
pub struct SessionRegistryService;

impl SessionRegistryService {
    pub fn list_sessions(brain: &Arc<BrainService>) -> anyhow::Result<ListSessionsResponse> {
        let sessions = brain.sessions().list_sessions()?;
        let sessions = sessions
            .into_iter()
            .map(|s| Self::to_session_info_dto(s, String::new()))
            .collect();
        Ok(ListSessionsResponse { sessions })
    }

    pub fn search_sessions(
        brain: &Arc<BrainService>,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<SearchSessionsResponse> {
        let sessions = brain
            .sessions()
            .search_sessions_with_snippet(query, limit.try_into().unwrap_or(i64::MAX))?;
        let sessions = sessions
            .into_iter()
            .map(|hit| Self::to_session_info_dto(hit.session, hit.search_snippet))
            .collect();
        Ok(SearchSessionsResponse { sessions })
    }

    pub fn get_session_info(
        brain: &Arc<BrainService>,
        session_id: &str,
    ) -> anyhow::Result<Option<SessionInfo>> {
        Ok(brain.session_get_raw(session_id)?)
    }

    pub fn get_session(
        brain: &Arc<BrainService>,
        session_id: &str,
    ) -> anyhow::Result<Option<SessionInfoDto>> {
        Ok(Self::get_session_info(brain, session_id)?
            .map(|session| Self::to_session_info_dto(session, String::new())))
    }

    pub fn ensure_session(
        brain: &Arc<BrainService>,
        session_id: &str,
        agent_id: &str,
        engine: &str,
        cwd: &str,
    ) -> anyhow::Result<()> {
        brain.session_ensure(session_id, agent_id, engine, cwd)?;
        Ok(())
    }

    pub fn delete_session(
        brain: &Arc<BrainService>,
        session_id: &str,
    ) -> anyhow::Result<DeleteSessionResponse> {
        brain.sessions().delete_session(session_id)?;
        Ok(DeleteSessionResponse { ok: true })
    }

    pub fn update_session_title(
        brain: &Arc<BrainService>,
        session_id: &str,
        title: &str,
    ) -> anyhow::Result<UpdateSessionTitleResponse> {
        brain.sessions().update_session_title(session_id, title)?;
        Ok(UpdateSessionTitleResponse { ok: true })
    }

    fn to_session_info_dto(session: SessionInfo, search_snippet: String) -> SessionInfoDto {
        SessionInfoDto {
            id: session.id,
            title: session.title,
            agent_id: session.agent_id,
            cwd: session.cwd,
            channel_id: session.channel_id,
            multi_agent: session.multi_agent,
            parent_session_id: session.parent_session_id,
            team_role: session.team_role,
            agent_status: session.agent_status,
            team_agent_count: session.team_agent_count,
            team_active_count: session.team_active_count,
            engine: session.engine,
            capabilities_override: session.capabilities_override,
            search_snippet,
            created_at: session.created_at,
            updated_at: session.updated_at,
            message_count: session.message_count,
        }
    }
}

/// Transcript-heavy persistence APIs. These are still proxy/brain-backed and should not be
/// wired into native Codex transcript handling as a second source of truth.
pub struct SessionPersistenceService;

impl SessionPersistenceService {
    pub fn load_session_messages(
        brain: &Arc<BrainService>,
        session_id: &str,
    ) -> anyhow::Result<LoadSessionMessagesResponse> {
        let messages = brain.sessions().load_session_messages(session_id)?;
        let messages = messages
            .into_iter()
            .map(|m| SessionMessageDto {
                id: m.id,
                session_id: m.session_id,
                role: m.role,
                content: m.content,
                timestamp: m.timestamp,
                agent_id: m.agent_id,
                thinking: m.thinking,
                tool_calls: m.tool_calls,
                content_blocks: m.content_blocks,
                channel_id: m.channel_id,
                input_tokens: m.input_tokens,
                output_tokens: m.output_tokens,
                total_tokens: m.total_tokens,
                duration_ms: m.duration_ms,
            })
            .collect();
        Ok(LoadSessionMessagesResponse { messages })
    }

    pub fn load_team_timeline(
        brain: &Arc<BrainService>,
        session_id: &str,
    ) -> anyhow::Result<LoadTeamTimelineResponse> {
        let store = brain.sessions().clone();
        let events = crate::team_timeline::load_session_timeline(&store, session_id)?;
        let events = events
            .into_iter()
            .map(|e| TeamTimelineEventDto {
                message_id: e.message_id,
                session_id: e.session_id,
                timestamp: e.timestamp,
                kind: serde_json::to_value(e.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "system_notice".to_string()),
                role: e.role,
                agent_id: e.agent_id,
                content: e.content,
                thinking: e.thinking,
                tool_calls: e.tool_calls_json,
                content_blocks: e.content_blocks_json,
                channel_id: e.channel_id,
                input_tokens: e.input_tokens,
                output_tokens: e.output_tokens,
                total_tokens: e.total_tokens,
                duration_ms: e.duration_ms,
                priority: e.priority,
                metadata: e.metadata,
            })
            .collect();
        Ok(LoadTeamTimelineResponse { events })
    }
}
