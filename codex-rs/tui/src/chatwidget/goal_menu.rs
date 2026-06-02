//! Goal summary for the bare `/goal` command.

use super::*;
use crate::goal_display::format_goal_elapsed_seconds;
use crate::status::format_tokens_compact;
use codex_app_server_protocol::ThreadGoalLoopHistoryEntry as AppThreadGoalLoopHistoryEntry;
use codex_app_server_protocol::ThreadGoalLoopPhase as AppThreadGoalLoopPhase;
use codex_app_server_protocol::ThreadGoalLoopStatus as AppThreadGoalLoopStatus;

impl ChatWidget {
    pub(crate) fn show_goal_summary(&mut self, goal: AppThreadGoal) {
        self.add_plain_history_lines(goal_summary_lines(&goal));
    }

    pub(crate) fn show_goal_loop_update(&mut self, goal: &AppThreadGoal) {
        if self
            .thread_id
            .is_some_and(|active_thread_id| active_thread_id.to_string() != goal.thread_id)
        {
            return;
        }
        let Some(loop_state) = goal.loop_state.as_ref() else {
            return;
        };
        if self
            .current_goal_status
            .as_ref()
            .is_some_and(|state| state.has_same_loop_state(goal))
        {
            return;
        }
        self.add_info_message(
            format!("Goal loop {}", goal_loop_status_label(loop_state.status)),
            Some(format!(
                "cycle {} - {}: {}",
                loop_state.cycle_number,
                goal_loop_phase_label(loop_state.phase),
                loop_state.summary
            )),
        );
    }

    pub(crate) fn show_goal_edit_prompt(&mut self, thread_id: ThreadId, goal: AppThreadGoal) {
        let tx = self.app_event_tx.clone();
        let status = edited_goal_status(goal.status);
        let token_budget = goal.token_budget;
        let view = CustomPromptView::new(
            "Edit goal".to_string(),
            "Type a goal objective and press Enter".to_string(),
            goal.objective,
            /*context_label*/ None,
            Box::new(move |objective: String| {
                tx.send(AppEvent::SetThreadGoalObjective {
                    thread_id,
                    objective,
                    mode: crate::app_event::ThreadGoalSetMode::UpdateExisting {
                        status,
                        token_budget,
                    },
                    superloop_enabled: None,
                });
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
    }

    pub(crate) fn show_resume_paused_goal_prompt(
        &mut self,
        thread_id: ThreadId,
        objective: String,
    ) {
        let resume_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::SetThreadGoalStatus {
                thread_id,
                status: AppThreadGoalStatus::Active,
            });
        })];
        self.show_selection_view(SelectionViewParams {
            title: Some("Resume paused goal?".to_string()),
            subtitle: Some(format!("Goal: {objective}")),
            footer_hint: Some(standard_popup_hint_line()),
            initial_selected_idx: Some(0),
            items: vec![
                SelectionItem {
                    name: "Resume goal".to_string(),
                    description: Some("Mark it active and continue when idle".to_string()),
                    actions: resume_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Leave paused".to_string(),
                    description: Some("Keep it paused; use /goal resume later".to_string()),
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
    }

    pub(crate) fn on_thread_goal_cleared(&mut self, thread_id: &str) {
        if self
            .thread_id
            .is_some_and(|active_thread_id| active_thread_id.to_string() == thread_id)
        {
            self.current_goal_status = None;
            self.update_collaboration_mode_indicator();
        }
    }
}

fn goal_summary_lines(goal: &AppThreadGoal) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from("Goal".bold()),
        Line::from(vec![
            "Status: ".dim(),
            goal_status_label(goal.status).to_string().into(),
        ]),
        Line::from(vec![
            "Super loop: ".dim(),
            if goal.superloop_enabled { "on" } else { "off" }.into(),
        ]),
        Line::from(vec!["Objective: ".dim(), goal.objective.clone().into()]),
        Line::from(vec![
            "Time used: ".dim(),
            format_goal_elapsed_seconds(goal.time_used_seconds).into(),
        ]),
        Line::from(vec![
            "Tokens used: ".dim(),
            format_tokens_compact(goal.tokens_used).into(),
        ]),
    ];
    if let Some(token_budget) = goal.token_budget {
        lines.push(Line::from(vec![
            "Token budget: ".dim(),
            format_tokens_compact(token_budget).into(),
        ]));
    }
    if let Some(loop_state) = goal.loop_state.as_ref() {
        lines.push(Line::from(vec![
            "Loop: ".dim(),
            format!(
                "cycle {} - {} - {}",
                loop_state.cycle_number,
                goal_loop_phase_label(loop_state.phase),
                goal_loop_status_label(loop_state.status)
            )
            .into(),
        ]));
        lines.push(Line::from(vec![
            "Last loop: ".dim(),
            loop_state.summary.clone().into(),
        ]));
    }
    if !goal.loop_history.is_empty() {
        lines.push(Line::from("Loop history".bold()));
        for entry in goal.loop_history.iter().take(3) {
            lines.extend(goal_loop_history_lines(entry));
        }
    }
    let command_hint = match goal.status {
        AppThreadGoalStatus::Active => {
            "Commands: /goal edit, /goal pause, /superloop <objective>, /goal clear"
        }
        AppThreadGoalStatus::Paused
        | AppThreadGoalStatus::Blocked
        | AppThreadGoalStatus::UsageLimited => {
            "Commands: /goal edit, /goal resume, /superloop <objective>, /goal clear"
        }
        AppThreadGoalStatus::BudgetLimited | AppThreadGoalStatus::Complete => {
            "Commands: /goal edit, /goal clear"
        }
    };
    lines.push(Line::default());
    lines.push(Line::from(command_hint.dim()));
    lines
}

fn goal_loop_history_lines(entry: &AppThreadGoalLoopHistoryEntry) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        format!(
            "  #{} {} {}: ",
            entry.cycle_number,
            goal_loop_phase_label(entry.phase),
            goal_loop_status_label(entry.status)
        )
        .dim(),
        entry.summary.clone().into(),
    ])];
    if let Some(detail) = entry
        .detail
        .as_ref()
        .filter(|detail| !detail.trim().is_empty())
    {
        lines.push(Line::from(vec!["    ".dim(), detail.clone().dim()]));
    }
    if let Some(error) = entry
        .error
        .as_ref()
        .filter(|error| !error.trim().is_empty())
    {
        lines.push(Line::from(vec!["    Error: ".red(), error.clone().red()]));
    }
    lines
}

