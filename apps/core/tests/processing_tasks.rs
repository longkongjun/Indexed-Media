mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::observations::FileObservation;
use mediaflow_core::discovery::revisions::{ObservationSource, RevisionObserver, RevisionService};
use mediaflow_core::platform::db::open_pool;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxReader;
use mediaflow_core::tasks::events::TaskEventEnvelope;
use mediaflow_core::tasks::processing::model::ProcessingStatus;
use mediaflow_core::tasks::processing::service::ProcessingTaskService;
use mediaflow_core::tasks::processing::store::ProcessingStore;

#[tokio::test]
async fn concurrent_ensure_creates_one_task_attempt_snapshot_and_event_for_one_revision() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = stable_revision(db.pool(), inbox, b"movies/movie.mkv", 1).await;
    let store = ProcessingStore::new(db.pool().clone());

    let (left, right) = tokio::join!(
        store.ensure_revision(revision, 90_000_001),
        store.ensure_revision(revision, 90_000_002),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.id, right.id);
    assert_eq!(left.status, ProcessingStatus::Queued);
    assert_eq!(left.attempt_count, 1);
    assert_eq!(left.current_task_decision_id, None);
    assert_eq!(left.decision_checkpoint, None);
    assert_eq!(store.get(account, left.id).await.unwrap(), left);
    assert_eq!(count(db.pool(), "tasks_processing_tasks").await, 1);
    assert_eq!(count(db.pool(), "tasks_processing_attempts").await, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM platform_outbox_events
             WHERE event_type='processing-task.state-changed'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
    let events = OutboxReader::new(db.pool().clone())
        .after(0, 10)
        .await
        .unwrap();
    assert!(matches!(
        events.as_slice(),
        [TaskEventEnvelope::ProcessingTaskStateChanged {
            task_id,
            payload,
            ..
        }] if *task_id == left.id
            && payload.status == ProcessingStatus::Queued
            && !payload.recovering
            && payload.reason.is_none()
    ));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM discovery_processing_requests WHERE revision_id=?",
        )
        .bind(revision.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "completed"
    );
}

#[tokio::test]
async fn ensure_transaction_failure_leaves_request_pending_and_no_task_attempt_or_event() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = stable_revision(db.pool(), inbox, b"movie.mkv", 2).await;
    sqlx::query(
        "CREATE TRIGGER fail_processing_task_insert BEFORE INSERT ON tasks_processing_tasks
         BEGIN SELECT RAISE(ABORT, 'fixture failure'); END",
    )
    .execute(db.pool())
    .await
    .unwrap();

    assert!(
        ProcessingStore::new(db.pool().clone())
            .ensure_revision(revision, 90_000_001)
            .await
            .is_err()
    );
    assert_eq!(count(db.pool(), "tasks_processing_tasks").await, 0);
    assert_eq!(count(db.pool(), "tasks_processing_attempts").await, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM platform_outbox_events
             WHERE event_type='processing-task.state-changed'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM discovery_processing_requests WHERE revision_id=?",
        )
        .bind(revision.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "pending"
    );
}

#[tokio::test]
async fn pending_request_bridge_is_drained_idempotently_before_any_worker_can_claim() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    stable_revision(db.pool(), inbox, b"one.mkv", 3).await;
    stable_revision(db.pool(), inbox, b"two.mkv", 4).await;
    let service = ProcessingTaskService::new(db.pool().clone());

    assert_eq!(service.ensure_pending(1, 90_000_010).await.unwrap(), 1);
    assert_eq!(service.ensure_pending(10, 90_000_011).await.unwrap(), 1);
    assert_eq!(service.ensure_pending(10, 90_000_012).await.unwrap(), 0);
    assert_eq!(count(db.pool(), "tasks_processing_tasks").await, 2);
    assert_eq!(count(db.pool(), "tasks_processing_attempts").await, 2);
}

#[tokio::test]
async fn lower_numbered_processing_migration_applies_after_tmdb_without_version_downgrade() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let baseline_migrations = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations")).unwrap() {
        let entry = entry.unwrap();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let version = file_name
            .split_once('_')
            .and_then(|(prefix, _)| prefix.parse::<u16>().ok());
        if version.is_some_and(|version| version <= 11 && version != 10) {
            std::fs::copy(
                entry.path(),
                baseline_migrations.path().join(file_name.as_ref()),
            )
            .unwrap();
        }
    }
    let baseline = sqlx::migrate::Migrator::new(baseline_migrations.path())
        .await
        .unwrap();
    let pool = open_pool(&fixture.database_path()).await.unwrap();
    baseline.run(&pool).await.unwrap();
    let baseline_version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;

    let upgraded = migrate_with_backup(fixture.config()).await.unwrap();
    assert_eq!(count(upgraded.pool(), "tasks_processing_tasks").await, 0);
    let upgraded_version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
        .fetch_one(upgraded.pool())
        .await
        .unwrap();
    let latest_version = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .split_once('_')
                .and_then(|(prefix, _)| prefix.parse::<i64>().ok())
        })
        .max()
        .unwrap();
    assert!(upgraded_version >= baseline_version);
    assert_eq!(upgraded_version, latest_version);
}

async fn stable_revision(
    pool: &sqlx::SqlitePool,
    inbox: uuid::Uuid,
    path: &[u8],
    suffix: u8,
) -> uuid::Uuid {
    let observation = FileObservation {
        inbox_directory_id: inbox,
        relative_path_bytes: path.to_vec(),
        relative_path_display: String::from_utf8_lossy(path).into_owned(),
        identity_snapshot: vec![suffix],
        size_bytes: 100,
        modified_at_ns: 0,
    };
    let revisions = RevisionService::new(pool.clone());
    revisions
        .observe(observation.clone(), ObservationSource::Watcher, 60_000_000)
        .await
        .unwrap();
    revisions
        .observe(observation, ObservationSource::Reconcile, 90_000_000)
        .await
        .unwrap()
        .revision_id
}

async fn seed_account(pool: &sqlx::SqlitePool) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(id.as_bytes().as_slice())
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn count(pool: &sqlx::SqlitePool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}
