use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use a2a_acp_adapter::{AcpEventMapper, AcpMappedEvent, a2a_metadata_to_acp_tool_call};
use a2a_rs::proxy::SessionContext;
use a2a_rs::{A2aProxy, StreamEvent};
use agent_client_protocol_schema::{
    ContentBlock, Meta, PromptRequest, PromptResponse, StopReason, TextContent,
};
use sacp::{Client, Conductor, ConnectionTo, UntypedMessage};
use serde_json::json;
use tracing::{info, warn};
use uuid::Uuid;

use crate::SharedState;
use crate::agent_router::extract_mention;
use crate::context_proxy::{load_team_runtime_config, normalize_team_role, prompt_blocks_to_text};

use super::execution_mode::ExecutionMode;

pub enum TeamPromptPreparation {
    NotApplicable,
    Prepared,
    Cancelled(PromptResponse),
}

fn extract_text_from_a2a_parts(parts: &[a2a_rs::Part]) -> String {
    parts
        .iter()
        .filter_map(|part| part.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n")
}

fn wants_background_subscribe(request_text: &str) -> bool {
    let lower = request_text.to_ascii_lowercase();
    [
        "background",
        "subscribe",
        "system alert",
        "wake",
        "wake-up",
        "비동기",
        "백그라운드",
        "구독",
        "알림",
        "나중에",
        "기다린 뒤",
        "완료 알림",
        "long-running",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn map_tool_status(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "success" | "completed" | "complete" => "completed",
        "failed" | "error" => "failed",
        "validating" | "scheduled" | "executing" | "running" | "working" => "in_progress",
        _ => "pending",
    }
}

fn extract_tool_call_from_a2a_part_data(data: &serde_json::Value) -> Option<serde_json::Value> {
    let request = data.get("request")?.as_object()?;
    let tool = data.get("tool").and_then(|value| value.as_object());
    let tool_call_id = request.get("callId")?.as_str()?;
    let name = request
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("tool");
    let title = tool
        .and_then(|tool| tool.get("displayName").or_else(|| tool.get("name")))
        .and_then(|value| value.as_str())
        .unwrap_or(name);
    let kind = tool
        .and_then(|tool| tool.get("kind"))
        .and_then(|value| value.as_str())
        .unwrap_or("other");
    let status = data
        .get("status")
        .and_then(|value| value.as_str())
        .map(map_tool_status)
        .unwrap_or("pending");
    let raw_input = request
        .get("args")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let raw_output = data
        .get("response")
        .and_then(|value| value.get("resultDisplay").or_else(|| value.get("output")))
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    Some(json!({
        "toolCallId": tool_call_id,
        "name": name,
        "title": title,
        "kind": kind,
        "status": status,
        "rawInput": raw_input,
        "rawOutput": raw_output,
        "source": "a2a-direct",
    }))
}

fn upsert_tool_call(tool_calls: &mut Vec<serde_json::Value>, tool_call: serde_json::Value) {
    let tool_call_id = tool_call
        .get("toolCallId")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();

    if tool_call_id.is_empty() {
        tool_calls.push(tool_call);
        return;
    }

    if let Some(existing) = tool_calls.iter_mut().find(|existing| {
        existing.get("toolCallId").and_then(|value| value.as_str()) == Some(tool_call_id.as_str())
    }) {
        if let (Some(existing_obj), Some(new_obj)) =
            (existing.as_object_mut(), tool_call.as_object())
        {
            for (key, value) in new_obj {
                if !value.is_null() {
                    existing_obj.insert(key.clone(), value.clone());
                }
            }
        }
    } else {
        tool_calls.push(tool_call);
    }
}

fn nested_tool_call_role(tool_call: &serde_json::Value) -> Option<String> {
    let candidates = [
        tool_call.pointer("/rawInput/role"),
        tool_call.pointer("/rawOutput/role"),
        tool_call.get("title"),
        tool_call.get("name"),
    ];
    for candidate in candidates.into_iter().flatten() {
        if let Some(value) = candidate.as_str() {
            if let Some(role) = normalize_team_role(value) {
                return Some(role.to_string());
            }
            let lower = value.to_ascii_lowercase();
            for token in lower.split(|ch: char| !ch.is_alphanumeric()) {
                if let Some(role) = normalize_team_role(token) {
                    return Some(role.to_string());
                }
            }
        }
    }
    None
}

fn nested_tool_call_query(tool_call: &serde_json::Value) -> Option<String> {
    [
        tool_call.pointer("/rawInput/query"),
        tool_call.pointer("/rawInput/message"),
        tool_call.pointer("/rawInput/prompt"),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| value.as_str())
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty())
}

fn nested_tool_call_response(tool_call: &serde_json::Value) -> Option<String> {
    let raw_output = tool_call.get("rawOutput")?;
    if let Some(text) = raw_output
        .get("response")
        .or_else(|| raw_output.get("result"))
        .or_else(|| raw_output.get("text"))
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Some(text);
    }
    raw_output
        .as_str()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn extract_blockquote_response(text: &str) -> Option<String> {
    let collected = text
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if !trimmed.starts_with('>') {
                return None;
            }
            Some(
                trimmed
                    .trim_start_matches('>')
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            )
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if collected.is_empty() {
        None
    } else {
        Some(collected.join("\n"))
    }
}

fn extract_quoted_response(text: &str) -> Option<String> {
    let mut captures = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut quote_char = '\0';
    for ch in text.chars() {
        if !in_quote && (ch == '"' || ch == '\'') {
            in_quote = true;
            quote_char = ch;
            current.clear();
            continue;
        }
        if in_quote && ch == quote_char {
            let trimmed = current
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .trim()
                .to_string();
            if !trimmed.is_empty() {
                captures.push(trimmed);
            }
            in_quote = false;
            quote_char = '\0';
            current.clear();
            continue;
        }
        if in_quote {
            current.push(ch);
        }
    }
    captures.into_iter().rev().find(|value| !value.is_empty())
}

fn extract_role_prefixed_response(text: &str, target_role: &str) -> Option<String> {
    let lower_role = target_role.trim().to_ascii_lowercase();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if !lower.starts_with(&lower_role) {
            continue;
        }
        let value = trimmed
            .split_once(':')
            .map(|(_, rhs)| rhs.trim())
            .unwrap_or("")
            .trim_matches('"')
            .trim_matches('\'')
            .trim()
            .to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn persist_nested_delegate_sessions(
    state: &Arc<SharedState>,
    parent_session_id: &str,
    delegator_role: &str,
    cwd: &str,
    tool_calls: &[serde_json::Value],
    parent_response_text: &str,
) {
    for tool_call in tool_calls {
        let Some(target_role) = nested_tool_call_role(tool_call) else {
            continue;
        };
        if target_role.eq_ignore_ascii_case(delegator_role) {
            continue;
        }
        let Some(query) = nested_tool_call_query(tool_call) else {
            continue;
        };

        let child_session_id = match state.infra.brain.sessions().ensure_team_sub_session(
            parent_session_id,
            &target_role,
            &target_role,
            cwd,
        ) {
            Ok(child_session_id) => child_session_id,
            Err(error) => {
                warn!(
                    "[TeamMode] Nested delegate child session create failed: parent={} delegator={} target={} error={}",
                    parent_session_id, delegator_role, target_role, error
                );
                continue;
            }
        };
        let _ = state
            .infra
            .brain
            .session_update_team_agent_status(&child_session_id, "running");
        let _ = state.infra.brain.sessions().add_message(
            &child_session_id,
            "user",
            &query,
            delegator_role,
        );

        let response = nested_tool_call_response(tool_call)
            .or_else(|| extract_blockquote_response(parent_response_text))
            .or_else(|| extract_quoted_response(parent_response_text))
            .or_else(|| extract_role_prefixed_response(parent_response_text, &target_role))
            .or_else(|| {
                let trimmed = parent_response_text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            });

        info!(
            "[TeamMode] Nested delegate materialize: parent={} delegator={} target={} query={} has_response={}",
            parent_session_id,
            delegator_role,
            target_role,
            query,
            response
                .as_ref()
                .map(|value| !value.is_empty())
                .unwrap_or(false)
        );

        if let Some(response) = response {
            let _ = state.infra.brain.session_add_message(
                &child_session_id,
                "assistant",
                &response,
                &target_role,
                delegator_role,
                "",
                0,
                0,
                0,
                0,
            );
            info!(
                "[TeamMode] Nested delegate assistant persisted: session={} role={} content={}",
                child_session_id, target_role, response
            );
            let _ = state
                .infra
                .brain
                .session_update_team_agent_status(&child_session_id, "done");
        }
    }
}

pub fn build_team_tools_only_instruction(
    team_cfg: &crate::context_proxy::team_a2a::TeamRuntimeConfig,
) -> String {
    let role_list = team_cfg
        .agents
        .iter()
        .map(|agent| agent.role.to_lowercase())
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "\n<system_directive priority=\"critical\">\n\
         TEAM ORCHESTRATION MODE IS ACTIVE.\n\
         This turn must use the MCP tool surface named `team-tools` for any multi-agent work.\n\
         Valid delegation tools are ONLY: `delegate`, `delegate_background`, and `propose`.\n\
         In Gemini tool-call UI, these may appear with fully-qualified MCP names such as `mcp_ilhae-tools_delegate` and `mcp_ilhae-tools_team_list`.\n\
         Available team roles for this session: {}.\n\
         The team roster is already configured and ready right now.\n\
         For normal task execution, call `delegate` immediately with one of those roles. If the tool picker shows fully-qualified names, choose `mcp_ilhae-tools_delegate`.\n\
         If the user explicitly asks for background, async, subscribe, alert, wake-up-on-completion, or long-running delegation, you MUST choose `delegate_background` (or the equivalent subscribe-capable delegation tool) instead of `delegate`.\n\
         In those background/subscribe cases, do not wait synchronously for the delegated result in the same turn. Start the background task, then rely on the later system alert/update to continue.\n\
         If the user explicitly says 'Researcher를 사용해 ...', 'Verifier로 검증해 ...', or 'Creator로 작성해 ...', translate that directly into `mcp_ilhae-tools_delegate` calls with `role` set to `Researcher`, `Verifier`, or `Creator`.\n\
         Example background action: call `delegate_background` with `role=\"Researcher\"` and a research query. If the user wants the completed result in this same conversation, your very next action should be `subscribe_task(task_id)` using the returned task_id. If the tool picker shows a fully-qualified name, choose the one whose base name is `delegate_background`; do not invent a qualified name.\n\
         Example first action for this kind of request: call `mcp_ilhae-tools_delegate` with `role=\"Researcher\"` and a focused research query.\n\
         DO NOT call `team_list`, `team_add`, `team_update`, `team_remove`, or `team_set_prompt` unless the user explicitly asks to inspect or modify team membership/configuration.\n\
         If you need help from Researcher, Verifier, Creator, Leader, or any team role above, you MUST call one of those `team-tools` tools.\n\
         DO NOT use `activate_skill` for team roles.\n\
         DO NOT use `generalist`, `cli_help`, or `codebase_investigator` as substitutes for team delegation.\n\
         DO NOT use shell or file-system tools just to rediscover team roles or endpoints; they are already available through `team-tools`.\n\
         Writing \"I'll delegate\" in plain text does not count; a real `team-tools` tool call is required.\n\
         Only answer directly without delegation if the request is trivial and clearly does not need another role.\n\
         </system_directive>\n",
        role_list
    )
}

pub async fn try_handle_direct_target_route(
    req_meta: Option<&Meta>,
    user_text: &str,
    session_id: &str,
    current_agent_id: &str,
    state: &Arc<SharedState>,
    cx: &ConnectionTo<Conductor>,
    ilhae_dir: &PathBuf,
) -> Result<Option<PromptResponse>, sacp::Error> {
    let Some(team_cfg) = load_team_runtime_config(ilhae_dir) else {
        return Ok(None);
    };

    let trimmed = user_text.trim().to_string();
    let direct_target = if trimmed.starts_with('@') {
        extract_mention(&trimmed, &team_cfg)
    } else if let Some(meta) = req_meta {
        if let Some(agent_id) = meta.get("agentId").and_then(|value| value.as_str()) {
            let requested = agent_id.trim().to_ascii_lowercase();
            let is_non_main_team_agent = team_cfg
                .agents
                .iter()
                .any(|agent| !agent.is_main && agent.role.eq_ignore_ascii_case(&requested));
            if is_non_main_team_agent
                && !requested.eq_ignore_ascii_case(current_agent_id)
                && requested != "team"
            {
                Some((requested, user_text.to_string()))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let Some((target_role, clean_prompt)) = direct_target else {
        return Ok(None);
    };

    let Some(target_agent) = team_cfg
        .agents
        .iter()
        .find(|agent| agent.role.eq_ignore_ascii_case(&target_role))
        .cloned()
    else {
        return Ok(None);
    };

    info!(
        "[TeamMode] Direct target via A2A: session={}, target={}",
        session_id, target_role
    );

    let session_cwd = state
        .infra
        .brain
        .sessions()
        .get_session(session_id)
        .ok()
        .and_then(|session| session.map(|value| value.cwd))
        .filter(|cwd| !cwd.trim().is_empty())
        .unwrap_or_else(|| "/".to_string());
    let child_session_id = match state.infra.brain.sessions().ensure_team_sub_session(
        session_id,
        &target_role,
        &target_role,
        &session_cwd,
    ) {
        Ok(child_session_id) => {
            info!(
                "[TeamMode] Direct target child session prepared: parent={} child={} role={}",
                session_id, child_session_id, target_role
            );
            child_session_id
        }
        Err(error) => {
            warn!(
                "[TeamMode] Direct target child session create failed: parent={} role={} error={}",
                session_id, target_role, error
            );
            session_id.to_string()
        }
    };
    let _ = state
        .infra
        .brain
        .session_update_team_agent_status(&child_session_id, "running");

    let session_ctx = SessionContext::new()
        .with_cwd(session_cwd.clone())
        .with_admin_skills(true)
        .with_disabled_skills(vec![])
        .with_extra_skills_dirs(vec![
            state
                .infra
                .brain
                .vault_dir()
                .join("skills")
                .to_string_lossy()
                .to_string(),
        ]);

    let proxy = A2aProxy::with_context(&target_agent.endpoint, &target_agent.role, session_ctx);

    let (host, port) = crate::parse_host_port(&target_agent.endpoint);
    let mut target_ready = crate::probe_tcp(&host, port);
    if !target_ready {
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if crate::probe_tcp(&host, port) {
                target_ready = true;
                break;
            }
        }
    }
    if !target_ready {
        warn!(
            "[TeamMode] Direct target {} not ready on {}:{}, fallback to leader path",
            target_role, host, port
        );
        return Ok(None);
    }

    let clean_prompt = if clean_prompt.trim().is_empty() {
        user_text.to_string()
    } else {
        clean_prompt
    };
    let mut actual_prompt = clean_prompt.clone();

    let mentions_other_role = team_cfg.agents.iter().any(|agent| {
        let normalized = normalize_team_role(&agent.role).unwrap_or(agent.role.as_str());
        if normalized.eq_ignore_ascii_case(&target_role) {
            return false;
        }
        let role_lower = normalized.to_ascii_lowercase();
        clean_prompt.to_ascii_lowercase().contains(&role_lower)
    });

    if mentions_other_role {
        actual_prompt = format!(
            "{}\n<system_directive priority=\"critical\">\n\
             You are handling a direct-target subagent turn for role {}.\n\
             Other teammate roles must be reached through the MCP team orchestration surface.\n\
             If this request mentions another team role, you MUST call `mcp_ilhae-tools_delegate` (or `delegate` if the tool picker only shows base names) with `role` set to that teammate before answering.\n\
             Do NOT attempt to call direct peer agent tools such as `verifier`, `creator`, or `leader` from this direct-target subagent turn; Gemini subagents do not expose other agents as callable tools here.\n\
             Do not answer from your own knowledge when the user explicitly asked for another team role to participate.\n\
             Do not use team_list unless the user asked to inspect the team.\n\
             </system_directive>\n\n{}",
            build_team_tools_only_instruction(&team_cfg),
            target_role,
            clean_prompt
        );

        match crate::process_supervisor::force_restart_team_role(
            &state.team.supervisor,
            &target_role,
        )
        .await
        {
            Ok(true) => {
                info!(
                    "[TeamMode] Direct target {} force-restarted before nested delegation turn",
                    target_role
                );
                if let Err(error) = crate::context_proxy::team_a2a::wait_for_a2a_health(
                    &target_agent.endpoint,
                    Duration::from_secs(30),
                )
                .await
                {
                    warn!(
                        "[TeamMode] Direct target {} not healthy after force-restart: {}",
                        target_role, error
                    );
                    return Ok(None);
                }
            }
            Ok(false) => {
                warn!(
                    "[TeamMode] Direct target {} not managed by supervisor for force-restart",
                    target_role
                );
            }
            Err(error) => {
                warn!(
                    "[TeamMode] Direct target {} force-restart failed: {}",
                    target_role, error
                );
            }
        }
    }

    match proxy
        .send_and_observe(&actual_prompt, Some(session_id.to_string()), None)
        .await
    {
        Ok((result_text, events)) => {
            let mut mapper = AcpEventMapper::new(&[
                "leader",
                "researcher",
                "verifier",
                "creator",
                "creator_1",
                "creator_2",
            ]);
            let mut mapped_text = result_text;
            let mut extra_tool_calls: Vec<serde_json::Value> = Vec::new();

            for event in &events {
                match event {
                    StreamEvent::StatusUpdate(status) => {
                        match mapper.map_status_update(status) {
                            AcpMappedEvent::TextUpdate(text) => {
                                if !text.trim().is_empty() {
                                    mapped_text = text;
                                }
                            }
                            AcpMappedEvent::ToolCallUpdate { tool_call, .. } => {
                                upsert_tool_call(&mut extra_tool_calls, tool_call);
                            }
                            AcpMappedEvent::DelegationEvent { fields } => {
                                upsert_tool_call(&mut extra_tool_calls, fields.to_acp_tool_call());
                            }
                            AcpMappedEvent::None => {}
                        }

                        if let Some(message) = &status.status.message {
                            for part in &message.parts {
                                if let Some(data) = &part.data
                                    && let Some(tool_call) = a2a_metadata_to_acp_tool_call(data)
                                        .or_else(|| extract_tool_call_from_a2a_part_data(data))
                                {
                                    upsert_tool_call(&mut extra_tool_calls, tool_call);
                                }
                            }
                        }
                    }
                    StreamEvent::ArtifactUpdate(artifact) => {
                        if let Some(text) = mapper.map_artifact_update(artifact)
                            && !text.trim().is_empty()
                        {
                            mapped_text = text;
                        }
                    }
                    StreamEvent::Task(task) => {
                        if let Some(message) = &task.status.message {
                            let text = extract_text_from_a2a_parts(&message.parts);
                            if !text.trim().is_empty() {
                                mapped_text = text;
                            }
                        }
                    }
                    StreamEvent::Message(message) => {
                        let text = extract_text_from_a2a_parts(&message.parts);
                        if !text.trim().is_empty() {
                            mapped_text = text;
                        }
                    }
                }
            }

            let mut tool_calls = mapper.tool_calls.clone();
            tool_calls.extend(extra_tool_calls);
            let tool_calls_json = if tool_calls.is_empty() {
                String::new()
            } else {
                serde_json::to_string(&tool_calls).unwrap_or_default()
            };

            let _ = state.infra.brain.sessions().add_message(
                &child_session_id,
                "user",
                &actual_prompt,
                &target_role,
            );

            let turn_id = format!("team-preflight-{}", Uuid::new_v4());
            if let Ok(notif) = UntypedMessage::new(
                crate::types::NOTIF_APP_SESSION_EVENT,
                crate::types::IlhaeAppSessionEventNotification {
                    engine: target_role.to_string(),
                    event: crate::types::IlhaeAppSessionEventDto::MessageDelta {
                        thread_id: session_id.to_string(),
                        turn_id: turn_id.clone(),
                        item_id: format!("{turn_id}:{target_role}"),
                        channel: "assistant".to_string(),
                        delta: mapped_text.clone(),
                    },
                },
            ) {
                let _ = cx.send_notification_to(Client, notif);
            }
            if let Ok(notif) = UntypedMessage::new(
                crate::types::NOTIF_APP_SESSION_EVENT,
                crate::types::IlhaeAppSessionEventNotification {
                    engine: target_role.to_string(),
                    event: crate::types::IlhaeAppSessionEventDto::TurnCompleted {
                        thread_id: session_id.to_string(),
                        turn_id,
                        status: "completed".to_string(),
                    },
                },
            ) {
                let _ = cx.send_notification_to(Client, notif);
            }

            let _ = state.infra.brain.session_add_message(
                &child_session_id,
                "assistant",
                &mapped_text,
                &target_role,
                current_agent_id,
                &tool_calls_json,
                0,
                0,
                0,
                0,
            );

            persist_nested_delegate_sessions(
                state,
                &child_session_id,
                &target_role,
                &session_cwd,
                &tool_calls,
                &mapped_text,
            );

            let _ = state.infra.brain.session_add_message(
                session_id,
                "assistant",
                &mapped_text,
                &target_role,
                current_agent_id,
                "",
                0,
                0,
                0,
                0,
            );
            let _ = state
                .infra
                .brain
                .session_update_team_agent_status(&child_session_id, "done");

            let response_meta = json!({
                "direct_agent": target_role,
                "transport": "a2a",
            })
            .as_object()
            .cloned()
            .unwrap_or_default();
            Ok(Some(
                PromptResponse::new(StopReason::EndTurn).meta(response_meta),
            ))
        }
        Err(error) => {
            warn!(
                "[TeamMode] Direct A2A route failed for {}: {} — fallback to leader path",
                target_role, error
            );
            Ok(None)
        }
    }
}

pub async fn prepare_team_prompt(
    mode: ExecutionMode,
    req: &mut PromptRequest,
    session_id: &str,
    current_agent_id: &str,
    user_text: &str,
    prompt_start_cancel_ver: u64,
    state: &Arc<SharedState>,
    ilhae_dir: &PathBuf,
) -> Result<TeamPromptPreparation, sacp::Error> {
    if !mode.is_team() {
        return Ok(TeamPromptPreparation::NotApplicable);
    }
    if mode.is_mock() {
        info!(
            "[TeamMode] mock_mode=true, skipping live A2A orchestration (session={})",
            session_id
        );
        return Ok(TeamPromptPreparation::NotApplicable);
    }

    let Some(mut team_cfg) = load_team_runtime_config(ilhae_dir) else {
        warn!(
            "[TeamMode] enabled but team.json missing/invalid (session={}) - fallback to single-agent path",
            session_id
        );
        return Ok(TeamPromptPreparation::NotApplicable);
    };

    if let Ok(Some(session)) = state.infra.brain.session_get_raw(session_id) {
        let override_obj: serde_json::Value =
            serde_json::from_str(&session.capabilities_override).unwrap_or(json!({}));
        if let Some(engines) = override_obj.get("engines").and_then(|v| v.as_object()) {
            for agent in &mut team_cfg.agents {
                let role_key = normalize_team_role(&agent.role)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| agent.role.clone());
                if let Some(e) = engines.get(role_key.as_str()).and_then(|v| v.as_str()) {
                    agent.engine = e.to_string();
                }
            }
        }
    }

    let latest_cancel_ver = {
        let map = &state.sessions.cancel_ver;
        map.get(session_id).unwrap_or(0)
    };
    if latest_cancel_ver > prompt_start_cancel_ver {
        return Ok(TeamPromptPreparation::Cancelled(PromptResponse::new(
            StopReason::Cancelled,
        )));
    }

    let request_text = if !user_text.trim().is_empty() {
        user_text.to_string()
    } else {
        prompt_blocks_to_text(&req.prompt)
    };
    {
        let delegation_mode = &state.sessions.delegation_mode;
        if wants_background_subscribe(&request_text) {
            delegation_mode.insert(session_id.to_string(), "background".to_string());
        } else {
            delegation_mode.remove(session_id);
        }
    }

    info!(
        "[TeamMode] Preparing team prompt (session={}, autonomous={}, agents={})",
        session_id,
        mode.is_autonomous(),
        team_cfg.agents.len()
    );

    let parent_cwd = state
        .infra
        .brain
        .sessions()
        .get_session(session_id)
        .ok()
        .and_then(|s| s.map(|v| v.cwd))
        .filter(|cwd| !cwd.trim().is_empty())
        .unwrap_or_else(|| "/".to_string());
    let _ = state
        .infra
        .brain
        .sessions()
        .mark_multi_agent_parent(session_id, team_cfg.agents.len() as i64);
    for agent in &team_cfg.agents {
        let role = &agent.role;
        if let Ok(child_id) = state.infra.brain.sessions().ensure_team_sub_session(
            session_id,
            role,
            current_agent_id,
            &parent_cwd,
        ) {
            let _ = state
                .infra
                .brain
                .session_update_team_agent_status(&child_id, "running");
        }
    }

    state.team.agent_pool.init_from_team_config(&team_cfg).await;
    info!(
        "[TeamMode] Setup complete (session={}, agents={}, request={}chars). Falling through to unified conductor chain.",
        session_id,
        team_cfg.agents.len(),
        request_text.len()
    );
    req.prompt.insert(
        0,
        ContentBlock::Text(TextContent::new(build_team_tools_only_instruction(
            &team_cfg,
        ))),
    );
    if wants_background_subscribe(&request_text) {
        req.prompt.insert(
            0,
            ContentBlock::Text(TextContent::new(
                "\n<system_directive priority=\"critical\">\n\
                 THIS TURN EXPLICITLY REQUIRES BACKGROUND/SUBSCRIBE EXECUTION.\n\
                 For this turn, synchronous `delegate` is forbidden unless background delegation fails completely.\n\
                 Your first orchestration action must be `mcp_ilhae-tools_delegate_background` (or the background/subscribe equivalent) targeting the appropriate team role.\n\
                 After starting the background task, do not wait synchronously for the delegated result in the same turn. Expect a later `[System Alert]` and continue from that alert.\n\
                 </system_directive>\n"
                    .to_string(),
            )),
        );
    }

    Ok(TeamPromptPreparation::Prepared)
}
