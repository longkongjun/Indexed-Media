mod common;

use std::sync::Arc;

use common::organization::{OrganizationFixture, organization_fixture};
use mediaflow_core::organization::executor::{
    ExecutionOutcome, OrganizationExecutor, RollbackCommand,
};
use mediaflow_core::organization::fs::ProcessingStopToken;
use mediaflow_core::organization::journal_store::{JournalStatus, JournalStore, LocalResultStatus};
use mediaflow_core::organization::model::OrganizationOperation;
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::platform::task_runtime::ManualTaskClock;

#[tokio::test]
async fn unchanged_copy_hardlink_and_move_are_compensated_safely() {
    for operation in [
        OrganizationOperation::Copy,
        OrganizationOperation::Hardlink,
        OrganizationOperation::Move,
    ] {
        let fixture = organization_fixture(operation).await;
        let executor = executor(&fixture);
        let ExecutionOutcome::Completed(result) = executor
            .run_next(&fixture.lease, ProcessingStopToken::default())
            .await
            .unwrap()
        else {
            panic!("operation must complete before rollback");
        };

        let compensated = executor
            .rollback(RollbackCommand {
                task_id: fixture.lease.task.id,
                result_version: result.version,
                idempotency_key: "rollback-once".to_owned(),
            })
            .await
            .unwrap();

        assert_eq!(compensated.status, LocalResultStatus::Compensated);
        assert!(!fixture.destination_path().exists());
        assert!(fixture.source_path().exists());
        let replay = executor
            .rollback(RollbackCommand {
                task_id: fixture.lease.task.id,
                result_version: result.version,
                idempotency_key: "rollback-once".to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(replay, compensated);
        let journal = JournalStore::new(fixture.db.pool().clone())
            .for_task(fixture.account_id, fixture.lease.task.id)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(journal.status, JournalStatus::Compensated);
    }
}

#[tokio::test]
async fn external_changes_and_missing_move_parent_require_manual_review() {
    let changed = organization_fixture(OrganizationOperation::Copy).await;
    let changed_executor = executor(&changed);
    let ExecutionOutcome::Completed(result) = changed_executor
        .run_next(&changed.lease, ProcessingStopToken::default())
        .await
        .unwrap()
    else {
        panic!("copy must complete");
    };
    std::fs::write(changed.destination_path(), b"external-change").unwrap();
    let manual = changed_executor
        .rollback(RollbackCommand {
            task_id: changed.lease.task.id,
            result_version: result.version,
            idempotency_key: "changed-target".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(manual.status, LocalResultStatus::ManualReview);
    assert_eq!(
        std::fs::read(changed.destination_path()).unwrap(),
        b"external-change"
    );

    let missing_parent = organization_fixture(OrganizationOperation::Move).await;
    let move_executor = executor(&missing_parent);
    let ExecutionOutcome::Completed(result) = move_executor
        .run_next(&missing_parent.lease, ProcessingStopToken::default())
        .await
        .unwrap()
    else {
        panic!("move must complete");
    };
    std::fs::remove_dir(missing_parent.source_path().parent().unwrap()).unwrap();
    let manual = move_executor
        .rollback(RollbackCommand {
            task_id: missing_parent.lease.task.id,
            result_version: result.version,
            idempotency_key: "missing-parent".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(manual.status, LocalResultStatus::ManualReview);
    assert!(missing_parent.destination_path().exists());
    assert!(!missing_parent.source_path().exists());
}

fn executor(fixture: &OrganizationFixture) -> OrganizationExecutor {
    OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fixture.fs.clone(),
        Arc::new(ManualTaskClock::new(5_000)),
    )
}
