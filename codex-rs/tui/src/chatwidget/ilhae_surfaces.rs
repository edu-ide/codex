//! Ilhae-specific profile and workflow surfaces for `ChatWidget`.
//!
//! Keeping these native-runtime UI surfaces out of `chatwidget.rs` reduces the
//! upstream merge radius on the main TUI event/render loop while preserving the
//! existing call sites.

use super::*;
use codex_ilhae::native_runtime_context;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffortPreset;

impl ChatWidget {
    pub(super) fn ilhae_local_model_presets(&self) -> Option<Vec<ModelPreset>> {
        let Some(_) = native_runtime_context() else {
            return None;
        };

        let model = self.current_model().trim();
        if model.is_empty() {
            return None;
        }

        let effort = self
            .current_reasoning_effort()
            .unwrap_or(ReasoningEffortConfig::Medium);

        Some(vec![ModelPreset {
            id: model.to_string(),
            model: model.to_string(),
            display_name: model.to_string(),
            description: "Local native ilhae runtime".to_string(),
            default_reasoning_effort: effort,
            supported_reasoning_efforts: vec![ReasoningEffortPreset {
                effort,
                description: "Default local runtime reasoning level".to_string(),
            }],
            supports_personality: false,
            is_default: true,
            upgrade: None,
            show_in_picker: true,
            availability_nux: None,
            supported_in_api: true,
            input_modalities: vec![InputModality::Text, InputModality::Image],
            additional_speed_tiers: Vec::new(),
        }])
    }

    pub(super) fn open_ilhae_profile_popup(&mut self) {
        let Some(_) = codex_ilhae::native_runtime_context() else {
            self.add_error_message(
                "`/profile` is only supported in the native embedded ilhae runtime.".to_string(),
            );
            return;
        };

        let config = codex_ilhae::config::load_ilhae_toml_config();
        let active_profile = config
            .profile
            .active
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let initial_selected_idx = config
            .profiles
            .iter()
            .position(|(profile_id, _)| profile_id == &active_profile);
        let items: Vec<SelectionItem> = config
            .profiles
            .into_iter()
            .map(|(profile_id, profile)| {
                let mut summary_parts =
                    codex_ilhae::config::profile_runtime_display_parts(&profile);
                if profile.agent.advisor {
                    summary_parts.push(format!("advisor:{}", profile.agent.advisor_preset.trim()));
                }
                if profile.agent.auto_mode {
                    summary_parts.push("auto".to_string());
                }
                if profile.agent.team_mode {
                    summary_parts.push("team".to_string());
                }
                if profile.agent.kairos {
                    summary_parts.push("kairos".to_string());
                }
                if profile.agent.self_improvement {
                    summary_parts.push("improve".to_string());
                }
                let description = (!summary_parts.is_empty()).then(|| summary_parts.join("  "));
                let is_current = profile_id == active_profile;
                let item_name = profile_id.clone();
                let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                    tx.send(AppEvent::SetIlhaeRuntimeProfile {
                        profile_id: profile_id.clone(),
                    })
                })];
                SelectionItem {
                    name: item_name,
                    description,
                    is_current,
                    actions,
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Select Runtime Profile".bold()));
        header.push(
            Line::from(
                "Choose which ilhae profile should drive advisor, auto, team, kairos, and improve.",
            )
            .dim(),
        );

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            initial_selected_idx,
            ..Default::default()
        });
    }

    pub(super) fn cycle_ilhae_advisor_preset(&mut self) {
        let current = codex_ilhae::native_runtime_context()
            .map(|runtime| runtime.settings_store.get().agent.advisor_preset)
            .unwrap_or_else(codex_ilhae::settings_types::default_advisor_preset);
        let next = match current.trim() {
            "review_first" => "risk_first",
            "risk_first" => "plan_first",
            _ => "review_first",
        };
        self.app_event_tx.send(AppEvent::SetIlhaeAdvisorMode {
            enabled: Some(true),
            preset: Some(next.to_string()),
        });
    }

    pub(super) fn show_tmux_workflow_surface(&mut self) {
        let tmux_on = std::env::var_os("TMUX").is_some();
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push("Workflow surface: tmux".into());
        lines.push(Line::from(format!(
            "Status: {}",
            if tmux_on { "on" } else { "off" }
        )));
        lines.push(Line::from(self.workflow_surface_status_text()));

        if tmux_on {
            lines.push("Current session is already inside tmux.".into());
            lines.push(
                "Recommended next steps: split a pane, open a new window, or keep teammate work isolated per pane."
                    .into(),
            );
            lines.push(
                "Examples: tmux split-window -h | tmux split-window -v | tmux new-window -n ilhae-team"
                    .into(),
            );
        } else {
            lines.push("This session is not running inside tmux.".into());
            lines.push(
                "Recommended next step: start a tmux session before spawning teammate or long-running workflow work."
                    .into(),
            );
            lines.push("Example: tmux new-session -s ilhae".into());
        }

        self.add_plain_history_lines(lines);
    }

    pub(super) fn show_worktree_workflow_surface(&mut self) {
        let cwd = self.status_line_cwd();
        let worktree = Self::workflow_surface_worktree_status(cwd);
        let repo_root = get_git_repo_root(cwd);
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push("Workflow surface: worktree".into());
        lines.push(Line::from(format!("Status: {worktree}")));
        lines.push(Line::from(self.workflow_surface_status_text()));

        match repo_root {
            None => {
                lines.push("Current directory is not inside a git repository.".into());
                lines.push(
                    "Recommended next step: move into a repository before using worktree-isolated task execution."
                        .into(),
                );
            }
            Some(repo_root) => {
                lines.push(Line::from(format!("Repo root: {}", repo_root.display())));
                if worktree == "linked" {
                    lines.push(
                        "Current session is already running inside a linked worktree.".into(),
                    );
                    lines.push(
                        "Recommended next step: keep task-specific edits isolated here and avoid mixing them back into the main repo tree."
                            .into(),
                    );
                } else {
                    lines.push(
                        "Current session is running from the main repository checkout.".into(),
                    );
                    lines.push(
                        "Recommended next step: create a linked worktree for isolated task or teammate execution."
                            .into(),
                    );
                    let repo_name = repo_root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("repo");
                    lines.push(Line::from(format!(
                        "Example: git worktree add ../{repo_name}-task <new-branch>"
                    )));
                }
            }
        }

        self.add_plain_history_lines(lines);
    }

    pub(super) fn show_remote_workflow_surface(&mut self) {
        let remote = if native_runtime_context().is_some() {
            "native"
        } else {
            "remote"
        };
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push("Workflow surface: remote-control".into());
        lines.push(Line::from(format!("Status: {remote}")));
        lines.push(Line::from(self.workflow_surface_status_text()));

        if remote == "native" {
            lines.push("Current session is using the native embedded ilhae runtime.".into());
            lines.push(
                "Recommended next step: use desktop/mobile or a remote app-server connection when you need handoff or remote-control style operation."
                    .into(),
            );
        } else {
            lines.push(
                "Current session is already running against a remote workflow surface.".into(),
            );
            lines.push(
                "Recommended next step: keep long-running work attached here and use local clients only as control surfaces."
                    .into(),
            );
        }

        self.add_plain_history_lines(lines);
    }
}
