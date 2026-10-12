use super::*;
use crate::config::test_env;
use chrono::TimeZone;
use std::cell::Cell;

fn add(store: &ScheduleStore, title: &str, prompt: Option<&str>) -> String {
    store
        .add(
            title,
            None,
            None,
            None,
            vec![],
            prompt,
            Some("1h"),
            None,
            None,
            None,
            None,
        )
        .unwrap()
        .id
}

fn followups(store: &ScheduleStore) -> usize {
    store
        .try_list()
        .unwrap()
        .into_iter()
        .filter(|item| item.super_loop.is_some())
        .count()
}

fn snapshot(store: &ScheduleStore, id: &str) -> (Option<String>, usize) {
    let item = store.try_get(id).unwrap().unwrap();
    (item.last_run, item.history.len())
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 12, 9, 0, 0).unwrap()
}

#[tokio::test]
#[serial_test::serial]
async fn loop_off_pro_queues_the_due_request_and_leaves_reminders() {
    let _home = test_env::empty_home();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ScheduleStore::new(dir.path()));
    let request = add(&store, "환율 확인", Some("오늘 환율 정리"));
    let reminder = add(&store, "물 마시기", None);
    let reminder_before = snapshot(&store, &reminder);
    let asked = Cell::new(0);

    let mut gate = WithoutKairos::default();
    let message = gate
        .tick(store.clone(), None, now(), || async {
            asked.set(asked.get() + 1);
            ScheduledPromptPlan::Pro
        })
        .await
        .expect("a fired request is announced");

    assert_eq!(asked.get(), 1);
    assert_eq!(followups(&store), 1);
    assert!(message.contains("실행 대기열"), "{message}");
    let history = store.try_get(&request).unwrap().unwrap().history;
    assert!(
        history
            .iter()
            .any(|entry| entry.action == "scheduled_prompt_enqueued")
    );
    assert_eq!(snapshot(&store, &reminder), reminder_before);

    // The same occurrence is not due again, so pay is not asked again.
    let again = gate
        .tick(store.clone(), None, now(), || async {
            asked.set(asked.get() + 1);
            ScheduledPromptPlan::Pro
        })
        .await;
    assert!(again.is_none());
    assert_eq!(asked.get(), 1);
    assert_eq!(followups(&store), 1);
}

#[tokio::test]
#[serial_test::serial]
async fn loop_off_free_consumes_nothing_and_queues_nothing() {
    let _home = test_env::empty_home();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ScheduleStore::new(dir.path()));
    let request = add(&store, "환율 확인", Some("오늘 환율 정리"));
    let before = snapshot(&store, &request);
    let asked = Cell::new(0);

    let mut gate = WithoutKairos::default();
    let message = gate
        .tick(store.clone(), None, now(), || async {
            asked.set(asked.get() + 1);
            ScheduledPromptPlan::Free
        })
        .await;
    assert!(message.is_none());
    assert_eq!(asked.get(), 1);
    assert_eq!(snapshot(&store, &request), before);
    assert_eq!(followups(&store), 0);

    // Within the quiet window pay is not asked again; the schedule stays due.
    let later = now() + chrono::Duration::minutes(FREE_RECHECK_MINUTES - 1);
    assert!(
        gate.tick(store.clone(), None, later, || async {
            asked.set(asked.get() + 1);
            ScheduledPromptPlan::Free
        })
        .await
        .is_none()
    );
    assert_eq!(asked.get(), 1);
    assert_eq!(snapshot(&store, &request), before);

    // After it, an upgraded account runs the request that was kept.
    let upgraded = now() + chrono::Duration::minutes(FREE_RECHECK_MINUTES);
    assert!(
        gate.tick(store.clone(), None, upgraded, || async {
            asked.set(asked.get() + 1);
            ScheduledPromptPlan::Pro
        })
        .await
        .is_some()
    );
    assert_eq!(asked.get(), 2);
    assert_eq!(followups(&store), 1);
}

#[tokio::test]
#[serial_test::serial]
async fn loop_off_without_a_due_request_never_asks_pay() {
    let _home = test_env::empty_home();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ScheduleStore::new(dir.path()));
    let reminder = add(&store, "물 마시기", None);
    let before = snapshot(&store, &reminder);
    let asked = Cell::new(0);

    let mut gate = WithoutKairos::default();
    assert!(
        gate.tick(store.clone(), None, now(), || async {
            asked.set(asked.get() + 1);
            ScheduledPromptPlan::Pro
        })
        .await
        .is_none()
    );
    assert_eq!(asked.get(), 0);
    assert_eq!(snapshot(&store, &reminder), before);
}

#[tokio::test]
#[serial_test::serial]
async fn loop_on_still_fires_everything_and_queues_on_pro() {
    let _home = test_env::empty_home();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ScheduleStore::new(dir.path()));
    add(&store, "환율 확인", Some("오늘 환율 정리"));
    let reminder = add(&store, "물 마시기", None);

    // The loop-on path: fire every due schedule, then dispatch.
    let triggered = store.run_with_scope(None, None);
    assert_eq!(triggered.len(), 2);
    assert!(
        store
            .try_get(&reminder)
            .unwrap()
            .unwrap()
            .last_run
            .is_some()
    );
    let message = dispatch_with_plan(
        store.clone(),
        &triggered,
        Utc::now(),
        ScheduledPromptPlan::Pro,
    )
    .await;
    assert_eq!(followups(&store), 1);
    assert!(
        message.starts_with("[Kairos] Triggered 2 scheduled tasks"),
        "{message}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn proxy_and_space_desktop_on_one_store_queue_the_request_once() {
    let _home = test_env::empty_home();
    let dir = tempfile::tempdir().unwrap();
    let proxy_store = Arc::new(ScheduleStore::new(dir.path()));
    add(&proxy_store, "환율 확인", Some("오늘 환율 정리"));
    // The Space desktop opens its own store on the same data directory and
    // drives the same work-rs ticker.
    let desktop_dir = dir.path().to_path_buf();
    let desktop = tokio::task::spawn_blocking(move || {
        let store = ScheduleStore::new(&desktop_dir);
        ScheduledPromptTicker::default()
            .tick(&store, None, now(), || ScheduledPromptPlan::Pro)
            .unwrap()
    });
    let mut proxy = WithoutKairos::default();
    let proxy_notice = proxy
        .tick(proxy_store.clone(), None, now(), || async {
            ScheduledPromptPlan::Pro
        })
        .await;
    let desktop_run = desktop.await.unwrap();

    assert_eq!(followups(&proxy_store), 1);
    let queued_by_proxy = proxy_notice.is_some_and(|notice| notice.contains("실행 대기열"));
    let queued_by_desktop = desktop_run.is_some_and(|run| run.enqueued() == 1);
    assert!(
        queued_by_proxy ^ queued_by_desktop,
        "exactly one host queues it (proxy {queued_by_proxy}, desktop {queued_by_desktop})"
    );
}
