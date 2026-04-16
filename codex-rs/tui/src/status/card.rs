use crate::history_cell::CompositeHistoryCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::history_cell::with_border_with_inner_width;
use crate::version::CODEX_CLI_VERSION;
use chrono::DateTime;
use chrono::Local;
use codex_core::config::Config;
use codex_ilhae::native_runtime_context;
use codex_git_utils::{get_git_repo_root, resolve_root_git_project_for_trust};
use codex_model_provider_info::WireApi;
use codex_protocol::ThreadId;
use codex_protocol::account::PlanType;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::NetworkAccess;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TokenUsageInfo;
use codex_utils_sandbox_summary::summarize_sandbox_policy;
use ratatui::prelude::*;
use ratatui::style::Stylize;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use url::Url;

use super::account::StatusAccountDisplay;
use super::format::FieldFormatter;
use super::format::line_display_width;
use super::format::push_label;
use super::format::truncate_line_to_width;
use super::helpers::compose_account_display;
use super::helpers::compose_agents_summary;
use super::helpers::compose_model_display;
use super::helpers::format_directory_display;
use super::helpers::format_tokens_compact;
use super::rate_limits::RateLimitSnapshotDisplay;
use super::rate_limits::StatusRateLimitData;
use super::rate_limits::StatusRateLimitRow;
use super::rate_limits::StatusRateLimitValue;
use super::rate_limits::compose_rate_limit_data;
use super::rate_limits::compose_rate_limit_data_many;
use super::rate_limits::format_status_limit_summary;
use super::rate_limits::render_status_limit_progress_bar;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_lines;
use std::sync::Arc;
use std::sync::RwLock;

