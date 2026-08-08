mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::tasks::processing::model::{
    ProcessingReason, ProcessingStage, ProcessingStatus,
};
use mediaflow_core::tasks::processing::store::{PROCESSING_LEASE_DURATION_US, ProcessingStore};

#[tokio::test]
async fn active_lease_is_exclusive_renewable_and_recovers_with_a_new_attempt() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"movie.mkv", vec![1]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, 90_000_001).await.unwrap();

    let lease = store
        .claim_next("worker-a", &[ProcessingStage::Identification], 100_000_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.task.id, task.id);
    assert!(
        store
            .claim_next("worker-b", &[ProcessingStage::Identification], 100_000_001)
            .await
            .unwrap()
            .is_none()
    );
    let renewed = store.renew(&lease, 110_000_000).await.unwrap();
    assert_eq!(
        renewed.expires_at_us,
        110_000_000 + PROCESSING_LEASE_DURATION_US
    );
    assert_eq!(
        store
            .reclaim_expired(renewed.expires_at_us - 1)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store.reclaim_expired(renewed.expires_at_us).await.unwrap(),
        1
    );
    let recovered = store.get_by_task_id(task.id).await.unwrap();
    assert_eq!(recovered.status, ProcessingStatus::Queued);
    assert!(recovered.recovering);
    assert_eq!(recovered.attempt_count, 2);
    let lease = store
        .claim_next(
            "worker-b",
            &[ProcessingStage::Identification],
            renewed.expires_at_us,
        )
        .await
        .unwrap()
        .unwrap();
    assert_ne!(lease.attempt_id, renewed.attempt_id);
}

#[tokio::test]
async fn due_dependency_retry_manual_retry_and_cancel_are_durable_and_idempotent() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"movie.mkv", vec![2]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, 90_000_001).await.unwrap();
    let lease = store
        .claim_next("worker", &[ProcessingStage::Identification], 100_000_000)
        .await
        .unwrap()
        .unwrap();
    store
        .finish_paused(
            &lease,
            ProcessingReason::IdentificationProviderUnavailable,
            200_000_000,
            100_000_001,
        )
        .await
        .unwrap();
    assert!(
        store
            .claim_next("worker", &[ProcessingStage::Identification], 199_999_999)
            .await
            .unwrap()
            .is_none()
    );
    let due = store
        .claim_next("worker", &[ProcessingStage::Identification], 200_000_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(due.task.attempt_count, 2);
    store
        .finish_paused(
            &due,
            ProcessingReason::IdentificationProviderUnavailable,
            300_000_000,
            200_000_001,
        )
        .await
        .unwrap();

    let retried = store
        .retry(account, task.id, "retry-key", 210_000_000)
        .await
        .unwrap();
    let duplicate = store
        .retry(account, task.id, "retry-key", 210_000_001)
        .await
        .unwrap();
    assert_eq!(retried, duplicate);
    assert_eq!(retried.attempt_count, 3);
    let conflict = store
        .request_cancel(account, task.id, "retry-key", 210_000_002)
        .await
        .unwrap_err();
    assert_eq!(conflict.code(), ErrorCode::RequestConflict);

    let cancelled = store
        .request_cancel(account, task.id, "cancel-key", 210_000_003)
        .await
        .unwrap();
    let duplicate = store
        .request_cancel(account, task.id, "cancel-key", 210_000_004)
        .await
        .unwrap();
    assert_eq!(cancelled, duplicate);
    assert_eq!(cancelled.status, ProcessingStatus::Cancelled);

    let plan = sqlx::query(
        "EXPLAIN QUERY PLAN SELECT id FROM tasks_processing_tasks
         WHERE status='paused' AND stage='identification' AND next_retry_at_us<=?
         ORDER BY next_retry_at_us,id LIMIT 1",
    )
    .bind(500_000_000_i64)
    .fetch_all(db.pool())
    .await
    .unwrap();
    let details = plan
        .iter()
        .map(|row| sqlx::Row::get::<String, _>(row, "detail"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(details.contains("tasks_processing_due_retry_idx"));
}

#[tokio::test]
async fn one_recovery_call_requeues_one_thousand_expired_tasks_without_scanning_paused_rows() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let mut tx = db.pool().begin().await.unwrap();
    for index in 0..1000_i64 {
        let file_id = uuid::Uuid::now_v7();
        let revision_id = uuid::Uuid::now_v7();
        let task_id = uuid::Uuid::now_v7();
        let attempt_id = uuid::Uuid::now_v7();
        let path = format!("bulk/{index}.mkv");
        sqlx::query(
            "INSERT INTO discovery_tracked_files
             (id,inbox_directory_id,relative_path_bytes,relative_path_display,current_revision_id,
              last_observed_at_us,missing_at_us,version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,1,NULL,1,1,1)",
        )
        .bind(file_id.as_bytes().as_slice())
        .bind(inbox.as_bytes().as_slice())
        .bind(path.as_bytes())
        .bind(&path)
        .bind(revision_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO discovery_file_revisions
             (id,tracked_file_id,identity_snapshot,size_bytes,modified_at_ns,policy_version,
              minimum_age_seconds,stable_observation_interval_seconds,created_at_us)
             VALUES (?,?,?,1,0,1,60,30,1)",
        )
        .bind(revision_id.as_bytes().as_slice())
        .bind(file_id.as_bytes().as_slice())
        .bind(index.to_be_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tasks_processing_tasks
             (id,account_id,discovered_file_id,file_revision_id,inbox_directory_id,
              current_attempt_id,status,stage,checkpoint,reason,recovering,attempt_count,
              cancel_requested,lease_owner,lease_expires_at_us,next_retry_at_us,
              config_snapshot_json,version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,'running','identification','pending',NULL,0,1,0,
                     'dead-worker',100,NULL,'{}',1,1,1)",
        )
        .bind(task_id.as_bytes().as_slice())
        .bind(account.as_bytes().as_slice())
        .bind(file_id.as_bytes().as_slice())
        .bind(revision_id.as_bytes().as_slice())
        .bind(inbox.as_bytes().as_slice())
        .bind(attempt_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tasks_processing_attempts
             (id,task_id,reason,ordinal,status,stage,lease_owner,started_at_us,created_at_us)
             VALUES (?,?,'initial',1,'running','identification','dead-worker',1,1)",
        )
        .bind(attempt_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    let started = std::time::Instant::now();
    assert_eq!(
        ProcessingStore::new(db.pool().clone())
            .reclaim_expired(100)
            .await
            .unwrap(),
        1000
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tasks_processing_tasks
             WHERE status='queued' AND recovering=1 AND attempt_count=2",
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1000
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks_processing_attempts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        2000
    );
}
