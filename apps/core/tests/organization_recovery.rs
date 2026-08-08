mod common;

use std::sync::Arc;

use common::organization::{OrganizationFixture, organization_fixture};
use mediaflow_core::organization::executor::{ExecutionOutcome, OrganizationExecutor};
use mediaflow_core::organization::fs::{FileOperationSpec, OrganizationFs, ProcessingStopToken};
use mediaflow_core::organization::journal_store::{JournalStatus, JournalStore};
use mediaflow_core::organization::model::OrganizationOperation;
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::platform::task_runtime::ManualTaskClock;

#[derive(Clone, Copy)]
enum CrashPoint {
    Prepared,
    Executing,
    FilesystemApplied,
    Applied,
    Verified,
}

#[tokio::test]
async fn every_crash_window_recovers_without_reapplying_a_matching_target() {
    for crash in [
        CrashPoint::Prepared,
        CrashPoint::Executing,
        CrashPoint::FilesystemApplied,
        CrashPoint::Applied,
        CrashPoint::Verified,
    ] {
        let fixture = organization_fixture(OrganizationOperation::Copy).await;
        arrange_crash(&fixture, crash).await;

        let outcome = executor(&fixture).recover(&fixture.lease).await.unwrap();

        assert!(matches!(outcome, ExecutionOutcome::Completed(_)));
        assert_eq!(
            std::fs::read(fixture.destination_path()).unwrap(),
            b"arrival-media"
        );
        let journals = JournalStore::new(fixture.db.pool().clone())
            .for_task(fixture.account_id, fixture.lease.task.id)
            .await
            .unwrap();
        assert_eq!(journals[0].status, JournalStatus::Verified);
        assert_eq!(journals[0].projection_version, 4);
    }
}

#[tokio::test]
async fn source_removal_response_loss_observes_absence_and_does_not_delete_twice() {
    let fixture = organization_fixture(OrganizationOperation::Move).await;
    let store = JournalStore::new(fixture.db.pool().clone());
    let source = fixture
        .fs
        .observe(&fixture.source_locator())
        .unwrap()
        .unwrap();
    let mut parent = store
        .prepare_file(fixture.account_id, &fixture.plan, source.clone(), 1_000)
        .await
        .unwrap();
    parent = store
        .mark_executing(parent.id, parent.projection_version, 1_100)
        .await
        .unwrap();
    let applied = fixture
        .fs
        .copy_no_clobber(
            &FileOperationSpec::new(
                fixture.plan.operations[0].id,
                source,
                fixture.destination_locator(),
            ),
            &ProcessingStopToken::default(),
        )
        .unwrap();
    parent = store
        .mark_applied(parent.id, parent.projection_version, &applied, 1_200)
        .await
        .unwrap();
    parent = store
        .verify(parent.id, parent.projection_version, 1_300)
        .await
        .unwrap();
    store
        .ensure_partial_result(fixture.account_id, &parent, 1_400)
        .await
        .unwrap();
    let mut child = store
        .prepare_source_removal(fixture.account_id, &parent, 1_500)
        .await
        .unwrap();
    child = store
        .mark_executing(child.id, child.projection_version, 1_600)
        .await
        .unwrap();
    fixture
        .fs
        .remove_verified_source(
            child.operation_id,
            child.expected_source.as_ref().unwrap(),
            parent.applied.as_ref().unwrap().sha256(),
        )
        .unwrap();
    assert!(!fixture.source_path().exists());

    let outcome = executor(&fixture).recover(&fixture.lease).await.unwrap();

    assert!(matches!(outcome, ExecutionOutcome::Completed(_)));
    assert!(!fixture.source_path().exists());
    let journals = store
        .for_task(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(journals.len(), 2);
    assert!(
        journals
            .iter()
            .all(|journal| journal.status == JournalStatus::Verified)
    );
}

#[tokio::test]
async fn executing_with_an_ambiguous_operation_temp_enters_manual_review() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let store = JournalStore::new(fixture.db.pool().clone());
    let expected = fixture
        .fs
        .observe(&fixture.source_locator())
        .unwrap()
        .unwrap();
    let mut journal = store
        .prepare_file(fixture.account_id, &fixture.plan, expected, 1_000)
        .await
        .unwrap();
    journal = store
        .mark_executing(journal.id, journal.projection_version, 1_100)
        .await
        .unwrap();
    let destination = fixture.destination_path();
    let parent = destination.parent().unwrap();
    std::fs::create_dir_all(parent).unwrap();
    let temporary = parent.join(format!(".mediaflow-{}.tmp", journal.operation_id));
    std::fs::write(&temporary, b"ambiguous").unwrap();

    let outcome = executor(&fixture).recover(&fixture.lease).await.unwrap();

    assert!(matches!(outcome, ExecutionOutcome::ManualReview(_)));
    assert!(temporary.exists());
    assert!(!fixture.destination_path().exists());
    let journal = store
        .get(fixture.account_id, journal.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(journal.status, JournalStatus::ManualReview);
}

#[tokio::test]
async fn journal_transitions_reject_skips_and_stale_projection_versions() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let store = JournalStore::new(fixture.db.pool().clone());
    let expected = fixture
        .fs
        .observe(&fixture.source_locator())
        .unwrap()
        .unwrap();
    let journal = store
        .prepare_file(fixture.account_id, &fixture.plan, expected, 1_000)
        .await
        .unwrap();

    assert!(
        store
            .verify(journal.id, journal.projection_version, 1_100)
            .await
            .is_err()
    );
    let executing = store
        .mark_executing(journal.id, journal.projection_version, 1_200)
        .await
        .unwrap();
    assert!(
        store
            .mark_executing(journal.id, journal.projection_version, 1_300)
            .await
            .is_err()
    );
    assert_eq!(executing.status, JournalStatus::Executing);
    assert_eq!(executing.projection_version, 2);
}

async fn arrange_crash(fixture: &OrganizationFixture, crash: CrashPoint) {
    let store = JournalStore::new(fixture.db.pool().clone());
    let expected = fixture
        .fs
        .observe(&fixture.source_locator())
        .unwrap()
        .unwrap();
    let mut journal = store
        .prepare_file(fixture.account_id, &fixture.plan, expected.clone(), 1_000)
        .await
        .unwrap();
    if matches!(crash, CrashPoint::Prepared) {
        return;
    }
    journal = store
        .mark_executing(journal.id, journal.projection_version, 1_100)
        .await
        .unwrap();
    if matches!(crash, CrashPoint::Executing) {
        return;
    }
    let applied = fixture
        .fs
        .copy_no_clobber(
            &FileOperationSpec::new(
                fixture.plan.operations[0].id,
                expected,
                fixture.destination_locator(),
            ),
            &ProcessingStopToken::default(),
        )
        .unwrap();
    if matches!(crash, CrashPoint::FilesystemApplied) {
        return;
    }
    journal = store
        .mark_applied(journal.id, journal.projection_version, &applied, 1_200)
        .await
        .unwrap();
    if matches!(crash, CrashPoint::Applied) {
        return;
    }
    store
        .verify(journal.id, journal.projection_version, 1_300)
        .await
        .unwrap();
}

fn executor(fixture: &OrganizationFixture) -> OrganizationExecutor {
    OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fixture.fs.clone(),
        Arc::new(ManualTaskClock::new(2_000)),
    )
}