#[derive(Debug, Clone)]
struct StatusContextWindowData {
    percent_remaining: i64,
    tokens_in_context: i64,
    window: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct StatusTokenUsageData {
    total: i64,
    input: i64,
    output: i64,
    context_window: Option<StatusContextWindowData>,
}

#[derive(Debug)]
struct StatusRateLimitState {
    rate_limits: StatusRateLimitData,
    refreshing_rate_limits: bool,
}

#[derive(Debug, Clone)]
struct StatusExecutionLoopData {
    profile: String,
    advisor_mode: String,
    auto_mode: String,
    team_mode: String,
    kairos: String,
    self_improvement: String,
    knowledge: String,
}

#[derive(Debug, Clone)]
struct StatusWorkflowSurfaceData {
    tmux: String,
    worktree: String,
    remote: String,
}

#[derive(Debug, Clone)]
pub(crate) struct StatusHistoryHandle {
    rate_limit_state: Arc<RwLock<StatusRateLimitState>>,
}

impl StatusHistoryHandle {
    pub(crate) fn finish_rate_limit_refresh(
        &self,
        rate_limits: &[RateLimitSnapshotDisplay],
        now: DateTime<Local>,
    ) {
        let rate_limits = if rate_limits.len() <= 1 {
            compose_rate_limit_data(rate_limits.first(), now)
        } else {
            compose_rate_limit_data_many(rate_limits, now)
        };
        #[expect(clippy::expect_used)]
        let mut state = self
            .rate_limit_state
            .write()
            .expect("status history rate-limit state poisoned");
        state.rate_limits = rate_limits;
        state.refreshing_rate_limits = false;
    }
}

#[derive(Debug)]
struct StatusHistoryCell {
    model_name: String,
    model_details: Vec<String>,
    directory: PathBuf,
    permissions: String,
    agents_summary: Arc<RwLock<String>>,
    collaboration_mode: Option<String>,
    workflow_surface: StatusWorkflowSurfaceData,
    execution_loop: Option<StatusExecutionLoopData>,
    model_provider: Option<String>,
    account: Option<StatusAccountDisplay>,
    thread_name: Option<String>,
    session_id: Option<String>,
    forked_from: Option<String>,
    token_usage: StatusTokenUsageData,
    rate_limit_state: Arc<RwLock<StatusRateLimitState>>,
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn new_status_output(
    config: &Config,
    account_display: Option<&StatusAccountDisplay>,
    token_info: Option<&TokenUsageInfo>,
    total_usage: &TokenUsage,
    session_id: &Option<ThreadId>,
    thread_name: Option<String>,
    forked_from: Option<ThreadId>,
    rate_limits: Option<&RateLimitSnapshotDisplay>,
    _plan_type: Option<PlanType>,
    now: DateTime<Local>,
    model_name: &str,
    collaboration_mode: Option<&str>,
    reasoning_effort_override: Option<Option<ReasoningEffort>>,
) -> CompositeHistoryCell {
    let snapshots = rate_limits.map(std::slice::from_ref).unwrap_or_default();
    new_status_output_with_rate_limits(
        config,
        account_display,
        token_info,
        total_usage,
        session_id,
        thread_name,
        forked_from,
        snapshots,
        _plan_type,
        now,
        model_name,
        collaboration_mode,
        reasoning_effort_override,
        /*refreshing_rate_limits*/ false,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn new_status_output_with_rate_limits(
    config: &Config,
    account_display: Option<&StatusAccountDisplay>,
    token_info: Option<&TokenUsageInfo>,
    total_usage: &TokenUsage,
    session_id: &Option<ThreadId>,
    thread_name: Option<String>,
    forked_from: Option<ThreadId>,
    rate_limits: &[RateLimitSnapshotDisplay],
    _plan_type: Option<PlanType>,
    now: DateTime<Local>,
    model_name: &str,
    collaboration_mode: Option<&str>,
    reasoning_effort_override: Option<Option<ReasoningEffort>>,
    refreshing_rate_limits: bool,
) -> CompositeHistoryCell {
    new_status_output_with_rate_limits_handle(
        config,
        account_display,
        token_info,
        total_usage,
        session_id,
        thread_name,
        forked_from,
        rate_limits,
        _plan_type,
        now,
        model_name,
        collaboration_mode,
        reasoning_effort_override,
        "<none>".to_string(),
        refreshing_rate_limits,
    )
    .0
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn new_status_output_with_rate_limits_handle(
    config: &Config,
    account_display: Option<&StatusAccountDisplay>,
    token_info: Option<&TokenUsageInfo>,
    total_usage: &TokenUsage,
    session_id: &Option<ThreadId>,
    thread_name: Option<String>,
    forked_from: Option<ThreadId>,
    rate_limits: &[RateLimitSnapshotDisplay],
    _plan_type: Option<PlanType>,
    now: DateTime<Local>,
    model_name: &str,
    collaboration_mode: Option<&str>,
    reasoning_effort_override: Option<Option<ReasoningEffort>>,
    agents_summary: String,
    refreshing_rate_limits: bool,
) -> (CompositeHistoryCell, StatusHistoryHandle) {
    let command = PlainHistoryCell::new(vec!["/status".magenta().into()]);
    let (card, handle) = StatusHistoryCell::new(
        config,
        account_display,
        token_info,
        total_usage,
        session_id,
        thread_name,
        forked_from,
        rate_limits,
        _plan_type,
        now,
        model_name,
        collaboration_mode,
        reasoning_effort_override,
        agents_summary,
        refreshing_rate_limits,
    );

    (
        CompositeHistoryCell::new(vec![Box::new(command), Box::new(card)]),
        handle,
    )
}

impl StatusHistoryCell {
    fn advisor_preset_label(advisor_preset: &str) -> &'static str {
        match advisor_preset {
            "risk_first" => "risk-first",
            "plan_first" => "plan-first",
            _ => "review-first",
        }
    }

    fn pause_policy_label(pause_on_error: bool) -> &'static str {
        if pause_on_error {
            "pause on error"
        } else {
            "continue on error"
        }
    }

    fn knowledge_mode_label(mode: &str) -> &'static str {
        match mode {
            "worker" => "worker",
            "kairos" => "kairos",
            "both" => "both",
            _ => "off",
        }
    }

    fn team_merge_policy_label(team_merge_policy: &str) -> &str {
        match team_merge_policy {
            "leader_only" => "leader-only",
            "append_all" => "append-all",
            other => other,
        }
    }

    fn execution_loop_data() -> Option<StatusExecutionLoopData> {
        let runtime = native_runtime_context()?;
        let settings = runtime.settings_store.get();
        let agent = &settings.agent;

        let advisor_mode = format_runtime_mode_value(
            agent.advisor_mode,
            agent.advisor_mode.then(|| Self::advisor_preset_label(&agent.advisor_preset).to_string()),
            "/advisor",
        );

        let auto_mode = format_runtime_mode_value(
            agent.autonomous_mode,
            agent.autonomous_mode.then(|| {
                format!(
                    "{} turns, {}m, {}",
                    agent.auto_max_turns.max(1),
                    agent.auto_timebox_minutes.max(1),
                    Self::pause_policy_label(agent.auto_pause_on_error)
                )
            }),
            "/auto",
        );

        let team_mode = format_runtime_mode_value(
            agent.team_mode,
            agent.team_mode.then(|| {
                format!(
                    "{}, retries {}, {}",
                    Self::team_merge_policy_label(&agent.team_merge_policy),
                    agent.team_max_retries.max(1),
                    Self::pause_policy_label(agent.team_pause_on_error)
                )
            }),
            "/team",
        );

        let kairos = format_scoped_runtime_mode_value(
            agent.kairos_enabled,
            agent.task_scope
                .as_deref()
                .filter(|scope| !scope.trim().is_empty())
                .map(ToString::to_string),
            "/kairos",
        );

        let self_improvement = format_scoped_runtime_mode_value(
            agent.self_improvement_enabled,
            agent.memory_scope
                .as_deref()
                .filter(|scope| !scope.trim().is_empty())
                .map(ToString::to_string),
            "/improve",
        );

        let knowledge = if agent.knowledge_mode == "off" {
            "disabled".to_string()
        } else {
            let runtime = &agent.knowledge_runtime;
            let result = if runtime.last_result.trim().is_empty() {
                "idle".to_string()
            } else {
                runtime.last_result.clone()
            };
            let mut details = vec![
                format!("mode {}", Self::knowledge_mode_label(&agent.knowledge_mode)),
                format!("result {result}"),
            ];
            if let Some(driver) = runtime
                .last_driver
                .as_deref()
                .filter(|driver| !driver.trim().is_empty())
            {
                details.push(format!("driver {driver}"));
            }
            if let Some(workspace_id) = runtime
                .last_workspace_id
                .as_deref()
                .filter(|workspace_id| !workspace_id.trim().is_empty())
            {
                details.push(format!("workspace {workspace_id}"));
            }
            if runtime.last_issue_count > 0 {
                details.push(format!("issues {}", runtime.last_issue_count));
            }
            if let Some(run_reason) = runtime
                .last_run_reason
                .as_deref()
                .filter(|run_reason| !run_reason.trim().is_empty())
            {
                details.push(format!("trigger {run_reason}"));
            }
            format!("enabled ({})", details.join(", "))
        };

        Some(StatusExecutionLoopData {
            profile: format_runtime_profile_value(
                agent.active_profile.as_deref().unwrap_or("default"),
            ),
            advisor_mode,
            auto_mode,
            team_mode,
            kairos,
            self_improvement,
            knowledge,
        })
    }

    fn workflow_surface_data(cwd: &Path) -> StatusWorkflowSurfaceData {
        StatusWorkflowSurfaceData {
            tmux: if std::env::var_os("TMUX").is_some() {
                "on".to_string()
            } else {
                "off".to_string()
            },
            worktree: Self::workflow_surface_worktree_status(cwd).to_string(),
            remote: if native_runtime_context().is_some() {
                "native".to_string()
            } else {
                "remote".to_string()
            },
        }
    }

    fn workflow_surface_worktree_status(cwd: &Path) -> &'static str {
        let Some(repo_root) = get_git_repo_root(cwd) else {
            return "none";
        };
        let Some(trust_root) = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(resolve_root_git_project_for_trust(cwd))) else {
            return "repo";
        };

