mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::coordinator::{
    CoordinatorStore, DiscoveryCoordinator, DiscoveryRuntime,
};
use mediaflow_core::discovery::watcher::{WatchHint, WatchIngress};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::{OutboxNotifier, OutboxReader};
use mediaflow_core::tasks::events::{
    InboxDiscoveryHealth, InboxDiscoveryHealthReason, TaskEventEnvelope,
};

#[tokio::test]
async fn overflow_failed_reconcile_and_restart_remain_bounded_durable_and_recoverable() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = seeded_bootstrapped_inbox(db.pool()).await;
    let notifier = OutboxNotifier::new();
    let (ingress, receiver) = WatchIngress::bounded(1);
    let store = CoordinatorStore::new(db.pool().clone(), notifier.clone());
    let mut coordinator = DiscoveryCoordinator::new(store.clone(), receiver);
    assert!(ingress.try_send(WatchHint::new(inbox, b"one.mkv".to_vec(), 1)));
    assert!(!ingress.try_send(WatchHint::new(inbox, b"two.mkv".to_vec(), 2)));

    coordinator.tick(10).await.unwrap();
    let task_id = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT active_reconcile_task_id FROM discovery_watch_states WHERE inbox_directory_id=?",
    )
    .bind(inbox.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .map(|bytes| uuid::Uuid::from_slice(&bytes).unwrap())
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT reason FROM tasks_scan_tasks WHERE id=?")
            .bind(task_id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "watch_recovery"
    );
    sqlx::query("UPDATE tasks_scan_tasks SET status='failed',stage='finished' WHERE id=?")
        .bind(task_id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    coordinator.tick(20).await.unwrap();
    let state: (String, Option<String>, Option<Vec<u8>>, i64) = sqlx::query_as(
        "SELECT health,last_error_code,active_reconcile_task_id,next_reconcile_at_us
         FROM discovery_watch_states WHERE inbox_directory_id=?",
    )
    .bind(inbox.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(state.0, "degraded");
    assert_eq!(state.1.as_deref(), Some("reconcile.failed"));
    assert!(state.2.is_none());
    assert_eq!(state.3, 60_000_020);

    let events = OutboxReader::new(db.pool().clone())
        .after(0, 20)
        .await
        .unwrap();
    let health_event = events.iter().find_map(|event| match event {
        TaskEventEnvelope::InboxDiscoveryHealthChanged {
            task_id, payload, ..
        } => Some((task_id, payload)),
        _ => None,
    });
    let (health_task_id, payload) = health_event.expect("durable discovery health event");
    assert!(health_task_id.is_none());
    assert_eq!(payload.inbox_directory_id, inbox);
    assert_eq!(payload.health, InboxDiscoveryHealth::Degraded);
    assert_eq!(
        payload.reason,
        Some(InboxDiscoveryHealthReason::ReconcileFailed)
    );
    assert!(payload.watcher_active);

    let (_restart_ingress, restart_receiver) = WatchIngress::bounded(1);
    let mut restarted = DiscoveryCoordinator::new(store, restart_receiver);
    let scheduled = restarted.startup(30).await.unwrap();
    assert_eq!(scheduled.len(), 1);
    assert_ne!(scheduled[0], task_id);
}

#[tokio::test]
async fn watcher_disabled_still_runs_startup_and_periodic_reconciliation() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = seeded_bootstrapped_inbox(db.pool()).await;
    sqlx::query("UPDATE discovery_inbox_policies SET watcher_enabled=0 WHERE inbox_directory_id=?")
        .bind(inbox.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    let (ingress, receiver) = WatchIngress::bounded(4);
    let mut coordinator = DiscoveryCoordinator::new(
        CoordinatorStore::new(db.pool().clone(), OutboxNotifier::new()),
        receiver,
    );
    let startup = coordinator.startup(1).await.unwrap();
    assert_eq!(startup.len(), 1);
    assert!(ingress.try_send(WatchHint::new(inbox, b"ignored.mkv".to_vec(), 2)));
    coordinator.tick(2).await.unwrap();
    coordinator.tick(2_000_002).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks_scan_tasks")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn deployment_root_snapshot_replacement_stops_watcher_and_forces_unavailable_reconcile() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let host_root = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join("incoming-root");
    std::fs::create_dir(&host_root).unwrap();
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming",
            "label":"Incoming",
            "container_path":host_root,
            "access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = seeded_bootstrapped_inbox(db.pool()).await;
    let notifier = OutboxNotifier::new();
    let mut runtime =
        DiscoveryRuntime::from_app(fixture.config(), &db, notifier).expect("valid runtime");
    runtime.prepare().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT watcher_active FROM discovery_watch_states WHERE inbox_directory_id=?",
        )
        .bind(inbox.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );

    std::fs::write(&fixture.config().deployment_roots_file, b"{\"roots\":[]}\n").unwrap();
    runtime
        .tick_once(chrono::Utc::now().timestamp_micros())
        .await
        .unwrap();
    let state: (String, i64, Option<String>) = sqlx::query_as(
        "SELECT health,watcher_active,last_error_code FROM discovery_watch_states
         WHERE inbox_directory_id=?",
    )
    .bind(inbox.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(state.0, "unavailable");
    assert_eq!(state.1, 0);
    assert_eq!(state.2.as_deref(), Some("watcher.unavailable"));
}

async fn seeded_bootstrapped_inbox(pool: &sqlx::SqlitePool) -> uuid::Uuid {
    let account = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account.as_bytes().as_slice())
    .execute(pool)
    .await
    .unwrap();
    common::seed_inbox(pool).await
}
