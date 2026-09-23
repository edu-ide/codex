use super::*;

#[test]
fn queued_followup_is_not_scored_as_running_and_refresh_preserves_queue() {
    let dir = tempfile::tempdir().unwrap();
    let brain = BrainService::new(dir.path(), /*repo_root*/ None).unwrap();
    let mut outcome = SuperLoopOutcome::default();
    let upsert = |outcome: &mut SuperLoopOutcome| {
        upsert_followup_task(
            &brain,
            "[super-loop] followup",
            "Investigate finding",
            Some("Review evidence"),
            Some("Include source references"),
            /*preferred_roles*/ None,
            "review",
            "Finding",
            outcome,
        )
        .unwrap()
    };
    let created = upsert(&mut outcome);
    let queued = maybe_run_followup_task(SuperLoopDriver::Worker, &brain, &created)
        .unwrap()
        .unwrap();
    let refreshed = upsert(&mut outcome);
    assert_eq!(created.target, refreshed.target);
    assert_eq!((outcome.created_tasks, outcome.updated_tasks), (1, 1));
    assert_eq!(queued.status, "queued");
    let scores = score_followup_tasks(&brain, &[created, queued, refreshed]).unwrap();
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0].resolution, SuperLoopResolution::Planned);
    let claim = brain
        .schedules()
        .claim_super_loop_followup("host")
        .unwrap()
        .unwrap();
    assert!(claim.prompt.contains("Investigate finding"));
    assert!(claim.prompt.contains("Include source references"));
}

#[test]
fn legacy_failure_text_does_not_resolve_a_followup() {
    let task: Schedule = serde_json::from_value(serde_json::json!({
        "id": "legacy", "title": "legacy", "created_at": "2026-09-23T00:00:00Z",
        "last_run_status": "unsuccessful; unresolved", "status": "open"
    }))
    .unwrap();
    assert_eq!(task_resolution(&task), SuperLoopResolution::Planned);
}
