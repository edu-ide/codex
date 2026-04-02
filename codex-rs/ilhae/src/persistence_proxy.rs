//! PersistenceProxy — Session CRUD, Load, and New Session handling.
//!
//! Manages all session persistence operations: list, load messages,
//! delete, rename, load session (with cross-agent continuity), and new session.

use std::sync::Arc;

use sacp::{Agent, Client, Conductor, ConnectTo, ConnectionTo, Proxy, Responder};
use tracing::{info, warn};

use crate::{
    DeleteSessionRequest, DeleteSessionResponse, ListSessionsRequest, ListSessionsResponse,
    LoadSessionMessagesRequest, LoadSessionMessagesResponse, LoadTeamTimelineRequest,
    LoadTeamTimelineResponse, SearchSessionsRequest, SearchSessionsResponse, SessionInfoDto,
    SessionMessageDto, TeamTimelineEventDto, UpdateSessionTitleRequest, UpdateSessionTitleResponse,
    enrich_response_with_config_options, infer_agent_id_from_command,
};
use agent_client_protocol_schema::{
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
};

// ─── PersistenceProxy state ─────────────────────────────────────────────

pub struct PersistenceProxy {
    pub state: Arc<crate::SharedState>,
}

impl ConnectTo<Conductor> for PersistenceProxy {
    async fn connect_to(self, conductor: impl ConnectTo<Proxy>) -> Result<(), sacp::Error> {
        let s = self.state;

        Proxy.builder()
            .name("persistence-proxy")
            // ═══ Client → Proxy: Session CRUD (handled locally) ═══
            .on_receive_request_from(Client, {
                let state = s.clone();
                async move |_req: ListSessionsRequest, responder: Responder<ListSessionsResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/list_sessions RPC");
                    match crate::session_persistence_service::SessionRegistryService::list_sessions(&state.infra.brain) {
                        Ok(response) => responder.respond(response),
                        Err(e) => responder.respond_with_error(sacp::util::internal_error(e.to_string())),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let state = s.clone();
                async move |req: SearchSessionsRequest, responder: Responder<SearchSessionsResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/search_sessions RPC query={}", req.query);
                    match crate::session_persistence_service::SessionRegistryService::search_sessions(
                        &state.infra.brain,
                        &req.query,
                        req.limit.try_into().unwrap_or(50),
                    ) {
                        Ok(response) => responder.respond(response),
                        Err(e) => responder.respond_with_error(sacp::util::internal_error(e.to_string())),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let state = s.clone();
                async move |req: LoadSessionMessagesRequest, responder: Responder<LoadSessionMessagesResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/load_session_messages RPC for {}", req.session_id);
                    match crate::session_persistence_service::SessionPersistenceService::load_session_messages(
                        &state.infra.brain,
                        &req.session_id,
                    ) {
                        Ok(response) => responder.respond(response),
                        Err(e) => responder.respond_with_error(sacp::util::internal_error(e.to_string())),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let state = s.clone();
                async move |req: LoadTeamTimelineRequest, responder: Responder<LoadTeamTimelineResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/load_team_timeline RPC for {}", req.session_id);
                    match crate::session_persistence_service::SessionPersistenceService::load_team_timeline(
                        &state.infra.brain,
                        &req.session_id,
                    ) {
                        Ok(response) => responder.respond(response),
                        Err(e) => responder.respond_with_error(sacp::util::internal_error(e.to_string())),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let store = s.infra.brain.sessions().clone();
                async move |req: crate::ListSwarmTasksRequest, responder: Responder<crate::ListSwarmTasksResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/list_swarm_schedules RPC for {}", req.session_id);
                    match store.list_tasks(&req.session_id) {
                        Ok(schedules) => responder.respond(crate::ListSwarmTasksResponse { schedules }),
                        Err(e) => responder.respond_with_error(sacp::util::internal_error(e.to_string())),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let state = s.clone();
                async move |req: DeleteSessionRequest, responder: Responder<DeleteSessionResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/delete_session RPC for {}", req.session_id);
                    match crate::session_persistence_service::SessionRegistryService::delete_session(
                        &state.infra.brain,
                        &req.session_id,
                    ) {
                        Ok(response) => responder.respond(response),
                        Err(e) => responder.respond_with_error(sacp::util::internal_error(e.to_string())),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let state = s.clone();
                async move |req: UpdateSessionTitleRequest, responder: Responder<UpdateSessionTitleResponse>, _cx: ConnectionTo<Conductor>| {
                    info!("ilhae/update_session_title RPC for {}", req.session_id);
                    match crate::session_persistence_service::SessionRegistryService::update_session_title(
                        &state.infra.brain,
                        &req.session_id,
                        &req.title,
                    ) {
                        Ok(response) => responder.respond(response),
                        Err(e) => responder.respond_with_error(sacp::util::internal_error(e.to_string())),
                    }
                }
            }, sacp::on_receive_request!())
            // ═══ Client → Agent: LoadSessionRequest (load from local DB) ═══
            // NOTE: Placed AFTER with_mcp_server() registrations so that
            // McpNewSessionHandler can inject ilhae-tools / browser-tools
            // ACP URLs into req.mcp_servers before this handler runs.
            .on_receive_request_from(Client, {
                let store = s.infra.brain.sessions().clone();
                let settings = s.infra.settings_store.clone();
                let pending_history = s.sessions.pending_history.clone();
                let session_id_map = s.sessions.id_map.clone();
                let reverse_session_map = s.sessions.reverse_map.clone();
                let cx_cache = s.infra.relay_conductor_cx.clone();
                move |req: LoadSessionRequest, responder: Responder<LoadSessionResponse>, cx: ConnectionTo<Conductor>| {
                    let store = store.clone();
                    let settings = settings.clone();
                    let pending_history = pending_history.clone();
                    let session_id_map = session_id_map.clone();
                    let reverse_session_map = reverse_session_map.clone();
                    let cx_cache = cx_cache.clone();
                    async move {
                        cx_cache.try_add(cx.clone()).await;
                        let session_id = req.session_id.0.to_string();
                        let current_agent_id = infer_agent_id_from_command(&settings.get().agent.command);
                        info!("LoadSessionRequest: session_id={}, mcp_servers={}", session_id, req.mcp_servers.len());

                        let loaded_messages = store.load_session_messages(&session_id).unwrap_or_default();

                    // 0. Send history sync notification to the client
                    // History messages are loaded by the frontend via ilhae/load_session_messages RPC
                    // after receiving LoadSessionResponse (avoids CEF RefCell panic from push notification).

                    // Check for cross-agent session continuity
                    let session_info = store.get_session(&session_id).unwrap_or(None);
                    let is_cross_agent = session_info.as_ref().map_or(false, |info| {
                        !info.agent_id.is_empty() && info.agent_id != current_agent_id
                    });

                    if is_cross_agent {
                        let info = session_info.as_ref().unwrap();
                        info!("Cross-agent session detected: session agent='{}', current agent='{}'", info.agent_id, current_agent_id);

                        // 1. Store previous conversation history for injection
                        if !loaded_messages.is_empty() {
                            let mut history_text = format!(
                                "<previous_conversation agent=\"{}\" session=\"{}\">\n",
                                info.agent_id, session_id
                            );
                            for msg in &loaded_messages {
                                let role_label = match msg.role.as_str() {
                                    "user" => "User",
                                    "assistant" => "Assistant",
                                    _ => &msg.role,
                                };
                                history_text.push_str(&format!("{}:\n{}\n\n", role_label, msg.content));
                            }
                            history_text.push_str("</previous_conversation>\n\n");
                            history_text.push_str(
                                "위 대화는 이전 에이전트와의 대화 기록입니다. 이 맥락을 참고하여 사용자의 다음 질문에 답변해주세요.\n"
                            );
                            info!("Stored {} messages as pending history for cross-agent continuity", loaded_messages.len());
                            let lock = pending_history;
                            lock.insert(session_id.clone(), history_text);
                        }

                        // 2. Create a NEW ACP session with the current agent instead of forwarding LoadSession
                        let cwd_path = std::path::PathBuf::from(
                            session_info.as_ref().map(|i| i.cwd.as_str()).unwrap_or("/")
                        );
                        let new_session_req = NewSessionRequest::new(cwd_path);
                        info!("Cross-agent: creating new ACP session for DB session {}", session_id);

                        let store_clone = store.clone();
                        let session_id_clone = session_id.clone();
                        let session_id_map_clone = session_id_map.clone();
                        let session_id_map_clone_rev = reverse_session_map.clone();
                        let current_agent_clone = current_agent_id.clone();
                        cx.send_request_to(Agent, new_session_req)
                            .on_receiving_result(async move |result| {
                                match result {
                                    Ok(new_session_response) => {
                                        let acp_session_id = new_session_response.session_id.0.to_string();
                                        info!("Cross-agent: mapped DB session {} → ACP session {}", session_id_clone, acp_session_id);

                                        // Store both mappings atomically to prevent inconsistent state
                                        {
                                            session_id_map_clone.insert(session_id_clone.clone(), acp_session_id.clone());
                                            session_id_map_clone_rev.insert(acp_session_id.clone(), session_id_clone.clone());
                                        }

                                        // Update the agent_id in the DB to reflect the new owner
                                        let _ = store_clone.update_session_agent_id(&session_id_clone, &current_agent_clone);

                                        // Update DB session to current agent
                                        let _ = store_clone.ensure_session(&session_id_clone, &current_agent_clone, &current_agent_clone, "/");

                                        // Respond with a default LoadSessionResponse (session was "loaded" from DB perspective)
                                        responder.respond(LoadSessionResponse::new())
                                    }
                                    Err(e) => {
                                        warn!("Cross-agent: failed to create new session for {}: {:?}", session_id_clone, e);
                                        // Fall back: respond with default (frontend will show DB history)
                                        responder.respond(LoadSessionResponse::new())
                                    }
                                }
                            })
                    } else {
                        // Same agent — forward LoadSessionRequest directly (mcp_servers already injected by McpNewSessionHandler)
                        cx.send_request_to(Agent, req).forward_response_to(responder)
                    }
                    }
                }
            }, sacp::on_receive_request!())
            // ═══ Client → Agent: NewSessionRequest (cleanup + create) ═══
            // NOTE: Placed AFTER with_mcp_server() registrations so that
            // McpNewSessionHandler can inject ilhae-tools / browser-tools
            // ACP URLs into req.mcp_servers before this handler runs.
            .on_receive_request_from(Client, {
                let store = s.infra.brain.sessions().clone();
                let settings = s.infra.settings_store.clone();
                let cx_cache = s.infra.relay_conductor_cx.clone();
                let config_cache = s.infra.cached_config_options.clone();
                let session_mcp_servers = s.sessions.mcp_servers.clone();
                async move |req: NewSessionRequest, responder: Responder<NewSessionResponse>, cx: ConnectionTo<Conductor>| {
                    cx_cache.try_add(cx.clone()).await;
                    let cwd = req.cwd.display().to_string();
                    
                    // Capture dynamic MCP servers before forwarding req
                    let mcp_servers_clone = req.mcp_servers.clone();

                    let store = store.clone();
                    let settings = settings.clone();
                    let config_cache = config_cache.clone();
                    let session_mcp_servers = session_mcp_servers.clone();
                    info!("NewSessionRequest: cwd={}, mcp_servers={}", cwd, req.mcp_servers.len());

                    // Delete existing empty Untitled sessions to prevent accumulation
                    if let Ok(sessions) = store.list_sessions() {
                        for s in sessions.iter().filter(|s| s.title == "Untitled" && s.message_count == 0) {
                            info!("Cleaning up empty Untitled session: {}", s.id);
                            let _ = store.delete_session(&s.id);
                        }
                    }

                    // Create new session via agent (req.mcp_servers already populated by McpNewSessionHandler chain)
                    cx.send_request_to(Agent, req)
                        .on_receiving_result(async move |result| {
                            match result {
                                Ok(response) => {
                                    let session_id = response.session_id.0.to_string();
                                    
                                    let is_team = settings.get().agent.team_mode;
                                    let agent_id = if is_team {
                                        "team".to_string()
                                    } else {
                                        infer_agent_id_from_command(&settings.get().agent.command)
                                    };
                                    
                                    info!("NewSession created by agent: session_id={}, has_config_options={}, has_modes={}",
                                        session_id,
                                        response.config_options.is_some(),
                                        response.modes.is_some(),
                                    );
                                    if is_team {
                                        let _ = store.ensure_session_with_channel_meta_engine(
                                            &session_id, &agent_id, &cwd, "team", "", "team",
                                        );
                                    } else {
                                        let _ = store.ensure_session(&session_id, &agent_id, &agent_id, &cwd);
                                    }

                                    // Store MCP servers for this session for A2A delegation
                                    if !mcp_servers_clone.is_empty() {
                                        info!("[McpCache] Caching {} MCP servers for session {}", mcp_servers_clone.len(), session_id);
                                        let servers_map = session_mcp_servers;
                                        servers_map.insert(session_id.clone(), mcp_servers_clone);
                                    }

                                    // Auto-create artifact directory for session
                                    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
                                    let session_dir = std::path::PathBuf::from(&home)
                                        .join(crate::helpers::ILHAE_DIR_NAME)
                                        .join("brain")
                                        .join("sessions")
                                        .join(&session_id);
                                    if let Err(e) = std::fs::create_dir_all(&session_dir) {
                                        warn!("Failed to create session artifact dir {:?}: {}", session_dir, e);
                                    } else {
                                        info!("Auto-created session artifact dir: {:?}", session_dir);
                                    }
                                    let response = enrich_response_with_config_options(response, &agent_id);

                                    // Cache configOptions for instant serving to UI
                                    if let Some(ref opts) = response.config_options {
                                        let vals: Vec<serde_json::Value> = opts.iter()
                                            .filter_map(|o| serde_json::to_value(o).ok())
                                            .collect();
                                        if !vals.is_empty() {
                                            let mut cache = config_cache.write().await;
                                            *cache = vals;
                                            info!("[ConfigCache] Cached {} configOptions from session/new", opts.len());
                                        }
                                    } else {
                                        // Fallback: check serialized JSON for camelCase variant
                                        if let Ok(json) = serde_json::to_value(&response) {
                                            if let Some(arr) = json.get("configOptions").and_then(|v| v.as_array()) {
                                                if !arr.is_empty() {
                                                    let mut cache = config_cache.write().await;
                                                    *cache = arr.clone();
                                                    info!("[ConfigCache] Cached {} configOptions (JSON fallback)", arr.len());
                                                }
                                            }
                                        }
                                    }

                                    responder.respond(response)
                                }
                                Err(e) => {
                                    responder.respond_with_result(Err(e))
                                }
                            }
                        })
                }
            }, sacp::on_receive_request!())
            .connect_with(conductor, async move |cx: ConnectionTo<Conductor>| {
                s.infra.relay_conductor_cx.try_add(cx).await;
                // SSoT: Push initial engine state to newly connected client
                crate::notify_engine_state(&s.infra.relay_conductor_cx, &s.infra.settings_store).await;
                std::future::pending::<Result<(), sacp::Error>>().await
            })
            .await
    }
}
