mod common;

use std::os::unix::fs::MetadataExt as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::organization::organization_fixture;
use mediaflow_core::discovery::model::{FsBoundaryError, RelativePath, RootId};
use mediaflow_core::organization::executor::{
    ExecutionOutcome, OrganizationExecutor, RollbackCommand,
};
use mediaflow_core::organization::fs::{
    AppliedFile, CompensationOutcome, FileLocator, FileOperationError, FileOperationSpec,
    NfoOperationSpec, ObservedFile, OrganizationFs, ProcessingStopToken, TargetCapabilitySnapshot,
    VerifiedOperation,
};
use mediaflow_core::organization::journal_store::{JournalStatus, JournalStore, LocalResultStatus};
use mediaflow_core::organization::model::OrganizationOperation;
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::organization::target_store::OrganizationTargetStore;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use uuid::Uuid;

#[tokio::test]
async fn successful_copy_move_and_hardlink_commit_verified_journals() {
    for operation in [
        OrganizationOperation::Copy,
        OrganizationOperation::Move,
        OrganizationOperation::Hardlink,
    ] {
        let fixture = organization_fixture(operation).await;
        let source_inode = std::fs::metadata(fixture.source_path()).unwrap().ino();
        let executor = executor(&fixture);

        let outcome = executor
            .run_next(&fixture.lease, ProcessingStopToken::default())
            .await
            .unwrap();

        assert!(matches!(outcome, ExecutionOutcome::Completed(_)));
        assert_eq!(
            std::fs::read(fixture.destination_path()).unwrap(),
            b"arrival-media"
        );
        assert_eq!(
            fixture.source_path().exists(),
            operation != OrganizationOperation::Move
        );
        if operation == OrganizationOperation::Hardlink {
            assert_eq!(
                std::fs::metadata(fixture.destination_path()).unwrap().ino(),
                source_inode
            );
        }
        let journals = JournalStore::new(fixture.db.pool().clone())
            .for_task(fixture.account_id, fixture.lease.task.id)
            .await
            .unwrap();
        assert_eq!(journals.len(), 1);
        assert_eq!(journals[0].status, JournalStatus::Verified);
        OrganizationTargetStore::new(fixture.db.pool().clone())
            .delete(
                fixture.account_id,
                fixture.plan.draft.target.id,
                fixture.plan.draft.target.config_version,
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn target_source_and_stop_failures_never_overwrite_or_skip_journal_boundaries() {
    let occupied = organization_fixture(OrganizationOperation::Copy).await;
    std::fs::create_dir_all(occupied.destination_path().parent().unwrap()).unwrap();
    std::fs::write(occupied.destination_path(), b"external").unwrap();
    let outcome = executor(&occupied)
        .run_next(&occupied.lease, ProcessingStopToken::default())
        .await
        .unwrap();
    assert!(matches!(outcome, ExecutionOutcome::ManualReview(_)));
    assert_eq!(
        std::fs::read(occupied.destination_path()).unwrap(),
        b"external"
    );

    let changed = organization_fixture(OrganizationOperation::Copy).await;
    std::fs::write(changed.source_path(), b"changed-after-revision").unwrap();
    let outcome = executor(&changed)
        .run_next(&changed.lease, ProcessingStopToken::default())
        .await
        .unwrap();
    assert!(matches!(outcome, ExecutionOutcome::Paused { .. }));
    assert!(!changed.destination_path().exists());

    let stopped = organization_fixture(OrganizationOperation::Copy).await;
    let stop = ProcessingStopToken::default();
    stop.stop();
    let outcome = executor(&stopped)
        .run_next(&stopped.lease, stop)
        .await
        .unwrap();
    assert_eq!(outcome, ExecutionOutcome::Stopped);
    assert!(!stopped.destination_path().exists());
    let journal = JournalStore::new(stopped.db.pool().clone())
        .for_task(stopped.account_id, stopped.lease.task.id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(journal.status, JournalStatus::Executing);
}

#[tokio::test]
async fn composite_move_uses_one_copy_and_one_verified_source_removal_journal() {
    let fixture = organization_fixture(OrganizationOperation::Move).await;
    let counts = Arc::new(CompositeCounts::default());
    let fs: Arc<dyn OrganizationFs> = Arc::new(CompositeFs {
        inner: fixture.fs.clone(),
        counts: counts.clone(),
    });
    let executor = OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fs,
        Arc::new(ManualTaskClock::new(10_000)),
    );

    let outcome = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap();

    let ExecutionOutcome::Completed(result) = outcome else {
        panic!("composite move must complete");
    };
    assert_eq!(counts.moves.load(Ordering::SeqCst), 1);
    assert_eq!(counts.copies.load(Ordering::SeqCst), 1);
    assert_eq!(counts.removals.load(Ordering::SeqCst), 1);
    assert!(!fixture.source_path().exists());
    assert_eq!(
        std::fs::read(fixture.destination_path()).unwrap(),
        b"arrival-media"
    );
    let journals = JournalStore::new(fixture.db.pool().clone())
        .for_task(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(journals.len(), 2);
    assert!(
        journals
            .iter()
            .all(|journal| journal.status == JournalStatus::Verified)
    );

    let compensated = executor
        .rollback(RollbackCommand {
            task_id: fixture.lease.task.id,
            result_version: result.version,
            idempotency_key: "rollback-composite".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(compensated.status, LocalResultStatus::Compensated);
    assert!(fixture.source_path().exists());
    assert!(!fixture.destination_path().exists());
    let journals = JournalStore::new(fixture.db.pool().clone())
        .for_task(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert!(
        journals
            .iter()
            .all(|journal| journal.status == JournalStatus::Compensated)
    );
}

fn executor(fixture: &common::organization::OrganizationFixture) -> OrganizationExecutor {
    OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fixture.fs.clone(),
        Arc::new(ManualTaskClock::new(1_000)),
    )
}

#[derive(Default)]
struct CompositeCounts {
    moves: AtomicUsize,
    copies: AtomicUsize,
    removals: AtomicUsize,
}

struct CompositeFs {
    inner: Arc<mediaflow_core::platform::capability_fs::OsCapabilityFs>,
    counts: Arc<CompositeCounts>,
}

impl OrganizationFs for CompositeFs {
    fn preflight_target(
        &self,
        root: &RootId,
        path: &RelativePath,
    ) -> Result<TargetCapabilitySnapshot, FsBoundaryError> {
        self.inner.preflight_target(root, path)
    }

    fn observe(&self, locator: &FileLocator) -> Result<Option<ObservedFile>, FsBoundaryError> {
        self.inner.observe(locator)
    }

    fn inspect(&self, locator: &FileLocator) -> Result<Option<AppliedFile>, FsBoundaryError> {
        self.inner.inspect(locator)
    }

    fn copy_no_clobber(
        &self,
        operation: &FileOperationSpec,
        stop: &ProcessingStopToken,
    ) -> Result<AppliedFile, FileOperationError> {
        self.counts.copies.fetch_add(1, Ordering::SeqCst);
        self.inner.copy_no_clobber(operation, stop)
    }

    fn move_no_clobber(
        &self,
        _operation: &FileOperationSpec,
    ) -> Result<AppliedFile, FileOperationError> {
        self.counts.moves.fetch_add(1, Ordering::SeqCst);
        Err(FileOperationError::CompositeMoveRequired)
    }

    fn hardlink_no_clobber(
        &self,
        operation: &FileOperationSpec,
    ) -> Result<AppliedFile, FileOperationError> {
        self.inner.hardlink_no_clobber(operation)
    }

    fn write_new_nfo(
        &self,
        operation: &NfoOperationSpec,
        bytes: &[u8],
    ) -> Result<AppliedFile, FileOperationError> {
        self.inner.write_new_nfo(operation, bytes)
    }

    fn remove_verified_source(
        &self,
        operation_id: Uuid,
        source: &ObservedFile,
        expected_sha256: &[u8; 32],
    ) -> Result<(), FileOperationError> {
        self.counts.removals.fetch_add(1, Ordering::SeqCst);
        self.inner
            .remove_verified_source(operation_id, source, expected_sha256)
    }

    fn compensate(
        &self,
        operation: &VerifiedOperation,
    ) -> Result<CompensationOutcome, FileOperationError> {
        self.inner.compensate(operation)
    }
}
