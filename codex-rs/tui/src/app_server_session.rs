use crate::bottom_pane::FeedbackAudience;
#[cfg(test)]
use crate::legacy_core::append_message_history_entry;
use crate::legacy_core::config::Config;
use crate::status::plan_type_display_name;
use crate::status::StatusAccountDisplay;
use agent_client_protocol_schema::CancelNotification;
use agent_client_protocol_schema::ContentBlock;
use agent_client_protocol_schema::RequestPermissionOutcome;
use agent_client_protocol_schema::RequestPermissionRequest;
use agent_client_protocol_schema::RequestPermissionResponse;
use agent_client_protocol_schema::SelectedPermissionOutcome;
use agent_client_protocol_schema::SessionId as AcpSessionId;
use agent_client_protocol_schema::SessionNotification;
use agent_client_protocol_schema::SessionUpdate;
use async_trait::async_trait;
use brain_session_rs::session_store::SessionInfo;
use chrono::DateTime;
use chrono::Utc;
use codex_app_server_client::AppServerClient;
use codex_app_server_client::AppServerEvent;
use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_client::TypedRequestError;
use codex_app_server_protocol::Account;
use codex_app_server_protocol::AgentMessageDeltaNotification;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::CommandExecutionApprovalDecision;
use codex_app_server_protocol::CommandExecutionRequestApprovalParams;
use codex_app_server_protocol::CommandExecutionRequestApprovalResponse;
use codex_app_server_protocol::ConfigBatchWriteParams;
use codex_app_server_protocol::ConfigWriteResponse;
use codex_app_server_protocol::DynamicToolCallStatus;
use codex_app_server_protocol::GetAccountParams;
use codex_app_server_protocol::GetAccountResponse;
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::LogoutAccountResponse;
use codex_app_server_protocol::MemoryResetResponse;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::ReasoningTextDeltaNotification;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ReviewDelivery;
use codex_app_server_protocol::ReviewStartParams;
use codex_app_server_protocol::ReviewStartResponse;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::SkillsListParams;
use codex_app_server_protocol::SkillsListResponse;
use codex_app_server_protocol::Thread;
use codex_app_server_protocol::ThreadBackgroundTerminalsCleanParams;
use codex_app_server_protocol::ThreadBackgroundTerminalsCleanResponse;
use codex_app_server_protocol::ThreadCompactStartParams;
use codex_app_server_protocol::ThreadCompactStartResponse;
use codex_app_server_protocol::ThreadForkResponse;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadListParams;
use codex_app_server_protocol::ThreadListResponse;
use codex_app_server_protocol::ThreadLoadedListParams;
use codex_app_server_protocol::ThreadLoadedListResponse;
use codex_app_server_protocol::ThreadMemoryMode;
use codex_app_server_protocol::ThreadMemoryModeSetParams;
use codex_app_server_protocol::ThreadMemoryModeSetResponse;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::ThreadReadResponse;
use codex_app_server_protocol::ThreadRealtimeAppendAudioParams;
use codex_app_server_protocol::ThreadRealtimeAppendAudioResponse;
use codex_app_server_protocol::ThreadRealtimeAppendTextParams;
use codex_app_server_protocol::ThreadRealtimeAppendTextResponse;
use codex_app_server_protocol::ThreadRealtimeStartParams;
use codex_app_server_protocol::ThreadRealtimeStartResponse;
use codex_app_server_protocol::ThreadRealtimeStartTransport;
use codex_app_server_protocol::ThreadRealtimeStopParams;
use codex_app_server_protocol::ThreadRealtimeStopResponse;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadRollbackParams;
use codex_app_server_protocol::ThreadRollbackResponse;
use codex_app_server_protocol::ThreadSetNameParams;
use codex_app_server_protocol::ThreadSetNameResponse;
use codex_app_server_protocol::ThreadShellCommandParams;
use codex_app_server_protocol::ThreadShellCommandResponse;
use codex_app_server_protocol::ThreadSortKey;
use codex_app_server_protocol::ThreadSourceKind;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadStartSource;
use codex_app_server_protocol::ThreadStartedNotification;
use codex_app_server_protocol::ThreadStatus;
use codex_app_server_protocol::ThreadUnsubscribeParams;
use codex_app_server_protocol::ThreadUnsubscribeResponse;
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnInterruptParams;
use codex_app_server_protocol::TurnInterruptResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStartedNotification;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::TurnSteerParams;
use codex_app_server_protocol::TurnSteerResponse;
use codex_ilhae::IlhaeAppSessionEventDto;
use codex_ilhae::IlhaeAppSessionEventNotification;
use codex_ilhae::IlhaeInteractiveOptionDto;
use codex_ilhae::IlhaeInteractiveRequestDto;
use codex_ilhae::IlhaeLoopLifecycleNotification;
use codex_otel::TelemetryAuthMode;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::ConversationAudioParams;
use codex_protocol::protocol::ConversationStartParams;
use codex_protocol::protocol::ConversationStartTransport;
use codex_protocol::protocol::ConversationTextParams;
use codex_protocol::protocol::LoopLifecycleKind;
use codex_protocol::protocol::LoopLifecycleStatus;
use codex_protocol::protocol::ReviewRequest;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionNetworkProxyRuntime;
use codex_protocol::ThreadId;
use codex_utils_absolute_path::AbsolutePathBuf;
use color_eyre::eyre::ContextCompat;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;
use sacp::util::MatchDispatch;
use sacp::ConnectionTo;
use sacp::Responder;
use sacp::SessionMessage;
use sacp_tokio::AcpAgent;
use sacp_tokio::AcpAgentSession;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

mod loop_lifecycle;
mod ilhae_bridge;
mod state_mapping;

use self::ilhae_bridge::acp_permission_response_from_exec_decision;
use self::ilhae_bridge::canonical_ilhae_event_from_app_server_event;
use self::ilhae_bridge::ilhae_interactive_option_kind;
use self::loop_lifecycle::VerificationLoopTracker;
use self::loop_lifecycle::acp_tool_content_items;
use self::loop_lifecycle::finish_verification_loop;
use self::loop_lifecycle::loop_lifecycle_server_notifications;
use self::loop_lifecycle::start_verification_loop;
pub(crate) use self::state_mapping::app_server_rate_limit_snapshot_to_core;
pub(crate) use self::state_mapping::app_server_rate_limit_snapshots_to_core;
use self::state_mapping::model_preset_from_api_model;
use self::state_mapping::review_target_to_app_server;
use self::state_mapping::started_thread_from_fork_response;
use self::state_mapping::started_thread_from_resume_response;
use self::state_mapping::started_thread_from_start_response;
pub(crate) use self::state_mapping::status_account_display_from_auth_mode;
use self::state_mapping::thread_fork_params_from_config;
use self::state_mapping::thread_resume_params_from_config;
use self::state_mapping::thread_session_state_from_thread_response;
use self::state_mapping::thread_start_params_from_config;

/// Data collected during the TUI bootstrap phase that the main event loop
/// needs to configure the UI, telemetry, and initial rate-limit prefetch.
///
/// Rate-limit snapshots are intentionally **not** included here; they are
/// fetched asynchronously after bootstrap returns so that the TUI can render
/// its first frame without waiting for the rate-limit round-trip.
pub(crate) struct AppServerBootstrap {
    pub(crate) account_email: Option<String>,
    pub(crate) auth_mode: Option<TelemetryAuthMode>,
    pub(crate) status_account_display: Option<StatusAccountDisplay>,
    pub(crate) plan_type: Option<codex_protocol::account::PlanType>,
    /// Whether the configured model provider needs OpenAI-style auth. Combined
    /// with `has_chatgpt_account` to decide if a startup rate-limit prefetch
    /// should be fired.
    pub(crate) requires_openai_auth: bool,
    pub(crate) default_model: String,
    pub(crate) feedback_audience: FeedbackAudience,
    pub(crate) has_chatgpt_account: bool,
    pub(crate) available_models: Vec<ModelPreset>,
}

pub(crate) struct AppServerSession {
    client: AppServerClient,
    next_request_id: i64,
    pending_ilhae_events: VecDeque<IlhaeAppSessionEventNotification>,
    remote_cwd_override: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ThreadSessionState {
    pub(crate) thread_id: ThreadId,
    pub(crate) forked_from_id: Option<ThreadId>,
    pub(crate) thread_name: Option<String>,
    pub(crate) model: String,
    pub(crate) model_provider_id: String,
    pub(crate) service_tier: Option<codex_protocol::config_types::ServiceTier>,
    pub(crate) approval_policy: AskForApproval,
    pub(crate) approvals_reviewer: codex_protocol::config_types::ApprovalsReviewer,
    pub(crate) sandbox_policy: SandboxPolicy,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) instruction_source_paths: Vec<AbsolutePathBuf>,
    pub(crate) reasoning_effort: Option<codex_protocol::openai_models::ReasoningEffort>,
    pub(crate) history_log_id: u64,
    pub(crate) history_entry_count: u64,
    pub(crate) network_proxy: Option<SessionNetworkProxyRuntime>,
    pub(crate) rollout_path: Option<PathBuf>,
}

#[derive(Clone, Copy)]
enum ThreadParamsMode {
    Embedded,
    Remote,
}

impl ThreadParamsMode {
    fn model_provider_from_config(self, config: &Config) -> Option<String> {
        match self {
            Self::Embedded => Some(config.model_provider_id.clone()),
            Self::Remote => None,
        }
    }
}

pub(crate) struct AppServerStartedThread {
    pub(crate) session: ThreadSessionState,
    pub(crate) turns: Vec<Turn>,
}

pub(crate) struct ConversationTurnRequest {
    pub(crate) thread_id: ThreadId,
    pub(crate) items: Vec<codex_protocol::user_input::UserInput>,
    pub(crate) cwd: PathBuf,
    pub(crate) approval_policy: AskForApproval,
    pub(crate) approvals_reviewer: codex_protocol::config_types::ApprovalsReviewer,
    pub(crate) sandbox_policy: SandboxPolicy,
    pub(crate) model: String,
    pub(crate) effort: Option<codex_protocol::openai_models::ReasoningEffort>,
    pub(crate) summary: Option<codex_protocol::config_types::ReasoningSummary>,
    pub(crate) service_tier: Option<Option<codex_protocol::config_types::ServiceTier>>,
    pub(crate) collaboration_mode: Option<codex_protocol::config_types::CollaborationMode>,
    pub(crate) personality: Option<codex_protocol::config_types::Personality>,
    pub(crate) output_schema: Option<serde_json::Value>,
}

pub(crate) struct AcpConversationRuntime {
    engine_id: String,
    session: Option<Arc<Mutex<AcpAgentSession>>>,
    active_global_thread_id: Option<String>,
    pending_events: VecDeque<AppServerEvent>,
    pending_ilhae_events: VecDeque<IlhaeAppSessionEventNotification>,
    event_tx: mpsc::UnboundedSender<AppServerEvent>,
    event_rx: mpsc::UnboundedReceiver<AppServerEvent>,
    active_turn: Option<AcpActiveTurn>,
    next_synthetic_id: u64,
    loop_lifecycle_rx: Option<broadcast::Receiver<IlhaeLoopLifecycleNotification>>,
    pending_permission_requests: Arc<Mutex<HashMap<String, PendingAcpPermissionRequest>>>,
    control_tx: mpsc::UnboundedSender<AcpRuntimeControlMessage>,
    control_rx: Arc<Mutex<mpsc::UnboundedReceiver<AcpRuntimeControlMessage>>>,
}

struct AcpActiveTurn {
    thread_id: String,
    turn_id: String,
    cancel_session_id: AcpSessionId,
    cancel_connection: ConnectionTo<sacp::Agent>,
    task: JoinHandle<()>,
}

struct PendingAcpPermissionRequest {
    request: RequestPermissionRequest,
    responder: Responder<RequestPermissionResponse>,
}

enum AcpRuntimeControlMessage {
    RegisterPermissionRequest {
        synthetic_id: String,
        interactive_request: IlhaeInteractiveRequestDto,
        request: RequestPermissionRequest,
        responder: Responder<RequestPermissionResponse>,
    },
}

