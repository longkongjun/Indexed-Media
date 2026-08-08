#![allow(clippy::too_many_lines)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use mediaflow_core::discovery::DiscoveryUseCases;
use mediaflow_core::discovery::capability::{CapabilityFs, DeploymentRootSet};
use mediaflow_core::discovery::model::CreateInboxCommand;
use mediaflow_core::discovery::observations::{FileObservation, ObservationSink};
use mediaflow_core::discovery::scanner::{ScanSource, ScanStopToken, Scanner};
use mediaflow_core::discovery::service::DiscoveryService;
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::task_runtime::{ManualTaskClock, TaskRuntime};
use mediaflow_core::shared::error::{AppError, ErrorCode};
use mediaflow_core::tasks::model::{NewScanTask, ScanCounts, ScanStatus};
use mediaflow_core::tasks::store::TaskStore;
use mediaflow_core::tasks::worker::ScanWorker;
use tokio::sync::Notify;
use uuid::Uuid;

struct BoundaryScanner {
    first_batch_committed: Arc<Notify>,
    continue_after_cancel: Arc<Notify>,
}

struct RenewalBlockingScanner {
    started: Arc<Notify>,
    active: Arc<AtomicBool>,
}

struct CrashAfterFirstBatchScanner {
    first_batch_committed: Arc<Notify>,
}

#[async_trait]
impl Scanner for CrashAfterFirstBatchScanner {
    async fn scan(
        &self,
        source: ScanSource,
        sink: Arc<dyn ObservationSink>,
        _stop: ScanStopToken,
    ) -> Result<ScanCounts, AppError> {
        let files = (0..500)
            .map(|index| FileObservation {
                inbox_directory_id: source.inbox_directory_id,
                relative_path_bytes: format!("file-{index:04}.mkv").into_bytes(),
                relative_path_display: format!("file-{index:04}.mkv"),
                identity_snapshot: vec![1; 25],
                size_bytes: 1,
                modified_at_ns: 1,
            })
            .collect();
        sink.write_batch(
            files,
            Vec::new(),
            ScanCounts {
                visited_directories: 1,
                observed_files: 500,
                skipped_entries: 0,
                errors: 0,
            },
        )
        .await?;
        self.first_batch_committed.notify_one();
        std::future::pending().await
    }
}

#[async_trait]
impl Scanner for RenewalBlockingScanner {
    async fn scan(
        &self,
        _source: ScanSource,
        _sink: Arc<dyn ObservationSink>,
        stop: ScanStopToken,
    ) -> Result<ScanCounts, AppError> {
        self.active.store(true, Ordering::SeqCst);
        self.started.notify_one();
        while !stop.is_stopped() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        self.active.store(false, Ordering::SeqCst);
        Ok(ScanCounts::default())
    }
}

#[async_trait]
impl Scanner for BoundaryScanner {
    async fn scan(
        &self,
        source: ScanSource,
        sink: Arc<dyn ObservationSink>,
        _stop: ScanStopToken,
    ) -> Result<mediaflow_core::tasks::model::ScanCounts, AppError> {
        for batch in 0..3 {
            let files = (0..500)
                .map(|index| FileObservation {
                    inbox_directory_id: source.inbox_directory_id,
                    relative_path_bytes: format!("batch-{batch}-{index:04}.mkv").into_bytes(),
                    relative_path_display: format!("batch-{batch}-{index:04}.mkv"),
                    identity_snapshot: vec![u8::try_from(batch).unwrap(); 25],
                    size_bytes: 1,
                    modified_at_ns: 1,
                })
                .collect();
            sink.write_batch(
                files,
                Vec::new(),
                ScanCounts {
                    visited_directories: 1,
                    observed_files: u64::try_from((batch + 1) * 500).unwrap(),
                    skipped_entries: 0,
                    errors: 0,
                },
            )
            .await?;
            if batch == 0 {
                self.first_batch_committed.notify_one();
                self.continue_after_cancel.notified().await;
            }
        }
        Ok(mediaflow_core::tasks::model::ScanCounts {
            visited_directories: 1,
            observed_files: 1_500,
            skipped_entries: 0,
            errors: 0,
        })
    }
}

async fn fixture() -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    TaskStore,
    Uuid,
    Uuid,
) {
    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = Uuid::now_v7();
    let inbox_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',X'2E','.',X'01',X'02','available',1,1,1,1)",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let store = TaskStore::new(db.pool().clone());
    (fixture, db, store, account_id, inbox_id)
}

