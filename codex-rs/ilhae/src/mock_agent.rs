//! MockAgent — ILHAE_MOCK 전용 에이전트
//!
//! 컨덕터 체인에 Agent 역할로 연결되어 PromptRequest를 처리하고,
//! session/update(Notification)를 발행합니다. 실제 도구 실행은 간소화하여
//! artifact_save/edit, memory_write만 프록시 프로세스 내에서 수행합니다.

use crate::{SetSessionConfigOptionRequest, SetSessionConfigOptionResponse};
use sacp::schema::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, SessionId, SessionNotification, SessionUpdate, StopReason, TextContent,
};
use sacp::{Agent, Client, ConnectTo, ConnectionTo, Responder};
use serde_json::json;
use tracing::warn;

use crate::mock_provider::get_mock_response;

#[derive(Clone, Default)]
pub struct MockAgent;

impl MockAgent {
    pub fn new() -> Self {
        Self
    }

    fn gen_session_id() -> SessionId {
        SessionId::new(uuid::Uuid::new_v4().to_string())
    }

    fn emit_tool_call(
        cx: &ConnectionTo<Client>,
        session_id: &SessionId,
        tool_call_id: &str,
        title: &str,
        raw_input: &serde_json::Value,
    ) {
        let payload = json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "tool_call",
                "toolCallId": tool_call_id,
                "title": title,
                "rawInput": raw_input,
                "status": "pending"
            }
        });
        if let Ok(untyped) = sacp::UntypedMessage::new(
            agent_client_protocol_schema::CLIENT_METHOD_NAMES.session_update,
            payload,
        ) {
            let _ = cx.send_notification_to(Client, untyped);
        }
    }

    fn emit_tool_update(
        cx: &ConnectionTo<Client>,
        session_id: &SessionId,
        tool_call_id: &str,
        status: &str,
        raw_output: &str,
    ) {
        let payload = json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": tool_call_id,
                "status": status,
                "rawOutput": raw_output
            }
        });
        if let Ok(untyped) = sacp::UntypedMessage::new(
            agent_client_protocol_schema::CLIENT_METHOD_NAMES.session_update,
            payload,
        ) {
            let _ = cx.send_notification_to(Client, untyped);
        }
    }

    fn artifact_filename(kind: &str) -> &'static str {
        match kind {
            "plan" | "implementation_plan" => "implementation_plan.md",
            "walkthrough" => "walkthrough.md",
            _ => "task.md",
        }
    }
}

impl ConnectTo<Client> for MockAgent {
    async fn connect_to(self, client: impl ConnectTo<Agent>) -> Result<(), sacp::Error> {
        Agent
            .builder()
            .name("mock-agent")
            // initialize
            .on_receive_request(
                async move |req: InitializeRequest,
                            responder: Responder<InitializeResponse>,
                            _cx| {
                    let caps = AgentCapabilities::new();
                    responder.respond(
                        InitializeResponse::new(req.protocol_version).agent_capabilities(caps),
                    )
                },
                sacp::on_receive_request!(),
            )
            // new session
            .on_receive_request(
                async move |_req: NewSessionRequest,
                            responder: Responder<NewSessionResponse>,
                            _cx| {
                    responder.respond(NewSessionResponse::new(Self::gen_session_id()))
                },
                sacp::on_receive_request!(),
            )
            // load session
            .on_receive_request(
                async move |_req: LoadSessionRequest,
                            responder: Responder<LoadSessionResponse>,
                            _cx| { responder.respond(LoadSessionResponse::new()) },
                sacp::on_receive_request!(),
            )
            // prompt
            .on_receive_request(
                async move |req: PromptRequest,
                            responder: Responder<PromptResponse>,
                            cx: ConnectionTo<Client>| {
                    let sid = req.session_id.clone();

                    // 프롬프트 텍스트 추출
                    let mut prompt_text = String::new();
                    for b in &req.prompt {
                        if let ContentBlock::Text(t) = b {
                            if !t.text.starts_with("__MCP_WIDGET_CTX__:") {
                                if !prompt_text.is_empty() {
                                    prompt_text.push_str("\n");
                                }
                                prompt_text.push_str(&t.text);
                            }
                        }
                    }
                    // 텍스트 응답 + 모크 도구 호출 로드
                    let mock = get_mock_response(&prompt_text);
                    let text = mock.as_ref().map(|m| m.text.clone()).unwrap_or_default();
                    if !text.is_empty() {
                        let chunk =
                            ContentChunk::new(ContentBlock::Text(TextContent::new(text.clone())));
                        let notif = SessionNotification::new(
                            sid.clone(),
                            SessionUpdate::AgentMessageChunk(chunk),
                        );
                        let _ = cx.send_notification_to(Client, notif);
                    }

                    if let Some(m) = mock.as_ref() {
                        for tc in &m.tool_calls {
                            let tc_id = format!("mock-tc-{}", uuid::Uuid::new_v4());
                            Self::emit_tool_call(&cx, &sid, &tc_id, &tc.tool_name, &tc.raw_input);

                            // 간소화된 실행: artifact_save/edit, memory_write
                            let tool = tc.tool_name.to_lowercase();
                            if tool == "artifact_save" || tool == "artifact_edit" {
                                let art_type = tc
                                    .raw_input
                                    .get("artifact_type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("task");
                                let filename = Self::artifact_filename(art_type);
                                let content = tc
                                    .raw_input
                                    .get("content")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                // 파일 쓰기
                                let vault_dir = crate::config::get_active_vault_dir();
                                let dir = vault_dir.join("sessions").join(&*sid.0);
                                let _ = std::fs::create_dir_all(&dir);
                                let path = dir.join(filename);
                                if let Err(e) = std::fs::write(&path, content) {
                                    warn!("[MockAgent] write {:?} failed: {}", path, e);
                                }
                                // 간단 결과 알림
                                let msg = format!(
                                    "✅ Artifact '{}' saved ({} bytes)",
                                    filename,
                                    content.len()
                                );
                                Self::emit_tool_update(&cx, &sid, &tc_id, "completed", &msg);
                            } else if tool == "memory_write" {
                                let val = tc
                                    .raw_input
                                    .get("value")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let msg = if val.is_empty() {
                                    "⚠️ Empty".to_string()
                                } else {
                                    format!("✅ Memory saved ({} chars)", val.len())
                                };
                                // 메모리는 store를 통해 저장할 수도 있으나 간소화
                                Self::emit_tool_update(&cx, &sid, &tc_id, "completed", &msg);
                            } else {
                                // 기타 도구는 성공 처리만
                                let msg = tc.result_text.as_str();
                                Self::emit_tool_update(&cx, &sid, &tc_id, "completed", msg);
                            }
                        }
                    }

                    // 종료
                    responder.respond(PromptResponse::new(StopReason::EndTurn))
                },
                sacp::on_receive_request!(),
            )
            // config option
            .on_receive_request(
                async move |_req: SetSessionConfigOptionRequest,
                            responder: Responder<SetSessionConfigOptionResponse>,
                            _cx| {
                    responder.respond(SetSessionConfigOptionResponse {
                        config_options: vec![],
                    })
                },
                sacp::on_receive_request!(),
            )
            .connect_to(client)
            .await
    }
}