impl AcpConversationRuntime {
    pub(crate) fn new(engine_id: impl Into<String>) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        Self {
            engine_id: engine_id.into(),
            session: None,
            active_global_thread_id: None,
            pending_events: VecDeque::new(),
            pending_ilhae_events: VecDeque::new(),
            event_tx,
            event_rx,
            active_turn: None,
            next_synthetic_id: 1,
            loop_lifecycle_rx: codex_ilhae::native_runtime_context()
                .map(|_| codex_ilhae::subscribe_native_loop_lifecycle()),
            pending_permission_requests: Arc::new(Mutex::new(HashMap::new())),
            control_tx,
            control_rx: Arc::new(Mutex::new(control_rx)),
        }
    }

    fn runtime_session_store(&self) -> Result<Vec<SessionInfo>> {
        let runtime = codex_ilhae::native_runtime_context()
            .context("ilhae native runtime not bootstrapped for ACP runtime")?;
        let sessions = runtime
            .brain
            .sessions()
            .list_sessions()
            .wrap_err("failed to list brain sessions for ACP runtime")?;
        Ok(sessions)
    }

    async fn drain_control_messages(&mut self) {
        let mut drained = Vec::new();
        {
            let mut rx = self.control_rx.lock().await;
            while let Ok(message) = rx.try_recv() {
                drained.push(message);
            }
        }

        if drained.is_empty() {
            return;
        }

        let mut pending = self.pending_permission_requests.lock().await;
        for message in drained {
            match message {
                AcpRuntimeControlMessage::RegisterPermissionRequest {
                    synthetic_id,
                    interactive_request,
                    request,
                    responder,
                } => {
                    pending.insert(
                        synthetic_id,
                        PendingAcpPermissionRequest { request, responder },
                    );
                    self.pending_ilhae_events
                        .push_back(IlhaeAppSessionEventNotification {
                            engine: self.engine_id.clone(),
                            event: IlhaeAppSessionEventDto::InteractiveRequest {
                                request: interactive_request,
                            },
                        });
                }
            }
        }
    }

    async fn resolve_server_request(
        &self,
        request_id: RequestId,
        result: serde_json::Value,
    ) -> std::io::Result<()> {
        let request_key = match request_id {
            RequestId::String(value) => value,
            RequestId::Integer(value) => value.to_string(),
        };

        let pending = {
            let mut pending = self.pending_permission_requests.lock().await;
            pending.remove(&request_key)
        }
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("unknown ACP synthetic request `{request_key}`"),
            )
        })?;

        let decision = serde_json::from_value::<CommandExecutionRequestApprovalResponse>(result)
            .map_err(|err| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("failed to decode ACP approval resolution: {err}"),
                )
            })?
            .decision;

        pending
            .responder
            .respond(acp_permission_response_from_exec_decision(
                &pending.request,
                decision,
            ))
            .map_err(|err| std::io::Error::other(err.to_string()))
    }

    fn matches_engine(&self, session: &SessionInfo) -> bool {
        session.engine.eq_ignore_ascii_case(&self.engine_id)
    }

    async fn acp_permission_prompt(
        &self,
        synthetic_id: &str,
    ) -> Option<IlhaeInteractiveRequestDto> {
        let pending = self.pending_permission_requests.lock().await;
        let pending = pending.get(synthetic_id)?;
        let tool_title = pending
            .request
            .tool_call
            .fields
            .title
            .clone()
            .unwrap_or_else(|| "ACP permission request".to_string());
        Some(IlhaeInteractiveRequestDto {
            source: "acp".to_string(),
            thread_id: self
                .active_global_thread_id
                .clone()
                .unwrap_or_else(|| synthetic_id.to_string()),
            turn_id: self
                .active_turn
                .as_ref()
                .map(|turn| turn.turn_id.clone())
                .unwrap_or_else(|| synthetic_id.to_string()),
            request_id: synthetic_id.to_string(),
            title: tool_title.clone(),
            reason: Some(format!(
                "ACP backend `{}` is requesting permission for `{}`",
                self.engine_id, tool_title
            )),
            requested_permissions: None,
            options: pending
                .request
                .options
                .iter()
                .map(|option| IlhaeInteractiveOptionDto {
                    id: option.option_id.to_string(),
                    label: option.name.clone(),
                    kind: ilhae_interactive_option_kind(option.kind),
                })
                .collect(),
        })
    }

    async fn resolve_acp_permission_request(
        &self,
        synthetic_id: &str,
        option_id: Option<String>,
    ) -> std::io::Result<()> {
        let pending = {
            let mut pending = self.pending_permission_requests.lock().await;
            pending.remove(synthetic_id)
        }
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("unknown ACP permission request `{synthetic_id}`"),
            )
        })?;

        let response = if let Some(option_id) = option_id {
            RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                SelectedPermissionOutcome::new(option_id),
            ))
        } else {
            RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
        };

        pending
            .responder
            .respond(response)
            .map_err(|err| std::io::Error::other(err.to_string()))
    }

    fn thread_source_matches(params: &ThreadListParams) -> bool {
        params
            .source_kinds
            .as_ref()
            .map(|kinds| {
                kinds.is_empty()
                    || kinds.iter().any(|kind| {
                        matches!(
                            kind,
                            ThreadSourceKind::AppServer
                                | ThreadSourceKind::Cli
                                | ThreadSourceKind::Unknown
                        )
                    })
            })
            .unwrap_or(true)
    }

    fn parse_cursor(cursor: Option<&str>) -> usize {
        cursor
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0)
    }

    fn to_unix_timestamp(timestamp: &str) -> i64 {
        DateTime::parse_from_rfc3339(timestamp)
            .map(|parsed| parsed.with_timezone(&Utc).timestamp())
            .unwrap_or(0)
    }

    fn to_thread(&self, session: SessionInfo) -> Thread {
        let preview = if session.title.trim().is_empty() {
            session.id.clone()
        } else {
            session.title.clone()
        };

        Thread {
            forked_from_id: None,
            id: session.id,
            preview,
            ephemeral: false,
            model_provider: self.engine_id.clone(),
            created_at: Self::to_unix_timestamp(&session.created_at),
            updated_at: Self::to_unix_timestamp(&session.updated_at),
            status: ThreadStatus::Idle,
            path: None,
            cwd: codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(
                std::path::PathBuf::from(session.cwd),
            )
            .unwrap(),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            source: SessionSource::AppServer,
            agent_nickname: None,
            agent_role: None,
            git_info: None,
            name: (!session.title.trim().is_empty()).then_some(session.title),
            turns: Vec::new(),
        }
    }

    fn unsupported<T>(&self, action: &str) -> Result<T> {
        color_eyre::eyre::bail!(
            "{action} is not implemented for ACP backend `{}` in ilhae TUI yet",
            self.engine_id
        )
    }

    fn runtime_context(&self) -> Result<codex_ilhae::BootstrappedIlhaeRuntime> {
        codex_ilhae::native_runtime_context().context("ilhae native runtime not bootstrapped")
    }

    fn next_synthetic_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}-{}", self.next_synthetic_id);
        self.next_synthetic_id += 1;
        id
    }

    fn drain_finished_turn(&mut self) {
        if self
            .active_turn
            .as_ref()
            .is_some_and(|active| active.task.is_finished())
        {
            self.active_turn = None;
        }
    }

    fn current_command(&self) -> Result<String> {
        Ok(self.runtime_context()?.settings_store.get().agent.command)
    }

    fn current_model(&self, config: &Config) -> String {
        config
            .model
            .clone()
            .unwrap_or_else(|| self.engine_id.clone())
    }

    fn minimal_model_preset(&self, config: &Config) -> ModelPreset {
        let model = self.current_model(config);
        let effort = config
            .model_reasoning_effort
            .unwrap_or(ReasoningEffort::Medium);
        ModelPreset {
            id: model.clone(),
            model: model.clone(),
            display_name: model.clone(),
            description: format!("ACP backend `{}`", self.engine_id),
            default_reasoning_effort: effort,
            supported_reasoning_efforts: vec![ReasoningEffortPreset {
                effort,
                description: "Default ACP runtime reasoning level".to_string(),
            }],
            supports_personality: false,
            is_default: true,
            upgrade: None,
            show_in_picker: true,
            availability_nux: None,
            supported_in_api: true,
            input_modalities: vec![InputModality::Text, InputModality::Image],
            additional_speed_tiers: Vec::new(),
        }
    }

    async fn bootstrap(&mut self, config: &Config) -> Result<AppServerBootstrap> {
        let default_model = self.current_model(config);
        Ok(AppServerBootstrap {
            requires_openai_auth: false,
            account_email: None,
            auth_mode: None,
            status_account_display: None,
            plan_type: None,
            default_model,
            feedback_audience: FeedbackAudience::External,
            has_chatgpt_account: false,
            available_models: vec![self.minimal_model_preset(config)],
        })
    }

    fn background_loop_thread_id(&self) -> Option<String> {
        self.active_global_thread_id.clone().or_else(|| {
            self.runtime_session_store()
                .ok()
                .and_then(|sessions| {
                    sessions
                        .into_iter()
                        .max_by_key(|session| session.updated_at.clone())
                })
                .map(|session| session.id)
        })
    }

    fn queue_loop_lifecycle_notification(
        &mut self,
        thread_id: &str,
        notification: IlhaeLoopLifecycleNotification,
    ) {
        let item_id = match &notification {
            IlhaeLoopLifecycleNotification::Started { item, .. }
            | IlhaeLoopLifecycleNotification::Completed { item, .. }
            | IlhaeLoopLifecycleNotification::Failed { item, .. } => item.id.clone(),
            IlhaeLoopLifecycleNotification::Progress { item_id, .. } => item_id.clone(),
        };
        let turn_id = format!("background-loop:{item_id}");
        for notification in loop_lifecycle_server_notifications(thread_id, &turn_id, notification) {
            self.pending_events
                .push_back(AppServerEvent::ServerNotification(notification));
        }
    }

    fn drain_native_loop_lifecycle(&mut self) {
        let mut drained = Vec::new();
        let mut closed = false;
        if let Some(rx) = self.loop_lifecycle_rx.as_mut() {
            loop {
                match rx.try_recv() {
                    Ok(notification) => drained.push(notification),
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            skipped,
                            "ACP runtime lagged while draining native loop lifecycle notifications"
                        );
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                        closed = true;
                        break;
                    }
                }
            }
        }
        if closed {
            self.loop_lifecycle_rx = None;
        }
        let Some(thread_id) = self.background_loop_thread_id() else {
            return;
        };
        for notification in drained {
            self.queue_loop_lifecycle_notification(&thread_id, notification);
        }
    }

    async fn next_event(&mut self) -> Option<AppServerEvent> {
        loop {
            self.drain_control_messages().await;
            self.drain_native_loop_lifecycle();
            if let Some(event) = self.pending_events.pop_front() {
                self.record_ilhae_event(&event);
                return Some(event);
            }

            match tokio::time::timeout(Duration::from_millis(250), self.event_rx.recv()).await {
                Ok(event) => {
                    self.drain_control_messages().await;
                    self.drain_native_loop_lifecycle();
                    if let Some(ref event) = event {
                        self.record_ilhae_event(event);
                    }
                    return event;
                }
                Err(_) => continue,
            }
        }
    }

    async fn next_ilhae_event(&mut self) -> Option<IlhaeAppSessionEventNotification> {
        self.drain_control_messages().await;
        self.pending_ilhae_events.pop_front()
    }

    fn current_thread_from_session(
        &self,
        session: SessionInfo,
        include_turns: bool,
    ) -> Result<Thread> {
        let mut thread = self.to_thread(session.clone());
        if include_turns {
            let messages = self
                .runtime_context()?
                .brain
                .session_load_messages(&session.id)
                .map_err(|err| {
                    color_eyre::eyre::eyre!("failed to load ACP session history from brain: {err}")
                })?;
            thread.turns = brain_messages_to_turns(messages);
        }
        Ok(thread)
    }

    fn record_ilhae_event(&mut self, event: &AppServerEvent) {
        if matches!(event, AppServerEvent::ServerRequest(_)) {
            return;
        }
        if let Some(notification) =
            canonical_ilhae_event_from_app_server_event(&self.engine_id, event)
        {
            self.pending_ilhae_events.push_back(notification);
        }
    }

    async fn ensure_session_handle(
        &mut self,
        global_session_id: &str,
        cwd: &std::path::Path,
    ) -> Result<Arc<Mutex<AcpAgentSession>>> {
        self.drain_finished_turn();
        if self.active_global_thread_id.as_deref() == Some(global_session_id) {
            if let Some(session) = self.session.as_ref() {
                return Ok(session.clone());
            }
        }

        let runtime = self.runtime_context()?;
        runtime
            .brain
            .session_ensure(
                global_session_id,
                &self.engine_id,
                &self.engine_id,
                &cwd.to_string_lossy(),
            )
            .map_err(|err| color_eyre::eyre::eyre!(err.to_string()))?;

        let command = self.current_command()?;
        let agent = AcpAgent::from_str(&command)
            .wrap_err_with(|| format!("failed to parse ACP backend command `{command}`"))?;
        let session = AcpAgentSession::connect(agent, cwd).await.map_err(|err| {
            color_eyre::eyre::eyre!("failed to connect ACP backend `{}`: {err}", self.engine_id)
        })?;
        let local_session_id = session.session_id().to_string();
        runtime
            .brain
            .session_upsert_engine_ref(global_session_id, &self.engine_id, &local_session_id)
            .map_err(|err| color_eyre::eyre::eyre!(err.to_string()))?;

        let session = Arc::new(Mutex::new(session));
        self.session = Some(session.clone());
        self.active_global_thread_id = Some(global_session_id.to_string());
        Ok(session)
    }

    async fn start_thread_internal(
        &mut self,
        config: &Config,
        global_session_id: ThreadId,
    ) -> Result<AppServerStartedThread> {
        let runtime = self.runtime_context()?;
        let global_session_id_str = global_session_id.to_string();
        runtime
            .brain
            .session_ensure(
                &global_session_id_str,
                &self.engine_id,
                &self.engine_id,
                &config.cwd.to_string_lossy(),
            )
            .map_err(|err| color_eyre::eyre::eyre!(err.to_string()))?;
        let existing_info = self
            .runtime_session_store()?
            .into_iter()
            .find(|info| info.id == global_session_id_str);

        let session = thread_session_state_from_thread_response(
            &global_session_id_str,
            None,
            existing_info
                .as_ref()
                .and_then(|info| (!info.title.trim().is_empty()).then_some(info.title.clone())),
            None,
            self.current_model(config),
            self.engine_id.clone(),
            config.service_tier.clone(),
            config.permissions.approval_policy.value(),
            config.approvals_reviewer,
            config.permissions.sandbox_policy.get().clone(),
            config.cwd.clone(),
            Vec::new(),
            config.model_reasoning_effort,
            config,
        )
        .await
        .map_err(color_eyre::eyre::Report::msg)?;

        let thread = existing_info
            .map(|info| self.current_thread_from_session(info, true))
            .transpose()?
            .unwrap_or(Thread {
                forked_from_id: None,
                id: global_session_id_str.clone(),
                preview: global_session_id_str.clone(),
                ephemeral: config.ephemeral,
                model_provider: self.engine_id.clone(),
                created_at: Utc::now().timestamp(),
                updated_at: Utc::now().timestamp(),
                status: ThreadStatus::Idle,
                path: None,
                cwd: config.cwd.clone(),
                cli_version: env!("CARGO_PKG_VERSION").to_string(),
                source: SessionSource::AppServer,
                agent_nickname: None,
                agent_role: None,
                git_info: None,
                name: None,
                turns: Vec::new(),
            });
        self.pending_events
            .push_back(AppServerEvent::ServerNotification(
                ServerNotification::ThreadStarted(ThreadStartedNotification {
                    thread: thread.clone(),
                }),
            ));

        Ok(AppServerStartedThread {
            session,
            turns: thread.turns.clone(),
        })
    }
}