#[tokio::test]
async fn active_lease_is_excluded_renewal_extends_it_and_expiry_recovers_same_task_batch() {
    let (_fixture, db, store, account_id, inbox_id) = fixture().await;
    let task = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_id,
            idempotency_key: "recover-create".to_owned(),
            now_us: 1_000_000,
        })
        .await
        .unwrap();
    let first = store
        .claim_next("worker-a", 2_000_000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        store
            .claim_next("worker-b", 2_000_001)
            .await
            .unwrap()
            .is_none()
    );

    let renewed = store.renew(&first, 11_000_000).await.unwrap();
    assert!(renewed.expires_at_us > first.expires_at_us);
    assert!(
        store
            .claim_next("worker-b", first.expires_at_us + 1)
            .await
            .unwrap()
            .is_none()
    );

    let recovered = store
        .claim_next("worker-b", renewed.expires_at_us + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.task.id, task.id);
    assert_eq!(recovered.batch_id, first.batch_id);
    assert_ne!(recovered.attempt_id, first.attempt_id);
    assert!(recovered.task.recovering);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks_scan_attempts WHERE task_id=?")
            .bind(task.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn manual_retry_reuses_task_and_batch_and_cancel_is_persistent_without_version_theft() {
    let (_fixture, db, store, account_id, inbox_id) = fixture().await;
    let task = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_id,
            idempotency_key: "retry-create".to_owned(),
            now_us: 1_000_000,
        })
        .await
        .unwrap();
    let lease = store
        .claim_next("worker-a", 2_000_000)
        .await
        .unwrap()
        .unwrap();
    let old_file = FileObservation {
        inbox_directory_id: inbox_id,
        relative_path_bytes: b"old.mkv".to_vec(),
        relative_path_display: "old.mkv".to_owned(),
        identity_snapshot: vec![1; 25],
        size_bytes: 1,
        modified_at_ns: 1,
    };
    let lease = store
        .record_observations(
            &lease,
            &[old_file],
            &[],
            ScanCounts {
                observed_files: 1,
                ..ScanCounts::default()
            },
            2_500_000,
        )
        .await
        .unwrap();
    let failed = store.finish_failed(&lease, 3_000_000).await.unwrap();
    assert_eq!(failed.status, ScanStatus::Failed);
    let retried = store
        .retry(account_id, task.id, "retry-key", 4_000_000)
        .await
        .unwrap();
    assert_eq!(retried.id, task.id);
    assert_eq!(retried.status, ScanStatus::Queued);
    let retry_lease = store
        .claim_next("worker-b", 5_000_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry_lease.batch_id, lease.batch_id);
    assert_ne!(retry_lease.attempt_id, lease.attempt_id);
    let new_file = FileObservation {
        inbox_directory_id: inbox_id,
        relative_path_bytes: b"new.mkv".to_vec(),
        relative_path_display: "new.mkv".to_owned(),
        identity_snapshot: vec![2; 25],
        size_bytes: 2,
        modified_at_ns: 2,
    };
    let retry_lease = store
        .record_observations(
            &retry_lease,
            &[new_file],
            &[],
            ScanCounts {
                observed_files: 1,
                ..ScanCounts::default()
            },
            5_500_000,
        )
        .await
        .unwrap();
    assert_eq!(retry_lease.task.counts.observed_files, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM discovery_scan_file_observations WHERE scan_batch_id=?"
        )
        .bind(retry_lease.batch_id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        2,
        "historical batch observations remain while current counts use latest attempt"
    );

    let before_version = retry_lease.version;
    let cancelling = store
        .request_cancel(account_id, task.id, "cancel-key", 6_000_000)
        .await
        .unwrap();
    assert_eq!(cancelling.status, ScanStatus::Running);
    let version = sqlx::query_scalar::<_, i64>("SELECT version FROM tasks_scan_tasks WHERE id=?")
        .bind(task.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        version, before_version,
        "cancel flag must not steal worker lease version"
    );
    let cancelled = store
        .finish_cancelled(&retry_lease, 7_000_000)
        .await
        .unwrap();
    assert_eq!(cancelled.status, ScanStatus::Cancelled);
}

