mod common;

use std::sync::Arc;
use std::time::Duration;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::DiscoveryUseCases;
use mediaflow_core::discovery::capability::{CapabilityFs, DeploymentRootSet};
use mediaflow_core::discovery::model::CreateInboxCommand;
use mediaflow_core::discovery::service::DiscoveryService;
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::db::Db;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::task_runtime::TaskRuntime;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::tasks::model::NewScanTask;
use mediaflow_core::tasks::store::TaskStore;
use uuid::Uuid;

type PersistedRootFault = (
    String,
    String,
    Vec<u8>,
    String,
    i64,
    String,
    i64,
    String,
    String,
    String,
    i64,
    i64,
);

#[tokio::test]
async fn failed_task_transaction_leaves_no_task_or_filesystem_fact() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let root = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join("database-failure-root");
    std::fs::create_dir(&root).unwrap();
    let sentinel = root.join("must-not-be-observed.media");
    std::fs::write(&sentinel, b"outside the failed transaction").unwrap();
    let (db, account_id, inbox_id) = initialize(&fixture, &root).await;
    sqlx::query(
        "CREATE TRIGGER fail_initial_task_event BEFORE INSERT ON platform_outbox_events
         WHEN NEW.event_type='task.state-changed'
         BEGIN SELECT RAISE(FAIL, 'forced task transaction failure'); END",
    )
    .execute(db.pool())
    .await
    .unwrap();

    let error = TaskStore::new(db.pool().clone())
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_id,
            idempotency_key: "transaction-failure".to_owned(),
            now_us: 1,
        })
        .await
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::Internal);
    for table in [
        "tasks_scan_tasks",
        "tasks_scan_attempts",
        "discovery_scan_batches",
        "discovery_scan_file_observations",
        "discovery_files",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} must roll back");
    }
    assert_eq!(
        std::fs::read(sentinel).unwrap(),
        b"outside the failed transaction"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn root_replaced_by_symlink_during_enumeration_never_observes_outside_sentinel() {
    use std::os::unix::fs::symlink;

    let (fixture, db, runtime, task_id, root) = running_fixture("symlink-race", 15_000).await;
    let outside = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("outside-sentinel.media"), b"never observe me").unwrap();
    let handle = runtime.start();
    wait_for_observations(&db, task_id, 500).await;
    let displaced = root.with_extension("displaced");
    std::fs::rename(&root, &displaced).unwrap();
    symlink(&outside, &root).unwrap();

    wait_for_status(&db, task_id, "failed").await;
    assert_persisted_root_fault(&db, task_id, "root.unavailable").await;
    handle.abort();
    let _ = handle.await;
    let outside_observations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM discovery_files WHERE relative_path_display LIKE '%outside-sentinel%'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(outside_observations, 0);
    assert_eq!(
        std::fs::read(outside.join("outside-sentinel.media")).unwrap(),
        b"never observe me"
    );
    std::fs::remove_file(&root).unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn permission_revocation_during_enumeration_fails_closed() {
    use std::os::unix::fs::PermissionsExt as _;

    let effective_uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .expect("Linux test requires id -u");
    assert!(effective_uid.status.success());
    if String::from_utf8_lossy(&effective_uid.stdout).trim() == "0" {
        eprintln!("skipped: permission revocation requires a non-root Linux runner");
        return;
    }

    let (_fixture, db, runtime, task_id, root) = running_fixture("permission-race", 15_000).await;
    let handle = runtime.start();
    wait_for_observations(&db, task_id, 500).await;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0,
        "Linux harness must actually revoke directory mode bits"
    );
    if std::fs::read_dir(&root).is_ok() {
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        handle.abort();
        let _ = handle.await;
        panic!("Linux harness did not revoke effective directory access");
    }
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        wait_for_status(&db, task_id, "failed"),
    )
    .await;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    result.expect("Linux permission revocation must fail closed");
    assert_persisted_root_fault(&db, task_id, "root.unavailable").await;
    handle.abort();
    let _ = handle.await;
}

async fn initialize(fixture: &common::TestConfigDir, root: &std::path::Path) -> (Db, Uuid, Uuid) {
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Security fixture","container_path":root,"access":"read-only"
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
        RunMode::Development,
    )
    .unwrap();
    let fs: Arc<dyn CapabilityFs> =
        Arc::new(OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap());
    let inbox = DiscoveryService::new(roots.view_map(), fs, db.pool().clone())
        .create_inbox(CreateInboxCommand {
            root_id: "incoming".to_owned(),
            relative_path: ".".to_owned(),
        })
        .await
        .unwrap();
    (db, account_id, inbox.id)
}

async fn running_fixture(
    label: &str,
    file_count: usize,
) -> (
    common::TestConfigDir,
    Db,
    TaskRuntime,
    Uuid,
    std::path::PathBuf,
) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let root = fixture.config().config_dir.parent().unwrap().join(label);
    std::fs::create_dir(&root).unwrap();
    for index in 0..file_count {
        std::fs::write(root.join(format!("file-{index:05}.media")), b"x").unwrap();
    }
    let (db, account_id, inbox_id) = initialize(&fixture, &root).await;
    let task = TaskStore::new(db.pool().clone())
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_id,
            idempotency_key: label.to_owned(),
            now_us: 1,
        })
        .await
        .unwrap();
    let runtime = TaskRuntime::from_app(fixture.config(), &db).unwrap();
    (fixture, db, runtime, task.id, root)
}

async fn wait_for_observations(db: &Db, task_id: Uuid, minimum: i64) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM discovery_scan_file_observations o
                 JOIN tasks_scan_tasks t ON t.scan_batch_id=o.scan_batch_id WHERE t.id=?",
            )
            .bind(task_id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap();
            if count >= minimum {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("worker must commit one real batch before fault injection");
}

async fn wait_for_status(db: &Db, task_id: Uuid, expected: &str) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let status: String =
                sqlx::query_scalar("SELECT status FROM tasks_scan_tasks WHERE id=?")
                    .bind(task_id.as_bytes().as_slice())
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            if status == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("worker must reach the expected terminal status");
}

async fn assert_persisted_root_fault(db: &Db, task_id: Uuid, expected_code: &str) {
    let fact: PersistedRootFault = sqlx::query_as(
        "SELECT e.scope,e.code,e.relative_path_bytes,e.relative_path_display,e.occurrences,
                a.reason,a.ordinal,a.status,t.status,t.stage,t.errors,
                (SELECT COUNT(*) FROM tasks_scan_errors all_errors
                   WHERE all_errors.attempt_id=t.current_attempt_id)
         FROM tasks_scan_tasks t
         JOIN tasks_scan_attempts a ON a.id=t.current_attempt_id
         JOIN tasks_scan_errors e ON e.attempt_id=t.current_attempt_id
         WHERE t.id=? ORDER BY e.first_seen_seq DESC LIMIT 1",
    )
    .bind(task_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(fact.0, "root");
    assert_eq!(fact.1, expected_code);
    assert_eq!(fact.2, b".");
    assert_eq!(fact.3, ".");
    assert_eq!(fact.4, 1, "root fault must have one causal occurrence");
    assert_eq!(fact.5, "initial");
    assert_eq!(fact.6, 1, "fault must belong to the latest initial attempt");
    assert_eq!(fact.7, "failed");
    assert_eq!(fact.8, "failed");
    assert_eq!(fact.9, "finished");
    assert_eq!(fact.10, 1, "terminal task error count");
    assert_eq!(fact.11, 1, "latest attempt persisted error count");
}