#[async_trait]
pub(crate) trait ConversationRuntime {
    fn is_remote(&self) -> bool;

    fn remote_cwd_override(&self) -> Option<&std::path::Path> {
        None
    }

    async fn list_threads(&mut self, params: ThreadListParams) -> Result<ThreadListResponse>;

    async fn read_thread(&mut self, thread_id: ThreadId, include_turns: bool) -> Result<Thread>;

    async fn start_thread(&mut self, config: &Config) -> Result<AppServerStartedThread>;

    async fn resume_thread(
        &mut self,
        config: Config,
        thread_id: ThreadId,
    ) -> Result<AppServerStartedThread>;

    async fn send_turn(&mut self, request: ConversationTurnRequest) -> Result<TurnStartResponse>;

    async fn interrupt_turn(&mut self, thread_id: ThreadId, turn_id: String) -> Result<()>;

    async fn close(self) -> std::io::Result<()>
    where
        Self: Sized;
}

#[async_trait]
impl ConversationRuntime for AcpConversationRuntime {
    fn is_remote(&self) -> bool {
        false
    }

    async fn list_threads(&mut self, params: ThreadListParams) -> Result<ThreadListResponse> {
        if params.archived == Some(true) || !Self::thread_source_matches(&params) {
            return Ok(ThreadListResponse {
                data: Vec::new(),
                next_cursor: None,
            });
        }

        if let Some(model_providers) = params.model_providers.as_ref() {
            let allow_current_engine = model_providers.is_empty()
                || model_providers
                    .iter()
                    .any(|provider| provider.eq_ignore_ascii_case(&self.engine_id));
            if !allow_current_engine {
                return Ok(ThreadListResponse {
                    data: Vec::new(),
                    next_cursor: None,
                });
            }
        }

        let mut sessions = self
            .runtime_session_store()?
            .into_iter()
            .filter(|session| self.matches_engine(session))
            .filter(|session| {
                params
                    .cwd
                    .as_ref()
                    .map(|cwd| session.cwd == *cwd)
                    .unwrap_or(true)
            })
            .filter(|session| {
                params
                    .search_term
                    .as_ref()
                    .map(|term| {
                        let term = term.to_ascii_lowercase();
                        session.title.to_ascii_lowercase().contains(&term)
                            || session.id.to_ascii_lowercase().contains(&term)
                    })
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>();

        match params.sort_key.unwrap_or(ThreadSortKey::UpdatedAt) {
            ThreadSortKey::CreatedAt => sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
            ThreadSortKey::UpdatedAt => sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
        }

        let start = Self::parse_cursor(params.cursor.as_deref());
        let limit = params.limit.unwrap_or(50) as usize;
        let end = start.saturating_add(limit).min(sessions.len());
        let next_cursor = (end < sessions.len()).then(|| end.to_string());
        let data = sessions[start.min(sessions.len())..end]
            .iter()
            .cloned()
            .map(|session| self.to_thread(session))
            .collect();

        Ok(ThreadListResponse { data, next_cursor })
    }

    async fn read_thread(&mut self, thread_id: ThreadId, _include_turns: bool) -> Result<Thread> {
        let session_id = thread_id.to_string();
        let session = self
            .runtime_session_store()?
            .into_iter()
            .find(|session| session.id == session_id && self.matches_engine(session))
            .context("ACP runtime thread not found")?;
        self.current_thread_from_session(session, _include_turns)
    }

    async fn start_thread(&mut self, config: &Config) -> Result<AppServerStartedThread> {
        self.start_thread_internal(config, ThreadId::new()).await
    }

    async fn resume_thread(
        &mut self,
        config: Config,
        thread_id: ThreadId,
    ) -> Result<AppServerStartedThread> {
        self.start_thread_internal(&config, thread_id).await
    }

    async fn send_turn(&mut self, request: ConversationTurnRequest) -> Result<TurnStartResponse> {
        self.drain_finished_turn();
        if self.active_turn.is_some() {
            color_eyre::eyre::bail!(
                "ACP backend `{}` already has an active turn in progress",
                self.engine_id
            );
        }

        let thread_id = request.thread_id.to_string();
        let session = self.ensure_session_handle(&thread_id, &request.cwd).await?;
        let raw_user_text = request
            .items
            .iter()
            .filter_map(|item| match item {
                codex_protocol::user_input::UserInput::Text { text, .. }
                    if !text.trim().is_empty() =>
                {
                    Some(text.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let items = codex_ilhae::prepare_session_turn_inputs(
            &thread_id,
            &self.engine_id,
            request.items.clone(),
        )
        .await
        .unwrap_or(request.items.clone());
        let prompt = items
            .iter()
            .filter_map(|item| match item {
                codex_protocol::user_input::UserInput::Text { text, .. }
                    if !text.trim().is_empty() =>
                {
                    Some(text.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let turn_id = self.next_synthetic_id("acp-turn");
        let assistant_item_id = self.next_synthetic_id("acp-agent");
        let reasoning_item_id = self.next_synthetic_id("acp-thought");
        let initial_turn = Turn {
            id: turn_id.clone(),
            items: Vec::new(),
            status: TurnStatus::InProgress,
            error: None,
            started_at: None,
            completed_at: None,
            duration_ms: None,
        };
        self.pending_events
            .push_back(AppServerEvent::ServerNotification(
                ServerNotification::TurnStarted(TurnStartedNotification {
                    thread_id: thread_id.clone(),
                    turn: initial_turn.clone(),
                }),
            ));
        let execution_loop_item_id = format!("{turn_id}:execution_loop");
        self.pending_events
            .push_back(AppServerEvent::ServerNotification(
                ServerNotification::ItemStarted(ItemStartedNotification {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: ThreadItem::LoopLifecycle {
                        id: execution_loop_item_id.clone(),
                        kind: LoopLifecycleKind::ExecutionLoop,
                        title: "Running Execution Loop".to_string(),
                        summary: "Executor turn started".to_string(),
                        detail: None,
                        status: LoopLifecycleStatus::InProgress,
                        reason: Some("turn_start".to_string()),
                        counts: None,
                        error: None,
                        duration_ms: None,
                        target_profile: None,
                    },
                }),
            ));

        let runtime = self.runtime_context()?;
        let _ = runtime.brain.session_add_message_simple(
            &thread_id,
            "user",
            &raw_user_text,
            &self.engine_id,
        );

        let (cancel_session_id, cancel_connection) = {
            let mut locked = session.lock().await;
            locked
                .send_prompt(&prompt)
                .map_err(|err| color_eyre::eyre::eyre!("failed to send ACP prompt: {err}"))?;
            (
                locked.active_session().session_id().clone(),
                locked.active_session().connection(),
            )
        };

        let event_tx = self.event_tx.clone();
        let control_tx = self.control_tx.clone();
        let brain = runtime.brain.clone();
        let engine_id = self.engine_id.clone();
        let thread_id_for_task = thread_id.clone();
        let turn_id_for_task = turn_id.clone();
        let execution_loop_item_id_for_task = execution_loop_item_id.clone();
        let assistant_item_id_for_task = assistant_item_id.clone();
        let reasoning_item_id_for_task = reasoning_item_id.clone();
        let task = tokio::spawn(async move {
            let mut assistant_text = String::new();
            let mut reasoning_text = String::new();
            let mut assistant_started = false;
            let mut reasoning_started = false;
            let mut tool_calls: HashMap<String, (String, serde_json::Value, bool)> = HashMap::new();
            let mut verification_loops: HashMap<String, VerificationLoopTracker> = HashMap::new();
            let final_status = loop {
                let update = {
                    let mut locked = session.lock().await;
                    locked.read_update().await
                };
                let update = match update {
                    Ok(update) => update,
                    Err(err) => {
                        let _ = event_tx.send(AppServerEvent::ServerNotification(
                            ServerNotification::TurnCompleted(TurnCompletedNotification {
                                thread_id: thread_id_for_task.clone(),
                                turn: Turn {
                                    id: turn_id_for_task.clone(),
                                    items: Vec::new(),
                                    status: TurnStatus::Failed,
                                    error: Some(codex_app_server_protocol::TurnError {
                                        message: format!("ACP read failed: {err}"),
                                        codex_error_info: None,
                                        additional_details: None,
                                    }),
                                    started_at: None,
                                    completed_at: None,
                                    duration_ms: None,
                                },
                            }),
                        ));
                        return;
                    }
                };

                match update {
                    SessionMessage::StopReason(stop_reason) => {
                        break match stop_reason {
                            agent_client_protocol_schema::StopReason::Cancelled => {
                                TurnStatus::Interrupted
                            }
                            agent_client_protocol_schema::StopReason::EndTurn
                            | agent_client_protocol_schema::StopReason::MaxTokens
                            | agent_client_protocol_schema::StopReason::MaxTurnRequests
                            | agent_client_protocol_schema::StopReason::Refusal => {
                                TurnStatus::Completed
                            }
                            _ => TurnStatus::Failed,
                        };
                    }
                    SessionMessage::SessionMessage(dispatch) => {
                        let _ = MatchDispatch::new(dispatch)
                            .if_request(
                                async |req: RequestPermissionRequest,
                                       responder: Responder<RequestPermissionResponse>| {
                                    let tool_title = req
                                        .tool_call
                                        .fields
                                        .title
                                        .clone()
                                        .unwrap_or_else(|| "ACP permission request".to_string());
                                    let synthetic_id = format!(
                                        "{turn_id_for_task}-acp-permission-{}",
                                        req.tool_call.tool_call_id
                                    );
                                    let interactive_request = IlhaeInteractiveRequestDto {
                                        source: "acp".to_string(),
                                        thread_id: thread_id_for_task.clone(),
                                        turn_id: turn_id_for_task.clone(),
                                        request_id: synthetic_id.clone(),
                                        title: tool_title.clone(),
                                        reason: Some(format!(
                                            "ACP backend `{}` is requesting permission for `{}`",
                                            engine_id.clone(),
                                            tool_title
                                        )),
                                        requested_permissions: None,
                                        options: req
                                            .options
                                            .iter()
                                            .map(|option| IlhaeInteractiveOptionDto {
                                                id: option.option_id.to_string(),
                                                label: option.name.clone(),
                                                kind: ilhae_interactive_option_kind(option.kind),
                                            })
                                            .collect(),
                                    };

                                    let command = req
                                        .tool_call
                                        .fields
                                        .raw_input
                                        .as_ref()
                                        .map(|raw| format!("{tool_title} {}", raw))
                                        .or_else(|| Some(tool_title.clone()));

                                    let _ = control_tx.send(
                                        AcpRuntimeControlMessage::RegisterPermissionRequest {
                                            synthetic_id: synthetic_id.clone(),
                                            interactive_request,
                                            request: req.clone(),
                                            responder,
                                        },
                                    );

                                    let _ = event_tx.send(AppServerEvent::ServerRequest(
                                        codex_app_server_protocol::ServerRequest::CommandExecutionRequestApproval {
                                            request_id: RequestId::String(synthetic_id.clone()),
                                            params: CommandExecutionRequestApprovalParams {
                                                thread_id: thread_id_for_task.clone(),
                                                turn_id: turn_id_for_task.clone(),
                                                item_id: req.tool_call.tool_call_id.to_string(),
                                                approval_id: Some(synthetic_id),
                                                reason: Some(format!(
                                                    "ACP backend `{}` is requesting permission for `{}`",
                                                    engine_id.clone(),
                                                    tool_title
                                                )),
                                                network_approval_context: None,
                                                command,
                                                cwd: None,
                                                command_actions: None,
                                                additional_permissions: None,
                                                proposed_execpolicy_amendment: None,
                                                proposed_network_policy_amendments: None,
                                                available_decisions: Some(vec![
                                                    CommandExecutionApprovalDecision::Accept,
                                                    CommandExecutionApprovalDecision::AcceptForSession,
                                                    CommandExecutionApprovalDecision::Decline,
                                                    CommandExecutionApprovalDecision::Cancel,
                                                ]),
                                            },
                                        },
                                    ));
                                    let _ = event_tx.send(AppServerEvent::ServerNotification(
                                        ServerNotification::LoopLifecycleProgress(
                                            codex_app_server_protocol::LoopLifecycleProgressNotification {
                                                thread_id: thread_id_for_task.clone(),
                                                turn_id: turn_id_for_task.clone(),
                                                item_id: execution_loop_item_id_for_task.clone(),
                                                kind: LoopLifecycleKind::ExecutionLoop,
                                                summary: "Waiting for approval".to_string(),
                                                detail: Some(tool_title),
                                                counts: None,
                                            },
                                        ),
                                    ));

                                    Ok(())
                                },
                            )
                            .await
                            .if_notification(async |notif: IlhaeLoopLifecycleNotification| {
                                for notification in loop_lifecycle_server_notifications(
                                    &thread_id_for_task,
                                    &turn_id_for_task,
                                    notif,
                                ) {
                                    let _ = event_tx
                                        .send(AppServerEvent::ServerNotification(notification));
                                }
                                Ok(())
                            })
                            .await
                            .if_notification(async |notif: SessionNotification| {
                                match notif.update {
                                    SessionUpdate::AgentMessageChunk(chunk) => {
                                        if let ContentBlock::Text(text) = chunk.content {
                                            if !assistant_started {
                                                assistant_started = true;
                                                let _ = event_tx.send(AppServerEvent::ServerNotification(
                                                    ServerNotification::ItemStarted(ItemStartedNotification {
                                                        thread_id: thread_id_for_task.clone(),
                                                        turn_id: turn_id_for_task.clone(),
                                                        item: ThreadItem::AgentMessage {
                                                            id: assistant_item_id_for_task.clone(),
                                                            text: String::new(),
                                                            phase: None,
                                                            memory_citation: None,
                                                        },
                                                    }),
                                                ));
                                            }
                                            assistant_text.push_str(&text.text);
                                            let _ = event_tx.send(AppServerEvent::ServerNotification(
                                                ServerNotification::AgentMessageDelta(
                                                    AgentMessageDeltaNotification {
                                                        thread_id: thread_id_for_task.clone(),
                                                        turn_id: turn_id_for_task.clone(),
                                                        item_id: assistant_item_id_for_task.clone(),
                                                        delta: text.text,
                                                    },
                                                ),
                                            ));
                                            let _ = event_tx.send(AppServerEvent::ServerNotification(
                                                ServerNotification::LoopLifecycleProgress(
                                                    codex_app_server_protocol::LoopLifecycleProgressNotification {
                                                        thread_id: thread_id_for_task.clone(),
                                                        turn_id: turn_id_for_task.clone(),
                                                        item_id: execution_loop_item_id_for_task.clone(),
                                                        kind: LoopLifecycleKind::ExecutionLoop,
                                                        summary: "Model response streaming".to_string(),
                                                        detail: None,
                                                        counts: None,
                                                    },
                                                ),
                                            ));
                                        }
                                        Ok(())
                                    }
                                    SessionUpdate::AgentThoughtChunk(chunk) => {
                                        if let ContentBlock::Text(text) = chunk.content {
                                            if !reasoning_started {
                                                reasoning_started = true;
                                                let _ = event_tx.send(AppServerEvent::ServerNotification(
                                                    ServerNotification::ItemStarted(ItemStartedNotification {
                                                        thread_id: thread_id_for_task.clone(),
                                                        turn_id: turn_id_for_task.clone(),
                                                        item: ThreadItem::Reasoning {
                                                            id: reasoning_item_id_for_task.clone(),
                                                            summary: Vec::new(),
                                                            content: Vec::new(),
                                                        },
                                                    }),
                                                ));
                                            }
                                            reasoning_text.push_str(&text.text);
                                            let _ = event_tx.send(AppServerEvent::ServerNotification(
                                                ServerNotification::ReasoningTextDelta(
                                                    ReasoningTextDeltaNotification {
                                                        thread_id: thread_id_for_task.clone(),
                                                        turn_id: turn_id_for_task.clone(),
                                                        item_id: reasoning_item_id_for_task.clone(),
                                                        delta: text.text,
                                                        content_index: 0,
                                                    },
                                                ),
                                            ));
                                        }
                                        Ok(())
                                    }
                                    SessionUpdate::ToolCall(tool_call) => {
                                        let tool_call_id = tool_call.tool_call_id.to_string();
                                        let tool = tool_call.title.clone();
                                        let arguments = tool_call
                                            .raw_input
                                            .clone()
                                            .unwrap_or_else(|| serde_json::json!({}));
                                        tool_calls.insert(
                                            tool_call_id.clone(),
                                            (tool.clone(), arguments.clone(), true),
                                        );
                                        let _ = event_tx.send(AppServerEvent::ServerNotification(
                                            ServerNotification::ItemStarted(ItemStartedNotification {
                                                thread_id: thread_id_for_task.clone(),
                                                turn_id: turn_id_for_task.clone(),
                                                item: ThreadItem::DynamicToolCall {
                                                    id: tool_call_id.clone(),
                                                    tool: tool.clone(),
                                                    arguments: arguments.clone(),
                                                    status: DynamicToolCallStatus::InProgress,
                                                    content_items: None,
                                                    success: None,
                                                    duration_ms: None,
                                                },
                                            }),
                                        ));
                                        start_verification_loop(
                                            &event_tx,
                                            &mut verification_loops,
                                            &tool_call_id,
                                            &tool,
                                            &arguments,
                                            &thread_id_for_task,
                                            &turn_id_for_task,
                                        );
                                        let _ = event_tx.send(AppServerEvent::ServerNotification(
                                            ServerNotification::LoopLifecycleProgress(
                                                codex_app_server_protocol::LoopLifecycleProgressNotification {
                                                    thread_id: thread_id_for_task.clone(),
                                                    turn_id: turn_id_for_task.clone(),
                                                    item_id: execution_loop_item_id_for_task.clone(),
                                                    kind: LoopLifecycleKind::ExecutionLoop,
                                                    summary: "Running tool step".to_string(),
                                                    detail: Some(tool),
                                                    counts: None,
                                                },
                                            ),
                                        ));
                                        Ok(())
                                    }
                                    SessionUpdate::ToolCallUpdate(update) => {
                                        let tool_call_id = update.tool_call_id.to_string();
                                        let entry = tool_calls.entry(tool_call_id.clone()).or_insert_with(|| {
                                            (
                                                update
                                                    .fields
                                                    .title
                                                    .clone()
                                                    .unwrap_or_else(|| "tool".to_string()),
                                                update
                                                    .fields
                                                    .raw_input
                                                    .clone()
                                                    .unwrap_or_else(|| serde_json::json!({})),
                                                false,
                                            )
                                        });
                                        if let Some(title) = update.fields.title.clone() {
                                            entry.0 = title;
                                        }
                                        if let Some(raw_input) = update.fields.raw_input.clone() {
                                            entry.1 = raw_input;
                                        }
                                        let Some(status) = update.fields.status.clone() else {
                                            return Ok(());
                                        };

                                        if matches!(
                                            status,
                                            agent_client_protocol_schema::ToolCallStatus::Pending
                                                | agent_client_protocol_schema::ToolCallStatus::InProgress
                                        ) && !entry.2
                                        {
                                            entry.2 = true;
                                            let _ = event_tx.send(AppServerEvent::ServerNotification(
                                                ServerNotification::ItemStarted(
                                                    ItemStartedNotification {
                                                        thread_id: thread_id_for_task.clone(),
                                                        turn_id: turn_id_for_task.clone(),
                                                        item: ThreadItem::DynamicToolCall {
                                                            id: tool_call_id.clone(),
                                                            tool: entry.0.clone(),
                                                            arguments: entry.1.clone(),
                                                            status: DynamicToolCallStatus::InProgress,
                                                            content_items: None,
                                                            success: None,
                                                            duration_ms: None,
                                                        },
                                                    },
                                                ),
                                            ));
                                            return Ok(());
                                        }

                                        if matches!(
                                            status,
                                            agent_client_protocol_schema::ToolCallStatus::Completed
                                                | agent_client_protocol_schema::ToolCallStatus::Failed
                                        ) {
                                            let _ = event_tx.send(AppServerEvent::ServerNotification(
                                                ServerNotification::ItemCompleted(
                                                    ItemCompletedNotification {
                                                        thread_id: thread_id_for_task.clone(),
                                                        turn_id: turn_id_for_task.clone(),
                                                        item: ThreadItem::DynamicToolCall {
                                                            id: tool_call_id.clone(),
                                                            tool: entry.0.clone(),
                                                            arguments: entry.1.clone(),
                                                            status: match status {
                                                                agent_client_protocol_schema::ToolCallStatus::Completed => DynamicToolCallStatus::Completed,
                                                                agent_client_protocol_schema::ToolCallStatus::Failed => DynamicToolCallStatus::Failed,
                                                                _ => DynamicToolCallStatus::InProgress,
                                                            },
                                                            content_items: update
                                                                .fields
                                                                .content
                                                                .as_ref()
                                                                .and_then(|content| acp_tool_content_items(content)),
                                                            success: Some(matches!(
                                                                status,
                                                                agent_client_protocol_schema::ToolCallStatus::Completed
                                                            )),
                                                            duration_ms: None,
                                                        },
                                                    },
                                                ),
                                            ));
                                            finish_verification_loop(
                                                &event_tx,
                                                &mut verification_loops,
                                                &tool_call_id,
                                                &thread_id_for_task,
                                                &turn_id_for_task,
                                                matches!(
                                                    status,
                                                    agent_client_protocol_schema::ToolCallStatus::Completed
                                                ),
                                            );
                                        }
                                        Ok(())
                                    }
                                    _ => Ok(()),
                                }
                            })
                            .await
                            .otherwise_ignore();
                    }
                    _ => {}
                }
            };

            if reasoning_started {
                let _ = event_tx.send(AppServerEvent::ServerNotification(
                    ServerNotification::ItemCompleted(ItemCompletedNotification {
                        thread_id: thread_id_for_task.clone(),
                        turn_id: turn_id_for_task.clone(),
                        item: ThreadItem::Reasoning {
                            id: reasoning_item_id_for_task.clone(),
                            summary: Vec::new(),
                            content: if reasoning_text.is_empty() {
                                Vec::new()
                            } else {
                                vec![reasoning_text]
                            },
                        },
                    }),
                ));
            }

            let assistant_char_count = assistant_text.chars().count() as i64;
            if assistant_started {
                let _ = brain.session_add_message_simple(
                    &thread_id_for_task,
                    "assistant",
                    &assistant_text,
                    &engine_id,
                );
                let _ = event_tx.send(AppServerEvent::ServerNotification(
                    ServerNotification::ItemCompleted(ItemCompletedNotification {
                        thread_id: thread_id_for_task.clone(),
                        turn_id: turn_id_for_task.clone(),
                        item: ThreadItem::AgentMessage {
                            id: assistant_item_id_for_task.clone(),
                            text: assistant_text,
                            phase: None,
                            memory_citation: None,
                        },
                    }),
                ));
            }

            for tool_call_id in verification_loops.keys().cloned().collect::<Vec<_>>() {
                finish_verification_loop(
                    &event_tx,
                    &mut verification_loops,
                    &tool_call_id,
                    &thread_id_for_task,
                    &turn_id_for_task,
                    matches!(final_status, TurnStatus::Completed),
                );
            }

            let _ = event_tx.send(AppServerEvent::ServerNotification(
                ServerNotification::ItemCompleted(ItemCompletedNotification {
                    thread_id: thread_id_for_task.clone(),
                    turn_id: turn_id_for_task.clone(),
                    item: ThreadItem::LoopLifecycle {
                        id: execution_loop_item_id_for_task,
                        kind: LoopLifecycleKind::ExecutionLoop,
                        title: "Running Execution Loop".to_string(),
                        summary: match final_status {
                            TurnStatus::Completed => "Execution loop completed".to_string(),
                            TurnStatus::Interrupted => "Execution loop interrupted".to_string(),
                            TurnStatus::Failed => "Execution loop failed".to_string(),
                            _ => "Execution loop stopped".to_string(),
                        },
                        detail: None,
                        status: match final_status {
                            TurnStatus::Completed => LoopLifecycleStatus::Completed,
                            TurnStatus::Interrupted | TurnStatus::Failed => {
                                LoopLifecycleStatus::Failed
                            }
                            _ => LoopLifecycleStatus::Completed,
                        },
                        reason: Some(match final_status {
                            TurnStatus::Completed => "turn_complete".to_string(),
                            TurnStatus::Interrupted => "turn_interrupted".to_string(),
                            TurnStatus::Failed => "turn_failed".to_string(),
                            _ => "turn_stopped".to_string(),
                        }),
                        counts: Some(std::collections::BTreeMap::from([
                            ("tool_calls".to_string(), tool_calls.len() as i64),
                            ("assistant_chars".to_string(), assistant_char_count),
                        ])),
                        error: None,
                        duration_ms: None,
                        target_profile: None,
                    },
                }),
            ));

            let _ = event_tx.send(AppServerEvent::ServerNotification(
                ServerNotification::TurnCompleted(TurnCompletedNotification {
                    thread_id: thread_id_for_task,
                    turn: Turn {
                        id: turn_id_for_task,
                        items: Vec::new(),
                        status: final_status,
                        error: None,
                        started_at: None,
                        completed_at: None,
                        duration_ms: None,
                    },
                }),
            ));
        });

        self.active_turn = Some(AcpActiveTurn {
            thread_id,
            turn_id: turn_id.clone(),
            cancel_session_id,
            cancel_connection,
            task,
        });
        Ok(TurnStartResponse { turn: initial_turn })
    }

    async fn interrupt_turn(&mut self, thread_id: ThreadId, turn_id: String) -> Result<()> {
        self.drain_finished_turn();
        if let Some(active) = self.active_turn.as_ref() {
            if active.thread_id == thread_id.to_string() && active.turn_id == turn_id {
                match active
                    .cancel_connection
                    .send_notification(CancelNotification::new(active.cancel_session_id.clone()))
                {
                    Ok(()) => return Ok(()),
                    Err(err) => {
                        tracing::warn!(
                            "failed to send ACP session/cancel for thread {} turn {}: {}",
                            thread_id,
                            turn_id,
                            err
                        );
                    }
                }
            }
        }
        if let Some(active) = self.active_turn.take() {
            if active.thread_id == thread_id.to_string() && active.turn_id == turn_id {
                active.task.abort();
                self.session = None;
                self.active_global_thread_id = None;
                self.pending_events
                    .push_back(AppServerEvent::ServerNotification(
                        ServerNotification::TurnCompleted(TurnCompletedNotification {
                            thread_id: thread_id.to_string(),
                            turn: Turn {
                                id: turn_id,
                                items: Vec::new(),
                                status: TurnStatus::Interrupted,
                                error: None,
                                started_at: None,
                                completed_at: None,
                                duration_ms: None,
                            },
                        }),
                    ));
                return Ok(());
            }
            self.active_turn = Some(active);
        }
        Ok(())
    }

    async fn close(mut self) -> std::io::Result<()> {
        if let Some(active) = self.active_turn.take() {
            active.task.abort();
        }
        Ok(())
    }
}

#[async_trait]
impl ConversationRuntime for AppServerSession {
    fn is_remote(&self) -> bool {
        AppServerSession::is_remote(self)
    }

    fn remote_cwd_override(&self) -> Option<&std::path::Path> {
        AppServerSession::remote_cwd_override(self)
    }

    async fn list_threads(&mut self, params: ThreadListParams) -> Result<ThreadListResponse> {
        AppServerSession::thread_list(self, params).await
    }

    async fn read_thread(&mut self, thread_id: ThreadId, include_turns: bool) -> Result<Thread> {
        AppServerSession::thread_read(self, thread_id, include_turns).await
    }

    async fn start_thread(&mut self, config: &Config) -> Result<AppServerStartedThread> {
        AppServerSession::start_thread(self, config).await
    }

    async fn resume_thread(
        &mut self,
        config: Config,
        thread_id: ThreadId,
    ) -> Result<AppServerStartedThread> {
        AppServerSession::resume_thread(self, config, thread_id).await
    }

    async fn send_turn(&mut self, request: ConversationTurnRequest) -> Result<TurnStartResponse> {
        AppServerSession::turn_start(
            self,
            request.thread_id,
            request.items,
            request.cwd,
            request.approval_policy,
            request.approvals_reviewer,
            request.sandbox_policy,
            request.model,
            request.effort,
            request.summary,
            request.service_tier,
            request.collaboration_mode,
            request.personality,
            request.output_schema,
        )
        .await
    }

    async fn interrupt_turn(&mut self, thread_id: ThreadId, turn_id: String) -> Result<()> {
        AppServerSession::turn_interrupt(self, thread_id, turn_id).await
    }

    async fn close(self) -> std::io::Result<()> {
        AppServerSession::shutdown(self).await
    }
}

pub(crate) enum SelectedConversationRuntime {
    AppServer(AppServerSession),
    Acp(AcpConversationRuntime),
}

impl SelectedConversationRuntime {
    pub(crate) fn with_remote_cwd_override(self, override_path: Option<PathBuf>) -> Self {
        match self {
            Self::AppServer(runtime) => {
                Self::AppServer(runtime.with_remote_cwd_override(override_path))
            }
            Self::Acp(runtime) => Self::Acp(runtime),
        }
    }

    pub(crate) fn from_native(app_server: AppServerSession) -> Self {
        Self::AppServer(app_server)
    }

    pub(crate) fn from_acp(engine_id: impl Into<String>) -> Self {
        Self::Acp(AcpConversationRuntime::new(engine_id))
    }

    fn unsupported<T>(&self, action: &str) -> Result<T> {
        match self {
            Self::AppServer(_) => unreachable!("native app-server runtime should support {action}"),
            Self::Acp(runtime) => runtime.unsupported(action),
        }
    }

    pub(crate) async fn memory_reset(&mut self) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.memory_reset().await,
            Self::Acp(_) => self.unsupported("memory_reset"),
        }
    }

    pub(crate) async fn thread_memory_mode_set(
        &mut self,
        thread_id: ThreadId,
        mode: ThreadMemoryMode,
    ) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.thread_memory_mode_set(thread_id, mode).await,
            Self::Acp(_) => self.unsupported("thread_memory_mode_set"),
        }
    }

    pub(crate) async fn start_thread_with_session_start_source(
        &mut self,
        config: &Config,
        session_start_source: Option<ThreadStartSource>,
    ) -> Result<AppServerStartedThread> {
        match self {
            Self::AppServer(runtime) => {
                runtime
                    .start_thread_with_session_start_source(config, session_start_source)
                    .await
            }
            Self::Acp(runtime) => runtime.start_thread(config).await,
        }
    }

    pub(crate) async fn bootstrap(&mut self, config: &Config) -> Result<AppServerBootstrap> {
        match self {
            Self::AppServer(runtime) => runtime.bootstrap(config).await,
            Self::Acp(runtime) => runtime.bootstrap(config).await,
        }
    }

    pub(crate) async fn next_event(&mut self) -> Option<AppServerEvent> {
        match self {
            Self::AppServer(runtime) => runtime.next_event().await,
            Self::Acp(runtime) => runtime.next_event().await,
        }
    }

    pub(crate) async fn next_ilhae_event(&mut self) -> Option<IlhaeAppSessionEventNotification> {
        match self {
            Self::AppServer(runtime) => runtime.next_ilhae_event().await,
            Self::Acp(runtime) => runtime.next_ilhae_event().await,
        }
    }

    pub(crate) async fn fork_thread(
        &mut self,
        config: Config,
        thread_id: ThreadId,
    ) -> Result<AppServerStartedThread> {
        match self {
            Self::AppServer(runtime) => runtime.fork_thread(config, thread_id).await,
            Self::Acp(_) => self.unsupported("thread/fork"),
        }
    }

    pub(crate) async fn thread_loaded_list(
        &mut self,
        params: ThreadLoadedListParams,
    ) -> Result<ThreadLoadedListResponse> {
        match self {
            Self::AppServer(runtime) => runtime.thread_loaded_list(params).await,
            Self::Acp(_) => Ok(ThreadLoadedListResponse {
                data: Vec::new(),
                next_cursor: None,
            }),
        }
    }

    pub(crate) async fn thread_read(
        &mut self,
        thread_id: ThreadId,
        include_turns: bool,
    ) -> Result<Thread> {
        match self {
            Self::AppServer(runtime) => runtime.thread_read(thread_id, include_turns).await,
            Self::Acp(runtime) => runtime.read_thread(thread_id, include_turns).await,
        }
    }

    pub(crate) async fn thread_unsubscribe(&mut self, thread_id: ThreadId) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.thread_unsubscribe(thread_id).await,
            Self::Acp(_) => self.unsupported("thread/unsubscribe"),
        }
    }

    pub(crate) async fn turn_steer(
        &mut self,
        thread_id: ThreadId,
        turn_id: String,
        items: Vec<codex_protocol::user_input::UserInput>,
    ) -> std::result::Result<TurnSteerResponse, TypedRequestError> {
        match self {
            Self::AppServer(runtime) => runtime.turn_steer(thread_id, turn_id, items).await,
            Self::Acp(_) => Err(TypedRequestError::Transport {
                method: "turn/steer".to_string(),
                source: std::io::Error::other("turn/steer is not implemented for ACP runtime"),
            }),
        }
    }

    pub(crate) async fn turn_interrupt(
        &mut self,
        thread_id: ThreadId,
        turn_id: String,
    ) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.turn_interrupt(thread_id, turn_id).await,
            Self::Acp(runtime) => runtime.interrupt_turn(thread_id, turn_id).await,
        }
    }

    pub(crate) async fn startup_interrupt(&mut self, thread_id: ThreadId) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.startup_interrupt(thread_id).await,
            Self::Acp(runtime) => runtime.interrupt_turn(thread_id, String::new()).await,
        }
    }

