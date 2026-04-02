use crate::context_proxy::{TeamRoleTarget, load_team_runtime_config};
use crate::team_timeline::{
    agent_response_event, delegation_completed_event, delegation_started_event, persist_events,
    task_status_event, task_submitted_event,
};
use a2a_rs::proxy::A2aProxy;
use sacp::mcp_server::McpConnectionTo;
use sacp::{Client, Conductor, ConnectionTo, UntypedMessage};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};
use uuid::Uuid;

pub const A2A_MAX_RETRY: u32 = 3;
pub const A2A_BASE_DELAY_MS: u64 = 2000;
pub const A2A_MAX_DELAY_MS: u64 = 30000;

pub fn a2a_retry_delay(attempt: u32) -> std::time::Duration {
    let ms = (A2A_BASE_DELAY_MS * 2u64.pow(attempt.saturating_sub(1))).min(A2A_MAX_DELAY_MS);
    std::time::Duration::from_millis(ms)
}

pub fn current_team_merge_policy(state: &Arc<crate::SharedState>) -> String {
    let policy = state.infra.settings_store.get().agent.team_merge_policy;
    let trimmed = policy.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        "append_all".to_string()
    } else {
        trimmed
    }
}

pub fn current_team_max_retries(state: &Arc<crate::SharedState>) -> u32 {
    state.infra.settings_store.get().agent.team_max_retries.max(1)
}

pub fn current_team_pause_on_error(state: &Arc<crate::SharedState>) -> bool {
    state.infra.settings_store.get().agent.team_pause_on_error
}

pub fn should_emit_team_response(state: &Arc<crate::SharedState>, role: &str) -> bool {
    match current_team_merge_policy(state).as_str() {
        "leader_only" => load_team_runtime_config(&state.infra.ilhae_dir)
            .and_then(|cfg| {
                cfg.agents
                    .into_iter()
                    .find(|agent| agent.role.eq_ignore_ascii_case(role))
            })
            .map(|agent| agent.is_main)
            .unwrap_or_else(|| role.eq_ignore_ascii_case("leader")),
        _ => true,
    }
}

pub fn current_main_team_role(state: &Arc<crate::SharedState>) -> String {
    load_team_runtime_config(&state.infra.ilhae_dir)
        .and_then(|cfg| cfg.agents.into_iter().find(|agent| agent.is_main))
        .map(|agent| agent.role.to_ascii_lowercase())
        .unwrap_or_else(|| "leader".to_string())
}

fn find_team_role_target(
    state: &Arc<crate::SharedState>,
    role: &str,
) -> Option<TeamRoleTarget> {
    load_team_runtime_config(&state.infra.ilhae_dir).and_then(|cfg| {
        cfg.agents
            .into_iter()
            .find(|agent| agent.role.eq_ignore_ascii_case(role))
    })
}

fn team_role_profile_label(target: &TeamRoleTarget) -> String {
    let mut parts = Vec::new();
    if !target.skills.is_empty() {
        parts.push(format!(
            "skills:{}",
            target.skills.iter().take(3).cloned().collect::<Vec<_>>().join("+")
        ));
    }
    if !target.mcp_servers.is_empty() {
        parts.push(format!(
            "mcp:{}",
            target
                .mcp_servers
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join("+")
        ));
    }
    if !target.engine.trim().is_empty() {
        parts.push(format!("engine:{}", target.engine.trim()));
    }
    if parts.is_empty() {
        target.role.clone()
    } else {
        format!("{} ({})", target.role, parts.join(", "))
    }
}

fn team_query_tokens(query: &str) -> Vec<String> {
    query
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-')
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() >= 3)
        .collect()
}

fn specialization_score(target: &TeamRoleTarget, query: &str) -> i32 {
    let lower_query = query.to_ascii_lowercase();
    let tokens = team_query_tokens(query);
    let mut score = 0i32;

    let role = target.role.to_ascii_lowercase();
    if lower_query.contains(&role) {
        score += 8;
    }
    if lower_query.contains(&target.engine.to_ascii_lowercase()) {
        score += 3;
    }

    for skill in &target.skills {
        let skill_lower = skill.to_ascii_lowercase();
        if lower_query.contains(&skill_lower) {
            score += 6;
        }
        for token in &tokens {
            if skill_lower.contains(token) || token.contains(&skill_lower) {
                score += 2;
            }
        }
    }

    for mcp in &target.mcp_servers {
        let mcp_lower = mcp.to_ascii_lowercase();
        if lower_query.contains(&mcp_lower) {
            score += 4;
        }
    }

    let prompt_lower = target.system_prompt.to_ascii_lowercase();
    for token in &tokens {
        if prompt_lower.contains(token) {
            score += 1;
        }
    }

    score
}

fn summarize_worker_response_for_leader(
    state: &Arc<crate::SharedState>,
    role: &str,
    text: &str,
) -> String {
    let normalized = text.trim().replace("\r\n", "\n");
    let mut bullets = normalized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .map(|line| {
            if line.starts_with("- ") || line.starts_with("* ") {
                line.to_string()
            } else {
                format!("- {line}")
            }
        })
        .collect::<Vec<_>>();

    if bullets.is_empty() {
        let compact = normalized.chars().take(280).collect::<String>();
        bullets.push(format!("- {}", compact.trim()));
    }

    let profile = find_team_role_target(state, role)
        .map(|target| team_role_profile_label(&target))
        .unwrap_or_else(|| role.to_string());

    format!(
        "Leader arbitration summary from {profile}:\n{}\n- Full worker output is preserved in the team timeline and audit log.",
        bullets.join("\n")
    )
}

