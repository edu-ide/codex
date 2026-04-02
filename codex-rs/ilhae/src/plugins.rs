//! Plugin system for ilhae-proxy.
//!
//! Built-in plugin definitions, plugin list builder, MCP preset normalization.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Built-in plugin definitions ─────────────────────────────────────────

/// Built-in tool definition: (name, description)
type BuiltinToolDef = (&'static str, &'static str);

/// Built-in plugin definition struct.
pub struct BuiltinPluginDef {
    pub id: &'static str,
    pub emoji: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub tools: &'static [BuiltinToolDef],
}

pub const BUILTIN_PLUGINS: &[BuiltinPluginDef] = &[
    BuiltinPluginDef {
        id: "session",
        emoji: "📋",
        label: "세션",
        description: "세션 관리 도구",
        tools: &[
            ("session_list", "모든 세션 목록 조회"),
            ("session_load", "특정 세션의 메시지 로드"),
            ("session_search", "세션 제목/메시지 전문 검색"),
            ("session_delete", "세션 삭제"),
            ("session_rename", "세션 이름 변경"),
        ],
    },
    BuiltinPluginDef {
        id: "memory",
        emoji: "🧠",
        label: "메모리",
        description: "에이전트 장기 메모리",
        tools: &[
            (
                "memory_read",
                "에이전트 컨텍스트 읽기 (IDENTITY, SOUL, USER, 핀된 메모리)",
            ),
            ("memory_write", "에이전트 컨텍스트 쓰기"),
            ("memory_search", "BM25 전문 검색으로 메모리 검색"),
            ("memory_store", "새로운 메모리 청크 저장"),
            ("memory_forget", "메모리 청크 삭제"),
            ("memory_list", "메모리 청크 목록 조회"),
            ("memory_stats", "메모리 통계 정보"),
            ("memory_pin", "메모리 청크 고정/해제 (프롬프트 주입)"),
            ("memory_promote", "세션 아티팩트를 지식 항목으로 승격"),
            ("memory_extract", "지식 항목을 볼트 노트로 추출"),
            ("memory_dream_preview", "드림 대기 그룹 미리보기"),
            ("memory_dream_analyze", "디렉터리 범위 드림 분석"),
            ("memory_dream_summarize", "드림 그룹을 요약 완료로 표시"),
            ("memory_dream_ignore", "드림 그룹을 무시 상태로 표시"),
        ],
    },
    BuiltinPluginDef {
        id: "skills",
        emoji: "🧩",
        label: "스킬",
        description: "brain/skills 기반 온디맨드 스킬 로딩",
        tools: &[
            ("skills_list", "사용 가능한 스킬 목록 조회"),
            ("skill_view", "스킬 본문 또는 지원 파일 로드"),
        ],
    },
    BuiltinPluginDef {
        id: "task",
        emoji: "✅",
        label: "작업 관리",
        description: "할 일, 스케줄, 자동화 미션 통합 관리",
        tools: &[
            ("task_list", "작업 목록 조회"),
            ("task_create", "새 작업 생성 (할 일/스케줄/크론/미션 통합)"),
            ("task_update", "작업 수정"),
            ("task_delete", "작업 삭제"),
            ("task_add_history", "작업 히스토리 추가"),
            ("task_run", "작업 즉시 실행"),
            ("project_list", "프로젝트 작업 목록 조회 (task_list alias)"),
            ("project_create", "프로젝트 작업 생성 (task_create alias)"),
            ("project_update", "프로젝트 작업 수정 (task_update alias)"),
            ("project_delete", "프로젝트 작업 삭제 (task_delete alias)"),
            (
                "project_add_history",
                "프로젝트 작업 히스토리 추가 (task_add_history alias)",
            ),
            ("project_run", "프로젝트 작업 즉시 실행 (task_run alias)"),
            (
                "delegate",
                "팀/서브 에이전트에게 작업 위임 후 결과가 올 때까지 동기 대기 (A2A)",
            ),
            (
                "delegate_background",
                "팀/서브 에이전트에게 작업 위임 후, 백그라운드에서 실행하고 완료 시 알림 수신 (A2A)",
            ),
            (
                "propose",
                "팀 에이전트에게 비동기로 보고, 제안, 피드백 전송 (응답 대기 안함) (A2A)",
            ),
            (
                "spawn_subagent",
                "일회성 서브 에이전트(워커) 생성 후 작업 결과 취합 (A2A)",
            ),
        ],
    },
    BuiltinPluginDef {
        id: "ui",
        emoji: "🔔",
        label: "UI 알림",
        description: "데스크톱 실시간 알림",
        tools: &[("ui_notify", "데스크톱 알림 전송")],
    },
    BuiltinPluginDef {
        id: "browser",
        emoji: "🌐",
        label: "브라우저",
        description: "CDP 기반 브라우저 자동화",
        tools: &[
            ("browser_navigate", "URL로 이동"),
            ("browser_go_back", "뒤로 가기"),
            ("browser_go_forward", "앞으로 가기"),
            ("browser_snapshot", "페이지 스냅샷 (인덱스된 요소 포함)"),
            ("browser_screenshot", "PNG 스크린샷 캡처"),
            ("browser_get_markdown", "페이지를 마크다운으로 변환"),
            ("browser_evaluate", "JavaScript 실행"),
            ("browser_click", "요소 클릭"),
            ("browser_hover", "요소 호버"),
            ("browser_select", "드롭다운 선택"),
            ("browser_input_fill", "입력 필드 텍스트 입력"),
            ("browser_press_key", "키보드 키 입력"),
            ("browser_scroll", "페이지 스크롤"),
            ("browser_wait", "요소 대기"),
            ("browser_new_tab", "새 탭 열기"),
            ("browser_tab_list", "탭 목록"),
            ("browser_switch_tab", "탭 전환"),
            ("browser_close_tab", "탭 닫기"),
            ("browser_close", "브라우저 종료"),
            ("browser_console_messages", "콘솔 메시지 수집"),
            ("browser_network_requests", "네트워크 요청 모니터링"),
            ("browser_handle_dialog", "다이얼로그 처리"),
            ("browser_drag", "드래그 앤 드롭"),
            ("browser_file_upload", "파일 업로드"),
            ("browser_pdf", "PDF 생성"),
            ("browser_resize", "뷰포트 리사이즈"),
        ],
    },
    // NOTE: "workflow" plugin removed — replaced by Superpowers skill files
    // in ~/.agents/skills/ (brainstorming, writing-plans, etc.)
    // See superpowers_skills.rs for provisioning.
    BuiltinPluginDef {
        id: "artifact",
        emoji: "📄",
        label: "아티팩트",
        description: "세션 아티팩트 (task, plan, walkthrough) 생성 및 버전 관리",
        tools: &[
            (
                "artifact_save",
                "아티팩트 생성 또는 업데이트. 자동 버저닝 지원. artifact_type: task, plan, walkthrough, other",
            ),
            ("artifact_list", "현재 세션의 모든 아티팩트 목록 조회"),
        ],
    },
];