        if repo_root == trust_root {
            "repo"
        } else {
            "linked"
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        config: &Config,
        account_display: Option<&StatusAccountDisplay>,
        token_info: Option<&TokenUsageInfo>,
        total_usage: &TokenUsage,
        session_id: &Option<ThreadId>,
        thread_name: Option<String>,
        forked_from: Option<ThreadId>,
        rate_limits: &[RateLimitSnapshotDisplay],
        _plan_type: Option<PlanType>,
        now: DateTime<Local>,
        model_name: &str,
        collaboration_mode: Option<&str>,
        reasoning_effort_override: Option<Option<ReasoningEffort>>,
        agents_summary: String,
        refreshing_rate_limits: bool,
    ) -> (Self, StatusHistoryHandle) {
        let mut config_entries = vec![
            ("workdir", config.cwd.display().to_string()),
            ("model", model_name.to_string()),
            ("provider", config.model_provider_id.clone()),
            (
                "approval",
                config.permissions.approval_policy.value().to_string(),
            ),
            (
                "sandbox",
                summarize_sandbox_policy(config.permissions.sandbox_policy.get()),
            ),
        ];
        if config.model_provider.wire_api == WireApi::Responses {
            let effort_value = reasoning_effort_override
                .unwrap_or(None)
                .map(|effort| effort.to_string())
                .unwrap_or_else(|| "none".to_string());
            config_entries.push(("reasoning effort", effort_value));
            config_entries.push((
                "reasoning summaries",
                config
                    .model_reasoning_summary
                    .map(|summary| summary.to_string())
                    .unwrap_or_else(|| "auto".to_string()),
            ));
        }
        let (model_name, model_details) = compose_model_display(model_name, &config_entries);
        let approval = config_entries
            .iter()
            .find(|(k, _)| *k == "approval")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "<unknown>".to_string());
        let sandbox = match config.permissions.sandbox_policy.get() {
            SandboxPolicy::DangerFullAccess => "danger-full-access".to_string(),
            SandboxPolicy::ReadOnly { .. } => "read-only".to_string(),
            SandboxPolicy::WorkspaceWrite {
                network_access: true,
                ..
            } => "workspace-write with network access".to_string(),
            SandboxPolicy::WorkspaceWrite { .. } => "workspace-write".to_string(),
            SandboxPolicy::ExternalSandbox { network_access } => {
                if matches!(network_access, NetworkAccess::Enabled) {
                    "external-sandbox (network access enabled)".to_string()
                } else {
                    "external-sandbox".to_string()
                }
            }
        };
        let permissions = if config.permissions.approval_policy.value() == AskForApproval::OnRequest
            && *config.permissions.sandbox_policy.get()
                == SandboxPolicy::new_workspace_write_policy()
        {
            "Default".to_string()
        } else if config.permissions.approval_policy.value() == AskForApproval::Never
            && *config.permissions.sandbox_policy.get() == SandboxPolicy::DangerFullAccess
        {
            "Full Access".to_string()
        } else {
            format!("Custom ({sandbox}, {approval})")
        };
        let agents_summary = compose_agents_summary(config, &[]);
        let execution_loop = Self::execution_loop_data();
        let workflow_surface = Self::workflow_surface_data(&config.cwd);
        let model_provider = format_model_provider(config);
        let account = compose_account_display(account_display);
        let session_id = session_id.as_ref().map(std::string::ToString::to_string);
        let forked_from = forked_from.map(|id| id.to_string());
        let default_usage = TokenUsage::default();
        let (context_usage, context_window) = match token_info {
            Some(info) => (&info.last_token_usage, info.model_context_window),
            None => (&default_usage, config.model_context_window),
        };
        let context_window = context_window.map(|window| StatusContextWindowData {
            percent_remaining: context_usage.percent_of_context_window_remaining(window),
            tokens_in_context: context_usage.tokens_in_context_window(),
            window,
        });