#[tokio::test]
async fn running_cancel_before_the_next_500_item_batch_preserves_only_committed_work() {
    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let root = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join("cancel-root");
    std::fs::create_dir(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Incoming","container_path":root,"access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let roots = DeploymentRootSet::load(
        &fixture.config().deployment_roots_file,
        mediaflow_core::bootstrap::config::RunMode::Development,
    )
    .unwrap();
    let fs: Arc<dyn CapabilityFs> = Arc::new(
        OsCapabilityFs::open(
            roots.declarations(),
            mediaflow_core::bootstrap::config::RunMode::Development,
        )
        .unwrap(),
    );
    let discovery = DiscoveryService::new(roots.view_map(), fs, db.pool().clone());
    let inbox = discovery
        .create_inbox(CreateInboxCommand {
            root_id: "incoming".to_owned(),
            relative_path: ".".to_owned(),
        })
        .await
        .unwrap();
    let store = TaskStore::new(db.pool().clone());
    let task = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox.id,
            idempotency_key: "cancel-boundary-create".to_owned(),
            now_us: 1_000_000,
        })
        .await
        .unwrap();
    let first_batch_committed = Arc::new(Notify::new());
    let continue_after_cancel = Arc::new(Notify::new());
    let worker = ScanWorker::new(
        "cancel-worker".to_owned(),
        store.clone(),
        discovery,
        Arc::new(BoundaryScanner {
            first_batch_committed: Arc::clone(&first_batch_committed),
            continue_after_cancel: Arc::clone(&continue_after_cancel),
        }),
        Arc::new(ManualTaskClock::new(2_000_000)),
    );
    let running = tokio::spawn(async move { worker.run_once().await });
    first_batch_committed.notified().await;
    let version_before_cancel =
        sqlx::query_scalar::<_, i64>("SELECT version FROM tasks_scan_tasks WHERE id=?")
            .bind(task.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap();
    store
        .request_cancel(account_id, task.id, "cancel-boundary", 2_100_000)
        .await
        .unwrap();
    let version_after_cancel =
        sqlx::query_scalar::<_, i64>("SELECT version FROM tasks_scan_tasks WHERE id=?")
            .bind(task.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(version_after_cancel, version_before_cancel);
    continue_after_cancel.notify_one();
    assert_eq!(running.await.unwrap().unwrap(), Some(task.id));

    let finished = store.get(account_id, task.id).await.unwrap();
    assert_eq!(finished.status, ScanStatus::Cancelled);
    assert_eq!(finished.counts.visited_directories, 1);
    assert_eq!(finished.counts.observed_files, 500);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM discovery_scan_file_observations WHERE scan_batch_id=(
                 SELECT scan_batch_id FROM tasks_scan_tasks WHERE id=?)",
        )
        .bind(task.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        500
    );
}

#[tokio::test]
async fn renewal_failure_stops_and_joins_scanner_and_runtime_reports_a_stable_failure() {
    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let root = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join("renewal-root");
    std::fs::create_dir(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Incoming","container_path":root,"access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let roots = DeploymentRootSet::load(
        &fixture.config().deployment_roots_file,
        mediaflow_core::bootstrap::config::RunMode::Development,
    )
    .unwrap();
    let fs: Arc<dyn CapabilityFs> = Arc::new(
        OsCapabilityFs::open(
            roots.declarations(),
            mediaflow_core::bootstrap::config::RunMode::Development,
        )
        .unwrap(),
    );
    let discovery = DiscoveryService::new(roots.view_map(), fs, db.pool().clone());
    let inbox = discovery
        .create_inbox(CreateInboxCommand {
            root_id: "incoming".to_owned(),
            relative_path: ".".to_owned(),
        })
        .await
        .unwrap();
    let store = TaskStore::new(db.pool().clone());
    store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox.id,
            idempotency_key: "renewal-create".to_owned(),
            now_us: 1_000_000,
        })
        .await
        .unwrap();
    let clock = Arc::new(ManualTaskClock::new(2_000_000));
    let started = Arc::new(Notify::new());
    let active = Arc::new(AtomicBool::new(false));
    let worker = ScanWorker::new(
        "renewal-worker".to_owned(),
        store,
        discovery,
        Arc::new(RenewalBlockingScanner {
            started: Arc::clone(&started),
            active: Arc::clone(&active),
        }),
        clock.clone(),
    )
    .with_renewal_interval(std::time::Duration::from_millis(5));
    let runtime = TaskRuntime::new(worker);
    let mut failures = runtime.subscribe_failures();
    let runtime_handle = runtime.start();
    started.notified().await;
    clock.set(40_000_001);

    tokio::time::timeout(std::time::Duration::from_secs(1), failures.changed())
        .await
        .unwrap()
        .unwrap();
    let failure = failures.borrow().clone().unwrap();
    assert_eq!(failure.code, ErrorCode::TaskLeaseLost);
    assert!(!active.load(Ordering::SeqCst));
    runtime_handle.abort();
}

