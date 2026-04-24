//! RPC type definitions for ilhae-proxy.
//!
//! All Request/Response structs, MCP tool input types, and DTO types
//! used across the proxy modules.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::notification_store;
use brain_knowledge_rs::memory_store;
use codex_protocol::items::LoopLifecycleItem;
use codex_protocol::protocol::LoopLifecycleKind;
use codex_protocol::request_permissions::RequestPermissionProfile;

// ─── Method Name Constants ───────────────────────────────────────────────
pub const NOTIF_ASSISTANT_TURN_PATCH: &str = "ilhae/assistant_turn_patch";
pub const NOTIF_A2A_EVENT: &str = "ilhae/a2a_event";
pub const NOTIF_BACKGROUND_TASK_COMPLETED: &str = "ilhae/background_task_completed";
pub const NOTIF_TASK_UPDATED: &str = "ilhae/task_updated";
pub const NOTIF_BROWSER_ACTIVITY: &str = "ilhae/browser_activity";
pub const NOTIF_RELAY_USER_MESSAGE: &str = "ilhae/relay_user_message";
pub const NOTIF_PROMPT_TRACE_START: &str = "ilhae/prompt_trace_start";
pub const NOTIF_PROMPT_TRACE_FORWARDED: &str = "ilhae/prompt_trace_forwarded";
pub const NOTIF_APPROVAL_TRACE_START: &str = "ilhae/approval_trace_start";
pub const NOTIF_AUTONOMOUS_STATE: &str = "ilhae/autonomous_state";
pub const NOTIF_COST_UPDATE: &str = "ilhae/cost_update";
pub const NOTIF_ENGINE_STATE: &str = "ilhae/engine_state";
pub const NOTIF_APP_SESSION_EVENT: &str = "ilhae/app_session_event";
pub const NOTIF_LOOP_LIFECYCLE: &str = "ilhae/loop_lifecycle";
pub const NOTIF_SESSION_INFO_UPDATE: &str = "session/session_info_update";
pub const NOTIF_HISTORY_SYNC: &str = "session/history_sync";
pub const NOTIF_CRON_TRIGGERED: &str = "ilhae/cron_triggered";

pub const REQ_SESSION_SET_MODEL: &str = "session/set_model";
pub const REQ_APP_SESSION_LIST: &str = "ilhae/app/session/list";
pub const REQ_APP_SESSION_GET: &str = "ilhae/app/session/get";
pub const REQ_APP_SESSION_CREATE: &str = "ilhae/app/session/create";
pub const REQ_APP_SESSION_SEARCH: &str = "ilhae/app/session/search";
pub const REQ_APP_SESSION_DELETE: &str = "ilhae/app/session/delete";
pub const REQ_APP_SESSION_UPDATE: &str = "ilhae/app/session/update";
pub const REQ_APP_ARTIFACT_LIST: &str = "ilhae/app/artifact/list";
pub const REQ_APP_ARTIFACT_VERSIONS: &str = "ilhae/app/artifact/versions";
pub const REQ_APP_ARTIFACT_GET: &str = "ilhae/app/artifact/get";
pub const REQ_APP_WORKFLOW_LIST: &str = "ilhae/app/workflow/list";
pub const REQ_APP_WORKFLOW_GET: &str = "ilhae/app/workflow/get";
pub const REQ_APP_TIMELINE_READ: &str = "ilhae/app/timeline/read";
pub const REQ_APP_TIMELINE_SUBSCRIBE: &str = "ilhae/app/timeline/subscribe";
pub const REQ_APP_TURN_START: &str = "ilhae/app/turn/start";
pub const REQ_APP_TURN_INTERRUPT: &str = "ilhae/app/turn/interrupt";
pub const REQ_APP_ENGINE_GET: &str = "ilhae/app/engine/get";
pub const REQ_APP_ENGINE_SET: &str = "ilhae/app/engine/set";
pub const REQ_APP_PROFILE_LIST: &str = "ilhae/app/profile/list";
pub const REQ_APP_PROFILE_GET: &str = "ilhae/app/profile/get";
pub const REQ_APP_PROFILE_SET: &str = "ilhae/app/profile/set";
pub const REQ_APP_PROFILE_UPSERT: &str = "ilhae/app/profile/upsert";
pub const REQ_APP_TASK_LIST: &str = "ilhae/app/task/list";
pub const REQ_APP_TASK_GET: &str = "ilhae/app/task/get";
pub const REQ_APP_TASK_CREATE: &str = "ilhae/app/task/create";
pub const REQ_APP_TASK_UPDATE: &str = "ilhae/app/task/update";
pub const REQ_APP_TASK_DELETE: &str = "ilhae/app/task/delete";
pub const REQ_APP_TASK_RUN: &str = "ilhae/app/task/run";
pub const REQ_APP_KB_WORKSPACE_LIST: &str = "ilhae/app/kb/workspace/list";
pub const REQ_APP_KB_WORKSPACE_UPSERT: &str = "ilhae/app/kb/workspace/upsert";
pub const REQ_APP_KB_INGEST: &str = "ilhae/app/kb/ingest";
pub const REQ_APP_KB_COMPILE: &str = "ilhae/app/kb/compile";
pub const REQ_APP_KB_LINT: &str = "ilhae/app/kb/lint";
pub const REQ_APP_KB_QUERY: &str = "ilhae/app/kb/query";
pub const REQ_APP_KB_FILE_BACK: &str = "ilhae/app/kb/file_back";

// ─── Canonical client-facing DTOs for ilhae app-server v1 ───────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IlhaeEngineStatePayload {
    pub engine: String,
    pub endpoint: String,
    pub team_mode: bool,
    #[serde(default)]
    pub team_backend: String,
    pub auto_mode: bool,
    pub advisor_mode: bool,
    pub kairos_enabled: bool,
    pub self_improvement_enabled: bool,
    #[serde(default)]
    pub self_improvement_preset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_scope: Option<String>,
    #[serde(default)]
    pub knowledge_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_last_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_last_driver: Option<String>,
    #[serde(default)]
    pub knowledge_last_issue_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_last_run_reason: Option<String>,
    pub approval_preset: String,
    pub command: String,
    pub capabilities: serde_json::Value,
    pub capability_matrix: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/session/list", response = IlhaeAppSessionListResponse)]