        let token_usage = StatusTokenUsageData {
            total: total_usage.blended_total(),
            input: total_usage.non_cached_input(),
            output: total_usage.output_tokens,
            context_window,
        };
        let rate_limits = if rate_limits.len() <= 1 {
            compose_rate_limit_data(rate_limits.first(), now)
        } else {
            compose_rate_limit_data_many(rate_limits, now)
        };
        let rate_limit_state = Arc::new(RwLock::new(StatusRateLimitState {
            rate_limits,
            refreshing_rate_limits,
        }));
        let agents_summary = Arc::new(RwLock::new(agents_summary));

        (
            Self {
                model_name,
                model_details,
                directory: config.cwd.to_path_buf(),
                permissions,
                collaboration_mode: collaboration_mode.map(ToString::to_string),
                workflow_surface,
                execution_loop,
                model_provider,
                account,
                thread_name,
                session_id,
                forked_from,
                token_usage,
                agents_summary,
                rate_limit_state: rate_limit_state.clone(),
            },
            StatusHistoryHandle { rate_limit_state },
        )
    }

    fn token_usage_spans(&self) -> Vec<Span<'static>> {
        let total_fmt = format_tokens_compact(self.token_usage.total);
        let input_fmt = format_tokens_compact(self.token_usage.input);
        let output_fmt = format_tokens_compact(self.token_usage.output);

