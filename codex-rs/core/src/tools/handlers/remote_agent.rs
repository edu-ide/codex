// codex/codex-rs/core/src/tools/handlers/remote_agent.rs
use crate::tools::registry::{ToolHandler, ToolKind};
use crate::tools::context::{ToolInvocation, ToolOutput, ToolPayload};
use crate::function_tool::FunctionCallError;
use async_trait::async_trait;
use codex_protocol::models::FunctionCallOutputBody;
use serde::Deserialize;
use std::time::Instant;

pub struct RemoteAgentHandler {
    pub url: String,
    pub name: String,
}

impl RemoteAgentHandler {
    pub fn new(name: String, url: String) -> Self {
        Self { name, url }
    }
}

#[derive(Deserialize)]
struct RemoteAgentArgs {
    query: String,
    #[serde(default)]
    async_mode: bool,
    #[serde(default)]
    subscribe: bool,
}

#[async_trait]
impl ToolHandler for RemoteAgentHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            call_id,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments, .. } => arguments,
            _ => return Err(FunctionCallError::Fatal("Expected Function payload".into())),
        };

        // Pretend to be an MCP tool so the UI correctly shows the tool call
        let mcp_invocation = crate::protocol::McpInvocation {
            server: "remote".to_string(),
            tool: self.name.clone(),
            arguments: serde_json::from_str(&arguments).ok(),
        };
        session.send_event(
            &turn,
            crate::protocol::EventMsg::McpToolCallBegin(
                crate::protocol::McpToolCallBeginEvent {
                    call_id: call_id.clone(),
                    invocation: mcp_invocation.clone(),
                },
            ),
        ).await;
        
        let parsed: RemoteAgentArgs = serde_json::from_str(&arguments).unwrap_or(RemoteAgentArgs {
            query: arguments.clone(),
            async_mode: false,
            subscribe: false,
        });

        let base_url = self.url.replace("/.well-known/agent-card.json", "");
        let client = a2a_rs::client::A2AClient::new(base_url);
        
        let task_id = uuid::Uuid::new_v4().to_string();
        
        let req = a2a_rs::types::SendMessageRequest {
            message: a2a_rs::types::Message {
                message_id: uuid::Uuid::new_v4().to_string(),
                context_id: Some(session.conversation_id.to_string()),
                task_id: Some(task_id.clone()),
                role: a2a_rs::types::Role::User,
                parts: vec![a2a_rs::types::Part::text(&parsed.query)],
                metadata: None,
                extensions: vec![],
                reference_task_ids: None,
            },
            configuration: Some(a2a_rs::types::SendMessageConfiguration {
                accepted_output_modes: vec![],
                history_length: None,
                blocking: Some(!parsed.async_mode),
                push_notification_config: None,
            }),
            metadata: None,
        };

        let started_at = Instant::now();
        let events_webhook = std::env::var("TEAM_EVENT_WEBHOOK").ok();
        let stream_webhook = events_webhook.as_ref().map(|url| url.replace("/events", "/stream_patch"));
        let caller_name = std::env::var("CODER_AGENT_NAME").unwrap_or_else(|_| "leader".to_string());
        let http_client = reqwest::Client::new();
        
        // Send scheduled event to /events
        if let Some(url) = &events_webhook {
            let event = serde_json::json!({
                "from": caller_name,
                "to": self.name,
                "status": "scheduled",
                "taskId": task_id
            });
            let _ = http_client.post(url).json(&event).send().await;
        }

        if parsed.async_mode && !parsed.subscribe {
            let _ = client
                .send_message(req)
                .await
                .map_err(|e| FunctionCallError::Fatal(format!("A2A request failed: {}", e)))?;
            return Ok(ToolOutput::Function {
                body: FunctionCallOutputBody::Text("Background task started".into()),
                success: Some(true),
            });
        }

        if !parsed.subscribe {
            let response = client
                .send_message(req)
                .await
                .map_err(|e| FunctionCallError::Fatal(format!("A2A request failed: {}", e)))?;
            let final_text = match response {
                a2a_rs::types::SendMessageResponse::Message(message) => message
                    .parts
                    .into_iter()
                    .filter_map(|part| part.text)
                    .collect::<Vec<_>>()
                    .join(""),
                a2a_rs::types::SendMessageResponse::Task(task) => {
                    let mut text = String::new();
                    if let Some(message) = task.status.message {
                        for part in message.parts {
                            if let Some(chunk) = part.text {
                                text.push_str(&chunk);
                            }
                        }
                    }
                    for artifact in task.artifacts {
                        for part in artifact.parts {
                            if let Some(chunk) = part.text {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(&chunk);
                            }
                        }
                    }
                    text
                }
            };
            if let Some(url) = &events_webhook {
                let event = serde_json::json!({
                    "from": caller_name,
                    "to": self.name,
                    "status": "completed",
                    "taskId": task_id
                });
                let _ = http_client.post(url).json(&event).send().await;
            }
            return Ok(ToolOutput::Function {
                body: FunctionCallOutputBody::Text(final_text),
                success: Some(true),
            });
        }

        let mut rx = client.send_message_stream(req).await
            .map_err(|e| FunctionCallError::Fatal(format!("A2A request failed: {}", e)))?;

        let stream_webhook = stream_webhook.clone();
        let events_webhook = events_webhook.clone();
        let caller_name = caller_name.clone();
        let task_id = task_id.clone();
        let session_id = session.conversation_id.to_string();
        let agent_name = self.name.clone();
        let http_client = http_client.clone();
        let started_at = started_at;
        let is_async = parsed.async_mode;

        let process_task = async move {
            let mut final_text = String::new();
            let mut patch_seq = 0u64;

            // Send initial start event to /stream_patch
            if let Some(url) = &stream_webhook {
                let patch = serde_json::json!({
                    "agentId": agent_name,
                    "contextId": session_id,
                    "taskId": task_id,
                    "patchSeq": patch_seq,
                    "final": false,
                    "modelId": "unknown",
                    "content": [],
                    "text": "백그라운드 작업 시작됨 (async=true, subscribe=true)",
                    "thinking": "",
                    "toolCalls": [],
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                    "metrics": {
                        "elapsedMs": started_at.elapsed().as_millis() as i64
                    }
                });
                let _ = http_client.post(url).json(&patch).send().await;
                patch_seq += 1;
            }

            while let Some(res) = rx.recv().await {
                if let Ok(a2a_rs::StreamEvent::StatusUpdate(update)) = res {
                    if let Some(msg) = update.status.message {
                        for part in msg.parts {
                            if let Some(txt) = &part.text {
                                final_text.push_str(txt);
                                
                                if let Some(url) = &stream_webhook {
                                    let patch = serde_json::json!({
                                        "agentId": agent_name,
                                        "contextId": session_id,
                                        "taskId": &task_id,
                                        "patchSeq": patch_seq,
                                        "final": false,
                                        "modelId": "unknown",
                                        "content": [],
                                        "text": &final_text,
                                        "thinking": "",
                                        "toolCalls": [],
                                        "timestamp": chrono::Utc::now().timestamp_millis(),
                                        "metrics": {
                                            "elapsedMs": started_at.elapsed().as_millis() as i64
                                        }
                                    });
                                    let _ = http_client.post(url).json(&patch).send().await;
                                    patch_seq += 1;
                                }
                            }
                        }
                    }
                }
            }
            
            // Final completion event
            if let Some(url) = &events_webhook {
                let event = serde_json::json!({
                    "from": caller_name,
                    "to": agent_name,
                    "status": "completed",
                    "taskId": task_id
                });
                let _ = http_client.post(url).json(&event).send().await;
            }

            if let Some(url) = &stream_webhook {
                let patch = serde_json::json!({
                    "agentId": agent_name,
                    "contextId": session_id,
                    "taskId": task_id,
                    "patchSeq": patch_seq,
                    "final": true,
                    "modelId": "unknown",
                    "content": [],
                    "text": final_text.clone(),
                    "thinking": "",
                    "toolCalls": [],
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                    "metrics": {
                        "elapsedMs": started_at.elapsed().as_millis() as i64
                    }
                });
                let _ = http_client.post(url).json(&patch).send().await;
            }

            final_text
        };

        if is_async {
            tokio::spawn(process_task);
            
            let result = Ok(codex_protocol::mcp::CallToolResult {
                content: vec![serde_json::to_value(rmcp::model::Content::text("Background task started")).unwrap()],
                is_error: Some(false),
                structured_content: None,
                meta: None,
            });
            session.send_event(
                &turn,
                crate::protocol::EventMsg::McpToolCallEnd(
                    crate::protocol::McpToolCallEndEvent {
                        call_id: call_id.clone(),
                        invocation: mcp_invocation.clone(),
                        duration: std::time::Duration::from_millis(0),
                        result,
                    }
                )
            ).await;
            
            Ok(ToolOutput::Function {
                body: FunctionCallOutputBody::Text("Background task started".into()),
                success: Some(true),
            })
        } else {
            let final_text = process_task.await;
            
            let result = Ok(codex_protocol::mcp::CallToolResult {
                content: vec![serde_json::to_value(rmcp::model::Content::text(&final_text)).unwrap()],
                is_error: Some(false),
                structured_content: None,
                meta: None,
            });
            session.send_event(
                &turn,
                crate::protocol::EventMsg::McpToolCallEnd(
                    crate::protocol::McpToolCallEndEvent {
                        call_id: call_id.clone(),
                        invocation: mcp_invocation.clone(),
                        duration: started_at.elapsed(),
                        result,
                    }
                )
            ).await;

            Ok(ToolOutput::Function {
                body: FunctionCallOutputBody::Text(final_text),
                success: Some(true),
            })
        }
    }
}