/// Resolve tool name → plugin id
pub fn tool_to_plugin_id(tool_name: &str) -> Option<&'static str> {
    for plugin in BUILTIN_PLUGINS {
        for (t, _) in plugin.tools {
            if *t == tool_name {
                return Some(plugin.id);
            }
        }
    }
    None
}

// ─── Plugin RPC types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/list_plugins", response = ListPluginsResponse)]
pub struct ListPluginsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct ListPluginsResponse {
    pub plugins: Vec<PluginInfo>,
    #[serde(default)]
    pub builtin_plugins: Vec<BuiltinPluginInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub label: String,
    pub description: String,
    pub enabled: bool,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinToolInfo {
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinPluginInfo {
    pub id: String,
    pub emoji: String,
    pub label: String,
    pub description: String,
    pub enabled: bool,
    pub auto_approve: bool,
    pub tools: Vec<BuiltinToolInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcRequest)]
#[request(method = "ilhae/toggle_plugin", response = TogglePluginResponse)]
pub struct TogglePluginRequest {
    pub plugin_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sacp::JsonRpcResponse)]
pub struct TogglePluginResponse {
    pub plugins: Vec<PluginInfo>,
}

// ─── Plugin list builder ─────────────────────────────────────────────────

/// Build the built-in plugin list with enabled state from settings.
pub fn builtin_plugin_list(
    plugin_settings: &HashMap<String, bool>,
    auto_approve_plugins: &HashMap<String, bool>,
    browser_enabled: bool,
) -> Vec<BuiltinPluginInfo> {
    BUILTIN_PLUGINS
        .iter()
        .map(|def| {
            let enabled = if def.id == "browser" {
                browser_enabled
            } else {
                let default_enabled = true; // All other built-in plugins default to enabled
                plugin_settings
                    .get(def.id)
                    .copied()
                    .unwrap_or(default_enabled)
            };
            let auto_approve = auto_approve_plugins.get(def.id).copied().unwrap_or(false);
            let tools = def
                .tools
                .iter()
                .map(|(name, desc)| {
                    let tool_key = format!("{}.{}", def.id, name);
                    let tool_enabled = plugin_settings.get(&tool_key).copied().unwrap_or(true);
                    BuiltinToolInfo {
                        name: name.to_string(),
                        description: desc.to_string(),
                        enabled: tool_enabled,
                    }
                })
                .collect();
            BuiltinPluginInfo {
                id: def.id.to_string(),
                emoji: def.emoji.to_string(),
                label: def.label.to_string(),
                description: def.description.to_string(),
                enabled,
                auto_approve,
                tools,
            }
        })
        .collect()
}

// ─── MCP preset utilities ────────────────────────────────────────────────

pub fn normalize_mcp_preset_for_store(preset: &serde_json::Value) -> Option<serde_json::Value> {
    let id = preset.get("id").and_then(|v| v.as_str())?.trim();
    if id.is_empty() {
        return None;
    }

    let name = preset
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(id);

    let transport_type = preset
        .get("transport_type")
        .or_else(|| preset.get("transportType"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("streamable-http");

    let command = preset
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let args = preset
        .get("args")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let sse_url = preset
        .get("sse_url")
        .or_else(|| preset.get("sseUrl"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let enabled = preset
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Some(serde_json::json!({
        "id": id,
        "name": name,
        "transport_type": transport_type,
        "command": command,
        "args": args,
        "sse_url": sse_url,
        "enabled": enabled,
    }))
}

pub fn mcp_preset_description(preset: &serde_json::Value) -> String {
    let transport = preset
        .get("transport_type")
        .or_else(|| preset.get("transportType"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("stdio");

    let endpoint = preset
        .get("sse_url")
        .or_else(|| preset.get("sseUrl"))
        .or_else(|| preset.get("url"))
        .or_else(|| preset.get("server_url"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty());

    if let Some(url) = endpoint {
        return format!("{} · {}", transport.to_uppercase(), url);
    }

    if let Some(command) = preset
        .get("command")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
    {
        return format!("{} · {}", transport.to_uppercase(), command.trim());
    }

    transport.to_uppercase()
}
