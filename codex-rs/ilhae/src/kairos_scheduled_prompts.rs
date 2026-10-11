//! Kairos hand-off of due scheduled requests to the Work follow-up queue.
//!
//! The Desktop host claims queued follow-ups and runs them in an agent thread, so a
//! request the user scheduled actually runs. Running is a UGOT Pro feature; on the
//! free plan, or whenever the plan cannot be confirmed, Kairos only notifies, as
//! the browser bridge does. Queueing grants no approval: sending, deleting and
//! paying still wait for a person in the running agent.

use brain_rs::schedule::ScheduleStore;
use brain_rs::schedule::ScheduleTrigger;
use brain_rs::schedule::ScheduledPromptOutcome;
use brain_rs::schedule::ScheduledPromptPlan;
use brain_rs::schedule::scheduled_prompt_notice;
use brain_rs::schedule::scheduled_prompts_need_plan;
use std::sync::Arc;
use tracing::info;
use tracing::warn;
use ugot_local_session::entitlement::UgotPlan;

/// Queue what may run and return the single notification line for this tick.
pub(crate) async fn dispatch(
    schedules: Arc<ScheduleStore>,
    triggered: &[ScheduleTrigger],
) -> String {
    let now = chrono::Utc::now();
    // pay is asked only when some trigger would actually run.
    let plan = if scheduled_prompts_need_plan(triggered, now) {
        match ugot_local_session::entitlement::shared()
            .current_plan()
            .await
        {
            UgotPlan::Pro { .. } => ScheduledPromptPlan::Pro,
            UgotPlan::Free { reason } => {
                info!("[Kairos] Scheduled requests stay notifications: {reason}");
                ScheduledPromptPlan::Free
            }
        }
    } else {
        ScheduledPromptPlan::Free
    };
    let owned = triggered.to_vec();
    let outcomes = match tokio::task::spawn_blocking(move || {
        schedules.dispatch_scheduled_prompts(&owned, now, plan)
    })
    .await
    {
        Ok(outcomes) => outcomes,
        Err(error) => {
            warn!("[Kairos] Scheduled request dispatch stopped: {error}");
            vec![
                ScheduledPromptOutcome::Failed {
                    error: error.to_string(),
                };
                triggered.len()
            ]
        }
    };
    for (trigger, outcome) in triggered.iter().zip(&outcomes) {
        match outcome {
            ScheduledPromptOutcome::Enqueued { followup_id } => info!(
                "[Kairos] Queued scheduled request {} ({}) as follow-up {followup_id}",
                trigger.schedule_id, trigger.scheduled_at
            ),
            ScheduledPromptOutcome::SkippedStale => info!(
                "[Kairos] Skipped scheduled request {}: {} is past the catch-up window",
                trigger.schedule_id, trigger.scheduled_at
            ),
            ScheduledPromptOutcome::Failed { error } => warn!(
                "[Kairos] Could not queue scheduled request {}: {error}",
                trigger.schedule_id
            ),
            _ => {}
        }
    }
    scheduled_prompt_notice(triggered, &outcomes)
}
