use crate::bottom_pane::FeedbackAudience;
use crate::status::StatusAccountDisplay;
use crate::status::plan_type_display_name;
use codex_app_server_protocol::RequestId;
use agent_client_protocol_schema::CancelNotification;
use agent_client_protocol_schema::ContentBlock;
use agent_client_protocol_schema::PermissionOptionKind;
use agent_client_protocol_schema::RequestPermissionOutcome;
use agent_client_protocol_schema::RequestPermissionRequest;
use agent_client_protocol_schema::RequestPermissionResponse;
use agent_client_protocol_schema::SelectedPermissionOutcome;
use agent_client_protocol_schema::SessionNotification;
use agent_client_protocol_schema::SessionId as AcpSessionId;
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
use codex_app_server_protocol::AuthMode;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::CommandExecutionApprovalDecision;
use codex_app_server_protocol::CommandExecutionRequestApprovalParams;
use codex_app_server_protocol::CommandExecutionRequestApprovalResponse;
use codex_app_server_protocol::ConfigBatchWriteParams;
use codex_app_server_protocol::ConfigWriteResponse;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::DynamicToolCallStatus;
use codex_app_server_protocol::GetAccountParams;
use codex_app_server_protocol::GetAccountRateLimitsResponse;
use codex_app_server_protocol::GetAccountResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::Model as ApiModel;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::ReasoningTextDeltaNotification;
use codex_app_server_protocol::ReviewDelivery;
use codex_app_server_protocol::ReviewStartParams;
use codex_app_server_protocol::ReviewStartResponse;
use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::SkillsListParams;
use codex_app_server_protocol::SkillsListResponse;
use codex_app_server_protocol::Thread;
use codex_app_server_protocol::ThreadBackgroundTerminalsCleanParams;
use codex_app_server_protocol::ThreadBackgroundTerminalsCleanResponse;
use codex_app_server_protocol::ThreadCompactStartParams;
use codex_app_server_protocol::ThreadCompactStartResponse;
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadForkResponse;
use codex_app_server_protocol::ThreadListParams;
use codex_app_server_protocol::ThreadListResponse;
use codex_app_server_protocol::ThreadLoadedListParams;
use codex_app_server_protocol::ThreadLoadedListResponse;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::ThreadReadResponse;
use codex_app_server_protocol::ThreadSortKey;
use codex_app_server_protocol::ThreadSourceKind;
use codex_app_server_protocol::ThreadStatus;
use codex_app_server_protocol::ThreadRealtimeAppendAudioParams;
use codex_app_server_protocol::ThreadRealtimeAppendAudioResponse;
use codex_app_server_protocol::ThreadRealtimeAppendTextParams;
use codex_app_server_protocol::ThreadRealtimeAppendTextResponse;
use codex_app_server_protocol::ThreadRealtimeStartParams;
use codex_app_server_protocol::ThreadRealtimeStartResponse;
use codex_app_server_protocol::ThreadRealtimeStopParams;
use codex_app_server_protocol::ThreadRealtimeStopResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadRollbackParams;
use codex_app_server_protocol::ThreadRollbackResponse;
use codex_app_server_protocol::ThreadSetNameParams;
use codex_app_server_protocol::ThreadSetNameResponse;
use codex_app_server_protocol::ThreadShellCommandParams;
use codex_app_server_protocol::ThreadShellCommandResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadStartedNotification;
use codex_app_server_protocol::ThreadItem;
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
use codex_core::config::Config;
use codex_core::message_history;
use codex_otel::TelemetryAuthMode;
use codex_protocol::openai_models::InputModality;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ModelAvailabilityNux;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelUpgrade;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::ConversationAudioParams;
use codex_protocol::protocol::ConversationStartParams;
use codex_protocol::protocol::ConversationTextParams;
use codex_protocol::protocol::CreditsSnapshot;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;
use codex_protocol::protocol::ReviewRequest;
use codex_protocol::protocol::ReviewTarget as CoreReviewTarget;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionNetworkProxyRuntime;
use codex_ilhae::IlhaeInteractiveOptionDto;
use codex_ilhae::IlhaeInteractiveOptionKind;
use codex_ilhae::IlhaeInteractiveRequestDto;
use codex_ilhae::IlhaeAppSessionEventDto;
use codex_ilhae::IlhaeAppSessionEventNotification;
use color_eyre::eyre::ContextCompat;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;
use sacp::SessionMessage;
use sacp::ConnectionTo;
use sacp::Responder;
use sacp::util::MatchDispatch;
use sacp_tokio::AcpAgent;
use sacp_tokio::AcpAgentSession;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub(crate) struct AppServerBootstrap {
    pub(crate) account_auth_mode: Option<AuthMode>,
    pub(crate) account_email: Option<String>,
    pub(crate) auth_mode: Option<TelemetryAuthMode>,
    pub(crate) status_account_display: Option<StatusAccountDisplay>,
    pub(crate) plan_type: Option<codex_protocol::account::PlanType>,
    pub(crate) default_model: String,
    pub(crate) feedback_audience: FeedbackAudience,
    pub(crate) has_chatgpt_account: bool,
    pub(crate) available_models: Vec<ModelPreset>,
    pub(crate) rate_limit_snapshots: Vec<RateLimitSnapshot>,
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
    pub(crate) cwd: PathBuf,
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
            cwd: PathBuf::from(session.cwd),
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
        let effort = config.model_reasoning_effort.unwrap_or(ReasoningEffort::Medium);
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
        }
    }

    async fn bootstrap(&mut self, config: &Config) -> Result<AppServerBootstrap> {
        let default_model = self.current_model(config);
        Ok(AppServerBootstrap {
            account_auth_mode: None,
            account_email: None,
            auth_mode: None,
            status_account_display: None,
            plan_type: None,
            default_model,
            feedback_audience: FeedbackAudience::External,
            has_chatgpt_account: false,
            available_models: vec![self.minimal_model_preset(config)],
            rate_limit_snapshots: Vec::new(),
        })
    }

    async fn next_event(&mut self) -> Option<AppServerEvent> {
        self.drain_control_messages().await;
        if let Some(event) = self.pending_events.pop_front() {
            self.record_ilhae_event(&event);
            return Some(event);
        }
        let event = self.event_rx.recv().await;
        self.drain_control_messages().await;
        if let Some(ref event) = event {
            self.record_ilhae_event(event);
        }
        event
    }

    async fn next_ilhae_event(&mut self) -> Option<IlhaeAppSessionEventNotification> {
        self.drain_control_messages().await;
        self.pending_ilhae_events.pop_front()
    }

    fn current_thread_from_session(&self, session: SessionInfo, include_turns: bool) -> Result<Thread> {
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
        if let Some(notification) = canonical_ilhae_event_from_app_server_event(
            &self.engine_id,
            event,
        ) {
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
        runtime.brain.session_ensure(
            global_session_id,
            &self.engine_id,
            &self.engine_id,
            &cwd.to_string_lossy(),
        ).map_err(|err| color_eyre::eyre::eyre!(err.to_string()))?;

        let command = self.current_command()?;
        let agent = AcpAgent::from_str(&command)
            .wrap_err_with(|| format!("failed to parse ACP backend command `{command}`"))?;
        let session = AcpAgentSession::connect(agent, cwd)
            .await
            .map_err(|err| color_eyre::eyre::eyre!("failed to connect ACP backend `{}`: {err}", self.engine_id))?;
        let local_session_id = session.session_id().to_string();
        runtime.brain.session_upsert_engine_ref(
            global_session_id,
            &self.engine_id,
            &local_session_id,
        ).map_err(|err| color_eyre::eyre::eyre!(err.to_string()))?;

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
        runtime.brain.session_ensure(
            &global_session_id_str,
            &self.engine_id,
            &self.engine_id,
            &config.cwd.to_string_lossy(),
        ).map_err(|err| color_eyre::eyre::eyre!(err.to_string()))?;
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
            config.cwd.clone().to_path_buf(),
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
                cwd: config.cwd.clone().to_path_buf(),
                cli_version: env!("CARGO_PKG_VERSION").to_string(),
                source: SessionSource::AppServer,
                agent_nickname: None,
                agent_role: None,
                git_info: None,
                name: None,
                turns: Vec::new(),
            });
        self.pending_events.push_back(AppServerEvent::ServerNotification(
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
                codex_protocol::user_input::UserInput::Text { text, .. } if !text.trim().is_empty() => {
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
                codex_protocol::user_input::UserInput::Text { text, .. } if !text.trim().is_empty() => {
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
        };
        self.pending_events.push_back(AppServerEvent::ServerNotification(
            ServerNotification::TurnStarted(TurnStartedNotification {
                thread_id: thread_id.clone(),
                turn: initial_turn.clone(),
            }),
        ));

        let runtime = self.runtime_context()?;
        let _ = runtime
            .brain
            .session_add_message_simple(&thread_id, "user", &raw_user_text, &self.engine_id);

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
        let assistant_item_id_for_task = assistant_item_id.clone();
        let reasoning_item_id_for_task = reasoning_item_id.clone();
        let task = tokio::spawn(async move {
            let mut assistant_text = String::new();
            let mut reasoning_text = String::new();
            let mut assistant_started = false;
            let mut reasoning_started = false;
            let mut tool_calls: HashMap<String, (String, serde_json::Value, bool)> =
                HashMap::new();
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

                                    Ok(())
                                },
                            )
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
                                                    id: tool_call_id,
                                                    tool,
                                                    arguments,
                                                    status: DynamicToolCallStatus::InProgress,
                                                    content_items: None,
                                                    success: None,
                                                    duration_ms: None,
                                                },
                                            }),
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
                                                            id: tool_call_id,
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
                                                            id: tool_call_id,
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

            let _ = event_tx.send(AppServerEvent::ServerNotification(
                ServerNotification::TurnCompleted(TurnCompletedNotification {
                    thread_id: thread_id_for_task,
                    turn: Turn {
                        id: turn_id_for_task,
                        items: Vec::new(),
                        status: final_status,
                        error: None,
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
                    .send_notification(CancelNotification::new(
                        active.cancel_session_id.clone(),
                    ))
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
                self.pending_events.push_back(AppServerEvent::ServerNotification(
                    ServerNotification::TurnCompleted(TurnCompletedNotification {
                        thread_id: thread_id.to_string(),
                        turn: Turn {
                            id: turn_id,
                            items: Vec::new(),
                            status: TurnStatus::Interrupted,
                            error: None,
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
            Self::AppServer(runtime) => Self::AppServer(runtime.with_remote_cwd_override(override_path)),
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

fn acp_tool_content_items(
    content: &[agent_client_protocol_schema::ToolCallContent],
) -> Option<Vec<DynamicToolCallOutputContentItem>> {
    let items = content
        .iter()
        .filter_map(|item| match item {
            agent_client_protocol_schema::ToolCallContent::Content(content) => {
                match &content.content {
                    ContentBlock::Text(text) => Some(
                        DynamicToolCallOutputContentItem::InputText {
                            text: text.text.clone(),
                        },
                    ),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

fn select_acp_permission_option(
    request: &RequestPermissionRequest,
    preferred_kinds: &[PermissionOptionKind],
) -> Option<String> {
    preferred_kinds.iter().find_map(|preferred| {
        request
            .options
            .iter()
            .find(|option| option.kind == *preferred)
            .map(|option| option.option_id.to_string())
    })
}

fn ilhae_interactive_option_kind(kind: PermissionOptionKind) -> IlhaeInteractiveOptionKind {
    match kind {
        PermissionOptionKind::AllowOnce => IlhaeInteractiveOptionKind::ApproveOnce,
        PermissionOptionKind::AllowAlways => IlhaeInteractiveOptionKind::ApproveSession,
        PermissionOptionKind::RejectOnce => IlhaeInteractiveOptionKind::RejectOnce,
        PermissionOptionKind::RejectAlways => IlhaeInteractiveOptionKind::RejectSession,
        _ => IlhaeInteractiveOptionKind::Custom,
    }
}

fn acp_permission_response_from_exec_decision(
    request: &RequestPermissionRequest,
    decision: CommandExecutionApprovalDecision,
) -> RequestPermissionResponse {
    let selected = match decision {
        CommandExecutionApprovalDecision::Accept => select_acp_permission_option(
            request,
            &[
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::AllowAlways,
            ],
        ),
        CommandExecutionApprovalDecision::AcceptForSession
        | CommandExecutionApprovalDecision::AcceptWithExecpolicyAmendment { .. }
        | CommandExecutionApprovalDecision::ApplyNetworkPolicyAmendment { .. } => {
            select_acp_permission_option(
                request,
                &[
                    PermissionOptionKind::AllowAlways,
                    PermissionOptionKind::AllowOnce,
                ],
            )
        }
        CommandExecutionApprovalDecision::Decline => select_acp_permission_option(
            request,
            &[
                PermissionOptionKind::RejectOnce,
                PermissionOptionKind::RejectAlways,
            ],
        ),
        CommandExecutionApprovalDecision::Cancel => None,
    };

    if let Some(option_id) = selected {
        RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(option_id),
        ))
    } else {
        RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
    }
}

fn turn_status_label(status: TurnStatus) -> &'static str {
    match status {
        TurnStatus::Completed => "completed",
        TurnStatus::Interrupted => "interrupted",
        TurnStatus::Failed => "failed",
        TurnStatus::InProgress => "in_progress",
    }
}

fn ilhae_interactive_options_from_exec_decisions(
    decisions: Option<&Vec<CommandExecutionApprovalDecision>>,
) -> Vec<IlhaeInteractiveOptionDto> {
    decisions
        .cloned()
        .unwrap_or_else(|| {
            vec![
                CommandExecutionApprovalDecision::Accept,
                CommandExecutionApprovalDecision::AcceptForSession,
                CommandExecutionApprovalDecision::Decline,
                CommandExecutionApprovalDecision::Cancel,
            ]
        })
        .into_iter()
        .map(|decision| match decision {
            CommandExecutionApprovalDecision::Accept => IlhaeInteractiveOptionDto {
                id: "approve_once".to_string(),
                label: "Approve once".to_string(),
                kind: IlhaeInteractiveOptionKind::ApproveOnce,
            },
            CommandExecutionApprovalDecision::AcceptForSession => IlhaeInteractiveOptionDto {
                id: "approve_session".to_string(),
                label: "Approve for session".to_string(),
                kind: IlhaeInteractiveOptionKind::ApproveSession,
            },
            CommandExecutionApprovalDecision::Decline => IlhaeInteractiveOptionDto {
                id: "deny_once".to_string(),
                label: "Deny once".to_string(),
                kind: IlhaeInteractiveOptionKind::RejectOnce,
            },
            CommandExecutionApprovalDecision::Cancel => IlhaeInteractiveOptionDto {
                id: "cancel".to_string(),
                label: "Cancel".to_string(),
                kind: IlhaeInteractiveOptionKind::Cancel,
            },
            CommandExecutionApprovalDecision::AcceptWithExecpolicyAmendment { .. } => {
                IlhaeInteractiveOptionDto {
                    id: "approve_exec_policy".to_string(),
                    label: "Approve with exec policy amendment".to_string(),
                    kind: IlhaeInteractiveOptionKind::Custom,
                }
            }
            CommandExecutionApprovalDecision::ApplyNetworkPolicyAmendment { .. } => {
                IlhaeInteractiveOptionDto {
                    id: "approve_network_policy".to_string(),
                    label: "Approve with network policy amendment".to_string(),
                    kind: IlhaeInteractiveOptionKind::Custom,
                }
            }
        })
        .collect()
}

fn canonical_interactive_request_from_server_request(
    request: &codex_app_server_protocol::ServerRequest,
) -> Option<IlhaeInteractiveRequestDto> {
    match request {
        codex_app_server_protocol::ServerRequest::CommandExecutionRequestApproval {
            request_id,
            params,
        } => Some(IlhaeInteractiveRequestDto {
            source: "app_server".to_string(),
            thread_id: params.thread_id.clone(),
            turn_id: params.turn_id.clone(),
            request_id: match request_id {
                RequestId::String(value) => value.clone(),
                RequestId::Integer(value) => value.to_string(),
            },
            title: params
                .command
                .clone()
                .unwrap_or_else(|| "Command approval".to_string()),
            reason: params.reason.clone(),
            requested_permissions: None,
            options: ilhae_interactive_options_from_exec_decisions(
                params.available_decisions.as_ref(),
            ),
        }),
        codex_app_server_protocol::ServerRequest::PermissionsRequestApproval {
            request_id,
            params,
        } => Some(IlhaeInteractiveRequestDto {
            source: "app_server".to_string(),
            thread_id: params.thread_id.clone(),
            turn_id: params.turn_id.clone(),
            request_id: match request_id {
                RequestId::String(value) => value.clone(),
                RequestId::Integer(value) => value.to_string(),
            },
            title: "Permissions approval".to_string(),
            reason: params.reason.clone(),
            requested_permissions: Some(params.permissions.clone().into()),
            options: vec![
                IlhaeInteractiveOptionDto {
                    id: "approve_once".to_string(),
                    label: "Approve once".to_string(),
                    kind: IlhaeInteractiveOptionKind::ApproveOnce,
                },
                IlhaeInteractiveOptionDto {
                    id: "approve_session".to_string(),
                    label: "Approve for session".to_string(),
                    kind: IlhaeInteractiveOptionKind::ApproveSession,
                },
                IlhaeInteractiveOptionDto {
                    id: "cancel".to_string(),
                    label: "Cancel".to_string(),
                    kind: IlhaeInteractiveOptionKind::Cancel,
                },
            ],
        }),
        codex_app_server_protocol::ServerRequest::FileChangeRequestApproval {
            request_id,
            params,
        } => Some(IlhaeInteractiveRequestDto {
            source: "app_server".to_string(),
            thread_id: params.thread_id.clone(),
            turn_id: params.turn_id.clone(),
            request_id: match request_id {
                RequestId::String(value) => value.clone(),
                RequestId::Integer(value) => value.to_string(),
            },
            title: "File change approval".to_string(),
            reason: params.reason.clone(),
            requested_permissions: None,
            options: vec![
                IlhaeInteractiveOptionDto {
                    id: "approve_once".to_string(),
                    label: "Approve once".to_string(),
                    kind: IlhaeInteractiveOptionKind::ApproveOnce,
                },
                IlhaeInteractiveOptionDto {
                    id: "deny_once".to_string(),
                    label: "Deny once".to_string(),
                    kind: IlhaeInteractiveOptionKind::RejectOnce,
                },
                IlhaeInteractiveOptionDto {
                    id: "cancel".to_string(),
                    label: "Cancel".to_string(),
                    kind: IlhaeInteractiveOptionKind::Cancel,
                },
            ],
        }),
        codex_app_server_protocol::ServerRequest::McpServerElicitationRequest {
            request_id,
            params,
        } => Some(IlhaeInteractiveRequestDto {
            source: "app_server".to_string(),
            thread_id: params.thread_id.clone(),
            turn_id: params
                .turn_id
                .clone()
                .unwrap_or_else(|| "elicitation".to_string()),
            request_id: match request_id {
                RequestId::String(value) => value.clone(),
                RequestId::Integer(value) => value.to_string(),
            },
            title: format!("MCP elicitation: {}", params.server_name),
            reason: Some(match &params.request {
                codex_app_server_protocol::McpServerElicitationRequest::Form {
                    message,
                    ..
                }
                | codex_app_server_protocol::McpServerElicitationRequest::Url {
                    message,
                    ..
                } => message.clone(),
            }),
            requested_permissions: None,
            options: vec![
                IlhaeInteractiveOptionDto {
                    id: "accept".to_string(),
                    label: "Accept".to_string(),
                    kind: IlhaeInteractiveOptionKind::ApproveOnce,
                },
                IlhaeInteractiveOptionDto {
                    id: "decline".to_string(),
                    label: "Decline".to_string(),
                    kind: IlhaeInteractiveOptionKind::RejectOnce,
                },
                IlhaeInteractiveOptionDto {
                    id: "cancel".to_string(),
                    label: "Cancel".to_string(),
                    kind: IlhaeInteractiveOptionKind::Cancel,
                },
            ],
        }),
        _ => None,
    }
}

fn canonical_ilhae_event_from_app_server_event(
    engine_id: &str,
    event: &AppServerEvent,
) -> Option<IlhaeAppSessionEventNotification> {
    let event = match event {
        AppServerEvent::ServerRequest(request) => canonical_interactive_request_from_server_request(
            request,
        )
        .map(|request| IlhaeAppSessionEventDto::InteractiveRequest { request }),
        AppServerEvent::ServerNotification(notification) => match notification {
            ServerNotification::TurnStarted(notif) => Some(IlhaeAppSessionEventDto::TurnStarted {
                thread_id: notif.thread_id.clone(),
                turn_id: notif.turn.id.clone(),
            }),
            ServerNotification::TurnCompleted(notif) => {
                Some(IlhaeAppSessionEventDto::TurnCompleted {
                    thread_id: notif.thread_id.clone(),
                    turn_id: notif.turn.id.clone(),
                    status: turn_status_label(notif.turn.status.clone()).to_string(),
                })
            }
            ServerNotification::AgentMessageDelta(notif) => {
                Some(IlhaeAppSessionEventDto::MessageDelta {
                    thread_id: notif.thread_id.clone(),
                    turn_id: notif.turn_id.clone(),
                    item_id: notif.item_id.clone(),
                    channel: "assistant".to_string(),
                    delta: notif.delta.clone(),
                })
            }
            ServerNotification::ReasoningTextDelta(notif) => {
                Some(IlhaeAppSessionEventDto::MessageDelta {
                    thread_id: notif.thread_id.clone(),
                    turn_id: notif.turn_id.clone(),
                    item_id: notif.item_id.clone(),
                    channel: "reasoning".to_string(),
                    delta: notif.delta.clone(),
                })
            }
            ServerNotification::ItemStarted(notif) => match &notif.item {
                ThreadItem::DynamicToolCall {
                    id,
                    tool,
                    arguments,
                    ..
                } => Some(IlhaeAppSessionEventDto::ToolCallStarted {
                    thread_id: notif.thread_id.clone(),
                    turn_id: notif.turn_id.clone(),
                    call_id: id.clone(),
                    tool: tool.clone(),
                    arguments: arguments.clone(),
                }),
                _ => None,
            },
            ServerNotification::ItemCompleted(notif) => match &notif.item {
                ThreadItem::DynamicToolCall {
                    id,
                    tool,
                    success,
                    content_items,
                    ..
                } => Some(IlhaeAppSessionEventDto::ToolCallCompleted {
                    thread_id: notif.thread_id.clone(),
                    turn_id: notif.turn_id.clone(),
                    call_id: id.clone(),
                    tool: tool.clone(),
                    success: success.unwrap_or(false),
                    output_text: content_items
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|item| match item {
                            DynamicToolCallOutputContentItem::InputText { text } => Some(text),
                            _ => None,
                        })
                        .collect(),
                }),
                _ => None,
            },
            _ => None,
        },
        AppServerEvent::Lagged { .. } | AppServerEvent::Disconnected { .. } => None,
    }?;

    Some(IlhaeAppSessionEventNotification {
        engine: engine_id.to_string(),
        event,
    })
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
        let effort = config.model_reasoning_effort.unwrap_or(ReasoningEffort::Medium);
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
        }
    }

    pub(crate) async fn bootstrap(&mut self, config: &Config) -> Result<AppServerBootstrap> {
        let account_request_id = self.next_request_id();
        let account: GetAccountResponse = self
            .client
            .request_typed(ClientRequest::GetAccount {
                request_id: account_request_id,
                params: GetAccountParams {
                    refresh_token: false,
                },
            })
            .await
            .wrap_err("account/read failed during TUI bootstrap")?;
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
            account_auth_mode,
            account_email,
            auth_mode,
            status_account_display,
            plan_type,
            feedback_audience,
            has_chatgpt_account,
        ) = match account.account {
            Some(Account::ApiKey {}) => (
                Some(AuthMode::ApiKey),
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
                    Some(AuthMode::Chatgpt),
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
            None => (
                None,
                None,
                None,
                None,
                None,
                FeedbackAudience::External,
                false,
            ),
        };
        let rate_limit_snapshots = if account.requires_openai_auth && has_chatgpt_account {
            let rate_limit_request_id = self.next_request_id();
            match self
                .client
                .request_typed(ClientRequest::GetAccountRateLimits {
                    request_id: rate_limit_request_id,
                    params: None,
                })
                .await
            {
                Ok(rate_limits) => app_server_rate_limit_snapshots_to_core(rate_limits),
                Err(err) => {
                    tracing::warn!("account/rateLimits/read failed during TUI bootstrap: {err}");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        Ok(AppServerBootstrap {
            account_auth_mode,
            account_email,
            auth_mode,
            status_account_display,
            plan_type,
            default_model,
            feedback_audience,
            has_chatgpt_account,
            available_models,
            rate_limit_snapshots,
        })
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
        let request_id = self.next_request_id();
        let response: ThreadStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadStart {
                request_id,
                params: thread_start_params_from_config(
                    config,
                    self.thread_params_mode(),
                    self.remote_cwd_override.as_deref(),
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
                    prompt: params.prompt,
                    session_id: params.session_id,
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

pub(crate) fn status_account_display_from_auth_mode(
    auth_mode: Option<AuthMode>,
    plan_type: Option<codex_protocol::account::PlanType>,
) -> Option<StatusAccountDisplay> {
    match auth_mode {
        Some(AuthMode::ApiKey) => Some(StatusAccountDisplay::ApiKey),
        Some(AuthMode::Chatgpt) | Some(AuthMode::ChatgptAuthTokens) => {
            Some(StatusAccountDisplay::ChatGpt {
                email: None,
                plan: plan_type.map(plan_type_display_name),
            })
        }
        None => None,
    }
}

#[allow(dead_code)]
pub(crate) fn feedback_audience_from_account_email(
    account_email: Option<&str>,
) -> FeedbackAudience {
    match account_email {
        Some(email) if email.ends_with("@openai.com") => FeedbackAudience::OpenAiEmployee,
        Some(_) | None => FeedbackAudience::External,
    }
}

fn model_preset_from_api_model(model: ApiModel) -> ModelPreset {
    let upgrade = model.upgrade.map(|upgrade_id| {
        let upgrade_info = model.upgrade_info.clone();
        ModelUpgrade {
            id: upgrade_id,
            reasoning_effort_mapping: None,
            migration_config_key: model.model.clone(),
            model_link: upgrade_info
                .as_ref()
                .and_then(|info| info.model_link.clone()),
            upgrade_copy: upgrade_info
                .as_ref()
                .and_then(|info| info.upgrade_copy.clone()),
            migration_markdown: upgrade_info.and_then(|info| info.migration_markdown),
        }
    });

    ModelPreset {
        id: model.id,
        model: model.model,
        display_name: model.display_name,
        description: model.description,
        default_reasoning_effort: model.default_reasoning_effort,
        supported_reasoning_efforts: model
            .supported_reasoning_efforts
            .into_iter()
            .map(|effort| ReasoningEffortPreset {
                effort: effort.reasoning_effort,
                description: effort.description,
            })
            .collect(),
        supports_personality: model.supports_personality,
        is_default: model.is_default,
        upgrade,
        show_in_picker: !model.hidden,
        availability_nux: model.availability_nux.map(|nux| ModelAvailabilityNux {
            message: nux.message,
        }),
        // `model/list` already returns models filtered for the active client/auth context.
        supported_in_api: true,
        input_modalities: model.input_modalities,
    }
}

fn approvals_reviewer_override_from_config(
    config: &Config,
) -> Option<codex_app_server_protocol::ApprovalsReviewer> {
    Some(config.approvals_reviewer.into())
}

fn config_request_overrides_from_config(
    config: &Config,
) -> Option<HashMap<String, serde_json::Value>> {
    config.active_profile.as_ref().map(|profile| {
        HashMap::from([(
            "profile".to_string(),
            serde_json::Value::String(profile.clone()),
        )])
    })
}

fn sandbox_mode_from_policy(
    policy: SandboxPolicy,
) -> Option<codex_app_server_protocol::SandboxMode> {
    match policy {
        SandboxPolicy::DangerFullAccess => {
            Some(codex_app_server_protocol::SandboxMode::DangerFullAccess)
        }
        SandboxPolicy::ReadOnly { .. } => Some(codex_app_server_protocol::SandboxMode::ReadOnly),
        SandboxPolicy::WorkspaceWrite { .. } => {
            Some(codex_app_server_protocol::SandboxMode::WorkspaceWrite)
        }
        SandboxPolicy::ExternalSandbox { .. } => None,
    }
}

fn thread_start_params_from_config(
    config: &Config,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<&std::path::Path>,
) -> ThreadStartParams {
    ThreadStartParams {
        model: config.model.clone(),
        model_provider: thread_params_mode.model_provider_from_config(config),
        cwd: thread_cwd_from_config(config, thread_params_mode, remote_cwd_override),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: approvals_reviewer_override_from_config(config),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get().clone()),
        config: config_request_overrides_from_config(config),
        ephemeral: Some(config.ephemeral),
        persist_extended_history: true,
        ..ThreadStartParams::default()
    }
}

fn thread_resume_params_from_config(
    config: Config,
    thread_id: ThreadId,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<&std::path::Path>,
) -> ThreadResumeParams {
    ThreadResumeParams {
        thread_id: thread_id.to_string(),
        model: config.model.clone(),
        model_provider: thread_params_mode.model_provider_from_config(&config),
        cwd: thread_cwd_from_config(&config, thread_params_mode, remote_cwd_override),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: approvals_reviewer_override_from_config(&config),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get().clone()),
        config: config_request_overrides_from_config(&config),
        persist_extended_history: true,
        ..ThreadResumeParams::default()
    }
}

fn thread_fork_params_from_config(
    config: Config,
    thread_id: ThreadId,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<&std::path::Path>,
) -> ThreadForkParams {
    ThreadForkParams {
        thread_id: thread_id.to_string(),
        model: config.model.clone(),
        model_provider: thread_params_mode.model_provider_from_config(&config),
        cwd: thread_cwd_from_config(&config, thread_params_mode, remote_cwd_override),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: approvals_reviewer_override_from_config(&config),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get().clone()),
        config: config_request_overrides_from_config(&config),
        ephemeral: config.ephemeral,
        persist_extended_history: true,
        ..ThreadForkParams::default()
    }
}

fn thread_cwd_from_config(
    config: &Config,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<&std::path::Path>,
) -> Option<String> {
    match thread_params_mode {
        ThreadParamsMode::Embedded => Some(config.cwd.to_string_lossy().to_string()),
        ThreadParamsMode::Remote => {
            remote_cwd_override.map(|cwd| cwd.to_string_lossy().to_string())
        }
    }
}

async fn started_thread_from_start_response(
    response: ThreadStartResponse,
    config: &Config,
) -> Result<AppServerStartedThread> {
    let session = thread_session_state_from_thread_start_response(&response, config)
        .await
        .map_err(color_eyre::eyre::Report::msg)?;
    Ok(AppServerStartedThread {
        session,
        turns: response.thread.turns,
    })
}

async fn started_thread_from_resume_response(
    response: ThreadResumeResponse,
    config: &Config,
) -> Result<AppServerStartedThread> {
    let session = thread_session_state_from_thread_resume_response(&response, config)
        .await
        .map_err(color_eyre::eyre::Report::msg)?;
    Ok(AppServerStartedThread {
        session,
        turns: response.thread.turns,
    })
}

async fn started_thread_from_fork_response(
    response: ThreadForkResponse,
    config: &Config,
) -> Result<AppServerStartedThread> {
    let session = thread_session_state_from_thread_fork_response(&response, config)
        .await
        .map_err(color_eyre::eyre::Report::msg)?;
    Ok(AppServerStartedThread {
        session,
        turns: response.thread.turns,
    })
}

async fn thread_session_state_from_thread_start_response(
    response: &ThreadStartResponse,
    config: &Config,
) -> Result<ThreadSessionState, String> {
    thread_session_state_from_thread_response(
        &response.thread.id,
        response.thread.forked_from_id.clone(),
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.reasoning_effort,
        config,
    )
    .await
}

async fn thread_session_state_from_thread_resume_response(
    response: &ThreadResumeResponse,
    config: &Config,
) -> Result<ThreadSessionState, String> {
    thread_session_state_from_thread_response(
        &response.thread.id,
        response.thread.forked_from_id.clone(),
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.reasoning_effort,
        config,
    )
    .await
}

async fn thread_session_state_from_thread_fork_response(
    response: &ThreadForkResponse,
    config: &Config,
) -> Result<ThreadSessionState, String> {
    thread_session_state_from_thread_response(
        &response.thread.id,
        response.thread.forked_from_id.clone(),
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.reasoning_effort,
        config,
    )
    .await
}

fn review_target_to_app_server(
    target: CoreReviewTarget,
) -> codex_app_server_protocol::ReviewTarget {
    match target {
        CoreReviewTarget::UncommittedChanges => {
            codex_app_server_protocol::ReviewTarget::UncommittedChanges
        }
        CoreReviewTarget::BaseBranch { branch } => {
            codex_app_server_protocol::ReviewTarget::BaseBranch { branch }
        }
        CoreReviewTarget::Commit { sha, title } => {
            codex_app_server_protocol::ReviewTarget::Commit { sha, title }
        }
        CoreReviewTarget::Custom { instructions } => {
            codex_app_server_protocol::ReviewTarget::Custom { instructions }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "session mapping keeps explicit fields"
)]
async fn thread_session_state_from_thread_response(
    thread_id: &str,
    forked_from_id: Option<String>,
    thread_name: Option<String>,
    rollout_path: Option<PathBuf>,
    model: String,
    model_provider_id: String,
    service_tier: Option<codex_protocol::config_types::ServiceTier>,
    approval_policy: AskForApproval,
    approvals_reviewer: codex_protocol::config_types::ApprovalsReviewer,
    sandbox_policy: SandboxPolicy,
    cwd: PathBuf,
    reasoning_effort: Option<codex_protocol::openai_models::ReasoningEffort>,
    config: &Config,
) -> Result<ThreadSessionState, String> {
    let thread_id = ThreadId::from_string(thread_id)
        .map_err(|err| format!("thread id `{thread_id}` is invalid: {err}"))?;
    let forked_from_id = forked_from_id
        .as_deref()
        .map(ThreadId::from_string)
        .transpose()
        .map_err(|err| format!("forked_from_id is invalid: {err}"))?;
    let (history_log_id, history_entry_count) = message_history::history_metadata(config).await;
    let history_entry_count = u64::try_from(history_entry_count).unwrap_or(u64::MAX);

    Ok(ThreadSessionState {
        thread_id,
        forked_from_id,
        thread_name,
        model,
        model_provider_id,
        service_tier,
        approval_policy,
        approvals_reviewer,
        sandbox_policy,
        cwd,
        reasoning_effort,
        history_log_id,
        history_entry_count,
        network_proxy: None,
        rollout_path,
    })
}

pub(crate) fn app_server_rate_limit_snapshots_to_core(
    response: GetAccountRateLimitsResponse,
) -> Vec<RateLimitSnapshot> {
    let mut snapshots = Vec::new();
    snapshots.push(app_server_rate_limit_snapshot_to_core(response.rate_limits));
    if let Some(by_limit_id) = response.rate_limits_by_limit_id {
        snapshots.extend(
            by_limit_id
                .into_values()
                .map(app_server_rate_limit_snapshot_to_core),
        );
    }
    snapshots
}

pub(crate) fn app_server_rate_limit_snapshot_to_core(
    snapshot: codex_app_server_protocol::RateLimitSnapshot,
) -> RateLimitSnapshot {
    RateLimitSnapshot {
        limit_id: snapshot.limit_id,
        limit_name: snapshot.limit_name,
        primary: snapshot.primary.map(app_server_rate_limit_window_to_core),
        secondary: snapshot.secondary.map(app_server_rate_limit_window_to_core),
        credits: snapshot.credits.map(app_server_credits_snapshot_to_core),
        plan_type: snapshot.plan_type,
    }
}

fn app_server_rate_limit_window_to_core(
    window: codex_app_server_protocol::RateLimitWindow,
) -> RateLimitWindow {
    RateLimitWindow {
        used_percent: window.used_percent as f64,
        window_minutes: window.window_duration_mins,
        resets_at: window.resets_at,
    }
}

fn app_server_credits_snapshot_to_core(
    snapshot: codex_app_server_protocol::CreditsSnapshot,
) -> CreditsSnapshot {
    CreditsSnapshot {
        has_credits: snapshot.has_credits,
        unlimited: snapshot.unlimited,
        balance: snapshot.balance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::ThreadStatus;
    use codex_app_server_protocol::Turn;
    use codex_app_server_protocol::TurnStatus;
    use codex_core::config::ConfigBuilder;
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
        );

        assert_eq!(params.cwd, Some(config.cwd.to_string_lossy().to_string()));
        assert_eq!(params.model_provider, Some(config.model_provider_id));
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
                cwd: PathBuf::from("/tmp/project"),
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
                }],
            },
            model: "gpt-5.4".to_string(),
            model_provider: "openai".to_string(),
            service_tier: None,
            cwd: PathBuf::from("/tmp/project"),
            approval_policy: codex_protocol::protocol::AskForApproval::Never.into(),
            approvals_reviewer: codex_app_server_protocol::ApprovalsReviewer::User,
            sandbox: codex_protocol::protocol::SandboxPolicy::new_read_only_policy().into(),
            reasoning_effort: None,
        };

        let started = started_thread_from_resume_response(response.clone(), &config)
            .await
            .expect("resume response should map");
        assert_eq!(started.session.forked_from_id, Some(forked_from_id));
        assert_eq!(started.turns.len(), 1);
        assert_eq!(started.turns[0], response.thread.turns[0]);
    }

    #[tokio::test]
    async fn session_configured_populates_history_metadata() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;
        let thread_id = ThreadId::new();

        message_history::append_entry("older", &thread_id, &config)
            .await
            .expect("history append should succeed");
        message_history::append_entry("newer", &thread_id, &config)
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
            PathBuf::from("/tmp/project"),
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
            PathBuf::from("/tmp/project"),
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
}