#[tokio::test]
async fn idempotency_key_is_account_scoped_across_targets_actions_and_concurrent_requests() {
    let (_fixture, db, store, account_id, inbox_a) = fixture().await;
    let inbox_b = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',X'62','b',X'01',X'03','available',1,1,1,1)",
    )
    .bind(inbox_b.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();

    let first = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_a,
            idempotency_key: "account-bound".to_owned(),
            now_us: 1_000_000,
        })
        .await
        .unwrap();
    let different_target = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_b,
            idempotency_key: "account-bound".to_owned(),
            now_us: 1_000_001,
        })
        .await
        .unwrap_err();
    assert_eq!(different_target.code(), ErrorCode::RequestConflict);
    assert_eq!(
        store
            .create(NewScanTask {
                account_id,
                inbox_directory_id: inbox_a,
                idempotency_key: "account-bound".to_owned(),
                now_us: 1_000_002,
            })
            .await
            .unwrap()
            .id,
        first.id
    );

    let lease = store
        .claim_next("idempotency-worker", 2_000_000)
        .await
        .unwrap()
        .unwrap();
    store.finish_failed(&lease, 2_100_000).await.unwrap();
    store
        .retry(account_id, first.id, "cross-action", 2_200_000)
        .await
        .unwrap();
    let different_action = store
        .request_cancel(account_id, first.id, "cross-action", 2_300_000)
        .await
        .unwrap_err();
    assert_eq!(different_action.code(), ErrorCode::RequestConflict);

    let store_a = store.clone();
    let store_b = store.clone();
    let (left, right) = tokio::join!(
        store_a.create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_a,
            idempotency_key: "concurrent-bound".to_owned(),
            now_us: 3_000_000,
        }),
        store_b.create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_b,
            idempotency_key: "concurrent-bound".to_owned(),
            now_us: 3_000_001,
        })
    );
    let outcomes = [left, right];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter_map(|result| result.as_ref().err())
            .filter(|error| error.code() == ErrorCode::RequestConflict)
            .count(),
        1
    );
}

#[tokio::test]
async fn fresh_runtime_recovers_same_task_and_batch_and_reenumerates_unique_files() {
    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let root = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join("restart-root");
    std::fs::create_dir(&root).unwrap();
    for index in 0..620 {
        std::fs::write(root.join(format!("file-{index:04}.mkv")), b"x").unwrap();
    }
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Incoming","container_path":root,"access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let roots = DeploymentRootSet::load(
        &fixture.config().deployment_roots_file,
        mediaflow_core::bootstrap::config::RunMode::Development,
    )
    .unwrap();
    let fs: Arc<dyn CapabilityFs> = Arc::new(
        OsCapabilityFs::open(
            roots.declarations(),
            mediaflow_core::bootstrap::config::RunMode::Development,
        )
        .unwrap(),
    );
    let discovery = DiscoveryService::new(roots.view_map(), fs, db.pool().clone());
    let inbox = discovery
        .create_inbox(CreateInboxCommand {
            root_id: "incoming".to_owned(),
            relative_path: ".".to_owned(),
        })
        .await
        .unwrap();
    let store = TaskStore::new(db.pool().clone());
    let task = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox.id,
            idempotency_key: "restart-create".to_owned(),
            now_us: 1_000_000,
        })
        .await
        .unwrap();
    let first_batch_committed = Arc::new(Notify::new());
    let first_worker = ScanWorker::new(
        "first-process-worker".to_owned(),
        store.clone(),
        discovery,
        Arc::new(CrashAfterFirstBatchScanner {
            first_batch_committed: Arc::clone(&first_batch_committed),
        }),
        Arc::new(ManualTaskClock::new(2_000_000)),
    );
    let first_handle = tokio::spawn(async move { first_worker.run_once().await });
    first_batch_committed.notified().await;
    let original_batch =
        sqlx::query_scalar::<_, Vec<u8>>("SELECT scan_batch_id FROM tasks_scan_tasks WHERE id=?")
            .bind(task.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap();
    first_handle.abort();
    let _ = first_handle.await;
    sqlx::query("UPDATE tasks_scan_tasks SET lease_expires_at_us=0 WHERE id=?")
        .bind(task.id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    drop(store);
    db.pool().close().await;
    drop(db);

    let reopened = migrate_with_backup(fixture.config()).await.unwrap();
    let fresh_store = TaskStore::new(reopened.pool().clone());
    let runtime = TaskRuntime::from_app(fixture.config(), &reopened).unwrap();
    assert_eq!(runtime.prepare().await.unwrap(), 1);
    let runtime_handle = runtime.start();
    let finished = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let current = fresh_store.get(account_id, task.id).await.unwrap();
            if current.status == ScanStatus::Completed {
                break current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    runtime_handle.abort();

    assert_eq!(finished.counts.observed_files, 620);
    assert!(!finished.recovering);
    assert_eq!(
        sqlx::query_scalar::<_, Vec<u8>>("SELECT scan_batch_id FROM tasks_scan_tasks WHERE id=?",)
            .bind(task.id.as_bytes().as_slice())
            .fetch_one(reopened.pool())
            .await
            .unwrap(),
        original_batch
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tasks_scan_attempts WHERE task_id=? AND reason='recovery'",
        )
        .bind(task.id.as_bytes().as_slice())
        .fetch_one(reopened.pool())
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM discovery_scan_file_observations WHERE scan_batch_id=?",
        )
        .bind(&original_batch)
        .fetch_one(reopened.pool())
        .await
        .unwrap(),
        620
    );
}
