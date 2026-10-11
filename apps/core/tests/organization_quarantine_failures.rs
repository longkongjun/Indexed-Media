//! A2 闸门的 quarantine 失败注入矩阵：崩溃窗口恢复、跨盘组合、磁盘满（临时 I/O）与
//! 重复请求幂等。全部使用真实临时文件系统 + 真实 journal 状态机，失败点经装饰器注入。

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::organization::{OrganizationFixture, organization_fixture};
use mediaflow_core::discovery::model::{FsBoundaryError, RelativePath, RootId};
use mediaflow_core::organization::executor::{ExecutionOutcome, OrganizationExecutor};
use mediaflow_core::organization::fs::{
    AppliedFile, FileOperationError, FileOperationSpec, NfoOperationSpec, ObservedFile,
    OrganizationFs, ProcessingStopToken, TargetCapabilitySnapshot,
};
use mediaflow_core::organization::journal_store::{JournalStatus, JournalStore};
use mediaflow_core::organization::model::OrganizationOperation;
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::platform::task_runtime::ManualTaskClock;

/// quarantine 调用的可注入失败点。
#[derive(Clone, Copy, Debug, PartialEq)]
enum QuarantineFailure {
    /// 第 N 次调用返回临时 I/O（磁盘满/瞬时故障的恢复语义）。
    TemporaryIoOnAttempt(usize),
    /// 所有调用返回跨设备，强制走 copy+verified-removal 组合路径。
    AlwaysCrossDevice,
}

/// 包装真实能力文件系统的失败注入装饰器；仅拦截 quarantine，其余原样委托。
struct QuarantineFailpointFs {
    inner: Arc<mediaflow_core::platform::capability_fs::OsCapabilityFs>,
    failure: QuarantineFailure,
    calls: AtomicUsize,
    /// 跨盘注入时底层 copy 的调用计数（证明组合路径被使用）。
    copies: AtomicUsize,
    removals: AtomicUsize,
    fail_once: AtomicBool,
}

impl OrganizationFs for QuarantineFailpointFs {
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
        self.copies.fetch_add(1, Ordering::SeqCst);
        self.inner.copy_no_clobber(operation, stop)
    }

    fn move_no_clobber(
        &self,
        operation: &FileOperationSpec,
    ) -> Result<AppliedFile, FileOperationError> {
        self.inner.move_no_clobber(operation)
    }

    fn hardlink_no_clobber(
        &self,
        operation: &FileOperationSpec,
    ) -> Result<AppliedFile, FileOperationError> {
        self.inner.hardlink_no_clobber(operation)
    }

    fn quarantine_no_clobber(
        &self,
        operation: &FileOperationSpec,
    ) -> Result<AppliedFile, FileOperationError> {
        let attempt = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        match self.failure {
            QuarantineFailure::TemporaryIoOnAttempt(target) if attempt == target => {
                // 磁盘满/瞬时 I/O 的恢复语义：journal 保持可恢复，稍后重试。
                return Err(FileOperationError::IoTemporary);
            }
            QuarantineFailure::AlwaysCrossDevice => {
                return Err(FileOperationError::CompositeMoveRequired);
            }
            QuarantineFailure::TemporaryIoOnAttempt(_) => {}
        }
        if self.fail_once.swap(false, Ordering::SeqCst) {
            return Err(FileOperationError::IoTemporary);
        }
        self.inner.quarantine_no_clobber(operation)
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
        operation_id: uuid::Uuid,
        source: &ObservedFile,
        expected_sha256: &[u8; 32],
    ) -> Result<(), FileOperationError> {
        self.removals.fetch_add(1, Ordering::SeqCst);
        self.inner
            .remove_verified_source(operation_id, source, expected_sha256)
    }

    fn compensate(
        &self,
        operation: &mediaflow_core::organization::fs::VerifiedOperation,
    ) -> Result<mediaflow_core::organization::fs::CompensationOutcome, FileOperationError> {
        self.inner.compensate(operation)
    }
}

use mediaflow_core::organization::fs::FileLocator;

fn failpoint_executor(
    fixture: &OrganizationFixture,
    fs: Arc<QuarantineFailpointFs>,
) -> OrganizationExecutor {
    OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fs,
        Arc::new(ManualTaskClock::new(1_000)),
    )
}

fn real_executor(fixture: &OrganizationFixture) -> OrganizationExecutor {
    executor_from(fixture, fixture.fs.clone())
}

fn executor_from(
    fixture: &OrganizationFixture,
    fs: Arc<dyn OrganizationFs>,
) -> OrganizationExecutor {
    OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fs,
        Arc::new(ManualTaskClock::new(1_000)),
    )
}

