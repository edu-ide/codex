//! Native findings are inputs to Work; this adapter never owns execution or retries.
use super::*;
use brain_rs::schedule::{FollowupSpec, Schedule};

pub(super) fn upsert_followup_task(
    brain: &BrainService,
    title: &str,
    description: &str,
    prompt: Option<&str>,
    instructions: Option<&str>,
    preferred_roles: Option<&[&str]>,
    category: &str,
    action_detail: &str,
    outcome: &mut SuperLoopOutcome,
) -> Result<SuperLoopAction, String> {
    let (task, created) = brain.schedules().upsert_super_loop_followup(FollowupSpec {
        title: title.to_owned(),
        description: description.to_owned(),
        prompt: prompt.map(str::to_owned),
        instructions: append_preferred_roles_hint(instructions, preferred_roles),
        preferred_agent: first_preferred_role(preferred_roles),
        category: category.to_owned(),
        detail: action_detail.to_owned(),
    })?;
    if created {
        outcome.created_tasks += 1;
    } else {
        outcome.updated_tasks += 1;
    }
    Ok(SuperLoopAction {
        kind: SuperLoopActionKind::UpsertFollowupTask,
        target: task.id,
        detail: action_detail.to_owned(),
        status: if created { "created" } else { "updated" }.to_owned(),
        source_signature: None,
    })
}

pub(super) fn maybe_run_followup_task(
    driver: SuperLoopDriver,
    brain: &BrainService,
    action: &SuperLoopAction,
) -> Result<Option<SuperLoopAction>, String> {
    if driver != SuperLoopDriver::Worker
        || action.kind != SuperLoopActionKind::UpsertFollowupTask
        || action.target.trim().is_empty()
    {
        return Ok(None);
    }
    let task = brain
        .schedules()
        .request_super_loop_followup(&action.target)?;
    let state = task
        .super_loop
        .as_ref()
        .ok_or("Missing Work follow-up state")?;
    Ok(Some(SuperLoopAction {
        kind: SuperLoopActionKind::RunFollowupTask,
        target: task.id,
        detail: format!("Work host execution requested: {}", action.detail),
        // Queue acceptance is not evidence that an agent started running.
        status: state.state.clone(),
        source_signature: action.source_signature.clone(),
    }))
}

fn task_resolution(task: &Schedule) -> SuperLoopResolution {
    if let Some(followup) = &task.super_loop {
        return match followup.state.as_str() {
            "completed" => SuperLoopResolution::Resolved,
            "running" | "waiting_approval" => SuperLoopResolution::Running,
            "retry_wait" => SuperLoopResolution::Retry,
            "review" | "failed" => SuperLoopResolution::Escalated,
            "planned" | "queued" | "claimed" | "dispatched" => SuperLoopResolution::Planned,
            _ => SuperLoopResolution::Stale,
        };
    }
    // Compatibility for old records: only explicit lifecycle values count.
    // For example, "unsuccessful" and "unresolved" must not resolve a task.
    if task.done || matches!(task.status.as_str(), "done" | "completed" | "complete") {
        SuperLoopResolution::Resolved
    } else {
        match task.status.as_str() {
            "running" | "waiting_approval" => SuperLoopResolution::Running,
            "failed" | "error" | "blocked" | "review" => SuperLoopResolution::Escalated,
            _ => SuperLoopResolution::Planned,
        }
    }
}

pub(super) fn score_followup_tasks(
    brain: &BrainService,
    actions: &[SuperLoopAction],
) -> Result<Vec<SuperLoopTaskScore>, String> {
    let tasks = brain.schedules().try_list()?;
    let signature_by_id = actions
        .iter()
        .filter(|action| {
            matches!(
                action.kind,
                SuperLoopActionKind::UpsertFollowupTask | SuperLoopActionKind::RunFollowupTask
            )
        })
        .map(|action| (action.target.clone(), action.source_signature.clone()))
        .collect::<BTreeMap<_, _>>();
    Ok(signature_by_id
        .into_iter()
        .map(
            |(task_id, source_signature)| match tasks.iter().find(|task| task.id == task_id) {
                Some(task) => SuperLoopTaskScore {
                    task_id,
                    title: Some(task.title.clone()),
                    resolution: task_resolution(task),
                    detail: format!(
                        "Work status={}, retries={}/{}",
                        task.status, task.retry_count, task.max_retries
                    ),
                    source_signature,
                },
                None => SuperLoopTaskScore {
                    task_id,
                    title: None,
                    resolution: SuperLoopResolution::Stale,
                    detail: "follow-up missing from Work store".to_owned(),
                    source_signature,
                },
            },
        )
        .collect())
}

#[cfg(test)]
#[path = "super_loop_work_tests.rs"]
mod tests;
