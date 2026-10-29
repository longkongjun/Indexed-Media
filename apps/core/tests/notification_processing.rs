//! Business state conversions and notification facts share the same `SQLite` transaction.
mod common;
use common::organization::organization_fixture;
use mediaflow_core::organization::executor::{
    ExecutionOutcome, OrganizationExecutor, RollbackCommand,
};
use mediaflow_core::organization::fs::ProcessingStopToken;
use mediaflow_core::organization::model::OrganizationOperation;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use mediaflow_core::tasks::processing::store::ProcessingStore;
use std::sync::Arc;

#[tokio::test]
async fn organization_pause_creates_one_notification_and_rejected_repeat_does_not_duplicate() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let store = ProcessingStore::new(fixture.db.pool().clone());
    store
        .finish_organization_io_pending(&fixture.lease, 100, 20)
        .await
        .unwrap();
    assert!(
        store
            .finish_organization_io_pending(&fixture.lease, 100, 21)
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications WHERE kind='organization.paused' AND task_id=?",
    )
    .bind(fixture.lease.task.id.as_bytes().as_slice())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn safe_rollback_creates_one_notification_and_replay_does_not_duplicate() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let executor = OrganizationExecutor::new(
        fixture.account_id,
        mediaflow_core::organization::plan_store::OrganizationPlanStore::new(
            fixture.db.pool().clone(),
        ),
        mediaflow_core::organization::journal_store::JournalStore::new(fixture.db.pool().clone()),
        fixture.fs.clone(),
        Arc::new(ManualTaskClock::new(100)),
    );
    let ExecutionOutcome::Completed(result) = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap()
    else {
        panic!("copy must complete");
    };
    for _ in 0..2 {
        executor
            .rollback(RollbackCommand {
                task_id: fixture.lease.task.id,
                result_version: result.version,
                idempotency_key: "notification-rollback".to_owned(),
            })
            .await
            .unwrap();
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications WHERE kind='organization.rolled-back' AND task_id=?",
    )
    .bind(fixture.lease.task.id.as_bytes().as_slice())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(count, 1);
}
