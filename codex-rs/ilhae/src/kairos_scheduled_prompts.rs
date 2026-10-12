//! Kairos hand-off of due scheduled requests to the Work follow-up queue.
//!
//! The Desktop host claims queued follow-ups and runs them in an agent thread, so a
//! request the user scheduled actually runs. Running is a UGOT Pro feature; on the
//! free plan, or whenever the plan cannot be confirmed, Kairos only notifies, as
//! the browser bridge does. Queueing grants no approval: sending, deleting and
//! paying still wait for a person in the running agent.
//!
//! Saving a request with a prompt is itself the wish to run it, so this does not
//! wait for the Kairos task loop setting. With the loop off, [`WithoutKairos`]
//! peeks the due requests without firing them, asks pay only when one is due, and
//! fires only those requests on UGOT Pro. On the free plan it fires nothing, so the
//! loop-off behaviour there is unchanged: no schedule is consumed and nothing is
//! announced.

use brain_rs::schedule::ScheduleStore;
use brain_rs::schedule::ScheduleTrigger;
use brain_rs::schedule::ScheduledPromptOutcome;
use brain_rs::schedule::ScheduledPromptPlan;
use brain_rs::schedule::scheduled_prompt_notice;
use brain_rs::schedule::scheduled_prompts_need_plan;
use chrono::DateTime;
use chrono::Utc;
use std::future::Future;
use std::sync::Arc;
use tracing::info;
use tracing::warn;
use ugot_local_session::entitlement::UgotPlan;

/// After a free verdict with the Kairos loop off, how long the loop leaves due
/// requests alone before asking pay again. The entitlement cache alone would ask
/// every five minutes for as long as the request stays due.
const FREE_RECHECK_MINUTES: i64 = 30;

/// The signed-in plan, asked of pay through the shared entitlement cache.
pub(crate) async fn account_plan() -> ScheduledPromptPlan {
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
}

/// Queue what may run and return the single notification line for this tick.
pub(crate) async fn dispatch(
    schedules: Arc<ScheduleStore>,
    triggered: &[ScheduleTrigger],
) -> String {
    let now = Utc::now();
    // pay is asked only when some trigger would actually run.
    let plan = if scheduled_prompts_need_plan(triggered, now) {
        account_plan().await
    } else {
        ScheduledPromptPlan::Free
    };
    dispatch_with_plan(schedules, triggered, now, plan).await
}

async fn dispatch_with_plan(
    schedules: Arc<ScheduleStore>,
    triggered: &[ScheduleTrigger],
    now: DateTime<Utc>,
    plan: ScheduledPromptPlan,
) -> String {
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

/// Runs scheduled requests while the Kairos task loop is off. Reminders and
/// calendar browser batches are left to the loop, exactly as before.
#[derive(Debug, Default)]
pub(crate) struct WithoutKairos {
    quiet_until: Option<DateTime<Utc>>,
}

impl WithoutKairos {
    /// One loop tick. Returns the notification line when requests were fired.
    pub(crate) async fn tick<F, Fut>(
        &mut self,
        schedules: Arc<ScheduleStore>,
        task_scope: Option<String>,
        now: DateTime<Utc>,
        plan: F,
    ) -> Option<String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ScheduledPromptPlan>,
    {
        if self.quiet_until.is_some_and(|until| now < until) {
            return None;
        }
        let peeked = {
            let schedules = schedules.clone();
            let scope = task_scope.clone();
            tokio::task::spawn_blocking(move || {
                schedules.try_peek_scheduled_prompts_with_scope(scope.as_deref(), now)
            })
            .await
        };
        let peeked = match peeked {
            Ok(Ok(peeked)) => peeked,
            Ok(Err(error)) => {
                warn!("[Kairos] Could not read due scheduled requests: {error}");
                return None;
            }
            Err(error) => {
                warn!("[Kairos] Due scheduled request check stopped: {error}");
                return None;
            }
        };
        // Nothing would run: no pay request, nothing consumed.
        if !scheduled_prompts_need_plan(&peeked, now) {
            return None;
        }
        if plan().await != ScheduledPromptPlan::Pro {
            // Free or unconfirmed: leave the schedules untouched, as the loop-off
            // host always did, and stop asking pay for a while.
            self.quiet_until = Some(now + chrono::Duration::minutes(FREE_RECHECK_MINUTES));
            return None;
        }
        self.quiet_until = None;
        let fired = {
            let schedules = schedules.clone();
            tokio::task::spawn_blocking(move || {
                schedules.try_run_scheduled_prompts_with_scope(task_scope.as_deref(), now)
            })
            .await
        };
        let fired = match fired {
            Ok(Ok(fired)) => fired,
            Ok(Err(error)) => {
                warn!("[Kairos] Could not take due scheduled requests: {error}");
                return None;
            }
            Err(error) => {
                warn!("[Kairos] Taking due scheduled requests stopped: {error}");
                return None;
            }
        };
        if fired.is_empty() {
            return None;
        }
        Some(dispatch_with_plan(schedules, &fired, now, ScheduledPromptPlan::Pro).await)
    }
}

#[cfg(test)]
#[path = "kairos_scheduled_prompts_tests.rs"]
mod tests;
