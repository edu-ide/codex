//! AdminProxy — Settings, Context, Memory, Notification, Plugin, Task RPCs.
//!
//! All "Client → Proxy" RPC handlers that do NOT touch the agent message flow.
//! These are pure CRUD operations served entirely within the proxy.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use sacp::{Client, Conductor, ConnectTo, ConnectionTo, Proxy, Responder};
use serde_json::json;
use tracing::{info, warn};

use crate::notification_store;
use crate::relay_server::{RelayEvent, broadcast_event};
use brain_knowledge_rs::memory_store;

// Re-use stores from crate root

// ─── RPC type imports from main.rs ──────────────────────────────────────
// These will remain defined in main.rs (pub) for now and be
// gradually moved here in future phases.
use crate::{
    A2ACardRequest, A2ACardResponse, AgentTasksDto, BUILTIN_PLUGINS, ClaimSharedTaskRequest,
    ClaimSharedTaskResponse, CreateSharedTaskRequest, CreateSharedTaskResponse, CreateTaskRequest,
    CreateTaskResponse, DeleteTaskRequest, DeleteTaskResponse, GetA2ATimelineRequest,
    GetA2ATimelineResponse, GetArtifactVersionRequest, GetArtifactVersionResponse,
    GetConfigOptionsRequest, GetConfigOptionsResponse, GetEngineCapabilitiesRequest,
    GetEngineCapabilitiesResponse, ListA2ATasksRequest, ListA2ATasksResponse,
    ListArtifactVersionsRequest, ListArtifactVersionsResponse, ListPluginsRequest,
    ListPluginsResponse, ListProjectsRequest, ListProjectsResponse, ListSessionArtifactsRequest,
    ListSessionArtifactsResponse, ListSharedTasksRequest, ListSharedTasksResponse,
    ListTasksRequest, ListTasksResponse, ListWorkflowArtifactsRequest,
    ListWorkflowArtifactsResponse, MemoryForgetRequest, MemoryForgetResponse, MemoryListRequest,
    MemoryListResponse, MemoryPinRequest, MemoryPinResponse, MemorySearchRequest,
    MemorySearchResponse, MemoryStatsRequest, MemoryStatsResponse, MemoryStoreRequest,
    MemoryStoreResponse, NotificationListRequest, NotificationListResponse,
    NotificationMarkAllReadRequest, NotificationMarkAllReadResponse, NotificationMarkReadRequest,
    NotificationMarkReadResponse, NotificationStatsRequest, NotificationStatsResponse, PluginInfo,
    ReadContextRequest, ReadContextResponse, ReadMcpJsonRequest, ReadMcpJsonResponse,
    ReadSettingsRequest, ReadSettingsResponse, ReadWorkflowArtifactRequest,
    ReadWorkflowArtifactResponse, SharedTaskDto, TeamListRequest, TeamListResponse,
    TeamPresetsRequest, TeamPresetsResponse, TeamSaveRequest, TeamSaveResponse,
    TogglePluginRequest, TogglePluginResponse, UpdateTaskRequest, UpdateTaskResponse,
    WorkflowArtifactDto, WriteContextRequest, WriteContextResponse, WriteMcpJsonRequest,
    WriteMcpJsonResponse, WriteSettingRequest, WriteSettingResponse, builtin_plugin_list,
    mcp_preset_description,
};

// ─── Team presets ────────────────────────────────────────────────────────

pub fn team_presets() -> serde_json::Value {
    let ports = crate::port_config::team_ports(4);
    let get_prompt = |_id: &str| String::new();

    json!({
        "base": {
            "name": "Base",
            "description": "범용 협업 팀 — 리서치, 검증, 창의적 문제 해결",
            "agents": [
                {
                    "role": "Leader",
                    "endpoint": format!("http://localhost:{}", ports[0]),
                    "tags": ["leader", "integration", "coordination"],
                    "color": "#7c3aed",
                    "avatar": "👑",
                    "system_prompt": get_prompt("leader")
                },
                {
                    "role": "Researcher",
                    "endpoint": format!("http://localhost:{}", ports[1]),
                    "tags": ["research", "web", "analysis"],
                    "color": "#3b82f6",
                    "avatar": "🔍",
                    "system_prompt": get_prompt("researcher")
                },
                {
                    "role": "Verifier",
                    "endpoint": format!("http://localhost:{}", ports[2]),
                    "tags": ["verification", "strategy", "fact-check"],
                    "color": "#10b981",
                    "avatar": "✅",
                    "system_prompt": get_prompt("verifier")
                },
                {
                    "role": "Creator",
                    "endpoint": format!("http://localhost:{}", ports[3]),
                    "tags": ["logic", "creative", "technical"],
                    "color": "#f59e0b",
                    "avatar": "💡",
                    "system_prompt": get_prompt("creator")
                }
            ],
            "team_prompt": "Delegation protocol (mandatory): You are the Leader..."
        },
        "coding": {
            "name": "Coding",
            "description": "소프트웨어 개발 팀 — 설계, 구현, 리뷰, 테스트",
            "agents": [
                {
                    "role": "Planner",
                    "endpoint": format!("http://localhost:{}", ports[0]),
                    "tags": ["planning", "architecture", "design"],
                    "color": "#6366f1",
                    "avatar": "📋",
                    "system_prompt": get_prompt("planner")
                },
                {
                    "role": "Coder",
                    "endpoint": format!("http://localhost:{}", ports[1]),
                    "tags": ["coding", "implementation"],
                    "color": "#06b6d4",
                    "avatar": "💻",
                    "system_prompt": get_prompt("coder")
                },
                {
                    "role": "Reviewer",
                    "endpoint": format!("http://localhost:{}", ports[2]),
                    "tags": ["review", "quality", "standards"],
                    "color": "#f43f5e",
                    "avatar": "🔎",
                    "system_prompt": get_prompt("reviewer")
                },
                {
                    "role": "QA",
                    "endpoint": format!("http://localhost:{}", ports[3]),
                    "tags": ["testing", "debugging", "validation"],
                    "color": "#f97316",
                    "avatar": "🧪",
                    "system_prompt": get_prompt("qa")
                }
            ],
            "team_prompt": "Software development team: Planner designs, Coder implements, Reviewer ensures quality, QA validates correctness."
        }
    })
}

pub fn default_team_config() -> serde_json::Value {
    let presets = team_presets();
    let base = presets.get("base").unwrap();
    json!({
        "agents": base["agents"],
        "team_prompt": base["team_prompt"],
        "auto_approve": true
    })
}

// ─── AdminProxy state ───────────────────────────────────────────────────

pub struct AdminProxy {
    pub state: Arc<crate::SharedState>,
}

impl ConnectTo<Conductor> for AdminProxy {
    async fn connect_to(self, conductor: impl ConnectTo<Proxy>) -> Result<(), sacp::Error> {
        let s = self.state;

        let builder = Proxy.builder().name("admin-proxy");
        let builder = crate::register_admin_settings_handlers!(builder, s);
        let builder = crate::register_admin_notification_handlers!(builder, s);
        let builder = crate::register_admin_task_handlers!(builder, s);
        let builder = crate::register_admin_memory_handlers!(builder, s);
        let builder = crate::register_admin_plugin_handlers!(builder, s);
        let builder = crate::register_admin_team_handlers!(builder, s);
        let builder = crate::register_admin_artifact_handlers!(builder, s);

        builder
            .connect_with(conductor, async move |cx: ConnectionTo<Conductor>| {
                s.infra.relay_conductor_cx.try_add(cx).await;
                std::future::pending::<Result<(), sacp::Error>>().await
            })
            .await
    }
}
