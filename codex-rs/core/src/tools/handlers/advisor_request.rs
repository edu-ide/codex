use crate::compact::content_items_to_text;
use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use codex_protocol::items::LoopLifecycleItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::LoopLifecycleKind;
use codex_protocol::protocol::LoopLifecycleProgressEvent;
use codex_protocol::protocol::LoopLifecycleStatus;
use serde::Deserialize;
use std::time::Instant;

const SYSTEM2_ENABLED_ENV: &str = "ILHAE_SYSTEM2_ENABLED";
const SYSTEM2_PROFILE_ENV: &str = "ILHAE_SYSTEM2_PROFILE";
const SYSTEM2_BASE_URL_ENV: &str = "ILHAE_SYSTEM2_BASE_URL";
const SYSTEM2_MODEL_ENV: &str = "ILHAE_SYSTEM2_MODEL";

pub struct AdvisorRequestHandler;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvisorTargetConfig {
    pub profile: String,
    pub base_url: String,
    pub model: String,
}

#[derive(Deserialize)]
struct AdvisorRequestArgs {
    question: String,
    context: Option<String>,
}

pub fn advisor_target_from_env() -> Option<AdvisorTargetConfig> {
    let enabled = std::env::var(SYSTEM2_ENABLED_ENV).ok()?;
    if enabled.trim() != "1" {
        return None;
    }

    let profile = std::env::var(SYSTEM2_PROFILE_ENV).ok()?.trim().to_string();
    let base_url = std::env::var(SYSTEM2_BASE_URL_ENV).ok()?.trim().to_string();
    let model = std::env::var(SYSTEM2_MODEL_ENV).ok()?.trim().to_string();
    if profile.is_empty() || base_url.is_empty() || model.is_empty() {
        return None;
    }

    Some(AdvisorTargetConfig {
        profile,
        base_url,
        model,
    })
}

fn advisor_chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

fn summarize_recent_history(items: &[ResponseItem], max_entries: usize) -> String {
    let mut entries = Vec::new();

    for item in items.iter().rev() {
        let entry = match item {
            ResponseItem::Message { role, content, .. } => {
                let text = content_items_to_text(content)
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty());
                text.map(|text| format!("{}: {}", role.to_ascii_lowercase(), text))
            }
            ResponseItem::FunctionCallOutput { output, .. } => output
                .body
                .to_text()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .map(|text| format!("tool_output: {}", text)),
            _ => None,
        };

        let Some(entry) = entry else {
            continue;
        };
        entries.push(entry);
        if entries.len() >= max_entries {
            break;
        }
    }

    entries.reverse();
    entries.join("\n")
}

fn build_advisor_messages(
    transcript_summary: &str,
    args: &AdvisorRequestArgs,
) -> serde_json::Value {
    let mut user_context = String::new();
    if !transcript_summary.trim().is_empty() {
        user_context.push_str("Recent conversation context:\n");
        user_context.push_str(transcript_summary.trim());
        user_context.push_str("\n\n");
    }
    if let Some(extra) = args
        .context
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        user_context.push_str("Executor context:\n");
        user_context.push_str(extra);
        user_context.push_str("\n\n");
    }
    user_context.push_str("Advisor question:\n");
    user_context.push_str(args.question.trim());

    serde_json::json!([
        {
            "role": "system",
            "content": "You are a high-reasoning advisor for a coding executor. Give concise, practical planning advice only. Focus on strategy, trade-offs, failure modes, and next actions. Do not address the user directly. Do not claim to have executed anything."
        },
        {
            "role": "user",
            "content": user_context
        }
    ])
}

fn advisor_loop_item_id(call_id: &str) -> String {
    format!("advisor:{call_id}")
}

#[allow(clippy::too_many_arguments)]
fn build_advisor_loop_item(
    item_id: String,
    target_profile: Option<String>,
    summary: String,
    detail: Option<String>,
    status: LoopLifecycleStatus,
    error: Option<String>,
    duration_ms: Option<i64>,
    reason: Option<String>,
) -> LoopLifecycleItem {
    LoopLifecycleItem {
        id: item_id,
        kind: LoopLifecycleKind::Advisor,
        title: "Escalating to Advisor".to_string(),
        summary,
        detail,
        status,
        reason,
        counts: None,
        error,
        duration_ms,
        target_profile,
    }
}

async fn call_advisor_model(
    target: &AdvisorTargetConfig,
    messages: serde_json::Value,
) -> Result<String, FunctionCallError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;

    let response = client
        .post(advisor_chat_completions_url(&target.base_url))
        .json(&serde_json::json!({
            "model": target.model,
            "messages": messages,
            "stream": false,
            "temperature": 0.1,
            "max_tokens": 800,
        }))
        .send()
        .await
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;

    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;

    if !status.is_success() {
        return Err(FunctionCallError::RespondToModel(body.to_string()));
    }

    body.pointer("/choices/0/message/content")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            body.pointer("/choices/0/message/reasoning_content")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .ok_or_else(|| FunctionCallError::RespondToModel(body.to_string()))
}