    pub(crate) async fn logout_account(&mut self) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.logout_account().await,
            Self::Acp(_) => self.unsupported("account/logout"),
        }
    }

    pub(crate) async fn thread_set_name(
        &mut self,
        thread_id: ThreadId,
        name: String,
    ) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.thread_set_name(thread_id, name).await,
            Self::Acp(_) => self.unsupported("thread/name/set"),
        }
    }

    pub(crate) async fn thread_compact_start(&mut self, thread_id: ThreadId) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.thread_compact_start(thread_id).await,
            Self::Acp(_) => self.unsupported("thread/compact/start"),
        }
    }

    pub(crate) async fn thread_background_terminals_clean(
        &mut self,
        thread_id: ThreadId,
    ) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.thread_background_terminals_clean(thread_id).await,
            Self::Acp(_) => self.unsupported("thread/backgroundTerminals/clean"),
        }
    }

    pub(crate) async fn thread_rollback(
        &mut self,
        thread_id: ThreadId,
        num_turns: u32,
    ) -> Result<ThreadRollbackResponse> {
        match self {
            Self::AppServer(runtime) => runtime.thread_rollback(thread_id, num_turns).await,
            Self::Acp(_) => self.unsupported("thread/rollback"),
        }
    }

    pub(crate) async fn review_start(
        &mut self,
        thread_id: ThreadId,
        review_request: ReviewRequest,
    ) -> Result<ReviewStartResponse> {
        match self {
            Self::AppServer(runtime) => runtime.review_start(thread_id, review_request).await,
            Self::Acp(_) => self.unsupported("review/start"),
        }
    }

    pub(crate) async fn skills_list(
        &mut self,
        params: SkillsListParams,
    ) -> Result<SkillsListResponse> {
        match self {
            Self::AppServer(runtime) => runtime.skills_list(params).await,
            Self::Acp(_) => self.unsupported("skills/list"),
        }
    }

    pub(crate) async fn reload_user_config(&mut self) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.reload_user_config().await,
            Self::Acp(_) => self.unsupported("config/reload"),
        }
    }

    pub(crate) async fn thread_realtime_start(
        &mut self,
        thread_id: ThreadId,
        params: ConversationStartParams,
    ) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.thread_realtime_start(thread_id, params).await,
            Self::Acp(_) => self.unsupported("thread/realtime/start"),
        }
    }

    pub(crate) async fn thread_realtime_audio(
        &mut self,
        thread_id: ThreadId,
        params: ConversationAudioParams,
    ) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.thread_realtime_audio(thread_id, params).await,
            Self::Acp(_) => self.unsupported("thread/realtime/appendAudio"),
        }
    }

    pub(crate) async fn thread_realtime_text(
        &mut self,
        thread_id: ThreadId,
        params: ConversationTextParams,
    ) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.thread_realtime_text(thread_id, params).await,
            Self::Acp(_) => self.unsupported("thread/realtime/appendText"),
        }
    }

    pub(crate) async fn thread_realtime_stop(&mut self, thread_id: ThreadId) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.thread_realtime_stop(thread_id).await,
            Self::Acp(_) => self.unsupported("thread/realtime/stop"),
        }
    }

    pub(crate) async fn thread_shell_command(
        &mut self,
        thread_id: ThreadId,
        command: String,
    ) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.thread_shell_command(thread_id, command).await,
            Self::Acp(_) => self.unsupported("thread/shellCommand"),
        }
    }

    pub(crate) fn request_handle(&self) -> Option<AppServerRequestHandle> {
        match self {
            Self::AppServer(runtime) => Some(runtime.request_handle()),
            Self::Acp(_) => None,
        }
    }

    pub(crate) async fn acp_permission_prompt(
        &self,
        synthetic_id: &str,
    ) -> Option<IlhaeInteractiveRequestDto> {
        match self {
            Self::AppServer(_) => None,
            Self::Acp(runtime) => runtime.acp_permission_prompt(synthetic_id).await,
        }
    }

    pub(crate) async fn resolve_acp_permission_request(
        &self,
        synthetic_id: &str,
        option_id: Option<String>,
    ) -> std::io::Result<()> {
        match self {
            Self::AppServer(_) => Err(std::io::Error::other(
                "ACP permission resolution is unavailable for app-server runtime",
            )),
            Self::Acp(runtime) => {
                runtime
                    .resolve_acp_permission_request(synthetic_id, option_id)
                    .await
            }
        }
    }

    pub(crate) async fn resolve_server_request(
        &self,
        request_id: RequestId,
        result: serde_json::Value,
    ) -> std::io::Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.resolve_server_request(request_id, result).await,
            Self::Acp(runtime) => runtime.resolve_server_request(request_id, result).await,
        }
    }

    pub(crate) async fn reject_server_request(
        &self,
        request_id: RequestId,
        error: JSONRPCErrorError,
    ) -> std::io::Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.reject_server_request(request_id, error).await,
            Self::Acp(_) => Err(std::io::Error::other(
                "rejecting native app-server requests is unavailable for ACP runtime",
            )),
        }
    }

    pub(crate) async fn shutdown(self) -> std::io::Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.shutdown().await,
            Self::Acp(runtime) => runtime.close().await,
        }
    }
}