pub struct IlhaeAppSessionListRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppSessionListResponse {
    pub sessions: Vec<SessionInfoDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/session/get", response = IlhaeAppSessionGetResponse)]
pub struct IlhaeAppSessionGetRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppSessionGetResponse {
    pub session: Option<SessionInfoDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/session/create", response = IlhaeAppSessionCreateResponse)]
pub struct IlhaeAppSessionCreateRequest {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(rename = "agentId", default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppSessionCreateResponse {
    pub session: Option<SessionInfoDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/session/search", response = IlhaeAppSessionSearchResponse)]
pub struct IlhaeAppSessionSearchRequest {
    pub query: String,
    #[serde(default = "default_search_sessions_limit")]
    pub limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppSessionSearchResponse {
    pub sessions: Vec<SessionInfoDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/session/delete", response = IlhaeAppSessionDeleteResponse)]
pub struct IlhaeAppSessionDeleteRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppSessionDeleteResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/session/update", response = IlhaeAppSessionUpdateResponse)]
pub struct IlhaeAppSessionUpdateRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppSessionUpdateResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/artifact/list", response = IlhaeAppArtifactListResponse)]
pub struct IlhaeAppArtifactListRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppArtifactListResponse {
    pub artifacts: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/artifact/versions", response = IlhaeAppArtifactVersionsResponse)]
pub struct IlhaeAppArtifactVersionsRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub filename: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppArtifactVersionsResponse {
    pub versions: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/artifact/get", response = IlhaeAppArtifactGetResponse)]
pub struct IlhaeAppArtifactGetRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub filename: String,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppArtifactGetResponse {
    pub artifact: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/workflow/list", response = IlhaeAppWorkflowListResponse)]
pub struct IlhaeAppWorkflowListRequest {
    #[serde(rename = "projectPath", default)]
    pub project_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppWorkflowListResponse {
    pub artifacts: Vec<WorkflowArtifactDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/workflow/get", response = IlhaeAppWorkflowGetResponse)]
pub struct IlhaeAppWorkflowGetRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppWorkflowGetResponse {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/timeline/read", response = IlhaeAppTimelineReadResponse)]
pub struct IlhaeAppTimelineReadRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "includeTeam", default)]
    pub include_team: bool,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(rename = "beforeId", default)]
    pub before_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppTimelineReadResponse {
    pub messages: Vec<SessionMessageDto>,
    #[serde(rename = "teamEvents", default, skip_serializing_if = "Vec::is_empty")]
    pub team_events: Vec<TeamTimelineEventDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/timeline/subscribe", response = IlhaeAppTimelineSubscribeResponse)]
pub struct IlhaeAppTimelineSubscribeRequest {
    #[serde(rename = "sessionId", default)]
    pub session_id: String,
    #[serde(rename = "sessionIds", default, skip_serializing_if = "Vec::is_empty")]
    pub session_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppTimelineSubscribeResponse {
    pub ok: bool,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "sessionIds", default, skip_serializing_if = "Vec::is_empty")]
    pub session_ids: Vec<String>,
    #[serde(rename = "notificationMethod")]
    pub notification_method: String,
}

impl IlhaeAppTimelineSubscribeRequest {
    pub fn normalized_session_ids(&self) -> Vec<String> {
        let mut normalized = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for session_id in std::iter::once(&self.session_id).chain(self.session_ids.iter()) {
            let trimmed = session_id.trim();
            if trimmed.is_empty() {
                continue;
            }
            if seen.insert(trimmed.to_string()) {
                normalized.push(trimmed.to_string());
            }
        }

        normalized
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/turn/start", response = IlhaeAppTurnStartResponse)]
pub struct IlhaeAppTurnStartRequest {
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    pub input: String,
    #[serde(rename = "agentId", default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppTurnStartResponse {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "stopReason")]
    pub stop_reason: String,
    #[serde(default)]
    pub meta: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/turn/interrupt", response = IlhaeAppTurnInterruptResponse)]
pub struct IlhaeAppTurnInterruptRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "turnId", default)]
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppTurnInterruptResponse {
    pub ok: bool,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "turnId", default)]
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/engine/get", response = IlhaeAppEngineGetResponse)]
pub struct IlhaeAppEngineGetRequest {
    #[serde(rename = "engineId", default)]
    pub engine_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppEngineGetResponse {
    #[serde(rename = "currentEngine")]
    pub current_engine: String,
    pub command: String,
    #[serde(rename = "teamMode")]
    pub team_mode: bool,
    #[serde(rename = "teamBackend", default)]
    pub team_backend: String,
    pub endpoint: String,
    #[serde(rename = "enabledEngines")]
    pub enabled_engines: Vec<String>,
    pub profile: serde_json::Value,
    pub matrix: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/engine/set", response = IlhaeAppEngineSetResponse)]
pub struct IlhaeAppEngineSetRequest {
    #[serde(rename = "engineId")]
    pub engine_id: String,
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppEngineSetResponse {
    pub ok: bool,
    #[serde(rename = "currentEngine")]
    pub current_engine: String,
    pub command: String,
    #[serde(rename = "teamMode")]
    pub team_mode: bool,
    #[serde(rename = "teamBackend", default)]
    pub team_backend: String,
    pub endpoint: String,
    #[serde(rename = "enabledEngines")]
    pub enabled_engines: Vec<String>,
    pub profile: serde_json::Value,
    pub matrix: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct IlhaeAppProfileAgentDto {
    #[serde(rename = "engineId", default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(rename = "teamMode")]
    pub team_mode: bool,
    #[serde(rename = "dreamMode", default)]
    pub dream_mode: bool,
    #[serde(rename = "embedMode", default)]
    pub embed_mode: bool,
    #[serde(rename = "teamBackend", default)]
    pub team_backend: String,
    #[serde(rename = "teamMergePolicy", default)]
    pub team_merge_policy: String,
    #[serde(rename = "teamMaxRetries", default)]
    pub team_max_retries: u32,
    #[serde(rename = "teamPauseOnError", default)]
    pub team_pause_on_error: bool,
    #[serde(rename = "autoMode")]
    pub auto_mode: bool,
    pub advisor: bool,
    #[serde(rename = "advisorPreset", default)]
    pub advisor_preset: String,
    #[serde(rename = "autoMaxTurns", default)]
    pub auto_max_turns: u32,
    #[serde(rename = "autoTimeboxMinutes", default)]
    pub auto_timebox_minutes: u32,
    #[serde(rename = "autoPauseOnError", default)]
    pub auto_pause_on_error: bool,
    pub kairos: bool,
    #[serde(
        rename = "selfImprovement",
        default = "crate::settings_types::default_self_improvement_enabled"
    )]
    pub self_improvement: bool,
    #[serde(rename = "selfImprovementPreset", default)]
    pub self_improvement_preset: String,
}

impl Default for IlhaeAppProfileAgentDto {
    fn default() -> Self {
        Self {
            engine_id: None,
            command: None,
            team_mode: false,
            dream_mode: false,
            embed_mode: false,
            team_backend: String::new(),
            team_merge_policy: String::new(),
            team_max_retries: 0,
            team_pause_on_error: false,
            auto_mode: false,
            advisor: false,
            advisor_preset: String::new(),
            auto_max_turns: 0,
            auto_timebox_minutes: 0,
            auto_pause_on_error: false,
            kairos: false,
            self_improvement: crate::settings_types::default_self_improvement_enabled(),
            self_improvement_preset: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub struct IlhaeAppProfilePermissionsDto {
    #[serde(rename = "approvalPreset")]
    pub approval_preset: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub struct IlhaeAppProfileScopeDto {
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub struct IlhaeAppProfileKnowledgeDto {
    #[serde(default)]
    pub mode: String,
    #[serde(
        rename = "workspaceId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_id: Option<String>,
    #[serde(rename = "pollIntervalSecs", default)]
    pub poll_interval_secs: u64,
    #[serde(rename = "periodicIntervalSecs", default)]
    pub periodic_interval_secs: u64,
    #[serde(rename = "reportTarget", default)]
    pub report_target: String,
    #[serde(rename = "reportRelativePath", default)]
    pub report_relative_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub struct IlhaeAppProfileDto {
    pub id: String,
    pub agent: IlhaeAppProfileAgentDto,
    pub permissions: IlhaeAppProfilePermissionsDto,
    pub memory: IlhaeAppProfileScopeDto,
    pub task: IlhaeAppProfileScopeDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<IlhaeAppProfileKnowledgeDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/profile/list", response = IlhaeAppProfileListResponse)]
pub struct IlhaeAppProfileListRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppProfileListResponse {
    #[serde(
        rename = "activeProfile",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub active_profile: Option<String>,
    pub profiles: Vec<IlhaeAppProfileDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/profile/get", response = IlhaeAppProfileGetResponse)]
pub struct IlhaeAppProfileGetRequest {
    #[serde(rename = "profileId", default)]
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppProfileGetResponse {
    #[serde(
        rename = "activeProfile",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub active_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<IlhaeAppProfileDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/profile/set", response = IlhaeAppProfileSetResponse)]
pub struct IlhaeAppProfileSetRequest {
    #[serde(rename = "profileId")]
    pub profile_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppProfileSetResponse {
    pub ok: bool,
    #[serde(rename = "activeProfile")]
    pub active_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<IlhaeAppProfileDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/profile/upsert", response = IlhaeAppProfileUpsertResponse)]
pub struct IlhaeAppProfileUpsertRequest {
    pub profile: IlhaeAppProfileDto,
    #[serde(default)]
    pub activate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppProfileUpsertResponse {
    pub ok: bool,
    #[serde(
        rename = "activeProfile",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub active_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<IlhaeAppProfileDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/task/list", response = IlhaeAppTaskListResponse)]
pub struct IlhaeAppTaskListRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppTaskListResponse {
    pub tasks: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/task/get", response = IlhaeAppTaskGetResponse)]
pub struct IlhaeAppTaskGetRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppTaskGetResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/task/create", response = IlhaeAppTaskCreateResponse)]
pub struct IlhaeAppTaskCreateRequest {
    pub task: TaskCreateInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppTaskCreateResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/task/update", response = IlhaeAppTaskUpdateResponse)]
pub struct IlhaeAppTaskUpdateRequest {
    pub task: TaskUpdateInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppTaskUpdateResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/task/delete", response = IlhaeAppTaskDeleteResponse)]
pub struct IlhaeAppTaskDeleteRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppTaskDeleteResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/task/run", response = IlhaeAppTaskRunResponse)]
pub struct IlhaeAppTaskRunRequest {
    #[serde(rename = "taskId", default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppTaskRunResponse {
    pub triggered: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IlhaeAppKbWorkspaceDto {
    pub id: String,
    pub name: String,
    #[serde(rename = "rootPath")]
    pub root_path: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IlhaeAppKbSourceDto {
    #[serde(rename = "sourceId")]
    pub source_id: String,
    #[serde(rename = "relativePath")]
    pub relative_path: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub size: u64,
    #[serde(rename = "modifiedAt")]
    pub modified_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IlhaeAppKbLintIssueDto {
    pub kind: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/kb/workspace/list", response = IlhaeAppKbWorkspaceListResponse)]
pub struct IlhaeAppKbWorkspaceListRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppKbWorkspaceListResponse {
    pub workspaces: Vec<IlhaeAppKbWorkspaceDto>,
    #[serde(
        rename = "activeWorkspace",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub active_workspace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(
    method = "ilhae/app/kb/workspace/upsert",
    response = IlhaeAppKbWorkspaceUpsertResponse
)]
pub struct IlhaeAppKbWorkspaceUpsertRequest {
    #[serde(rename = "workspaceId", default)]
    pub workspace_id: Option<String>,
    pub name: String,
    #[serde(rename = "rootPath")]
    pub root_path: String,
    #[serde(default)]
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppKbWorkspaceUpsertResponse {
    pub ok: bool,
    #[serde(
        rename = "activeWorkspace",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub active_workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<IlhaeAppKbWorkspaceDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/kb/ingest", response = IlhaeAppKbIngestResponse)]
pub struct IlhaeAppKbIngestRequest {
    #[serde(rename = "workspaceId", default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppKbIngestResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<IlhaeAppKbWorkspaceDto>,
    pub sources: Vec<IlhaeAppKbSourceDto>,
    #[serde(
        rename = "inventoryPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub inventory_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/kb/compile", response = IlhaeAppKbCompileResponse)]
pub struct IlhaeAppKbCompileRequest {
    #[serde(rename = "workspaceId", default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppKbCompileResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<IlhaeAppKbWorkspaceDto>,
    #[serde(rename = "compiledSources")]
    pub compiled_sources: usize,
    #[serde(rename = "conceptCount")]
    pub concept_count: usize,
    #[serde(rename = "generatedFiles")]
    pub generated_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/kb/lint", response = IlhaeAppKbLintResponse)]
pub struct IlhaeAppKbLintRequest {
    #[serde(rename = "workspaceId", default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppKbLintResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<IlhaeAppKbWorkspaceDto>,
    pub issues: Vec<IlhaeAppKbLintIssueDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/kb/query", response = IlhaeAppKbQueryResponse)]
pub struct IlhaeAppKbQueryRequest {
    #[serde(rename = "workspaceId", default)]
    pub workspace_id: Option<String>,
    pub query: String,
    #[serde(rename = "outputPath", default)]
    pub output_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppKbQueryResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<IlhaeAppKbWorkspaceDto>,
    pub answer: String,
    #[serde(rename = "matchedPaths")]
    pub matched_paths: Vec<String>,
    #[serde(
        rename = "reportPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub report_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/app/kb/file_back", response = IlhaeAppKbFileBackResponse)]
pub struct IlhaeAppKbFileBackRequest {
    #[serde(rename = "workspaceId", default)]
    pub workspace_id: Option<String>,
    pub target: String,
    #[serde(rename = "relativePath")]
    pub relative_path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct IlhaeAppKbFileBackResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IlhaeEngineStateNotification {
    pub engine: String,
    pub endpoint: String,
    pub team_mode: bool,
    #[serde(default)]
    pub team_backend: String,
    #[serde(default)]
    pub team_merge_policy: String,
    #[serde(default)]
    pub team_max_retries: u32,
    #[serde(default)]
    pub team_pause_on_error: bool,
    pub auto_mode: bool,
    pub advisor_mode: bool,
    #[serde(default)]
    pub advisor_preset: String,
    #[serde(default)]
    pub auto_max_turns: u32,
    #[serde(default)]
    pub auto_timebox_minutes: u32,
    #[serde(default)]
    pub auto_pause_on_error: bool,
    pub kairos_enabled: bool,
    pub self_improvement_enabled: bool,
    #[serde(default)]
    pub self_improvement_preset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_scope: Option<String>,
    #[serde(default)]
    pub knowledge_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_last_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_last_driver: Option<String>,
    #[serde(default)]
    pub knowledge_last_issue_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_last_run_reason: Option<String>,
    pub approval_preset: String,
    pub command: String,
    pub capabilities: serde_json::Value,
    pub capability_matrix: serde_json::Value,
}

impl sacp::JsonRpcMessage for IlhaeEngineStateNotification {
    fn to_untyped_message(&self) -> Result<sacp::UntypedMessage, sacp::Error> {
        sacp::UntypedMessage::new(NOTIF_ENGINE_STATE, self)
    }
    fn method(&self) -> &str {
        NOTIF_ENGINE_STATE
    }
    fn parse_message(_method: &str, params: &impl Serialize) -> Result<Self, sacp::Error> {
        let s = serde_json::to_string(params).map_err(sacp::Error::into_internal_error)?;
        serde_json::from_str(&s).map_err(sacp::Error::into_internal_error)
    }
    fn matches_method(method: &str) -> bool {
        method == NOTIF_ENGINE_STATE
    }
}
impl sacp::JsonRpcNotification for IlhaeEngineStateNotification {}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IlhaeInteractiveOptionKind {
    ApproveOnce,
    ApproveSession,
    RejectOnce,
    RejectSession,
    Cancel,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IlhaeInteractiveOptionDto {
    pub id: String,
    pub label: String,
    pub kind: IlhaeInteractiveOptionKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IlhaeInteractiveRequestDto {
    pub source: String,
    pub thread_id: String,
    pub turn_id: String,
    pub request_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_permissions: Option<RequestPermissionProfile>,
    pub options: Vec<IlhaeInteractiveOptionDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum IlhaeAppSessionEventDto {
    InteractiveRequest {
        request: IlhaeInteractiveRequestDto,
    },
    TurnStarted {
        thread_id: String,
        turn_id: String,
    },
    TurnCompleted {
        thread_id: String,
        turn_id: String,
        status: String,
    },
    MessageDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        channel: String,
        delta: String,
    },
    ToolCallStarted {
        thread_id: String,
        turn_id: String,
        call_id: String,
        tool: String,
        arguments: serde_json::Value,
    },
    ToolCallCompleted {
        thread_id: String,
        turn_id: String,
        call_id: String,
        tool: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        output_text: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IlhaeAppSessionEventNotification {
    pub engine: String,
    pub event: IlhaeAppSessionEventDto,
}

impl sacp::JsonRpcMessage for IlhaeAppSessionEventNotification {
    fn to_untyped_message(&self) -> Result<sacp::UntypedMessage, sacp::Error> {
        sacp::UntypedMessage::new(NOTIF_APP_SESSION_EVENT, self)
    }
    fn method(&self) -> &str {
        NOTIF_APP_SESSION_EVENT
    }
    fn parse_message(_method: &str, params: &impl Serialize) -> Result<Self, sacp::Error> {
        let s = serde_json::to_string(params).map_err(sacp::Error::into_internal_error)?;
        serde_json::from_str(&s).map_err(sacp::Error::into_internal_error)
    }
    fn matches_method(method: &str) -> bool {
        method == NOTIF_APP_SESSION_EVENT
    }
}
impl sacp::JsonRpcNotification for IlhaeAppSessionEventNotification {}

impl IlhaeAppSessionEventNotification {
    pub fn session_id(&self) -> &str {
        match &self.event {
            IlhaeAppSessionEventDto::InteractiveRequest { request } => &request.thread_id,
            IlhaeAppSessionEventDto::TurnStarted { thread_id, .. }
            | IlhaeAppSessionEventDto::TurnCompleted { thread_id, .. }
            | IlhaeAppSessionEventDto::MessageDelta { thread_id, .. }
            | IlhaeAppSessionEventDto::ToolCallStarted { thread_id, .. }
            | IlhaeAppSessionEventDto::ToolCallCompleted { thread_id, .. } => thread_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum IlhaeLoopLifecycleNotification {
    Started {
        #[serde(rename = "sessionId")]
        session_id: String,
        item: LoopLifecycleItem,
    },
    Progress {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "itemId")]
        item_id: String,
        kind: LoopLifecycleKind,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        counts: Option<BTreeMap<String, i64>>,
    },
    Completed {
        #[serde(rename = "sessionId")]
        session_id: String,
        item: LoopLifecycleItem,
    },
    Failed {
        #[serde(rename = "sessionId")]
        session_id: String,
        item: LoopLifecycleItem,
    },
}

impl sacp::JsonRpcMessage for IlhaeLoopLifecycleNotification {
    fn to_untyped_message(&self) -> Result<sacp::UntypedMessage, sacp::Error> {
        sacp::UntypedMessage::new(NOTIF_LOOP_LIFECYCLE, self)
    }

    fn method(&self) -> &str {
        NOTIF_LOOP_LIFECYCLE
    }

    fn parse_message(_method: &str, params: &impl Serialize) -> Result<Self, sacp::Error> {
        let s = serde_json::to_string(params).map_err(sacp::Error::into_internal_error)?;
        serde_json::from_str(&s).map_err(sacp::Error::into_internal_error)
    }

    fn matches_method(method: &str) -> bool {
        method == NOTIF_LOOP_LIFECYCLE
    }
}

impl sacp::JsonRpcNotification for IlhaeLoopLifecycleNotification {}

impl IlhaeLoopLifecycleNotification {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Started { session_id, .. }
            | Self::Progress { session_id, .. }
            | Self::Completed { session_id, .. }
            | Self::Failed { session_id, .. } => session_id,
        }
    }
}

// ─── Misc arg types ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EmptyArgs {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct MemoryWriteArgs {
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CronAddArgs {
    pub cron_expression: String,
    pub prompt: String,
}

// ─── Cron notification ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronTriggeredNotification {
    pub message: String,
}

impl sacp::JsonRpcMessage for CronTriggeredNotification {
    fn to_untyped_message(&self) -> Result<sacp::UntypedMessage, sacp::Error> {
        sacp::UntypedMessage::new(NOTIF_CRON_TRIGGERED, self)
    }
    fn method(&self) -> &str {
        NOTIF_CRON_TRIGGERED
    }
    fn parse_message(_method: &str, params: &impl Serialize) -> Result<Self, sacp::Error> {
        let s = serde_json::to_string(params).map_err(sacp::Error::into_internal_error)?;
        serde_json::from_str(&s).map_err(sacp::Error::into_internal_error)
    }
    fn matches_method(method: &str) -> bool {
        method == NOTIF_CRON_TRIGGERED
    }
}
impl sacp::JsonRpcNotification for CronTriggeredNotification {}

// ─── Session Read/Write RPC types ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/list_sessions", response = ListSessionsResponse)]
pub struct ListSessionsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionInfoDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/search_sessions", response = SearchSessionsResponse)]
pub struct SearchSessionsRequest {
    pub query: String,
    #[serde(default = "default_search_sessions_limit")]
    pub limit: i64,
}

fn default_search_sessions_limit() -> i64 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct SearchSessionsResponse {
    pub sessions: Vec<SessionInfoDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfoDto {
    pub id: String,
    pub title: String,
    pub agent_id: String,
    pub cwd: String,
    #[serde(default)]
    pub channel_id: String,
    #[serde(default)]
    pub multi_agent: bool,
    #[serde(default)]
    pub parent_session_id: String,
    #[serde(default)]
    pub team_role: String,
    #[serde(default)]
    pub agent_status: String,
    #[serde(default)]
    pub team_agent_count: i64,
    #[serde(default)]
    pub team_active_count: i64,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub capabilities_override: String,
    #[serde(default)]
    pub search_snippet: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/load_session_messages", response = LoadSessionMessagesResponse)]
pub struct LoadSessionMessagesRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct LoadSessionMessagesResponse {
    pub messages: Vec<SessionMessageDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/load_team_timeline", response = LoadTeamTimelineResponse)]
pub struct LoadTeamTimelineRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct LoadTeamTimelineResponse {
    pub events: Vec<TeamTimelineEventDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamTimelineEventDto {
    pub message_id: i64,
    pub session_id: String,
    pub timestamp: String,
    pub kind: String,
    pub role: String,
    pub agent_id: String,
    pub content: String,
    pub thinking: String,
    pub tool_calls: String,
    pub content_blocks: String,
    pub channel_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub duration_ms: i64,
    pub priority: i32,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessageDto {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub agent_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub thinking: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub tool_calls: String,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(default)]
    pub duration_ms: i64,
    /// JSON-serialized contentBlocks for interleaved order
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub content_blocks: String,
    /// A2A delegation event channel (e.g. "a2a:delegation_start")
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub channel_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/delete_session", response = DeleteSessionResponse)]
pub struct DeleteSessionRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct DeleteSessionResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/update_session_title", response = UpdateSessionTitleResponse)]
pub struct UpdateSessionTitleRequest {
    pub session_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct UpdateSessionTitleResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistorySyncPayload {
    pub session_id: String,
    pub messages: Vec<SessionMessageDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistorySyncNotification {
    pub session_id: String,
    pub messages: Vec<SessionMessageDto>,
}

impl sacp::JsonRpcMessage for HistorySyncNotification {
    fn to_untyped_message(&self) -> Result<sacp::UntypedMessage, sacp::Error> {
        sacp::UntypedMessage::new(NOTIF_HISTORY_SYNC, self)
    }
    fn method(&self) -> &str {
        NOTIF_HISTORY_SYNC
    }
    fn parse_message(_method: &str, params: &impl Serialize) -> Result<Self, sacp::Error> {
        let s = serde_json::to_string(params).map_err(sacp::Error::into_internal_error)?;
        serde_json::from_str(&s).map_err(sacp::Error::into_internal_error)
    }
    fn matches_method(method: &str) -> bool {
        method == NOTIF_HISTORY_SYNC
    }
}
impl sacp::JsonRpcNotification for HistorySyncNotification {}

// ─── ACP SetSessionConfigOption RPC types ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "session/set_config_option", response = SetSessionConfigOptionResponse)]
pub struct SetSessionConfigOptionRequest {
    #[serde(rename = "sessionId")]
    pub session_id: serde_json::Value,
    #[serde(rename = "configId")]
    pub config_id: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct SetSessionConfigOptionResponse {
    /// ACP SDK expects configOptions array; empty = no options to report back
    #[serde(rename = "configOptions", default)]
    pub config_options: Vec<serde_json::Value>,
}

// ─── Settings Read/Write RPC types ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/read_settings", response = ReadSettingsResponse)]
pub struct ReadSettingsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ReadSettingsResponse {
    pub settings: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/write_setting", response = WriteSettingResponse)]
pub struct WriteSettingRequest {
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct WriteSettingResponse {
    pub settings: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/engine/get_capabilities", response = GetEngineCapabilitiesResponse)]
pub struct GetEngineCapabilitiesRequest {
    #[serde(rename = "engineId", default)]
    pub engine_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct GetEngineCapabilitiesResponse {
    #[serde(rename = "currentEngine")]
    pub current_engine: String,
    pub profile: serde_json::Value,
    pub matrix: serde_json::Value,
}

// ─── Brain MCP JSON Read/Write RPC types ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/read_mcp_json", response = ReadMcpJsonResponse)]
pub struct ReadMcpJsonRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ReadMcpJsonResponse {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/write_mcp_json", response = WriteMcpJsonResponse)]
pub struct WriteMcpJsonRequest {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct WriteMcpJsonResponse {
    pub ok: bool,
    pub error: Option<String>,
}

// ─── Agent Context Read/Write RPC types ──────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/read_context", response = ReadContextResponse)]
pub struct ReadContextRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ReadContextResponse {
    #[serde(default)]
    pub system: String,
    pub identity: String,
    pub soul: String,
    pub user: String,
    pub memory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/write_context", response = WriteContextResponse)]
pub struct WriteContextRequest {
    pub file: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct WriteContextResponse {
    pub ok: bool,
}

// ─── Memory Search/CRUD RPC types ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/memory_search", response = MemorySearchResponse)]
pub struct MemorySearchRequest {
    pub query: String,
    #[serde(default = "default_memory_limit")]
    pub limit: usize,
}

// ─── Workflow Artifacts (DESIGN, PLAN, VERIFICATION) ─────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/list_workflow_artifacts", response = ListWorkflowArtifactsResponse)]
pub struct ListWorkflowArtifactsRequest {
    #[serde(default)]
    pub project_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ListWorkflowArtifactsResponse {
    pub artifacts: Vec<WorkflowArtifactDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowArtifactDto {
    pub id: String,            // filename
    pub artifact_type: String, // DESIGN, PLAN, VERIFICATION
    pub project_path: Option<String>,
    pub date: Option<String>,
    pub timestamp: i64, // for sorting
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/read_workflow_artifact", response = ReadWorkflowArtifactResponse)]
pub struct ReadWorkflowArtifactRequest {
    pub id: String, // filename
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ReadWorkflowArtifactResponse {
    pub content: String,
}

pub fn default_memory_limit() -> usize {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct MemorySearchResponse {
    pub chunks: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/memory_list", response = MemoryListResponse)]
pub struct MemoryListRequest {
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_memory_limit")]
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct MemoryListResponse {
    pub chunks: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/memory_store", response = MemoryStoreResponse)]
pub struct MemoryStoreRequest {
    pub text: String,
    #[serde(default = "default_manual_source")]
    pub source: String,
}
pub fn default_manual_source() -> String {
    "manual".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct MemoryStoreResponse {
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/memory_forget", response = MemoryForgetResponse)]
pub struct MemoryForgetRequest {
    pub chunk_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct MemoryForgetResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/memory_stats", response = MemoryStatsResponse)]
pub struct MemoryStatsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct MemoryStatsResponse {
    pub stats: memory_store::MemoryStats,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/memory_pin", response = MemoryPinResponse)]
pub struct MemoryPinRequest {
    pub chunk_id: i64,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct MemoryPinResponse {
    pub ok: bool,
}

// ─── Notification RPC types ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/notification_list", response = NotificationListResponse)]
pub struct NotificationListRequest {
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_notif_limit")]
    pub limit: usize,
}
pub fn default_notif_limit() -> usize {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct NotificationListResponse {
    pub notifications: Vec<notification_store::Notification>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/notification_stats", response = NotificationStatsResponse)]
pub struct NotificationStatsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct NotificationStatsResponse {
    pub stats: notification_store::NotificationStats,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/notification_mark_read", response = NotificationMarkReadResponse)]
pub struct NotificationMarkReadRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct NotificationMarkReadResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/notification_mark_all_read", response = NotificationMarkAllReadResponse)]
pub struct NotificationMarkAllReadRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct NotificationMarkAllReadResponse {
    pub count: usize,
}

// ─── Task List RPC types ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/list_schedules", response = ListTasksResponse)]
pub struct ListTasksRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ListTasksResponse {
    pub schedules: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/list_projects", response = ListProjectsResponse)]
pub struct ListProjectsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ListProjectsResponse {
    pub projects: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/create_task", response = CreateTaskResponse)]
pub struct CreateTaskRequest {
    pub id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct CreateTaskResponse {
    pub task: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/update_task", response = UpdateTaskResponse)]
pub struct UpdateTaskRequest {
    pub id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub done: Option<bool>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct UpdateTaskResponse {
    pub task: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/delete_task", response = DeleteTaskResponse)]
pub struct DeleteTaskRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct DeleteTaskResponse {
    pub ok: bool,
    pub error: Option<String>,
}

// ─── Swarm Task RPC types (Low-level A2A execution status) ───────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/list_swarm_schedules", response = ListSwarmTasksResponse)]
pub struct ListSwarmTasksRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ListSwarmTasksResponse {
    pub schedules: Vec<serde_json::Value>,
}

// ─── A2A Task Aggregation RPC types (fan-out across agents) ──────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/list_a2a_schedules", response = ListA2ATasksResponse)]
pub struct ListA2ATasksRequest {
    /// Optional filter by task status (e.g. "working", "completed")
    #[serde(default)]
    pub status_filter: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ListA2ATasksResponse {
    pub agents: Vec<AgentTasksDto>,
    pub total_schedules: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/get_a2a_timeline", response = GetA2ATimelineResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetA2ATimelineRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetA2ATimelineResponse {
    pub session_id: String,
    pub events: Vec<brain_session_rs::session_store::A2ATimelineEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTasksDto {
    pub role: String,
    pub endpoint: String,
    pub schedules: Vec<serde_json::Value>,
    pub task_count: usize,
    pub error: Option<String>,
}

// ─── Shared Task Pool RPC types (unassigned → claim) ─────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedTaskDto {
    pub id: String,
    pub description: String,
    pub created_at: String,
    /// None = unassigned, Some(role) = claimed by agent
    pub claimed_by: Option<String>,
    /// A2A task state after claim
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/create_shared_task", response = CreateSharedTaskResponse)]
pub struct CreateSharedTaskRequest {
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct CreateSharedTaskResponse {
    pub task: SharedTaskDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/list_shared_schedules", response = ListSharedTasksResponse)]
pub struct ListSharedTasksRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ListSharedTasksResponse {
    pub schedules: Vec<SharedTaskDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/claim_shared_task", response = ClaimSharedTaskResponse)]
pub struct ClaimSharedTaskRequest {
    pub schedule_id: String,
    pub agent_role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ClaimSharedTaskResponse {
    pub success: bool,
    pub task: Option<SharedTaskDto>,
    pub error: Option<String>,
}

// ─── Team Agent RPC types ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/team_list", response = TeamListResponse)]
pub struct TeamListRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct TeamListResponse {
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/team_save", response = TeamSaveResponse)]
pub struct TeamSaveRequest {
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct TeamSaveResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/team_presets", response = TeamPresetsResponse)]
pub struct TeamPresetsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct TeamPresetsResponse {
    pub presets: serde_json::Value,
}

// ─── A2A Card Fetch RPC types ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/a2a_card", response = A2ACardResponse)]
pub struct A2ACardRequest {
    /// The A2A server endpoint URL (e.g. "http://127.0.0.1:41242")
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct A2ACardResponse {
    /// The parsed agent card JSON, or null if unreachable
    pub card: Option<serde_json::Value>,
}

// ─── Cached ConfigOptions RPC types ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/get_config_options", response = GetConfigOptionsResponse)]
pub struct GetConfigOptionsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct GetConfigOptionsResponse {
    /// Cached configOptions from the last session/new, or empty if not yet available
    pub config_options: Vec<serde_json::Value>,
}

// ─── Artifact Versioning RPC types ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/list_session_artifacts", response = ListSessionArtifactsResponse)]
pub struct ListSessionArtifactsRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ListSessionArtifactsResponse {
    pub artifacts: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/list_artifact_versions", response = ListArtifactVersionsResponse)]
pub struct ListArtifactVersionsRequest {
    pub session_id: String,
    pub filename: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ListArtifactVersionsResponse {
    pub versions: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/get_artifact_version", response = GetArtifactVersionResponse)]
pub struct GetArtifactVersionRequest {
    pub session_id: String,
    pub filename: String,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct GetArtifactVersionResponse {
    pub artifact: Option<serde_json::Value>,
}

// ─── Built-in MCP Tool input/output types ────────────────────────────────

// 🧠 Memory
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryReadInput {
    /// Which context section to read: "identity", "soul", "user", "memory", or "all"
    #[serde(default = "default_all")]
    pub section: String,
}
pub fn default_all() -> String {
    "all".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryWriteInput {
    /// Which context section: "identity", "soul", "user", "memory"
    pub section: String,
    /// New content for the context file
    pub content: String,
}

// 🧠 Memory search/store/forget MCP tool inputs
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolSearchInput {
    /// Search query for BM25 full-text search over memory.
    pub query: String,
    /// Max results (default 5)
    #[serde(default = "default_five")]
    pub limit: usize,
}
pub fn default_five() -> usize {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolStoreInput {
    /// Memory category. Must be one of: 'user_preference', 'project_context', 'bug_fix_pattern', 'api_usage', or 'general'.
    #[serde(default = "default_memory_type")]
    pub memory_type: String,
    /// Text to store as a memory chunk.
    pub text: String,
    /// Source label: "manual", "auto-capture", etc.
    #[serde(default = "default_manual")]
    pub source: String,
}
pub fn default_memory_type() -> String {
    "general".to_string()
}
pub fn default_manual() -> String {
    "manual".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolForgetInput {
    /// Chunk ID to delete.
    pub chunk_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolListInput {
    /// Pagination offset (default 0)
    #[serde(default)]
    pub offset: usize,
    /// Max results (default 20)
    #[serde(default = "default_twenty")]
    pub limit: usize,
}
pub fn default_twenty() -> usize {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolStatsInput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolPinInput {
    /// Chunk ID to pin/unpin.
    pub chunk_id: i64,
    /// Whether to pin (true) or unpin (false).
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolPromoteInput {
    /// Source session id containing the artifact to promote.
    pub session_id: String,
    /// Artifact type, e.g. task, plan, walkthrough.
    pub artifact_type: String,
    /// Target knowledge item id.
    pub ki_id: String,
    /// Target knowledge item title.
    pub title: String,
    /// Optional tags for the promoted KI.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolExtractInput {
    /// Existing knowledge item id.
    pub ki_id: String,
    /// Optional namespace for the KI.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Optional vault path/name override.
    #[serde(default)]
    pub vault_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolDreamPromoteInput {
    /// Array of raw memory chunk IDs that were synthesized into this knowledge item.
    pub chunk_ids: Vec<i64>,
    /// Target knowledge item ID (e.g. "api-auth-flow").
    pub ki_id: String,
    /// Target knowledge item title.
    pub title: String,
    /// Synthesized markdown content for the knowledge item.
    pub content: String,
    /// Optional tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolDreamPreviewInput {
    /// Maximum number of pending groups to inspect.
    #[serde(default = "default_five")]
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolDreamAnalyzeInput {
    /// Directory scope to analyze.
    pub dir: String,
    /// Maximum number of important groups to inspect.
    #[serde(default = "default_five")]
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolDreamApplyInput {
    /// Memory chunk ids to update.
    pub ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KbWorkspaceListInput {
    #[serde(rename = "workspace_id", default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KbIngestInput {
    #[serde(rename = "workspace_id", default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KbCompileInput {
    #[serde(rename = "workspace_id", default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KbLintInput {
    #[serde(rename = "workspace_id", default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KbQueryInput {
    #[serde(rename = "workspace_id", default)]
    pub workspace_id: Option<String>,
    pub query: String,
    #[serde(rename = "output_path", default)]
    pub output_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KbFileBackInput {
    #[serde(rename = "workspace_id", default)]
    pub workspace_id: Option<String>,
    pub target: String,
    #[serde(rename = "relative_path")]
    pub relative_path: String,
    pub content: String,
}

// 💬 Session
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmptyInput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionIdInput {
    /// Session ID
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionRenameInput {
    /// Session ID
    pub session_id: String,
    /// New title
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionSearchInput {
    /// Full-text query for session title/message search.
    pub query: String,
    /// Maximum number of results to return.
    #[serde(default = "default_session_search_limit")]
    pub limit: i64,
}

fn default_session_search_limit() -> i64 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillViewInput {
    /// Skill name or relative path under brain/skills.
    pub name: String,
    /// Optional supporting file path relative to the skill directory.
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillUpsertInput {
    /// Skill folder name or relative path. Generated skills are always stored under brain/skills/custom/.
    pub name: String,
    /// Full SKILL.md content with YAML frontmatter containing name and description.
    pub content: String,
    /// Set true only after reading the existing skill and intentionally updating it.
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IdInput {
    /// Item ID
    pub id: String,
}

// 📄 Artifact
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactSaveInput {
    /// Artifact type: "task", "plan", "walkthrough", or "other"
    pub artifact_type: String,
    /// Full markdown content of the artifact
    pub content: String,
    /// Brief summary of this version (optional)
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactListInput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactGetInput {
    /// Artifact type: "task", "plan", "walkthrough", or "other"
    pub artifact_type: String,
    /// Specific version number to retrieve (optional, defaults to latest)
    #[serde(default)]
    pub version: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactEditInput {
    /// Artifact type: "task", "plan", "walkthrough", or "other"
    pub artifact_type: String,
    /// Full updated markdown content of the artifact
    pub content: String,
    /// Brief summary of what was changed in this edit (optional)
    #[serde(default)]
    pub summary: Option<String>,
}

// NOTE: BrainstormInput, CreateExecutionPlanInput, VerifyExecutionInput removed.
// Replaced by Superpowers skill files in ~/.agents/skills/ (prompt injection).

// ✅ Task
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskCreateInput {
    /// Task title
    pub title: String,
    /// Task description (optional)
    pub description: Option<String>,
    /// Schedule time in HH:MM format, e.g. "09:00" (optional)
    pub schedule: Option<String>,
    /// Category/tag ID (optional)
    pub category: Option<String>,
    /// Days of week: 0=Sun, 1=Mon, 2=Tue, 3=Wed, 4=Thu, 5=Fri, 6=Sat. Empty array = everyday. (optional)
    pub days: Option<Vec<u8>>,
    /// Agent prompt to execute when this task runs (optional)
    pub prompt: Option<String>,
    /// Cron-like interval expression: "30m", "1h", "24h" (optional, makes task recurring)
    pub cron_expr: Option<String>,
    /// Target URL for web automation schedules (optional)
    pub target_url: Option<String>,
    /// Detailed instructions for the agent (optional)
    pub instructions: Option<String>,
    /// Preferred team roles for delegation/reassignment hints (optional)
    pub preferred_roles: Option<Vec<String>>,
    /// Whether this recurring task is enabled (default: true)
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskUpdateInput {
    /// Task ID
    pub id: String,
    /// New title (optional)
    pub title: Option<String>,
    /// New description (optional)
    pub description: Option<String>,
    /// Mark as done (optional)
    pub done: Option<bool>,
    /// Status text (optional)
    pub status: Option<String>,
    /// Schedule time in HH:MM format (optional)
    pub schedule: Option<String>,
    /// Category/tag ID (optional)
    pub category: Option<String>,
    /// Days of week: 0=Sun..6=Sat (optional)
    pub days: Option<Vec<u8>>,
    /// Agent prompt (optional)
    pub prompt: Option<String>,
    /// Cron-like interval (optional)
    pub cron_expr: Option<String>,
    /// Target URL (optional)
    pub target_url: Option<String>,
    /// Instructions (optional)
    pub instructions: Option<String>,
    /// Preferred team roles for delegation/reassignment hints (optional)
    pub preferred_roles: Option<Vec<String>>,
    /// Enable/disable (optional)
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskAddHistoryInput {
    /// Task ID
    pub id: String,
    /// Action type (e.g. "agent_action", "cron_run", "result", "observation")
    pub action: String,
    /// Details about the action (optional)
    pub detail: Option<String>,
    /// Session ID related to this action (optional)
    pub session_id: Option<String>,
}

// 🔔 UI
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UiNotifyInput {
    /// Notification message
    pub message: String,
    /// Notification level: "info", "success", "warning", "error"
    #[serde(default = "default_info")]
    pub level: String,
}
pub fn default_info() -> String {
    "info".to_string()
}

// 📤 Propose to Leader
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProposeToLeaderInput {
    /// The message, question, or proposal intended for the leader.
    pub message: String,
    /// The level of the proposal: "info", "suggestion", "blocker", "review_request"
    #[serde(default = "default_info")]
    pub level: String,
}

// 🤝 Team Delegation (A2A)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SpawnSubagentInput {
    /// The role or name of the ephemeral worker (e.g. "Researcher", "Reviewer").
    pub role: String,
    /// The specific task or goal this subagent needs to accomplish.
    pub goal: String,
    /// Any necessary context, history, or partial results to pass to the subagent.
    #[serde(default)]
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TeamDelegateInput {
    /// Task description to delegate to the team agent.
    pub query: String,
    /// Delegation mode: "sync" (wait for result), "async" (fire-and-forget),
    /// "subscribe" (fire + background subscription, wakes up with System Alert).
    /// Default: "sync".
    #[serde(default = "default_sync")]
    pub mode: String,
}
pub fn default_sync() -> String {
    "sync".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TeamProposeInput {
    /// 대상 에이전트 이름 (예: "Leader", "Developer", "Planner")
    pub agent: String,
    /// 대상 에이전트에게 보낼 제안, 보고, 피드백 내용
    pub message: String,
}

// 📦 Team State Channel
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TeamUpdateChannelInput {
    /// The specific channel variable or state key you want to update
    pub key: String,
    /// The new JSON value string to store in the channel for this key
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TeamReadChannelInput {
    /// The specific channel variable to read. Leave empty to read all.
    #[serde(default)]
    pub key: Option<String>,
}

// ⏱ Time-Travel Resume & Checkpoint
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TeamSaveCheckpointInput {
    /// Target session ID
    pub session_id: String,
    /// Thread ID (default: "main")
    pub thread_id: String,
    /// Checkpoint version string (e.g., "v1.2", or UUID)
    pub version: String,
    /// Parent version, if any
    #[serde(default)]
    pub parent_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TeamResumeTaskInput {
    /// Target session ID
    pub session_id: String,
    /// Thread ID
    pub thread_id: String,
    /// Target version to resume/rollback from
    pub version: String,
}

// ─── AssistantBuffer ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContentBlock {
    Thinking {
        text: String,
    },
    Text {
        text: String,
    },
    ToolCalls {
        #[serde(rename = "toolCallIds")]
        tool_call_ids: Vec<String>,
    },
}

/// AssistantBuffer is now a type alias for TurnAccumulator.
/// This unifies the Solo (RelayProxy) and Team (leader_loop)
/// response processing paths into a single buffer type.
pub type AssistantBuffer = crate::turn_accumulator::TurnAccumulator;

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "_agent/capabilities", response = CapabilitiesResponse)]
pub struct CapabilitiesRequest {
    #[serde(rename = "agentId", default)]
    pub agent_id: Option<String>,
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct CapabilitiesResponse {
    pub skills: Vec<serde_json::Value>,
    pub mcps: Vec<serde_json::Value>,
    #[serde(default)]
    pub engines: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "_agent/skill/toggle", response = ToggleSkillResponse)]
pub struct ToggleSkillRequest {
    #[serde(rename = "agentId", default)]
    pub agent_id: Option<String>,
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    pub name: String,
    pub enable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ToggleSkillResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "_agent/mcp/toggle", response = ToggleMcpResponse)]
pub struct ToggleMcpRequest {
    #[serde(rename = "agentId", default)]
    pub agent_id: Option<String>,
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    pub name: String,
    pub enable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ToggleMcpResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/set_team_agent_engine", response = SetTeamAgentEngineResponse)]
pub struct SetTeamAgentEngineRequest {
    pub session_id: String,
    pub role: String,
    pub engine: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct SetTeamAgentEngineResponse {
    pub success: bool,
}