impl ToolHandler for AdvisorRequestHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;
        let item_id = advisor_loop_item_id(&call_id);

        let target = match advisor_target_from_env() {
            Some(target) => target,
            None => {
                session
                    .send_event(
                        turn.as_ref(),
                        build_advisor_loop_item(
                            item_id,
                            None,
                            "Advisor unavailable".to_string(),
                            None,
                            LoopLifecycleStatus::Failed,
                            Some(
                                "system2 advisor is not configured for the active profile"
                                    .to_string(),
                            ),
                            None,
                            Some("configuration".to_string()),
                        )
                        .as_failed_event(),
                    )
                    .await;
                return Err(FunctionCallError::RespondToModel(
                    "system2 advisor is not configured for the active profile".to_string(),
                ));
            }
        };

        let args: AdvisorRequestArgs = match payload {
            ToolPayload::Function { arguments } => parse_arguments(&arguments)?,
            _ => {
                session
                    .send_event(
                        turn.as_ref(),
                        build_advisor_loop_item(
                            item_id,
                            Some(target.profile.clone()),
                            "Advisor request rejected".to_string(),
                            None,
                            LoopLifecycleStatus::Failed,
                            Some("advisor_request received unsupported payload".to_string()),
                            None,
                            Some("invalid_payload".to_string()),
                        )
                        .as_failed_event(),
                    )
                    .await;
                return Err(FunctionCallError::RespondToModel(
                    "advisor_request received unsupported payload".to_string(),
                ));
            }
        };
        if args.question.trim().is_empty() {
            session
                .send_event(
                    turn.as_ref(),
                    build_advisor_loop_item(
                        item_id,
                        Some(target.profile.clone()),
                        "Advisor request rejected".to_string(),
                        None,
                        LoopLifecycleStatus::Failed,
                        Some("advisor_request.question cannot be empty".to_string()),
                        None,
                        Some("validation".to_string()),
                    )
                    .as_failed_event(),
                )
                .await;
            return Err(FunctionCallError::RespondToModel(
                "advisor_request.question cannot be empty".to_string(),
            ));
        }

        session
            .send_event(
                turn.as_ref(),
                build_advisor_loop_item(
                    item_id.clone(),
                    Some(target.profile.clone()),
                    format!("Consulting {}", target.profile),
                    Some(args.question.trim().to_string()),
                    LoopLifecycleStatus::InProgress,
                    None,
                    None,
                    Some("deep_reasoning".to_string()),
                )
                .as_started_event(),
            )
            .await;

        let started_at = Instant::now();
        let history = session.clone_history().await;
        let transcript_summary = summarize_recent_history(history.raw_items(), 6);
        let messages = build_advisor_messages(&transcript_summary, &args);
        session
            .send_event(
                turn.as_ref(),
                EventMsg::LoopLifecycleProgress(LoopLifecycleProgressEvent {
                    item_id: item_id.clone(),
                    kind: LoopLifecycleKind::Advisor,
                    summary: "Waiting for advisor response".to_string(),
                    detail: Some(format!("target={}", target.profile)),
                    counts: None,
                }),
            )
            .await;
        let advice = match call_advisor_model(&target, messages).await {
            Ok(advice) => advice,
            Err(err) => {
                session
                    .send_event(
                        turn.as_ref(),
                        build_advisor_loop_item(
                            item_id,
                            Some(target.profile.clone()),
                            "Advisor request failed".to_string(),
                            Some(args.question.trim().to_string()),
                            LoopLifecycleStatus::Failed,
                            Some(err.to_string()),
                            Some(started_at.elapsed().as_millis() as i64),
                            Some("upstream_error".to_string()),
                        )
                        .as_failed_event(),
                    )
                    .await;
                return Err(err);
            }
        };
        session
            .send_event(
                turn.as_ref(),
                build_advisor_loop_item(
                    item_id,
                    Some(target.profile.clone()),
                    format!("Advisor guidance received from {}", target.profile),
                    Some(args.question.trim().to_string()),
                    LoopLifecycleStatus::Completed,
                    None,
                    Some(started_at.elapsed().as_millis() as i64),
                    Some("deep_reasoning".to_string()),
                )
                .as_completed_event(),
            )
            .await;

        Ok(FunctionToolOutput::from_text(
            format!("[Advisor:{}]\n{}", target.profile, advice.trim()),
            Some(true),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::FunctionCallOutputPayload;

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn advisor_target_from_env_requires_complete_config() {
        let _enabled = EnvGuard::set(SYSTEM2_ENABLED_ENV, "1");
        let _profile = EnvGuard::set(SYSTEM2_PROFILE_ENV, "minimax");
        unsafe {
            std::env::remove_var(SYSTEM2_BASE_URL_ENV);
            std::env::remove_var(SYSTEM2_MODEL_ENV);
        }

        assert!(advisor_target_from_env().is_none());
    }

    #[test]
    fn summarize_recent_history_keeps_recent_messages_and_tool_outputs() {
        let items = vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "첫 질문".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
            ResponseItem::FunctionCallOutput {
                call_id: "tool-1".to_string(),
                output: FunctionCallOutputPayload::from_text("계획 초안".to_string()),
            },
            ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "다음 단계로 갑니다".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
        ];

        let summary = summarize_recent_history(&items, 3);
        assert!(summary.contains("user: 첫 질문"));
        assert!(summary.contains("tool_output: 계획 초안"));
        assert!(summary.contains("assistant: 다음 단계로 갑니다"));
    }
}