fn brain_messages_to_turns(
    messages: Vec<brain_session_rs::session_store::SessionMessage>,
) -> Vec<Turn> {
    let mut turns = Vec::new();
    let mut current_turn: Option<Turn> = None;

    for message in messages {
        match message.role.as_str() {
            "user" => {
                if let Some(turn) = current_turn.take() {
                    turns.push(turn);
                }
                current_turn = Some(Turn {
                    id: format!("brain-turn-{}", message.id),
                    items: vec![ThreadItem::UserMessage {
                        id: format!("user-{}", message.id),
                        content: vec![codex_app_server_protocol::UserInput::Text {
                            text: message.content,
                            text_elements: Vec::new(),
                        }],
                    }],
                    status: TurnStatus::Completed,
                    error: None,
                    started_at: None,
                    completed_at: None,
                    duration_ms: None,
                });
            }
            "assistant" => {
                let item = ThreadItem::AgentMessage {
                    id: format!("assistant-{}", message.id),
                    text: message.content,
                    phase: None,
                    memory_citation: None,
                };
                if let Some(turn) = current_turn.as_mut() {
                    turn.items.push(item);
                } else {
                    current_turn = Some(Turn {
                        id: format!("brain-turn-{}", message.id),
                        items: vec![item],
                        status: TurnStatus::Completed,
                        error: None,
                        started_at: None,
                        completed_at: None,
                        duration_ms: None,
                    });
                }
            }
            _ => {}
        }
    }

    if let Some(turn) = current_turn {
        turns.push(turn);
    }

    turns
}

