use codex_ilhae::gpu_queue::api::LeaseMode;
use codex_ilhae::gpu_queue::api::LeaseRequest;
use codex_ilhae::gpu_queue::api::LeaseState;
use codex_ilhae::gpu_queue::scheduler::LeaseScheduler;

fn exclusive_request(owner: &str, kind: &str, ttl_seconds: u64) -> LeaseRequest {
    LeaseRequest {
        owner: owner.to_string(),
        kind: kind.to_string(),
        mode: LeaseMode::Exclusive,
        preempt_llm: true,
        ttl_seconds,
        wait_timeout_seconds: None,
    }
}

#[test]
fn exclusive_leases_are_granted_one_at_a_time_and_promoted_fifo() {
    let mut scheduler = LeaseScheduler::new();

    let first = scheduler
        .request_lease(exclusive_request("videoeditor", "video", 60), true, 100)
        .expect("first lease should be accepted");
    let second = scheduler
        .request_lease(exclusive_request("cli", "video", 60), false, 101)
        .expect("second lease should be accepted");

    assert_eq!(first.state, LeaseState::Granted);
    assert_eq!(second.state, LeaseState::Pending);

    let status = scheduler.status(101);
    assert_eq!(
        status
            .active_lease
            .as_ref()
            .map(|lease| lease.lease_id.clone()),
        Some(first.lease_id.clone())
    );
    assert_eq!(status.pending_leases.len(), 1);

    let outcome = scheduler
        .release_lease(&first.lease_id, 102)
        .expect("active lease should release");

    assert_eq!(outcome.released.lease_id, first.lease_id);
    assert_eq!(
        outcome.promoted.as_ref().map(|lease| lease.owner.clone()),
        Some("cli".to_string())
    );

    let status = scheduler.status(102);
    assert_eq!(
        status
            .active_lease
            .as_ref()
            .map(|lease| lease.owner.clone()),
        Some("cli".to_string())
    );
    assert_eq!(status.pending_leases, Vec::new());
}

#[test]
fn expired_active_lease_is_released_and_pending_lease_is_promoted() {
    let mut scheduler = LeaseScheduler::new();

    let first = scheduler
        .request_lease(exclusive_request("videoeditor", "video", 10), true, 200)
        .expect("first lease should be accepted");
    scheduler
        .request_lease(exclusive_request("cli", "image", 60), false, 201)
        .expect("second lease should be accepted");

    let expired = scheduler.expire_leases(211);

    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].lease_id, first.lease_id);

    let status = scheduler.status(211);
    assert_eq!(
        status
            .active_lease
            .as_ref()
            .map(|lease| lease.owner.clone()),
        Some("cli".to_string())
    );
    assert_eq!(
        status
            .active_lease
            .as_ref()
            .and_then(|lease| lease.expires_at),
        Some(271)
    );
}