        vec![
            Span::from(total_fmt),
            Span::from(" total "),
            Span::from(" (").dim(),
            Span::from(input_fmt).dim(),
            Span::from(" input").dim(),
            Span::from(" + ").dim(),
            Span::from(output_fmt).dim(),
            Span::from(" output").dim(),
            Span::from(")").dim(),
        ]
    }

    fn context_window_spans(&self) -> Option<Vec<Span<'static>>> {
        let context = self.token_usage.context_window.as_ref()?;
        let percent = context.percent_remaining;
        let used_fmt = format_tokens_compact(context.tokens_in_context);
        let window_fmt = format_tokens_compact(context.window);

        Some(vec![
            Span::from(format!("{percent}% left")),
            Span::from(" (").dim(),
            Span::from(used_fmt).dim(),
            Span::from(" used / ").dim(),
            Span::from(window_fmt).dim(),
            Span::from(")").dim(),
        ])
    }

    fn rate_limit_lines(
        &self,
        state: &StatusRateLimitState,
        available_inner_width: usize,
        formatter: &FieldFormatter,
    ) -> Vec<Line<'static>> {
        match &state.rate_limits {
            StatusRateLimitData::Available(rows_data) => {
                if rows_data.is_empty() {
                    return vec![formatter.line(
                        "Limits",
                        vec![Span::from("not available for this account").dim()],
                    )];
                }

                self.rate_limit_row_lines(rows_data, available_inner_width, formatter)
            }
            StatusRateLimitData::Stale(rows_data) => {
                let mut lines =
                    self.rate_limit_row_lines(rows_data, available_inner_width, formatter);
                lines.push(formatter.line(
                    "Warning",
                    vec![Span::from(if state.refreshing_rate_limits {
                        "limits may be stale - run /status again shortly."
                    } else {
                        "limits may be stale - start new turn to refresh."
                    })
                    .dim()],
                ));
                lines
            }
            StatusRateLimitData::Unavailable => {
                vec![formatter.line(
                    "Limits",
                    vec![Span::from("not available for this account").dim()],
                )]
            }
            StatusRateLimitData::Missing => {
                vec![formatter.line(
                    "Limits",
                    vec![Span::from(if state.refreshing_rate_limits {
                        "refresh requested; run /status again shortly."
                    } else {
                        "data not available yet"
                    })
                    .dim()],
                )]
            }
        }
    }

    fn rate_limit_row_lines(
        &self,
        rows: &[StatusRateLimitRow],
        available_inner_width: usize,
        formatter: &FieldFormatter,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(rows.len().saturating_mul(2));

        for row in rows {
            match &row.value {
                StatusRateLimitValue::Window {
                    percent_used,
                    resets_at,
                } => {
                    let percent_remaining = (100.0 - percent_used).clamp(0.0, 100.0);
                    let summary = format_status_limit_summary(percent_remaining);
                    let full_value_spans = vec![
                        Span::from(render_status_limit_progress_bar(percent_remaining)),
                        Span::from(" "),
                        Span::from(summary.clone()),
                    ];
                    // On narrow terminals, keep the percentage visible rather than
                    // letting the fixed-width progress bar crowd out the reset time.
                    let value_spans = if line_display_width(&Line::from(full_value_spans.clone()))
                        <= formatter.value_width(available_inner_width)
                    {
                        full_value_spans
                    } else {
                        vec![Span::from(summary)]
                    };
                    let base_spans = formatter.full_spans(row.label.as_str(), value_spans);
                    let base_line = Line::from(base_spans.clone());

                    if let Some(resets_at) = resets_at.as_ref() {
                        let resets_span = Span::from(format!("(resets {resets_at})")).dim();
                        let mut inline_spans = base_spans.clone();
                        inline_spans.push(Span::from(" ").dim());
                        inline_spans.push(resets_span.clone());

                        if line_display_width(&Line::from(inline_spans.clone()))
                            <= available_inner_width
                        {
                            lines.push(Line::from(inline_spans));
                        } else {
                            lines.push(base_line);
                            let reset_text = format!("(resets {resets_at})");
                            let reset_width = formatter.value_width(available_inner_width).max(1);
                            let wrap_options =
                                textwrap::Options::new(reset_width).break_words(false);
                            // Reset timestamps are the actionable part of this row, so wrap them
                            // onto continuation lines instead of truncating partial times/dates.
                            lines.extend(
                                textwrap::wrap(reset_text.as_str(), wrap_options)
                                    .into_iter()
                                    .map(|wrapped| {
                                        formatter.continuation(vec![
                                            Span::from(wrapped.into_owned()).dim(),
                                        ])
                                    }),
                            );
                        }
                    } else {
                        lines.push(base_line);
                    }
                }
                StatusRateLimitValue::Text(text) => {
                    let label = row.label.clone();
                    let spans =
                        formatter.full_spans(label.as_str(), vec![Span::from(text.clone())]);
                    lines.push(Line::from(spans));
                }
            }
        }

        lines
    }

    fn collect_rate_limit_labels(
        &self,
        state: &StatusRateLimitState,
        seen: &mut BTreeSet<String>,
        labels: &mut Vec<String>,
    ) {
        match &state.rate_limits {
            StatusRateLimitData::Available(rows) => {
                if rows.is_empty() {
                    push_label(labels, seen, "Limits");
                } else {
                    for row in rows {
                        push_label(labels, seen, row.label.as_str());
                    }
                }
            }
            StatusRateLimitData::Stale(rows) => {
                for row in rows {
                    push_label(labels, seen, row.label.as_str());
                }
                push_label(labels, seen, "Warning");
            }
            StatusRateLimitData::Unavailable => push_label(labels, seen, "Limits"),
            StatusRateLimitData::Missing => push_label(labels, seen, "Limits"),
        }
    }
}