#[async_trait]
impl ConversationRuntime for SelectedConversationRuntime {
    fn is_remote(&self) -> bool {
        match self {
            Self::AppServer(runtime) => runtime.is_remote(),
            Self::Acp(runtime) => runtime.is_remote(),
        }
    }

    async fn list_threads(&mut self, params: ThreadListParams) -> Result<ThreadListResponse> {
        match self {
            Self::AppServer(runtime) => runtime.list_threads(params).await,
            Self::Acp(runtime) => runtime.list_threads(params).await,
        }
    }

    async fn read_thread(&mut self, thread_id: ThreadId, include_turns: bool) -> Result<Thread> {
        match self {
            Self::AppServer(runtime) => runtime.read_thread(thread_id, include_turns).await,
            Self::Acp(runtime) => runtime.read_thread(thread_id, include_turns).await,
        }
    }

    async fn start_thread(&mut self, config: &Config) -> Result<AppServerStartedThread> {
        match self {
            Self::AppServer(runtime) => runtime.start_thread(config).await,
            Self::Acp(runtime) => runtime.start_thread(config).await,
        }
    }

    async fn resume_thread(
        &mut self,
        config: Config,
        thread_id: ThreadId,
    ) -> Result<AppServerStartedThread> {
        match self {
            Self::AppServer(runtime) => runtime.resume_thread(config, thread_id).await,
            Self::Acp(runtime) => runtime.resume_thread(config, thread_id).await,
        }
    }

    async fn send_turn(&mut self, request: ConversationTurnRequest) -> Result<TurnStartResponse> {
        match self {
            Self::AppServer(runtime) => runtime.send_turn(request).await,
            Self::Acp(runtime) => runtime.send_turn(request).await,
        }
    }

    async fn interrupt_turn(&mut self, thread_id: ThreadId, turn_id: String) -> Result<()> {
        match self {
            Self::AppServer(runtime) => runtime.interrupt_turn(thread_id, turn_id).await,
            Self::Acp(runtime) => runtime.interrupt_turn(thread_id, turn_id).await,
        }
    }

    async fn close(self) -> std::io::Result<()> {
        self.shutdown().await
    }
}

impl AppServerSession {
    pub(crate) fn new(client: AppServerClient) -> Self {
        Self {
            client,
            next_request_id: 1,
            pending_ilhae_events: VecDeque::new(),
            remote_cwd_override: None,
        }
    }

    pub(crate) fn with_remote_cwd_override(mut self, remote_cwd_override: Option<PathBuf>) -> Self {
        self.remote_cwd_override = remote_cwd_override;
        self
    }

    pub(crate) fn remote_cwd_override(&self) -> Option<&std::path::Path> {
        self.remote_cwd_override.as_deref()
    }

    pub(crate) fn is_remote(&self) -> bool {
        matches!(self.client, AppServerClient::Remote(_))
    }

