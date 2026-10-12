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
//! announced. That decision is work-rs `ScheduledPromptTicker`, which the UGOT Space
//! desktop also ticks (it runs `ilhae app-server` without this proxy); the store's
//! file lock keeps the two hosts from firing one occurrence twice.

use brain_rs::schedule::ScheduleStore;
use brain_rs::schedule::ScheduleTrigger;
use brain_rs::schedule::ScheduledPromptTicker;
use brain_rs::schedule::ScheduledPromptOutcome;
use brain_rs::schedule::ScheduledPromptPlan;
use brain_rs::schedule::scheduled_prompt_notice;
use brain_rs::schedule::scheduled_prompts_need_plan;
#[cfg(test)]
use brain_rs::schedule::FREE_RECHECK_MINUTES;
use chrono::DateTime;
use chrono::Utc;
use std::future::Future;
use std::sync::Arc;
use tracing::info;
use tracing::warn;
use ugot_local_session::entitlement::UgotPlan;

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
    log_outcomes(triggered, &outcomes);
    scheduled_prompt_notice(triggered, &outcomes)
}

fn log_outcomes(triggered: &[ScheduleTrigger], outcomes: &[ScheduledPromptOutcome]) {
    for (trigger, outcome) in triggered.iter().zip(outcomes) {
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
}

/// Runs scheduled requests while the Kairos task loop is off. Reminders and
/// calendar browser batches are left to the loop, exactly as before. The decision
/// itself lives in work-rs ([`ScheduledPromptTicker`]) and is shared with the UGOT
/// Space desktop, which never starts this proxy; this wrapper only moves the
/// file-locked store calls off the async runtime and asks pay between them.
#[derive(Debug, Default)]
pub(crate) struct WithoutKairos {
    ticker: ScheduledPromptTicker,
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
        let ticker = self.ticker;
        let checked = {
            let schedules = schedules.clone();
            let scope = task_scope.clone();
            tokio::task::spawn_blocking(move || {
                ticker.needs_plan(&schedules, scope.as_deref(), now)
            })
            .await
        };
        match checked {
            Ok(Ok(true)) => {}
            // Nothing would run: no pay request, nothing consumed.
            Ok(Ok(false)) => return None,
            Ok(Err(error)) => {
                warn!("[Kairos] Could not read due scheduled requests: {error}");
                return None;
            }
            Err(error) => {
                warn!("[Kairos] Due scheduled request check stopped: {error}");
                return None;
            }
        }
        let plan = plan().await;
        let mut ticker = self.ticker;
        let ran = tokio::task::spawn_blocking(move || {
            let run = ticker.run_with_plan(&schedules, task_scope.as_deref(), now, plan);
            (ticker, run)
        })
        .await;
        match ran {
            Ok((ticker, Ok(run))) => {
                self.ticker = ticker;
                let run = run?;
                log_outcomes(&run.triggers, &run.outcomes);
                Some(run.notice)
            }
            Ok((ticker, Err(error))) => {
                self.ticker = ticker;
                warn!("[Kairos] Could not take due scheduled requests: {error}");
                None
            }
            Err(error) => {
                warn!("[Kairos] Taking due scheduled requests stopped: {error}");
                None
            }
        }
    }
}

#[cfg(test)]
#[path = "kairos_scheduled_prompts_tests.rs"]
mod tests;
