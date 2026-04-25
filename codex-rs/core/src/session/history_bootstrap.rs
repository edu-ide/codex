use tracing::warn;

use super::PreviousTurnSettings;
use super::Session;
use super::TurnContext;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::TokenUsageInfo;
use codex_protocol::protocol::WarningEvent;

impl Session {
    pub(crate) async fn record_initial_history(&self, conversation_history: InitialHistory) {
        let turn_context = self.new_default_turn().await;
        let is_subagent = {
            let state = self.state.lock().await;
            matches!(
                state.session_configuration.session_source,
                SessionSource::SubAgent(_)
            )
        };

        match conversation_history {
            InitialHistory::New | InitialHistory::Cleared => {
                // Defer initial context insertion until the first real turn starts so
                // turn/start overrides can be merged before we write model-visible context.
                self.set_previous_turn_settings(/*previous_turn_settings*/ None)
                    .await;
            }
            InitialHistory::Resumed(resumed_history) => {
                let rollout_items = resumed_history.history;
                let previous_turn_settings = self
                    .apply_rollout_reconstruction(&turn_context, &rollout_items)
                    .await;

                // If resuming, warn when the last recorded model differs from the current one.
                let curr: &str = turn_context.model_info.slug.as_str();
                if let Some(prev) = previous_turn_settings
                    .as_ref()
                    .map(|settings| settings.model.as_str())
                    .filter(|model| *model != curr)
                {
                    warn!("resuming session with different model: previous={prev}, current={curr}");
                    self.send_event(
                        &turn_context,
                        EventMsg::Warning(WarningEvent {
                            message: format!(
                                "This session was recorded with model `{prev}` but is resuming with `{curr}`. \
                         Consider switching back to `{prev}` as it may affect Codex performance."
                            ),
                        }),
                    )
                    .await;
                }

                // Seed usage info from the recorded rollout so UIs can show token counts
                // immediately on resume/fork.
                if let Some(info) = Self::last_token_info_from_rollout(&rollout_items) {
                    let mut state = self.state.lock().await;
                    state.set_token_info(Some(info));
                }

                // Defer seeding the session's initial context until the first turn starts so
                // turn/start overrides can be merged before we write to the rollout.
                if !is_subagent {
                    let _ = self.flush_rollout().await;
                }
            }
            InitialHistory::Forked(rollout_items) => {
                self.apply_rollout_reconstruction(&turn_context, &rollout_items)
                    .await;

                // Seed usage info from the recorded rollout so UIs can show token counts
                // immediately on resume/fork.
                if let Some(info) = Self::last_token_info_from_rollout(&rollout_items) {
                    let mut state = self.state.lock().await;
                    state.set_token_info(Some(info));
                }

                // If persisting, persist all rollout items as-is (recorder filters)
                if !rollout_items.is_empty() {
                    self.persist_rollout_items(&rollout_items).await;
                }

                // Forked threads should remain file-backed immediately after startup.
                self.ensure_rollout_materialized().await;

                // Flush after seeding history and any persisted rollout copy.
                if !is_subagent {
                    let _ = self.flush_rollout().await;
                }
            }
        }
    }

    pub(crate) async fn apply_rollout_reconstruction(
        &self,
        turn_context: &TurnContext,
        rollout_items: &[RolloutItem],
    ) -> Option<PreviousTurnSettings> {
        let reconstructed_rollout = self
            .reconstruct_history_from_rollout(turn_context, rollout_items)
            .await;
        let previous_turn_settings = reconstructed_rollout.previous_turn_settings.clone();
        self.replace_history(
            reconstructed_rollout.history,
            reconstructed_rollout.reference_context_item,
        )
        .await;
        self.set_previous_turn_settings(previous_turn_settings.clone())
            .await;
        previous_turn_settings
    }

    fn last_token_info_from_rollout(rollout_items: &[RolloutItem]) -> Option<TokenUsageInfo> {
        rollout_items.iter().rev().find_map(|item| match item {
            RolloutItem::EventMsg(EventMsg::TokenCount(ev)) => ev.info.clone(),
            _ => None,
        })
    }

    pub(crate) async fn previous_turn_settings(&self) -> Option<PreviousTurnSettings> {
        let state = self.state.lock().await;
        state.previous_turn_settings()
    }

    pub(crate) async fn set_previous_turn_settings(
        &self,
        previous_turn_settings: Option<PreviousTurnSettings>,
    ) {
        let mut state = self.state.lock().await;
        state.set_previous_turn_settings(previous_turn_settings);
    }
}