    fn using_local_ilhae_runtime() -> bool {
        codex_ilhae::native_runtime_context()
            .map(|runtime| {
                runtime
                    .settings_store
                    .get()
                    .agent
                    .command
                    .split_whitespace()
                    .next()
                    .map(|command| command == "ilhae")
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    fn local_native_model_preset(config: &Config) -> ModelPreset {
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| "ilhae-local".to_string());
        let effort = config
            .model_reasoning_effort
            .unwrap_or(ReasoningEffort::Medium);
        ModelPreset {
            id: model.clone(),
            model: model.clone(),
            display_name: model.clone(),
            description: "Local native ilhae runtime".to_string(),
            default_reasoning_effort: effort,
            supported_reasoning_efforts: vec![ReasoningEffortPreset {
                effort,
                description: "Default local runtime reasoning level".to_string(),
            }],
            supports_personality: false,
            is_default: true,
            upgrade: None,
            show_in_picker: true,
            availability_nux: None,
            supported_in_api: true,
            input_modalities: vec![InputModality::Text, InputModality::Image],
            additional_speed_tiers: Vec::new(),
        }
    }

    pub(crate) async fn bootstrap(&mut self, config: &Config) -> Result<AppServerBootstrap> {
        let account = self.read_account().await?;
        let model_request_id = self.next_request_id();
        let models: ModelListResponse = self
            .client
            .request_typed(ClientRequest::ModelList {
                request_id: model_request_id,
                params: ModelListParams {
                    cursor: None,
                    limit: None,
                    include_hidden: Some(true),
                },
            })
            .await
            .wrap_err("model/list failed during TUI bootstrap")?;
        let available_models = if Self::using_local_ilhae_runtime() {
            vec![Self::local_native_model_preset(config)]
        } else {
            models
                .data
                .into_iter()
                .map(model_preset_from_api_model)
                .collect::<Vec<_>>()
        };
        let default_model = config
            .model
            .clone()
            .or_else(|| {
                available_models
                    .iter()
                    .find(|model| model.is_default)
                    .map(|model| model.model.clone())
            })
            .or_else(|| available_models.first().map(|model| model.model.clone()))
            .wrap_err("model/list returned no models for TUI bootstrap")?;

        let (
            account_email,
            auth_mode,
            status_account_display,
            plan_type,
            feedback_audience,
            has_chatgpt_account,
        ) = match account.account {
            Some(Account::ApiKey {}) => (
                None,
                Some(TelemetryAuthMode::ApiKey),
                Some(StatusAccountDisplay::ApiKey),
                None,
                FeedbackAudience::External,
                false,
            ),
            Some(Account::Chatgpt { email, plan_type }) => {
                let feedback_audience = if email.ends_with("@openai.com") {
                    FeedbackAudience::OpenAiEmployee
                } else {
                    FeedbackAudience::External
                };
                (
                    Some(email.clone()),
                    Some(TelemetryAuthMode::Chatgpt),
                    Some(StatusAccountDisplay::ChatGpt {
                        email: Some(email),
                        plan: Some(plan_type_display_name(plan_type)),
                    }),
                    Some(plan_type),
                    feedback_audience,
                    true,
                )
            }
            None => (None, None, None, None, FeedbackAudience::External, false),
        };
        Ok(AppServerBootstrap {
            account_email,
            auth_mode,
            status_account_display,
            plan_type,
            requires_openai_auth: account.requires_openai_auth,
            default_model,
            feedback_audience,
            has_chatgpt_account,
            available_models,
        })
    }

    /// Fetches the current account info without refreshing the auth token.
    ///
    /// Used by both `bootstrap` (to populate the initial UI) and `get_login_status`
    /// (to check auth mode without the overhead of a full bootstrap).
    pub(crate) async fn read_account(&mut self) -> Result<GetAccountResponse> {
        let account_request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::GetAccount {
                request_id: account_request_id,
                params: GetAccountParams {
                    refresh_token: false,
                },
            })
            .await
            .wrap_err("account/read failed during TUI bootstrap")
    }

    pub(crate) async fn next_event(&mut self) -> Option<AppServerEvent> {
        let event = self.client.next_event().await;
        if let Some(ref event) = event {
            self.record_ilhae_event(event);
        }
        event
    }

    pub(crate) async fn next_ilhae_event(&mut self) -> Option<IlhaeAppSessionEventNotification> {
        self.pending_ilhae_events.pop_front()
    }

    fn record_ilhae_event(&mut self, event: &AppServerEvent) {
        if let Some(engine_id) = codex_ilhae::current_native_backend_engine() {
            if let Some(notification) =
                canonical_ilhae_event_from_app_server_event(&engine_id, event)
            {
                self.pending_ilhae_events.push_back(notification);
            }
        }
    }

    pub(crate) async fn start_thread(&mut self, config: &Config) -> Result<AppServerStartedThread> {
        self.start_thread_with_session_start_source(config, /*session_start_source*/ None)
            .await
    }

    pub(crate) async fn start_thread_with_session_start_source(
        &mut self,
        config: &Config,
        session_start_source: Option<ThreadStartSource>,
    ) -> Result<AppServerStartedThread> {
        let request_id = self.next_request_id();
        let response: ThreadStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadStart {
                request_id,
                params: thread_start_params_from_config(
                    config,
                    self.thread_params_mode(),
                    self.remote_cwd_override.as_deref(),
                    session_start_source,
                ),
            })
            .await
            .wrap_err("thread/start failed during TUI bootstrap")?;
        started_thread_from_start_response(response, config).await
    }

    pub(crate) async fn resume_thread(
        &mut self,
        config: Config,
        thread_id: ThreadId,
    ) -> Result<AppServerStartedThread> {
        let request_id = self.next_request_id();
        let response: ThreadResumeResponse = self
            .client
            .request_typed(ClientRequest::ThreadResume {
                request_id,
                params: thread_resume_params_from_config(
                    config.clone(),
                    thread_id,
                    self.thread_params_mode(),
                    self.remote_cwd_override.as_deref(),
                ),
            })
            .await
            .wrap_err("thread/resume failed during TUI bootstrap")?;
        started_thread_from_resume_response(response, &config).await
    }

    pub(crate) async fn fork_thread(
        &mut self,
        config: Config,
        thread_id: ThreadId,
    ) -> Result<AppServerStartedThread> {
        let request_id = self.next_request_id();
        let response: ThreadForkResponse = self
            .client
            .request_typed(ClientRequest::ThreadFork {
                request_id,
                params: thread_fork_params_from_config(
                    config.clone(),
                    thread_id,
                    self.thread_params_mode(),
                    self.remote_cwd_override.as_deref(),
                ),
            })
            .await
            .wrap_err("thread/fork failed during TUI bootstrap")?;
        started_thread_from_fork_response(response, &config).await
    }

    fn thread_params_mode(&self) -> ThreadParamsMode {
        match &self.client {
            AppServerClient::InProcess(_) => ThreadParamsMode::Embedded,
            AppServerClient::Remote(_) => ThreadParamsMode::Remote,
        }
    }