fn client_facing_team_response(
    state: &Arc<crate::SharedState>,
    role: &str,
    text: &str,
) -> Option<(String, String)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    match current_team_merge_policy(state).as_str() {
        "leader_only" if !should_emit_team_response(state, role) => Some((
            current_main_team_role(state),
            summarize_worker_response_for_leader(state, role, trimmed),
        )),
        _ => Some((role.to_string(), trimmed.to_string())),
    }
}

pub(crate) async fn is_team_role_healthy(state: &crate::SharedState, role_lower: &str) -> bool {
    let key = format!("team-{}", role_lower.to_ascii_lowercase());
    let sv = state.team_state().supervisor.read().await;
    sv.processes
        .get(&key)
        .map(|proc| {
            proc.enabled
                && proc
                    .last_healthy
                    .map(|ts| ts.elapsed() <= std::time::Duration::from_secs(90))
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}

pub(crate) async fn select_fallback_targets(
    state: &crate::SharedState,
    failed_role: &str,
    query: &str,
) -> Vec<TeamRoleTarget> {
    let Some(team_cfg) = load_team_runtime_config(&state.infra.ilhae_dir) else {
        return Vec::new();
    };
    let failed_role = failed_role.to_ascii_lowercase();
    let mut healthy_non_leaders = Vec::new();
    let mut degraded_non_leaders = Vec::new();
    let mut healthy_leader = None;
    let mut degraded_leader = None;

    for agent in team_cfg.agents {
        let role_lower = agent.role.to_ascii_lowercase();
        if role_lower == failed_role {
            continue;
        }
        if !crate::process_supervisor::is_delegation_allowed(
            &state.team_state().delegation_metrics,
            &role_lower,
        )
        .await
        {
            continue;
        }
        let healthy = is_team_role_healthy(state, &role_lower).await;
        if agent.is_main {
            if healthy {
                healthy_leader = Some(agent);
            } else {
                degraded_leader = Some(agent);
            }
        } else if healthy {
            healthy_non_leaders.push(agent);
        } else {
            degraded_non_leaders.push(agent);
        }
    }

    healthy_non_leaders.sort_by_key(|agent| -specialization_score(agent, query));
    degraded_non_leaders.sort_by_key(|agent| -specialization_score(agent, query));

    let mut ordered = Vec::new();
    ordered.extend(healthy_non_leaders);
    ordered.extend(degraded_non_leaders);
    if let Some(agent) = healthy_leader {
        ordered.push(agent);
    }
    if let Some(agent) = degraded_leader {
        ordered.push(agent);
    }
    ordered
}

pub(crate) async fn try_sync_fallback_chain(
    state: &Arc<crate::SharedState>,
    cx: &ConnectionTo<Conductor>,
    active_session_id: Option<&str>,
    failed_role: &str,
    query: &str,
    started_at: Instant,
) -> Result<Option<String>, sacp::Error> {
    let fallback_targets = select_fallback_targets(state, failed_role, query).await;
    if fallback_targets.is_empty() {
        return Ok(None);
    }

    let mut last_err = String::new();
    for fallback_agent in fallback_targets {
        let fallback_role = fallback_agent.role.to_ascii_lowercase();
        tracing::warn!(
            "[Team-Tools] {} failed; attempting sync redistribution to {}",
            failed_role,
            fallback_role
        );
        let (fallback_proxy, fallback_query, fallback_role) =
            build_proxy_for_target(state, &fallback_agent, query).await?;
        match fallback_proxy
            .send_and_subscribe(&fallback_query, None, None)
            .await
        {
            Ok((schedule_id, mut rx)) => {
                let mut accumulated_text = String::new();
                while let Some(result) = rx.recv().await {
                    match result {
                        Ok(event) => {
                            let text = a2a_rs::proxy::extract_text_from_stream_event(&event);
                            if !text.is_empty() {
                                accumulated_text = text;
                            }
                        }
                        Err(e) => {
                            last_err = format!("{}", e);
                            break;
                        }
                    }
                }
                if let Some(session_id) = active_session_id {
                    if !accumulated_text.is_empty() {
                        emit_team_assistant_response(
                            state,
                            cx,
                            session_id,
                            &fallback_role,
                            &accumulated_text,
                        )
                        .await;
                    }
                    emit_team_delegation_complete(
                        state,
                        cx,
                        session_id,
                        &fallback_role,
                        "sync-fallback",
                        Some(&schedule_id),
                        &accumulated_text,
                    )
                    .await;
                }
                record_team_delegation_outcome(state, &fallback_role, true, started_at).await;
                return Ok(Some(if accumulated_text.is_empty() {
                    format!(
                        "[{}] Fallback task {} completed with no text output.",
                        fallback_role, schedule_id
                    )
                } else {
                    format!("[{}] {}", fallback_role, accumulated_text)
                }));
            }
            Err(e) => {
                last_err = format!("{}", e);
                record_team_delegation_outcome(state, &fallback_role, false, started_at).await;
            }
        }
    }

    if !last_err.is_empty() {
        tracing::warn!(
            "[Team-Tools] sync fallback chain exhausted after {} failed: {}",
            failed_role,
            last_err
        );
    }
    Ok(None)
}

pub(crate) async fn try_background_fallback_chain(
    state: &Arc<crate::SharedState>,
    failed_role: &str,
    query: &str,
    started_at: Instant,
) -> Result<Option<String>, sacp::Error> {
    let fallback_targets = select_fallback_targets(state, failed_role, query).await;
    if fallback_targets.is_empty() {
        return Ok(None);
    }

    let mut last_err = String::new();
    for fallback_agent in fallback_targets {
        let fallback_role = fallback_agent.role.to_ascii_lowercase();
        tracing::warn!(
            "[Team-Tools] {} background delegation failed; attempting redistribution to {}",
            failed_role,
            fallback_role
        );
        let (fallback_proxy, fallback_query, fallback_role) =
            build_proxy_for_target(state, &fallback_agent, query).await?;
        match fallback_proxy.fire_and_forget(&fallback_query, None, None).await {
            Ok(schedule_id) => {
                record_team_delegation_outcome(state, &fallback_role, true, started_at).await;
                return Ok(Some(format!(
                    "[{}] background fallback delegated. schedule_id={}",
                    fallback_role, schedule_id
                )));
            }
            Err(e) => {
                last_err = format!("{}", e);
                record_team_delegation_outcome(state, &fallback_role, false, started_at).await;
            }
        }
    }

    if !last_err.is_empty() {
        tracing::warn!(
            "[Team-Tools] background fallback chain exhausted after {} failed: {}",
            failed_role,
            last_err
        );
    }
    Ok(None)
}

pub(crate) async fn build_proxy_for_target(
    state: &crate::SharedState,
    target_role: &TeamRoleTarget,
    actual_query: &str,
) -> Result<(A2aProxy, String, String), sacp::Error> {
    info!(
        "[Team-Tools] target '{}' at {} : {:?}",
        target_role.role, target_role.endpoint, actual_query
    );

    let leader_cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());
    let enriched_query = crate::memory_provider::inject_context(actual_query, leader_cwd.as_deref());

    let active_sid = state.sessions.active_session_id.read().await.clone();
    let mcp_servers_json = if !active_sid.is_empty() {
        let map = &state.sessions.mcp_servers;
        map.get(&active_sid)
            .and_then(|servers| serde_json::to_value(servers).ok())
    } else {
        None
    };

    let mut session_ctx = a2a_rs::proxy::SessionContext::new()
        .with_admin_skills(true)
        .with_disabled_skills(vec![])
        .with_extra_skills_dirs(vec![state
            .infra
            .brain
            .vault_dir()
            .join("skills")
            .to_string_lossy()
            .to_string()]);

    if let Some(cwd) = leader_cwd.as_deref() {
        session_ctx = session_ctx.with_cwd(cwd);
    }
    if let Some(mcp_json) = mcp_servers_json {
        session_ctx = session_ctx.with_mcp_servers(mcp_json);
    }

    let proxy = A2aProxy::with_context(&target_role.endpoint, &target_role.role, session_ctx);
    let role_lower = target_role.role.to_lowercase();
    Ok((proxy, enriched_query, role_lower))
}

pub(crate) async fn record_team_delegation_outcome(
    state: &Arc<crate::SharedState>,
    role: &str,
    success: bool,
    started_at: Instant,
) {
    crate::process_supervisor::record_delegation(
        &state.team_state().delegation_metrics,
        role,
        success,
        started_at.elapsed().as_millis() as i64,
    )
    .await;
}

#[macro_export]
macro_rules! with_team_server {
    ($builder:expr, $state:expr) => {{
        $builder.with_mcp_server({
            let state = $state.clone();
            sacp::mcp_server::McpServer::<sacp::Conductor, _>::builder("team-tools".to_string())
                .instructions("Delegate schedules to specialist team agents. Each tool accepts a `query` parameter.\nOnly available when team mode is enabled.\nCall these tools to delegate work; results are streamed in real-time.")
                .tool_fn(
                    "delegate",
                    "팀/서브 에이전트에게 작업 위임 후 결과가 올 때까지 동기 대기. 즉시 다음 논리를 진행해야 할 때만 사용한다. background/async/subscribe/alert/wake-up 요청에는 이 도구를 쓰지 말고 `delegate_background`를 사용한다.",
                    {
                        let state = state.clone();
                        async move |input: $crate::TeamDelegateInput, cx: sacp::mcp_server::McpConnectionTo<sacp::Conductor>| {
                            let (proxy, enriched_query, role_lower) =
                                $crate::builtins::team::setup_a2a_proxy(&state, input.query.trim(), "delegate").await?;
                            if $crate::builtins::team::current_session_prefers_background(&state).await {
                                tracing::info!(
                                    "[Team-Tools] {} delegate coerced to background mode due to current session intent",
                                    role_lower
                                );
                                return $crate::builtins::team::run_background_delegate(state.clone(), proxy, enriched_query, role_lower, cx).await;
                            }
                            let cx = cx.connection_to();
                            let active_session_id = $crate::builtins::team::current_active_session_id(&state).await;
                            let max_retries = $crate::builtins::team::current_team_max_retries(&state);
                            let pause_on_error = $crate::builtins::team::current_team_pause_on_error(&state);
                            let emit_responses = $crate::builtins::team::should_emit_team_response(&state, &role_lower);
                            let started_at = std::time::Instant::now();

                            let mut last_err = String::new();
                            for attempt in 0..max_retries {
                                if attempt > 0 {
                                    let delay = $crate::builtins::team::a2a_retry_delay(attempt);
                                    tracing::warn!("[Team-Tools] {} delegate retry {}/{} in {:?}", role_lower, attempt, max_retries, delay);
                                    let retry_msg = format!("[System] {} 에이전트 재시도 중... ({}/{})", role_lower, attempt, max_retries);
                                    let patch = serde_json::json!({
                                        "agentId": role_lower,
                                        "content": retry_msg,
                                        "thinking": "",
                                        "toolCalls": [],
                                        "contentBlocks": [{"type": "text", "text": &retry_msg}],
                                        "final": false,
                                    });
                                    if let Ok(notif) = sacp::UntypedMessage::new("ilhae/assistant_turn_patch", patch) {
                                        let _ = cx.send_notification_to(sacp::Client, notif);
                                    }
                                    tokio::time::sleep(delay).await;
                                }

                                match proxy.send_and_subscribe(&enriched_query, None, None).await {
                                    Ok((schedule_id, mut rx)) => {
                                        if let Some(session_id) = active_session_id.as_deref() {
                                            $crate::builtins::team::emit_team_delegation_start(
                                                &state, &cx, session_id, &role_lower, "sync", &enriched_query, Some(&schedule_id),
                                            ).await;
                                        }
                                        let mut accumulated_text = String::new();
                                        let mut event_count = 0usize;

                                        while let Some(result) = rx.recv().await {
                                            match result {
                                                Ok(event) => {
                                                    let text = a2a_rs::proxy::extract_text_from_stream_event(&event);
                                                    if !text.is_empty() {
                                                        accumulated_text = text;
                                                        if emit_responses {
                                                            let patch = serde_json::json!({
                                                                "agentId": role_lower,
                                                                "content": accumulated_text,
                                                                "thinking": "",
                                                                "toolCalls": [],
                                                                "contentBlocks": [{"type": "text", "text": &accumulated_text}],
                                                                "final": false,
                                                            });
                                                            if let Ok(notif) = sacp::UntypedMessage::new("ilhae/assistant_turn_patch", patch) {
                                                                let _ = cx.send_notification_to(sacp::Client, notif.clone());
                                                            }
                                                        }
                                                    }
                                                    event_count += 1;
                                                }
                                                Err(e) => {
                                                    tracing::warn!("[Team-Tools] {} sync stream error: {}", role_lower, e);
                                                    break;
                                                }
                                            }
                                        }

                                        if !accumulated_text.is_empty() {
                                            if let Some(session_id) = active_session_id.as_deref() {
                                                $crate::builtins::team::emit_team_assistant_response(
                                                    &state, &cx, session_id, &role_lower, &accumulated_text,
                                                ).await;
                                                $crate::builtins::team::emit_team_delegation_complete(
                                                    &state, &cx, session_id, &role_lower, "sync", Some(&schedule_id), &accumulated_text,
                                                ).await;
                                                // Auto-Checkpoint: delegation 완료 시 자동 스냅샷
                                                $crate::builtins::team::auto_save_checkpoint(
                                                    &state, session_id, &role_lower, "sync", Some(&schedule_id),
                                                ).await;
                                            }
                                            let patch = serde_json::json!({
                                                "agentId": role_lower,
                                                "content": accumulated_text,
                                                "thinking": "",
                                                "toolCalls": [],
                                                "contentBlocks": [{"type": "text", "text": &accumulated_text}],
                                                "final": true,
                                            });
                                            if emit_responses {
                                                if let Ok(notif) = sacp::UntypedMessage::new("ilhae/assistant_turn_patch", patch) {
                                                    let _ = cx.send_notification_to(sacp::Client, notif);
                                                }
                                            }
                                        }

                                        tracing::info!("[Team-Tools] {} sync completed: {}B text, {} events, schedule_id={}", role_lower, accumulated_text.len(), event_count, schedule_id);
                                        $crate::builtins::team::record_team_delegation_outcome(&state, &role_lower, true, started_at).await;
                                        return Ok::<String, sacp::Error>(if accumulated_text.is_empty() {
                                            format!("[{}] Task {} completed with no text output.", role_lower, schedule_id)
                                        } else {
                                            format!("[{}] {}", role_lower, accumulated_text)
                                        });
                                    }
                                    Err(e) => {
                                        last_err = format!("{}", e);
                                        tracing::warn!("[Team-Tools] {} sync error (attempt {}): {}", role_lower, attempt + 1, e);
                                    }
                                }
                            }

                            $crate::builtins::team::record_team_delegation_outcome(&state, &role_lower, false, started_at).await;
                            if !pause_on_error {
                                if let Some(message) = $crate::builtins::team::try_sync_fallback_chain(
                                    &state,
                                    &cx,
                                    active_session_id.as_deref(),
                                    &role_lower,
                                    &enriched_query,
                                    started_at,
                                ).await? {
                                    return Ok::<String, sacp::Error>(message);
                                }
                            }

                            if pause_on_error {
                                Err(sacp::Error::internal_error().data(format!(
                                    "A2A delegation to {} failed after {} retries: {}", role_lower, max_retries, last_err
                                )))
                            } else {
                                Ok::<String, sacp::Error>(format!(
                                    "[{}] delegation failed after {} retries, continuing: {}",
                                    role_lower, max_retries, last_err
                                ))
                            }
                        }
                    },
                    sacp::tool_fn!(),
                )
                .tool_fn(
                    "delegate_background",
                    "팀/서브 에이전트에게 작업을 백그라운드로 위임한다. background, async, subscribe, long-running, wait for alert, wake me later 같은 요청에는 반드시 이 도구를 사용한다. 완료되면 시스템 알림(System Alert)로 이어받는다.",
                    {
                        let state = state.clone();
                        async move |input: $crate::TeamDelegateInput, cx: sacp::mcp_server::McpConnectionTo<sacp::Conductor>| {
                            let (proxy, enriched_query, role_lower) =
                                $crate::builtins::team::setup_a2a_proxy(&state, input.query.trim(), "delegate_background").await?;
                            $crate::builtins::team::run_background_delegate(state.clone(), proxy, enriched_query, role_lower, cx).await
                        }
                    },
                    sacp::tool_fn!(),
                )
                .tool_fn(
                    "propose",
                    "팀 에이전트(리더 포함)에게 비동기로 보고, 제안, 피드백 전송. 응답을 대기하지 않고 즉시 종료 (Fire-and-forget).",
                    {
                        let state = state.clone();
                        async move |input: $crate::TeamProposeInput, _cx: sacp::mcp_server::McpConnectionTo<sacp::Conductor>| {
                            let query_str = format!("@{} {}", input.agent, input.message);
                            let (proxy, enriched_query, role_lower) =
                                $crate::builtins::team::setup_a2a_proxy(&state, &query_str, "propose").await?;

                            match proxy.fire_and_forget(&enriched_query, None, None).await {
                                Ok(schedule_id) => {
                                    tracing::info!("[Team-Tools] {} propose schedule_id: {}", role_lower, schedule_id);
                                    Ok::<String, sacp::Error>(format!(
                                        "[{}] Message proposed successfully (async). schedule_id={}", role_lower, schedule_id
                                    ))
                                }
                                Err(e) => {
                                    tracing::warn!("[Team-Tools] {} propose error: {}", role_lower, e);
                                    Err(sacp::Error::internal_error().data(format!("A2A propose to {} failed: {}", role_lower, e)))
                                }
                            }
                        }
                    },
                    sacp::tool_fn!(),
                )
                .tool_fn(
                    "team_update_channel",
                    "저장된 Team Shared State Channel 내의 특정 식별자(key)에 JSON 값(value)을 업데이트합니다. 에이전트 간 공유되어야 하는 데이터(예: 상태, 결과, 파라미터)에 접근할 때 사용합니다.",
                    {
                        let state = state.clone();
                        async move |input: $crate::TeamUpdateChannelInput, _cx: sacp::mcp_server::McpConnectionTo<sacp::Conductor>| {
                            let parsed_value: serde_json::Value = serde_json::from_str(&input.value)
                                .unwrap_or_else(|_| serde_json::Value::String(input.value.clone()));

                            let mut mem = state.team.channel_memory.write().await;
                            mem.insert(input.key.clone(), parsed_value);

                            Ok::<String, sacp::Error>(format!("Channel memory updated successfully for key: {}", input.key))
                        }
                    },
                    sacp::tool_fn!(),
                )
                .tool_fn(
                    "team_read_channel",
                    "팀 간 통신을 위해 유지되는 Shared State Channel의 데이터(JSON)를 읽어옵니다. 특정 식별자(key)를 지정하여 해당 값을 확인하거나, 비워두면 채널 전체 데이터를 조회합니다.",
                    {
                        let state = state.clone();
                        async move |input: $crate::TeamReadChannelInput, _cx: sacp::mcp_server::McpConnectionTo<sacp::Conductor>| {
                            let mem = state.team.channel_memory.read().await;

                            if let Some(ref key) = input.key {
                                if let Some(val) = mem.get(key) {
                                    Ok::<String, sacp::Error>(serde_json::to_string_pretty(val).unwrap_or_default())
                                } else {
                                    Ok::<String, sacp::Error>(format!("Key '{}' not found in channel memory", key))
                                }
                            } else {
                                let map_val = mem.clone();
                                let serialized = serde_json::to_string_pretty(&map_val).unwrap_or_default();
                                Ok::<String, sacp::Error>(serialized)
                            }
                        }
                    },
                    sacp::tool_fn!(),
                )
                .tool_fn(
                    "team_save_checkpoint",
                    "현재 시점의 Team Shared State Channel 내의 전체 데이터를 지정한 버전(version) 번호로 DB에 안전하게 스냅샷(저장)합니다. 오류 복구나 롤백이 필요해지는 분기점 직전에 사용하세요.",
                    {
                        let state = state.clone();
                        async move |input: $crate::TeamSaveCheckpointInput, _cx: sacp::mcp_server::McpConnectionTo<sacp::Conductor>| {
                            let mem = state.team.channel_memory.read().await;
                            let map_val = mem.clone();
                            let checkpoint_data = serde_json::to_string(&map_val).unwrap_or_else(|_| "{}".to_string());

                            let metadata = serde_json::json!({
                                "save_reason": "Manual agent checkpoint"
                            }).to_string();

                            match state.infra.brain.session_put_checkpoint(
                                &input.session_id,
                                &input.thread_id,
                                &input.version,
                                input.parent_version.as_deref(),
                                &checkpoint_data,
                                &metadata,
                            ) {
                                Ok(_) => Ok::<String, sacp::Error>(format!("Checkpoint {} saved successfully for session {}", input.version, input.session_id)),
                                Err(e) => Err(sacp::Error::internal_error().data(format!("Failed to save checkpoint: {}", e)))
                            }
                        }
                    },
                    sacp::tool_fn!(),
                )
                .tool_fn(
                    "team_resume_task",
                    "지정한 session_id와 version의 체크포인트를 DB에서 읽어와, 현재 Team Shared State Channel 메모리를 해당 시점의 데이터로 덮어씌워 롤백(Resume) 합니다.",
                    {
                        let state = state.clone();
                        async move |input: $crate::TeamResumeTaskInput, _cx: sacp::mcp_server::McpConnectionTo<sacp::Conductor>| {
                            match state.infra.brain.session_get_checkpoint(&input.session_id, &input.thread_id, &input.version) {
                                Ok(Some(chk)) => {
                                    if let Ok(parsed) = serde_json::from_str::<std::collections::HashMap<String, serde_json::Value>>(&chk.checkpoint_data) {
                                        let mut mem = state.team.channel_memory.write().await;
                                        *mem = parsed;
                                        Ok::<String, sacp::Error>(format!("State channel successfully rolled back to version {}", input.version))
                                    } else {
                                        Err(sacp::Error::internal_error().data("Invalid JSON checkpoint map".to_string()))
                                    }
                                }
                                Ok(None) => Err(sacp::Error::invalid_request().data(format!("Checkpoint {} not found", input.version))),
                                Err(e) => Err(sacp::Error::internal_error().data(format!("DB error reading checkpoint: {}", e)))
                            }
                        }
                    },
                    sacp::tool_fn!(),
                )
                .build()
        })
    }};
}

pub async fn setup_a2a_proxy(
    state: &crate::SharedState,
    query_trimmed: &str,
    mode: &str,
) -> Result<(A2aProxy, String, String), sacp::Error> {
    let settings = state.infra.settings_store.get();
    if !settings.agent.team_mode {
        return Err(sacp::Error::invalid_request()
            .data("Team mode is not enabled. Enable it in Settings.".to_string()));
    }

    let ilhae_dir = state.infra.brain.ilhae_data_dir().to_path_buf();

    let team_cfg = load_team_runtime_config(&ilhae_dir).ok_or_else(|| {
        sacp::Error::invalid_request().data("No team configuration found.".to_string())
    })?;

    let (target_role, actual_query) = if query_trimmed.starts_with('@') {
        let after_at = &query_trimmed[1..];
        let space_idx = after_at.find(char::is_whitespace).unwrap_or(after_at.len());
        let mentioned = after_at[..space_idx].to_lowercase();
        let rest = after_at[space_idx..].trim().to_string();

        let matched = team_cfg.agents.iter().find(|a| {
            let r = a.role.to_lowercase();
            r == mentioned || r.starts_with(&mentioned)
        });

        match matched {
            Some(a) => (
                a.clone(),
                if rest.is_empty() {
                    query_trimmed.to_string()
                } else {
                    rest
                },
            ),
            None => {
                return Err(sacp::Error::invalid_request().data(format!(
                    "Unknown agent: @{}. Available: {}",
                    mentioned,
                    team_cfg
                        .agents
                        .iter()
                        .map(|a| a.role.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
    } else {
        let other_agents: Vec<_> = team_cfg.agents.iter().filter(|a| !a.is_main).collect();

        if other_agents.len() == 1 {
            (other_agents[0].clone(), query_trimmed.to_string())
        } else {
            return Err(sacp::Error::invalid_request().data(format!(
                "Specify agent with @mention. Available: {}",
                team_cfg
                    .agents
                    .iter()
                    .map(|a| format!("@{}", a.role.to_lowercase()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    };

    info!(
        "[Team-Tools] {} to '{}' at {} : {:?}",
        mode, target_role.role, target_role.endpoint, actual_query
    );

    build_proxy_for_target(state, &target_role, &actual_query).await
}

pub fn background_keywords(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
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

pub async fn current_session_prefers_background(state: &crate::SharedState) -> bool {
    let active_sid = state.sessions.active_session_id.read().await.clone();
    if active_sid.is_empty() {
        return false;
    }
    {
        let map = &state.sessions.delegation_mode;
        if map
            .get(&active_sid)
            .map(|mode| mode == "background" || mode == "subscribe")
            .unwrap_or(false)
        {
            return true;
        }
    }
    let Ok(messages) = state.infra.brain.session_load_messages(&active_sid) else {
        return false;
    };
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| background_keywords(&message.content))
        .unwrap_or(false)
}

pub async fn run_background_delegate(
    state: Arc<crate::SharedState>,
    proxy: A2aProxy,
    enriched_query: String,
    role_lower: String,
    cx: McpConnectionTo<Conductor>,
) -> Result<String, sacp::Error> {
    let cx = cx.connection_to();
    let active_session_id = current_active_session_id(&state).await;
    let max_retries = current_team_max_retries(&state);
    let pause_on_error = current_team_pause_on_error(&state);
    let started_at = Instant::now();
    let mut last_err = String::new();
    for attempt in 0..max_retries {
        if attempt > 0 {
            let delay = a2a_retry_delay(attempt);
            warn!(
                "[Team-Tools] {} background retry {}/{} in {:?}",
                role_lower, attempt, max_retries, delay
            );
            tokio::time::sleep(delay).await;
        }

        match proxy.fire_and_forget(&enriched_query, None, None).await {
            Ok(schedule_id) => {
                let proxy_bg = proxy.clone();
                let tid = schedule_id.clone();
                let cx_bg = cx.clone();
                let role_bg = role_lower.clone();
                let state_bg = state.clone();
                let active_sid_bg = active_session_id.clone();
                let max_retries_bg = max_retries;

                if let Some(session_id) = active_session_id.as_deref() {
                    emit_team_delegation_start(
                        &state,
                        &cx,
                        session_id,
                        &role_lower,
                        "background",
                        &enriched_query,
                        Some(&schedule_id),
                    )
                    .await;
                    emit_team_task_update(
                        &state,
                        &cx,
                        session_id,
                        &role_lower,
                        &schedule_id,
                        "submitted",
                        &enriched_query,
                    )
                    .await;
                }

                tokio::spawn(async move {
                    let mut sub_last_err = String::new();
                    for sub_attempt in 0..max_retries_bg {
                        if sub_attempt > 0 {
                            let delay = a2a_retry_delay(sub_attempt);
                            warn!(
                                "[Team-Tools] {} subscribe retry {}/{}",
                                role_bg, sub_attempt, max_retries_bg
                            );
                            let retry_alert = format!(
                                "[System] Task {} from {} subscriber retrying ({}/{})...",
                                tid, role_bg, sub_attempt, max_retries_bg
                            );
                            let patch = json!({
                                "agentId": role_bg,
                                "content": retry_alert,
                                "thinking": "",
                                "toolCalls": [],
                                "contentBlocks": [{"type": "text", "text": &retry_alert}],
                                "final": false,
                            });
                            if let Ok(notif) =
                                UntypedMessage::new("ilhae/assistant_turn_patch", patch)
                            {
                                let _ = cx_bg.send_notification_to(Client, notif);
                            }
                            tokio::time::sleep(delay).await;
                        }

                        match proxy_bg.subscribe_to_task(&tid).await {
                            Ok(events) => {
                                let text = events
                                    .iter()
                                    .map(|e| a2a_rs::proxy::extract_text_from_stream_event(e))
                                    .collect::<Vec<_>>()
                                    .join("");
                                if let Some(session_id) = active_sid_bg.as_deref() {
                                    emit_team_task_update(
                                        &state_bg,
                                        &cx_bg,
                                        session_id,
                                        &role_bg,
                                        &tid,
                                        "completed",
                                        if text.is_empty() {
                                            "(no output)"
                                        } else {
                                            &text
                                        },
                                    )
                                    .await;
                                    if !text.is_empty() {
                                        emit_team_assistant_response(
                                            &state_bg, &cx_bg, session_id, &role_bg, &text,
                                        )
                                        .await;
                                    }
                                    emit_team_delegation_complete(
                                        &state_bg,
                                        &cx_bg,
                                        session_id,
                                        &role_bg,
                                        "background",
                                        Some(&tid),
                                        &text,
                                    )
                                    .await;
                                    // Auto-Checkpoint: background delegation 완료 시 자동 스냅샷
                                    auto_save_checkpoint(
                                        &state_bg,
                                        session_id,
                                        &role_bg,
                                        "background",
                                        Some(&tid),
                                    )
                                    .await;
                                    if let Ok(notif) = UntypedMessage::new(
                                        crate::types::NOTIF_BACKGROUND_TASK_COMPLETED,
                                        json!({
                                            "taskId": tid,
                                            "agentRole": role_bg,
                                            "sessionId": session_id,
                                        }),
                                    ) {
                                        let _ = cx_bg.send_notification_to(Client, notif);
                                    }
                                }
                                record_team_delegation_outcome(&state_bg, &role_bg, true, started_at).await;
                                let alert = format!(
                                    "[System Alert] Task {} from {} completed:\n{}",
                                    tid,
                                    role_bg,
                                    if text.is_empty() {
                                        "(no output)"
                                    } else {
                                        &text
                                    }
                                );
                                let patch = json!({
                                    "agentId": role_bg,
                                    "content": alert,
                                    "thinking": "",
                                    "toolCalls": [],
                                    "contentBlocks": [{"type": "text", "text": alert}],
                                    "final": true,
                                });
                                if let Ok(notif) =
                                    UntypedMessage::new("ilhae/assistant_turn_patch", patch)
                                {
                                    let _ = cx_bg.send_notification_to(Client, notif);
                                }
                                return;
                            }
                            Err(e) => {
                                sub_last_err = format!("{}", e);
                                warn!(
                                    "[Team-Tools] {} subscribe bg error (attempt {}): {}",
                                    role_bg,
                                    sub_attempt + 1,
                                    e
                                );
                            }
                        }
                    }
                    let fail_alert = format!(
                        "[System Alert] Task {} from {} failed after {} retries: {}",
                        tid, role_bg, max_retries_bg, sub_last_err
                    );
                    record_team_delegation_outcome(&state_bg, &role_bg, false, started_at).await;
                    let patch = json!({
                        "agentId": role_bg,
                        "content": fail_alert,
                        "thinking": "",
                        "toolCalls": [],
                        "contentBlocks": [{"type": "text", "text": &fail_alert}],
                        "final": true,
                    });
                    if let Ok(notif) = UntypedMessage::new("ilhae/assistant_turn_patch", patch) {
                        let _ = cx_bg.send_notification_to(Client, notif);
                    }
                });

                info!(
                    "[Team-Tools] {} subscribe schedule_id: {}",
                    role_lower, schedule_id
                );
                return Ok(format!(
                    "[{}] Task delegated (background). schedule_id={}\nYou will receive a [System Alert] when this task completes.",
                    role_lower, schedule_id
                ));
            }
            Err(e) => {
                last_err = format!("{}", e);
                warn!(
                    "[Team-Tools] {} background send error (attempt {}): {}",
                    role_lower,
                    attempt + 1,
                    e
                );
            }
        }
    }

    record_team_delegation_outcome(&state, &role_lower, false, started_at).await;
    if !pause_on_error {
        if let Some(message) =
            try_background_fallback_chain(&state, &role_lower, &enriched_query, started_at)
                .await?
        {
            return Ok(message);
        }
    }

    if pause_on_error {
        Err(sacp::Error::internal_error().data(format!(
            "A2A background delegation to {} failed after {} retries: {}",
            role_lower, max_retries, last_err
        )))
    } else {
        Ok(format!(
            "[{}] background delegation failed after {} retries, continuing: {}",
            role_lower, max_retries, last_err
        ))
    }
}

pub async fn current_active_session_id(state: &Arc<crate::SharedState>) -> Option<String> {
    let session_id = state.sessions.active_session_id.read().await.clone();
    if session_id.is_empty() {
        None
    } else {
        Some(session_id)
    }
}

pub async fn emit_team_delegation_start(
    state: &Arc<crate::SharedState>,
    cx: &ConnectionTo<Conductor>,
    session_id: &str,
    role: &str,
    mode: &str,
    query: &str,
    task_id: Option<&str>,
) {
    state.infra.brain.session_write_delegation_event(
        session_id,
        role,
        mode,
        task_id,
        if query.is_empty() { None } else { Some(query) },
        None,
    );

    let msg = format!("🛰️ Delegating to {} (mode: {})", role, mode);
    let event = json!({
        "session_id": session_id,
        "source_role": "Leader",
        "assigned_role": role,
        "event_type": "delegation_start",
        "message": msg,
        "event_line": format!("🛰️ Leader → {} [{}]", role, mode),
        "mode": mode,
        "task_id": task_id,
        "request": query,
    });
    if let Ok(notif) = UntypedMessage::new(crate::types::NOTIF_A2A_EVENT, event) {
        let _ = cx.send_notification_to(Client, notif);
    }

    persist_events(
        &state.infra.brain,
        session_id,
        [delegation_started_event(role, mode, query, task_id, None)],
    );
}

pub async fn emit_team_task_update(
    state: &Arc<crate::SharedState>,
    cx: &ConnectionTo<Conductor>,
    session_id: &str,
    role: &str,
    task_id: &str,
    state_name: &str,
    preview: &str,
) {
    let payload = json!({
        "sessionId": session_id,
        "agentRole": role,
        "taskId": task_id,
        "state": state_name,
        "preview": preview,
        "eventCount": 0,
    });
    if let Ok(notif) = UntypedMessage::new("ilhae/a2a_task_update", payload) {
        let _ = cx.send_notification_to(Client, notif);
    }

    let event = if state_name == "submitted" {
        task_submitted_event(role, task_id, preview, state_name)
    } else {
        task_status_event(role, task_id, preview, state_name, None)
    };
    persist_events(&state.infra.brain, session_id, [event]);
}

pub async fn emit_team_assistant_response(
    state: &Arc<crate::SharedState>,
    cx: &ConnectionTo<Conductor>,
    session_id: &str,
    role: &str,
    text: &str,
) {
    let Some((visible_role, visible_text)) = client_facing_team_response(state, role, text) else {
        return;
    };
    let turn_id = format!("team-response-{}", Uuid::new_v4());
    if !visible_text.trim().is_empty() {
        if let Ok(notif) = UntypedMessage::new(
            crate::types::NOTIF_APP_SESSION_EVENT,
            crate::types::IlhaeAppSessionEventNotification {
                engine: visible_role.clone(),
                event: crate::types::IlhaeAppSessionEventDto::MessageDelta {
                    thread_id: session_id.to_string(),
                    turn_id: turn_id.clone(),
                    item_id: format!("{turn_id}:{visible_role}"),
                    channel: "assistant".to_string(),
                    delta: visible_text.clone(),
                },
            },
        ) {
            let _ = cx.send_notification_to(Client, notif);
        }
        if let Ok(notif) = UntypedMessage::new(
            crate::types::NOTIF_APP_SESSION_EVENT,
            crate::types::IlhaeAppSessionEventNotification {
                engine: visible_role,
                event: crate::types::IlhaeAppSessionEventDto::TurnCompleted {
                    thread_id: session_id.to_string(),
                    turn_id,
                    status: "completed".to_string(),
                },
            },
        ) {
            let _ = cx.send_notification_to(Client, notif);
        }
    }
    persist_events(
        &state.infra.brain,
        session_id,
        [agent_response_event(role, text, "[]", "sync", None)],
    );
}

pub async fn emit_team_delegation_complete(
    state: &Arc<crate::SharedState>,
    cx: &ConnectionTo<Conductor>,
    session_id: &str,
    role: &str,
    mode: &str,
    task_id: Option<&str>,
    response_text: &str,
) {
    state.infra.brain.session_write_delegation_event(
        session_id,
        role,
        mode,
        task_id,
        None,
        if response_text.trim().is_empty() {
            None
        } else {
            Some(response_text)
        },
    );

    let msg = format!("✅ {} completed delegation", role);
    let event = json!({
        "session_id": session_id,
        "source_role": role,
        "assigned_role": "Leader",
        "event_type": "delegation_complete",
        "message": msg,
        "event_line": format!("✅ {} → Leader [completed]", role),
        "mode": mode,
        "task_id": task_id,
        "response": response_text,
    });
    if let Ok(notif) = UntypedMessage::new(crate::types::NOTIF_A2A_EVENT, event) {
        let _ = cx.send_notification_to(Client, notif);
    }
    persist_events(
        &state.infra.brain,
        session_id,
        [delegation_completed_event(
            role,
            task_id,
            "completed",
            response_text,
            mode,
        )],
    );
}

/// Auto-Checkpoint: delegate 완료 시 Proxy가 자동으로 channel_memory 스냅샷을 DB에 저장.
/// LangGraph의 BaseCheckpointSaver가 매 노드 실행 후 자동 저장하는 패턴과 동일.
pub async fn auto_save_checkpoint(
    state: &Arc<crate::SharedState>,
    session_id: &str,
    role: &str,
    mode: &str,
    task_id: Option<&str>,
) {
    let mem = state.team.channel_memory.read().await;
    let checkpoint_data = serde_json::to_string(&*mem).unwrap_or_else(|_| "{}".to_string());

    let version = format!(
        "auto-{}-{}",
        role,
        chrono::Utc::now().format("%Y%m%dT%H%M%S")
    );
    let metadata = serde_json::json!({
        "auto": true,
        "role": role,
        "mode": mode,
        "task_id": task_id,
    })
    .to_string();

    match state.infra.brain.session_put_checkpoint(
        session_id,
        "main",
        &version,
        None,
        &checkpoint_data,
        &metadata,
    ) {
        Ok(_) => tracing::info!(
            "[Auto-Checkpoint] saved {} for session {}",
            version,
            session_id
        ),
        Err(e) => tracing::warn!("[Auto-Checkpoint] failed to save: {}", e),
    }
}