fn goal_loop_phase_label(phase: AppThreadGoalLoopPhase) -> &'static str {
    match phase {
        AppThreadGoalLoopPhase::KnowledgeLoop => "knowledge loop",
        AppThreadGoalLoopPhase::KairosLoop => "kairos loop",
        AppThreadGoalLoopPhase::SuperLoop => "super loop",
        AppThreadGoalLoopPhase::PlanLoop => "plan loop",
        AppThreadGoalLoopPhase::BrainResearchLoop => "brain research loop",
        AppThreadGoalLoopPhase::CodebaseResearchLoop => "codebase research loop",
        AppThreadGoalLoopPhase::AgentSkillResearchLoop => "agent skill research loop",
        AppThreadGoalLoopPhase::WebResearchLoop => "web research loop",
        AppThreadGoalLoopPhase::ResearchLoop => "research loop",
        AppThreadGoalLoopPhase::DecisionLoop => "decision loop",
        AppThreadGoalLoopPhase::WikiLoop => "wiki loop",
        AppThreadGoalLoopPhase::LogLoop => "log loop",
        AppThreadGoalLoopPhase::ImprovementLoop => "improvement loop",
        AppThreadGoalLoopPhase::CleanupLoop => "cleanup loop",
        AppThreadGoalLoopPhase::ExecutionLoop => "execution loop",
        AppThreadGoalLoopPhase::VerificationLoop => "verification loop",
        AppThreadGoalLoopPhase::ContextInjection => "context injection",
    }
}

fn goal_loop_status_label(status: AppThreadGoalLoopStatus) -> &'static str {
    match status {
        AppThreadGoalLoopStatus::InProgress => "in progress",
        AppThreadGoalLoopStatus::Completed => "completed",
        AppThreadGoalLoopStatus::Failed => "failed",
    }
}

fn goal_status_label(status: AppThreadGoalStatus) -> &'static str {
    match status {
        AppThreadGoalStatus::Active => "active",
        AppThreadGoalStatus::Paused => "paused",
        AppThreadGoalStatus::Blocked => "blocked",
        AppThreadGoalStatus::UsageLimited => "usage limited",
        AppThreadGoalStatus::BudgetLimited => "limited by budget",
        AppThreadGoalStatus::Complete => "complete",
    }
}

fn edited_goal_status(status: AppThreadGoalStatus) -> AppThreadGoalStatus {
    match status {
        AppThreadGoalStatus::Active => AppThreadGoalStatus::Active,
        AppThreadGoalStatus::Paused
        | AppThreadGoalStatus::Blocked
        | AppThreadGoalStatus::UsageLimited => status,
        AppThreadGoalStatus::BudgetLimited | AppThreadGoalStatus::Complete => {
            AppThreadGoalStatus::Active
        }
    }
}
