mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::coordinator::{CoordinatorStore, DiscoveryCoordinator, ScanReason};
use mediaflow_core::discovery::observations::FileObservation;
use mediaflow_core::discovery::revisions::{ObservationSource, RevisionObserver, RevisionService};
use mediaflow_core::discovery::watcher::{WatchHint, WatchIngress};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::shared::page::PageRequest;
use mediaflow_core::tasks::model::{NewScanTask, ScanCounts};
use mediaflow_core::tasks::store::TaskStore;

#[tokio::test]
async fn startup_is_immediate_periodic_is_persistent_and_active_reconcile_is_unique() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = seeded_bootstrapped_inbox(db.pool()).await;
    let (ingress, receiver) = WatchIngress::bounded(8);
    let store = CoordinatorStore::new(db.pool().clone(), OutboxNotifier::new());
    let mut coordinator = DiscoveryCoordinator::new(store, receiver);

    let first = coordinator.startup(1_000_000).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(coordinator.startup(1_000_001).await.unwrap(), first);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tasks_scan_tasks WHERE reason='startup'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
    let account = account_id(db.pool()).await;
    let manual_tasks = TaskStore::new(db.pool().clone())
        .list_tasks(account, &PageRequest::new(None, None).unwrap())
        .await
        .unwrap();
    assert!(manual_tasks.items.is_empty());
    let error = TaskStore::new(db.pool().clone())
        .get(account, first[0])
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::TaskNotFound);

    complete(first[0], db.pool(), "completed").await;
    coordinator.tick(2_000_000).await.unwrap();
    let next = sqlx::query_scalar::<_, i64>(
        "SELECT next_reconcile_at_us FROM discovery_watch_states WHERE inbox_directory_id=?",
    )
    .bind(inbox.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(next, 902_000_000);
    coordinator.tick(next - 1).await.unwrap();
    assert_eq!(task_count(db.pool()).await, 1);
    coordinator.tick(next).await.unwrap();
    assert_eq!(task_count(db.pool()).await, 2);
    assert_eq!(
        latest_reason(db.pool()).await,
        ScanReason::Periodic.as_str()
    );

    assert!(ingress.try_send(WatchHint::new(inbox, b"movie.mkv".to_vec(), next + 1)));
    coordinator.tick(next + 1).await.unwrap();
    coordinator.tick(next + 2_000_001).await.unwrap();
    assert_eq!(
        task_count(db.pool()).await,
        2,
        "active periodic scan deduplicates watch hint"
    );
}

#[tokio::test]
async fn successful_full_scan_marks_unseen_current_revisions_missing_in_the_terminal_transaction() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = seeded_bootstrapped_inbox(db.pool()).await;
    let account_bytes =
        sqlx::query_scalar::<_, Vec<u8>>("SELECT id FROM identity_accounts WHERE singleton_key=1")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let account = uuid::Uuid::from_slice(&account_bytes).unwrap();
    let revisions = RevisionService::new(db.pool().clone());
    let present = file(inbox, b"present.mkv", b"present");
    let missing = file(inbox, b"missing.mkv", b"missing");
    revisions
        .observe(present.clone(), ObservationSource::Reconcile, 1)
        .await
        .unwrap();
    revisions
        .observe(missing, ObservationSource::Reconcile, 1)
        .await
        .unwrap();

    let tasks = TaskStore::new(db.pool().clone());
    tasks
        .create(NewScanTask {
            account_id: account,
            inbox_directory_id: inbox,
            idempotency_key: "full-reconcile-missing".to_owned(),
            now_us: 2,
        })
        .await
        .unwrap();
    let lease = tasks
        .claim_next("reconcile-worker", 60_000_000)
        .await
        .unwrap()
        .unwrap();
    let lease = tasks
        .record_observations(&lease, &[present], &[], ScanCounts::default(), 60_100_000)
        .await
        .unwrap();
    tasks
        .finish_success(&lease, lease.task.counts, 60_200_000)
        .await
        .unwrap();
    let states = sqlx::query_as::<_, (String, String)>(
        "SELECT f.relative_path_display,s.status FROM discovery_tracked_files f
         JOIN discovery_file_revision_states s ON s.revision_id=f.current_revision_id
         ORDER BY f.relative_path_display",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(
        states,
        vec![
            ("missing.mkv".to_owned(), "missing".to_owned()),
            ("present.mkv".to_owned(), "stable".to_owned()),
        ]
    );
}

fn file(inbox_directory_id: uuid::Uuid, path: &[u8], identity: &[u8]) -> FileObservation {
    FileObservation {
        inbox_directory_id,
        relative_path_bytes: path.to_vec(),
        relative_path_display: String::from_utf8_lossy(path).into_owned(),
        identity_snapshot: identity.to_vec(),
        size_bytes: 1,
        modified_at_ns: 0,
    }
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

async fn account_id(pool: &sqlx::SqlitePool) -> uuid::Uuid {
    let bytes =
        sqlx::query_scalar::<_, Vec<u8>>("SELECT id FROM identity_accounts WHERE singleton_key=1")
            .fetch_one(pool)
            .await
            .unwrap();
    uuid::Uuid::from_slice(&bytes).unwrap()
}

async fn complete(task_id: uuid::Uuid, pool: &sqlx::SqlitePool, status: &str) {
    sqlx::query("UPDATE tasks_scan_tasks SET status=?,stage='finished' WHERE id=?")
        .bind(status)
        .bind(task_id.as_bytes().as_slice())
        .execute(pool)
        .await
        .unwrap();
}

async fn task_count(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM tasks_scan_tasks")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn latest_reason(pool: &sqlx::SqlitePool) -> String {
    sqlx::query_scalar(
        "SELECT reason FROM tasks_scan_tasks ORDER BY created_at_us DESC,id DESC LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}