    pub(crate) async fn thread_list(
        &mut self,
        params: ThreadListParams,
    ) -> Result<ThreadListResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ThreadList { request_id, params })
            .await
            .wrap_err("thread/list failed during TUI session lookup")
    }

    /// Lists thread ids that the app server currently holds in memory.
    ///
    /// Used by `App::backfill_loaded_subagent_threads` to discover subagent threads that were
    /// spawned before the TUI connected. The caller then fetches full metadata per thread via
    /// `thread_read` and walks the spawn tree.
    pub(crate) async fn thread_loaded_list(
        &mut self,
        params: ThreadLoadedListParams,
    ) -> Result<ThreadLoadedListResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ThreadLoadedList { request_id, params })
            .await
            .wrap_err("failed to list loaded threads from app server")
    }

    pub(crate) async fn thread_read(
        &mut self,
        thread_id: ThreadId,
        include_turns: bool,
    ) -> Result<Thread> {
        let request_id = self.next_request_id();
        let response: ThreadReadResponse = self
            .client
            .request_typed(ClientRequest::ThreadRead {
                request_id,
                params: ThreadReadParams {
                    thread_id: thread_id.to_string(),
                    include_turns,
                },
            })
            .await
            .wrap_err("thread/read failed during TUI session lookup")?;
        Ok(response.thread)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn turn_start(
        &mut self,
        thread_id: ThreadId,
        items: Vec<codex_protocol::user_input::UserInput>,
        cwd: PathBuf,
        approval_policy: AskForApproval,
        approvals_reviewer: codex_protocol::config_types::ApprovalsReviewer,
        sandbox_policy: SandboxPolicy,
        model: String,
        effort: Option<codex_protocol::openai_models::ReasoningEffort>,
        summary: Option<codex_protocol::config_types::ReasoningSummary>,
        service_tier: Option<Option<codex_protocol::config_types::ServiceTier>>,
        collaboration_mode: Option<codex_protocol::config_types::CollaborationMode>,
        personality: Option<codex_protocol::config_types::Personality>,
        output_schema: Option<serde_json::Value>,
    ) -> Result<TurnStartResponse> {
        let request_id = self.next_request_id();
        let items = codex_ilhae::prepare_native_turn_inputs(&thread_id.to_string(), items.clone())
            .await
            .unwrap_or(items);
        self.client
            .request_typed(ClientRequest::TurnStart {
                request_id,
                params: TurnStartParams {
                    thread_id: thread_id.to_string(),
                    input: items.into_iter().map(Into::into).collect(),
                    responsesapi_client_metadata: None,
                    cwd: Some(cwd),
                    approval_policy: Some(approval_policy.into()),
                    approvals_reviewer: Some(approvals_reviewer.into()),
                    sandbox_policy: Some(sandbox_policy.into()),
                    model: Some(model),
                    service_tier,
                    effort,
                    summary,
                    personality,
                    output_schema,
                    collaboration_mode,
                },
            })
            .await
            .wrap_err("turn/start failed in TUI")
    }

    pub(crate) async fn turn_interrupt(
        &mut self,
        thread_id: ThreadId,
        turn_id: String,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: TurnInterruptResponse = self
            .client
            .request_typed(ClientRequest::TurnInterrupt {
                request_id,
                params: TurnInterruptParams {
                    thread_id: thread_id.to_string(),
                    turn_id,
                },
            })
            .await
            .wrap_err("turn/interrupt failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn startup_interrupt(&mut self, thread_id: ThreadId) -> Result<()> {
        self.turn_interrupt(thread_id, String::new()).await
    }

    pub(crate) async fn turn_steer(
        &mut self,
        thread_id: ThreadId,
        turn_id: String,
        items: Vec<codex_protocol::user_input::UserInput>,
    ) -> std::result::Result<TurnSteerResponse, TypedRequestError> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::TurnSteer {
                request_id,
                params: TurnSteerParams {
                    thread_id: thread_id.to_string(),
                    input: items.into_iter().map(Into::into).collect(),
                    responsesapi_client_metadata: None,
                    expected_turn_id: turn_id,
                },
            })
            .await
    }

    pub(crate) async fn thread_set_name(
        &mut self,
        thread_id: ThreadId,
        name: String,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadSetNameResponse = self
            .client
            .request_typed(ClientRequest::ThreadSetName {
                request_id,
                params: ThreadSetNameParams {
                    thread_id: thread_id.to_string(),
                    name,
                },
            })
            .await
            .wrap_err("thread/name/set failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_memory_mode_set(
        &mut self,
        thread_id: ThreadId,
        mode: ThreadMemoryMode,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadMemoryModeSetResponse = self
            .client
            .request_typed(ClientRequest::ThreadMemoryModeSet {
                request_id,
                params: ThreadMemoryModeSetParams {
                    thread_id: thread_id.to_string(),
                    mode,
                },
            })
            .await
            .wrap_err("thread/memoryMode/set failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn memory_reset(&mut self) -> Result<()> {
        let request_id = self.next_request_id();
        let _: MemoryResetResponse = self
            .client
            .request_typed(ClientRequest::MemoryReset {
                request_id,
                params: None,
            })
            .await
            .wrap_err("memory/reset failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn logout_account(&mut self) -> Result<()> {
        let request_id = self.next_request_id();
        let _: LogoutAccountResponse = self
            .client
            .request_typed(ClientRequest::LogoutAccount {
                request_id,
                params: None,
            })
            .await
            .wrap_err("account/logout failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_unsubscribe(&mut self, thread_id: ThreadId) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadUnsubscribeResponse = self
            .client
            .request_typed(ClientRequest::ThreadUnsubscribe {
                request_id,
                params: ThreadUnsubscribeParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/unsubscribe failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_compact_start(&mut self, thread_id: ThreadId) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadCompactStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadCompactStart {
                request_id,
                params: ThreadCompactStartParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/compact/start failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_shell_command(
        &mut self,
        thread_id: ThreadId,
        command: String,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadShellCommandResponse = self
            .client
            .request_typed(ClientRequest::ThreadShellCommand {
                request_id,
                params: ThreadShellCommandParams {
                    thread_id: thread_id.to_string(),
                    command,
                },
            })
            .await
            .wrap_err("thread/shellCommand failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_background_terminals_clean(
        &mut self,
        thread_id: ThreadId,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadBackgroundTerminalsCleanResponse = self
            .client
            .request_typed(ClientRequest::ThreadBackgroundTerminalsClean {
                request_id,
                params: ThreadBackgroundTerminalsCleanParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/backgroundTerminals/clean failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_rollback(
        &mut self,
        thread_id: ThreadId,
        num_turns: u32,
    ) -> Result<ThreadRollbackResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ThreadRollback {
                request_id,
                params: ThreadRollbackParams {
                    thread_id: thread_id.to_string(),
                    num_turns,
                },
            })
            .await
            .wrap_err("thread/rollback failed in TUI")
    }

    pub(crate) async fn review_start(
        &mut self,
        thread_id: ThreadId,
        review_request: ReviewRequest,
    ) -> Result<ReviewStartResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ReviewStart {
                request_id,
                params: ReviewStartParams {
                    thread_id: thread_id.to_string(),
                    target: review_target_to_app_server(review_request.target),
                    delivery: Some(ReviewDelivery::Inline),
                },
            })
            .await
            .wrap_err("review/start failed in TUI")
    }

    pub(crate) async fn skills_list(
        &mut self,
        params: SkillsListParams,
    ) -> Result<SkillsListResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::SkillsList { request_id, params })
            .await
            .wrap_err("skills/list failed in TUI")
    }

    pub(crate) async fn reload_user_config(&mut self) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ConfigWriteResponse = self
            .client
            .request_typed(ClientRequest::ConfigBatchWrite {
                request_id,
                params: ConfigBatchWriteParams {
                    edits: Vec::new(),
                    file_path: None,
                    expected_version: None,
                    reload_user_config: true,
                },
            })
            .await
            .wrap_err("config/batchWrite failed while reloading user config in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_start(
        &mut self,
        thread_id: ThreadId,
        params: ConversationStartParams,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeStart {
                request_id,
                params: ThreadRealtimeStartParams {
                    thread_id: thread_id.to_string(),
                    output_modality: params.output_modality,
                    prompt: params.prompt,
                    session_id: params.session_id,
                    voice: params.voice,
                    transport: params.transport.map(|transport| match transport {
                        ConversationStartTransport::Websocket => {
                            ThreadRealtimeStartTransport::Websocket
                        }
                        ConversationStartTransport::Webrtc { sdp } => {
                            ThreadRealtimeStartTransport::Webrtc { sdp }
                        }
                    }),
                },
            })
            .await
            .wrap_err("thread/realtime/start failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_audio(
        &mut self,
        thread_id: ThreadId,
        params: ConversationAudioParams,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeAppendAudioResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeAppendAudio {
                request_id,
                params: ThreadRealtimeAppendAudioParams {
                    thread_id: thread_id.to_string(),
                    audio: params.frame.into(),
                },
            })
            .await
            .wrap_err("thread/realtime/appendAudio failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_text(
        &mut self,
        thread_id: ThreadId,
        params: ConversationTextParams,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeAppendTextResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeAppendText {
                request_id,
                params: ThreadRealtimeAppendTextParams {
                    thread_id: thread_id.to_string(),
                    text: params.text,
                },
            })
            .await
            .wrap_err("thread/realtime/appendText failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_stop(&mut self, thread_id: ThreadId) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeStopResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeStop {
                request_id,
                params: ThreadRealtimeStopParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/realtime/stop failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn reject_server_request(
        &self,
        request_id: RequestId,
        error: JSONRPCErrorError,
    ) -> std::io::Result<()> {
        self.client.reject_server_request(request_id, error).await
    }

    pub(crate) async fn resolve_server_request(
        &self,
        request_id: RequestId,
        result: serde_json::Value,
    ) -> std::io::Result<()> {
        self.client.resolve_server_request(request_id, result).await
    }

    pub(crate) async fn shutdown(self) -> std::io::Result<()> {
        self.client.shutdown().await
    }

    pub(crate) fn request_handle(&self) -> AppServerRequestHandle {
        self.client.request_handle()
    }

    fn next_request_id(&mut self) -> RequestId {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        RequestId::Integer(request_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_core::config::ConfigBuilder;
    use codex_app_server_protocol::ThreadStatus;
    use codex_app_server_protocol::Turn;
    use codex_app_server_protocol::TurnStatus;
    use codex_utils_absolute_path::test_support::test_path_buf;
    use codex_utils_absolute_path::test_support::PathBufExt;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    async fn build_config(temp_dir: &TempDir) -> Config {
        ConfigBuilder::default()
            .codex_home(temp_dir.path().to_path_buf())
            .build()
            .await
            .expect("config should build")
    }

    #[tokio::test]
    async fn thread_start_params_include_cwd_for_embedded_sessions() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;

        let params = thread_start_params_from_config(
            &config,
            ThreadParamsMode::Embedded,
            /*remote_cwd_override*/ None,
            /*session_start_source*/ None,
        );

        assert_eq!(params.cwd, Some(config.cwd.to_string_lossy().to_string()));
        assert_eq!(params.model_provider, Some(config.model_provider_id));
    }

    #[tokio::test]
    async fn thread_start_params_can_mark_clear_source() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;

        let params = thread_start_params_from_config(
            &config,
            ThreadParamsMode::Embedded,
            /*remote_cwd_override*/ None,
            Some(ThreadStartSource::Clear),
        );

        assert_eq!(params.session_start_source, Some(ThreadStartSource::Clear));
    }

    #[tokio::test]
    async fn thread_lifecycle_params_omit_cwd_without_remote_override_for_remote_sessions() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;
        let thread_id = ThreadId::new();

        let start = thread_start_params_from_config(
            &config,
            ThreadParamsMode::Remote,
            /*remote_cwd_override*/ None,
            /*session_start_source*/ None,
        );
        let resume = thread_resume_params_from_config(
            config.clone(),
            thread_id,
            ThreadParamsMode::Remote,
            /*remote_cwd_override*/ None,
        );
        let fork = thread_fork_params_from_config(
            config,
            thread_id,
            ThreadParamsMode::Remote,
            /*remote_cwd_override*/ None,
        );

        assert_eq!(start.cwd, None);
        assert_eq!(resume.cwd, None);
        assert_eq!(fork.cwd, None);
        assert_eq!(start.model_provider, None);
        assert_eq!(resume.model_provider, None);
        assert_eq!(fork.model_provider, None);
    }

    #[tokio::test]
    async fn thread_lifecycle_params_forward_explicit_remote_cwd_override_for_remote_sessions() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;
        let thread_id = ThreadId::new();
        let remote_cwd = PathBuf::from("repo/on/server");

        let start = thread_start_params_from_config(
            &config,
            ThreadParamsMode::Remote,
            Some(remote_cwd.as_path()),
            /*session_start_source*/ None,
        );
        let resume = thread_resume_params_from_config(
            config.clone(),
            thread_id,
            ThreadParamsMode::Remote,
            Some(remote_cwd.as_path()),
        );
        let fork = thread_fork_params_from_config(
            config,
            thread_id,
            ThreadParamsMode::Remote,
            Some(remote_cwd.as_path()),
        );

        assert_eq!(start.cwd.as_deref(), Some("repo/on/server"));
        assert_eq!(resume.cwd.as_deref(), Some("repo/on/server"));
        assert_eq!(fork.cwd.as_deref(), Some("repo/on/server"));
        assert_eq!(start.model_provider, None);
        assert_eq!(resume.model_provider, None);
        assert_eq!(fork.model_provider, None);
    }

    #[tokio::test]
    async fn resume_response_restores_turns_from_thread_items() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;
        let thread_id = ThreadId::new();
        let forked_from_id = ThreadId::new();
        let response = ThreadResumeResponse {
            thread: codex_app_server_protocol::Thread {
                id: thread_id.to_string(),
                forked_from_id: Some(forked_from_id.to_string()),
                preview: "hello".to_string(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: 1,
                updated_at: 2,
                status: ThreadStatus::Idle,
                path: None,
                cwd: test_path_buf("/tmp/project").abs(),
                cli_version: "0.0.0".to_string(),
                source: codex_protocol::protocol::SessionSource::Cli.into(),
                agent_nickname: None,
                agent_role: None,
                git_info: None,
                name: None,
                turns: vec![Turn {
                    id: "turn-1".to_string(),
                    items: vec![
                        codex_app_server_protocol::ThreadItem::UserMessage {
                            id: "user-1".to_string(),
                            content: vec![codex_app_server_protocol::UserInput::Text {
                                text: "hello from history".to_string(),
                                text_elements: Vec::new(),
                            }],
                        },
                        codex_app_server_protocol::ThreadItem::AgentMessage {
                            id: "assistant-1".to_string(),
                            text: "assistant reply".to_string(),
                            phase: None,
                            memory_citation: None,
                        },
                    ],
                    status: TurnStatus::Completed,
                    error: None,
                    started_at: None,
                    completed_at: None,
                    duration_ms: None,
                    started_at: None,
                    completed_at: None,
                    duration_ms: None,
                }],
            },
            model: "gpt-5.4".to_string(),
            model_provider: "openai".to_string(),
            service_tier: None,
            cwd: test_path_buf("/tmp/project").abs(),
            instruction_sources: vec![test_path_buf("/tmp/project/AGENTS.md").abs()],
            approval_policy: codex_protocol::protocol::AskForApproval::Never.into(),
            approvals_reviewer: codex_app_server_protocol::ApprovalsReviewer::User,
            sandbox: codex_protocol::protocol::SandboxPolicy::new_read_only_policy().into(),
            reasoning_effort: None,
        };

        let started = started_thread_from_resume_response(response.clone(), &config)
            .await
            .expect("resume response should map");
        assert_eq!(started.session.forked_from_id, Some(forked_from_id));
        assert_eq!(
            started.session.instruction_source_paths,
            response.instruction_sources
        );
        assert_eq!(started.turns.len(), 1);
        assert_eq!(started.turns[0], response.thread.turns[0]);
    }

    #[tokio::test]
    async fn session_configured_populates_history_metadata() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;
        let thread_id = ThreadId::new();

        append_message_history_entry("older", &thread_id, &config)
            .await
            .expect("history append should succeed");
        append_message_history_entry("newer", &thread_id, &config)
            .await
            .expect("history append should succeed");

        let session = thread_session_state_from_thread_response(
            &thread_id.to_string(),
            /*forked_from_id*/ None,
            Some("restore".to_string()),
            /*rollout_path*/ None,
            "gpt-5.4".to_string(),
            "openai".to_string(),
            /*service_tier*/ None,
            AskForApproval::Never,
            codex_protocol::config_types::ApprovalsReviewer::User,
            SandboxPolicy::new_read_only_policy(),
            test_path_buf("/tmp/project").abs(),
            Vec::new(),
            /*reasoning_effort*/ None,
            &config,
        )
        .await
        .expect("session should map");

        assert_ne!(session.history_log_id, 0);
        assert_eq!(session.history_entry_count, 2);
    }

    #[tokio::test]
    async fn session_configured_preserves_fork_source_thread_id() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;
        let thread_id = ThreadId::new();
        let forked_from_id = ThreadId::new();

        let session = thread_session_state_from_thread_response(
            &thread_id.to_string(),
            Some(forked_from_id.to_string()),
            Some("restore".to_string()),
            /*rollout_path*/ None,
            "gpt-5.4".to_string(),
            "openai".to_string(),
            /*service_tier*/ None,
            AskForApproval::Never,
            codex_protocol::config_types::ApprovalsReviewer::User,
            SandboxPolicy::new_read_only_policy(),
            test_path_buf("/tmp/project").abs(),
            Vec::new(),
            /*reasoning_effort*/ None,
            &config,
        )
        .await
        .expect("session should map");

        assert_eq!(session.forked_from_id, Some(forked_from_id));
    }

    #[test]
    fn status_account_display_from_auth_mode_uses_remapped_plan_labels() {
        let business = status_account_display_from_auth_mode(
            Some(AuthMode::Chatgpt),
            Some(codex_protocol::account::PlanType::EnterpriseCbpUsageBased),
        );
        assert!(matches!(
            business,
            Some(StatusAccountDisplay::ChatGpt {
                email: None,
                plan: Some(ref plan),
            }) if plan == "Enterprise"
        ));

        let team = status_account_display_from_auth_mode(
            Some(AuthMode::Chatgpt),
            Some(codex_protocol::account::PlanType::SelfServeBusinessUsageBased),
        );
        assert!(matches!(
            team,
            Some(StatusAccountDisplay::ChatGpt {
                email: None,
                plan: Some(ref plan),
            }) if plan == "Business"
        ));
    }

    #[test]
    fn ilhae_loop_lifecycle_notifications_map_to_server_notifications() {
        let notifications = loop_lifecycle_server_notifications(
            "thread-1",
            "turn-1",
            IlhaeLoopLifecycleNotification::Progress {
                session_id: "thread-1".to_string(),
                item_id: "loop-1".to_string(),
                kind: LoopLifecycleKind::ContextInjection,
                summary: "Loaded session context".to_string(),
                detail: Some("session_context(2), session_recall(1)".to_string()),
                counts: Some(std::collections::BTreeMap::from([(
                    "session_blocks".to_string(),
                    2,
                )])),
            },
        );

        assert_eq!(notifications.len(), 1);
        let ServerNotification::LoopLifecycleProgress(progress) = &notifications[0] else {
            panic!("expected loop lifecycle progress notification");
        };
        assert_eq!(progress.thread_id, "thread-1");
        assert_eq!(progress.turn_id, "turn-1");
        assert_eq!(progress.item_id, "loop-1");
        assert_eq!(progress.kind, LoopLifecycleKind::ContextInjection);
    }
}
