use crate::legacy_core::config::Config;
use crate::legacy_core::message_history_metadata;
use crate::status::StatusAccountDisplay;
use crate::status::plan_type_display_name;
use codex_app_server_protocol::AuthMode;
use codex_app_server_protocol::CreditsSnapshot as AppServerCreditsSnapshot;
use codex_app_server_protocol::GetAccountRateLimitsResponse;
use codex_app_server_protocol::Model as ApiModel;
use codex_app_server_protocol::RateLimitSnapshot as AppServerRateLimitSnapshot;
use codex_app_server_protocol::RateLimitWindow as AppServerRateLimitWindow;
use codex_app_server_protocol::ReviewTarget as AppServerReviewTarget;
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadForkResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadStartSource;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ModelAvailabilityNux;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelUpgrade;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::CreditsSnapshot;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;
use codex_protocol::protocol::ReviewTarget as CoreReviewTarget;
use codex_protocol::protocol::SandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
use color_eyre::eyre::Result;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use super::AppServerStartedThread;
use super::ThreadParamsMode;
use super::ThreadSessionState;

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

pub(crate) fn model_preset_from_api_model(model: ApiModel) -> ModelPreset {
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
        additional_speed_tiers: model.additional_speed_tiers,
        is_default: model.is_default,
        upgrade,
        show_in_picker: !model.hidden,
        availability_nux: model.availability_nux.map(|nux| ModelAvailabilityNux {
            message: nux.message,
        }),
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

pub(crate) fn thread_start_params_from_config(
    config: &Config,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<&Path>,
    session_start_source: Option<ThreadStartSource>,
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
        session_start_source,
        persist_extended_history: true,
        ..ThreadStartParams::default()
    }
}

pub(crate) fn thread_resume_params_from_config(
    config: Config,
    thread_id: ThreadId,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<&Path>,
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

pub(crate) fn thread_fork_params_from_config(
    config: Config,
    thread_id: ThreadId,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<&Path>,
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
    remote_cwd_override: Option<&Path>,
) -> Option<String> {
    match thread_params_mode {
        ThreadParamsMode::Embedded => Some(config.cwd.to_string_lossy().to_string()),
        ThreadParamsMode::Remote => {
            remote_cwd_override.map(|cwd| cwd.to_string_lossy().to_string())
        }
    }
}

pub(crate) async fn started_thread_from_start_response(
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

pub(crate) async fn started_thread_from_resume_response(
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

pub(crate) async fn started_thread_from_fork_response(
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
        response.instruction_sources.clone(),
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
        response.instruction_sources.clone(),
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
        response.instruction_sources.clone(),
        response.reasoning_effort,
        config,
    )
    .await
}

pub(crate) fn review_target_to_app_server(target: CoreReviewTarget) -> AppServerReviewTarget {
    match target {
        CoreReviewTarget::UncommittedChanges => AppServerReviewTarget::UncommittedChanges,
        CoreReviewTarget::BaseBranch { branch } => AppServerReviewTarget::BaseBranch { branch },
        CoreReviewTarget::Commit { sha, title } => AppServerReviewTarget::Commit { sha, title },
        CoreReviewTarget::Custom { instructions } => AppServerReviewTarget::Custom { instructions },
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "session mapping keeps explicit fields"
)]
pub(crate) async fn thread_session_state_from_thread_response(
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
    cwd: AbsolutePathBuf,
    instruction_source_paths: Vec<AbsolutePathBuf>,
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
    let (history_log_id, history_entry_count) = message_history_metadata(config).await;
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
        instruction_source_paths,
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
    snapshot: AppServerRateLimitSnapshot,
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

fn app_server_rate_limit_window_to_core(window: AppServerRateLimitWindow) -> RateLimitWindow {
    RateLimitWindow {
        used_percent: window.used_percent as f64,
        window_minutes: window.window_duration_mins,
        resets_at: window.resets_at,
    }
}

fn app_server_credits_snapshot_to_core(snapshot: AppServerCreditsSnapshot) -> CreditsSnapshot {
    CreditsSnapshot {
        has_credits: snapshot.has_credits,
        unlimited: snapshot.unlimited,
        balance: snapshot.balance,
    }
}