impl HistoryCell for StatusHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(vec![
            Span::from(format!("{}>_ ", FieldFormatter::INDENT)).dim(),
            Span::from("Ilhae").bold(),
            Span::from(" ").dim(),
            Span::from(format!("(v{CODEX_CLI_VERSION})")).dim(),
        ]));
        lines.push(Line::from(Vec::<Span<'static>>::new()));

        let available_inner_width = usize::from(width.saturating_sub(4));
        if available_inner_width == 0 {
            return Vec::new();
        }

        let account_value = self.account.as_ref().map(|account| match account {
            StatusAccountDisplay::ChatGpt { email, plan } => match (email, plan) {
                (Some(email), Some(plan)) => format!("{email} ({plan})"),
                (Some(email), None) => email.clone(),
                (None, Some(plan)) => plan.clone(),
                (None, None) => "ChatGPT".to_string(),
            },
            StatusAccountDisplay::ApiKey => {
                "API key configured (run codex login to use ChatGPT)".to_string()
            }
        });

        let mut labels: Vec<String> = vec!["Model", "Directory", "Permissions", "Agents.md"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut seen: BTreeSet<String> = labels.iter().cloned().collect();
        let thread_name = self.thread_name.as_deref().filter(|name| !name.is_empty());
        #[expect(clippy::expect_used)]
        let rate_limit_state = self
            .rate_limit_state
            .read()
            .expect("status history rate-limit state poisoned");
        #[expect(clippy::expect_used)]
        let agents_summary = self
            .agents_summary
            .read()
            .expect("status history agents summary state poisoned")
            .clone();

        if self.model_provider.is_some() {
            push_label(&mut labels, &mut seen, "Model provider");
        }
        if account_value.is_some() {
            push_label(&mut labels, &mut seen, "Account");
        }
        if thread_name.is_some() {
            push_label(&mut labels, &mut seen, "Thread name");
        }
        if self.session_id.is_some() {
            push_label(&mut labels, &mut seen, "Session");
        }
        if self.session_id.is_some() && self.forked_from.is_some() {
            push_label(&mut labels, &mut seen, "Forked from");
        }
        if self.collaboration_mode.is_some() {
            push_label(&mut labels, &mut seen, "Collaboration mode");
        }
        push_label(&mut labels, &mut seen, "TMUX");
        push_label(&mut labels, &mut seen, "Worktree");
        push_label(&mut labels, &mut seen, "Remote");
        if self.execution_loop.is_some() {
            push_label(&mut labels, &mut seen, "Runtime profile");
            push_label(&mut labels, &mut seen, "Advisor");
            push_label(&mut labels, &mut seen, "Auto mode");
            push_label(&mut labels, &mut seen, "Team mode");
            push_label(&mut labels, &mut seen, "Kairos");
            push_label(&mut labels, &mut seen, "Self-improvement");
            push_label(&mut labels, &mut seen, "Knowledge loop");
        }
        push_label(&mut labels, &mut seen, "Token usage");
        if self.token_usage.context_window.is_some() {
            push_label(&mut labels, &mut seen, "Context window");
        }

        self.collect_rate_limit_labels(&rate_limit_state, &mut seen, &mut labels);

        let formatter = FieldFormatter::from_labels(labels.iter().map(String::as_str));
        let value_width = formatter.value_width(available_inner_width);

        let note_first_line = Line::from(vec![
            Span::from("Visit ").cyan(),
            "https://chatgpt.com/codex/settings/usage"
                .cyan()
                .underlined(),
            Span::from(" for up-to-date").cyan(),
        ]);
        let note_second_line = Line::from(vec![
            Span::from("information on rate limits and credits").cyan(),
        ]);
        let note_lines = adaptive_wrap_lines(
            [note_first_line, note_second_line],
            RtOptions::new(available_inner_width),
        );
        lines.extend(note_lines);
        lines.push(Line::from(Vec::<Span<'static>>::new()));

        let mut model_spans = vec![Span::from(self.model_name.clone())];
        if !self.model_details.is_empty() {
            model_spans.push(Span::from(" (").dim());
            model_spans.push(Span::from(self.model_details.join(", ")).dim());
            model_spans.push(Span::from(")").dim());
        }

        let directory_value = format_directory_display(&self.directory, Some(value_width));

        lines.push(formatter.line("Model", model_spans));
        if let Some(model_provider) = self.model_provider.as_ref() {
            lines.push(formatter.line("Model provider", vec![Span::from(model_provider.clone())]));
        }
        lines.push(formatter.line("Directory", vec![Span::from(directory_value)]));
        lines.push(formatter.line("Permissions", vec![Span::from(self.permissions.clone())]));
        lines.push(formatter.line("Agents.md", vec![Span::from(agents_summary)]));

        if let Some(account_value) = account_value {
            lines.push(formatter.line("Account", vec![Span::from(account_value)]));
        }

        if let Some(thread_name) = thread_name {
            lines.push(formatter.line("Thread name", vec![Span::from(thread_name.to_string())]));
        }
        if let Some(collab_mode) = self.collaboration_mode.as_ref() {
            lines.push(formatter.line("Collaboration mode", vec![Span::from(collab_mode.clone())]));
        }
        lines.push(formatter.line(
            "TMUX",
            vec![Span::from(self.workflow_surface.tmux.clone())],
        ));
        lines.push(formatter.line(
            "Worktree",
            vec![Span::from(self.workflow_surface.worktree.clone())],
        ));
        lines.push(formatter.line(
            "Remote",
            vec![Span::from(self.workflow_surface.remote.clone())],
        ));
        if let Some(execution_loop) = self.execution_loop.as_ref() {
            lines.push(formatter.line(
                "Runtime profile",
                vec![Span::from(execution_loop.profile.clone())],
            ));
            lines.push(formatter.line(
                "Advisor",
                vec![Span::from(execution_loop.advisor_mode.clone())],
            ));
            lines.push(formatter.line(
                "Auto mode",
                vec![Span::from(execution_loop.auto_mode.clone())],
            ));
            lines.push(formatter.line(
                "Team mode",
                vec![Span::from(execution_loop.team_mode.clone())],
            ));
            lines.push(formatter.line(
                "Kairos",
                vec![Span::from(execution_loop.kairos.clone())],
            ));
            lines.push(formatter.line(
                "Self-improvement",
                vec![Span::from(execution_loop.self_improvement.clone())],
            ));
            lines.push(formatter.line(
                "Knowledge loop",
                vec![Span::from(execution_loop.knowledge.clone())],
            ));
        }
        if let Some(session) = self.session_id.as_ref() {
            lines.push(formatter.line("Session", vec![Span::from(session.clone())]));
        }
        if self.session_id.is_some()
            && let Some(forked_from) = self.forked_from.as_ref()
        {
            lines.push(formatter.line("Forked from", vec![Span::from(forked_from.clone())]));
        }

        lines.push(Line::from(Vec::<Span<'static>>::new()));
        // Hide token usage only for ChatGPT subscribers
        if !matches!(self.account, Some(StatusAccountDisplay::ChatGpt { .. })) {
            lines.push(formatter.line("Token usage", self.token_usage_spans()));
        }

        if let Some(spans) = self.context_window_spans() {
            lines.push(formatter.line("Context window", spans));
        }

        lines.extend(self.rate_limit_lines(&rate_limit_state, available_inner_width, &formatter));

        let content_width = lines.iter().map(line_display_width).max().unwrap_or(0);
        let inner_width = content_width.min(available_inner_width);
        let truncated_lines: Vec<Line<'static>> = lines
            .into_iter()
            .map(|line| truncate_line_to_width(line, inner_width))
            .collect();

        with_border_with_inner_width(truncated_lines, inner_width)
    }
}

fn format_model_provider(config: &Config) -> Option<String> {
    let provider = &config.model_provider;
    let name = provider.name.trim();
    let provider_name = if name.is_empty() {
        config.model_provider_id.as_str()
    } else {
        name
    };
    let base_url = provider.base_url.as_deref().and_then(sanitize_base_url);
    let is_default_openai = provider.is_openai() && base_url.is_none();
    if is_default_openai {
        return None;
    }

    Some(match base_url {
        Some(base_url) => format!("{provider_name} - {base_url}"),
        None => provider_name.to_string(),
    })
}

fn sanitize_base_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let Ok(mut url) = Url::parse(trimmed) else {
        return None;
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string().trim_end_matches('/').to_string()).filter(|value| !value.is_empty())
}

fn format_runtime_profile_value(profile: &str) -> String {
    format!("{profile} (use /profile)")
}

fn format_runtime_mode_value(enabled: bool, detail: Option<String>, command: &str) -> String {
    if !enabled {
        return format!("disabled (use {command})");
    }

    match detail {
        Some(detail) => format!("enabled ({detail}; use {command})"),
        None => format!("enabled (use {command})"),
    }
}

fn format_scoped_runtime_mode_value(
    enabled: bool,
    scope: Option<String>,
    command: &str,
) -> String {
    if !enabled {
        return format!("disabled (use {command})");
    }

    match scope {
        Some(scope) => format!("enabled (scope: {scope}; use {command})"),
        None => format!("enabled (use {command})"),
    }
}

#[cfg(test)]
fn format_workflow_surface_value(tmux: &str, worktree: &str, remote: &str) -> String {
    format!("wf:tmux:{tmux} worktree:{worktree} remote:{remote}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_control_hints_are_spelled_out_in_status_values() {
        assert_eq!(
            format_runtime_profile_value("default"),
            "default (use /profile)"
        );
        assert_eq!(
            format_runtime_mode_value(
                /*enabled*/ true,
                Some("risk-first".to_string()),
                "/advisor"
            ),
            "enabled (risk-first; use /advisor)"
        );
        assert_eq!(
            format_runtime_mode_value(/*enabled*/ false, None, "/auto"),
            "disabled (use /auto)"
        );
        assert_eq!(
            format_scoped_runtime_mode_value(
                /*enabled*/ true,
                Some("repo".to_string()),
                "/kairos"
            ),
            "enabled (scope: repo; use /kairos)"
        );
        assert_eq!(
            format_scoped_runtime_mode_value(/*enabled*/ false, None, "/improve"),
            "disabled (use /improve)"
        );
    }

    #[test]
    fn workflow_surface_summary_is_structured() {
        assert_eq!(
            format_workflow_surface_value("on", "linked", "native"),
            "wf:tmux:on worktree:linked remote:native"
        );
    }
}