#[tokio::test]
async fn disk_full_style_temporary_failure_recovers_on_the_next_run() {
    let fixture = organization_fixture(OrganizationOperation::Quarantine).await;
    let fs = Arc::new(QuarantineFailpointFs {
        inner: fixture.fs.clone(),
        failure: QuarantineFailure::TemporaryIoOnAttempt(1),
        calls: AtomicUsize::new(0),
        copies: AtomicUsize::new(0),
        removals: AtomicUsize::new(0),
        fail_once: AtomicBool::new(false),
    });
    let executor = failpoint_executor(&fixture, fs.clone());

    // 第一次运行注入磁盘满语义：journal 保持可恢复而不是人工处理。
    let outcome = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap();
    assert!(
        matches!(outcome, ExecutionOutcome::RecoveryPending { .. }),
        "temporary I/O must stay recoverable: {outcome:?}"
    );
    assert!(fixture.source_path().exists(), "source must be untouched");

    // 重试收敛：同一 journal 继续，产物唯一且 verified。
    let outcome = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap();
    assert!(matches!(outcome, ExecutionOutcome::Completed(_)));
    assert!(!fixture.source_path().exists());
    assert_eq!(
        std::fs::read(fixture.destination_path()).unwrap(),
        b"arrival-media"
    );
    assert_eq!(
        fs.calls.load(Ordering::SeqCst),
        2,
        "retry must reuse the journal, not replay from scratch"
    );
}

#[tokio::test]
async fn cross_device_quarantine_composes_copy_and_verified_removal_with_manifest() {
    let fixture = organization_fixture(OrganizationOperation::Quarantine).await;
    let fs = Arc::new(QuarantineFailpointFs {
        inner: fixture.fs.clone(),
        failure: QuarantineFailure::AlwaysCrossDevice,
        calls: AtomicUsize::new(0),
        copies: AtomicUsize::new(0),
        removals: AtomicUsize::new(0),
        fail_once: AtomicBool::new(false),
    });
    let executor = failpoint_executor(&fixture, fs.clone());

    let outcome = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap();
    assert!(matches!(outcome, ExecutionOutcome::Completed(_)));
    assert!(
        !fixture.source_path().exists(),
        "cross-device quarantine must remove the verified source"
    );
    assert_eq!(
        std::fs::read(fixture.destination_path()).unwrap(),
        b"arrival-media"
    );
    assert!(
        fs.copies.load(Ordering::SeqCst) >= 1,
        "composite path must copy"
    );
    assert!(
        fs.removals.load(Ordering::SeqCst) >= 1,
        "composite path must remove the source"
    );
    let manifest = fixture.destination_path().parent().unwrap().join(format!(
        ".mediaflow-{}.quarantine.json",
        fixture.plan.operations[0].id
    ));
    assert!(
        manifest.is_file(),
        "cross-device quarantine must still publish the manifest"
    );
}

#[tokio::test]
async fn crash_after_filesystem_quarantine_recovers_without_a_second_move() {
    let fixture = organization_fixture(OrganizationOperation::Quarantine).await;
    // 排布崩溃窗口：journal 已 executing、文件已被真实 quarantine、DB 未记 applied。
    let store = JournalStore::new(fixture.db.pool().clone());
    let expected = fixture
        .fs
        .observe(&fixture.source_locator())
        .unwrap()
        .unwrap();
    let journal = store
        .prepare_file(fixture.account_id, &fixture.plan, expected.clone(), 1_000)
        .await
        .unwrap();
    store
        .mark_executing(journal.id, journal.projection_version, 1_100)
        .await
        .unwrap();
    fixture
        .fs
        .quarantine_no_clobber(&FileOperationSpec::new(
            fixture.plan.operations[0].id,
            expected,
            fixture.destination_locator(),
        ))
        .unwrap();

    let outcome = real_executor(&fixture)
        .recover(&fixture.lease)
        .await
        .unwrap();

    assert!(matches!(outcome, ExecutionOutcome::Completed(_)));
    assert!(!fixture.source_path().exists());
    assert_eq!(
        std::fs::read(fixture.destination_path()).unwrap(),
        b"arrival-media",
        "recovery must not duplicate or overwrite the quarantined file"
    );
    let journals = store
        .for_task(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(journals[0].status, JournalStatus::Verified);
    assert_eq!(
        journals[0].kind,
        mediaflow_core::organization::journal_store::JournalKind::Quarantine
    );
}

#[tokio::test]
async fn duplicate_run_next_after_verified_journal_is_idempotent() {
    let fixture = organization_fixture(OrganizationOperation::Quarantine).await;
    let executor = real_executor(&fixture);
    let first = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap();
    assert!(matches!(first, ExecutionOutcome::Completed(_)));
    let second = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap();
    assert!(
        matches!(
            second,
            ExecutionOutcome::Stopped | ExecutionOutcome::Paused { .. }
        ),
        "a verified journal must not be re-executed: {second:?}"
    );
    assert_eq!(
        std::fs::read(fixture.destination_path()).unwrap(),
        b"arrival-media",
        "duplicate request must not produce a second copy in trash"
    );
}
