mod common;

use async_trait::async_trait;
use common::organization::{organization_fixture, organization_fixture_with_nfo};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::model::{FsBoundaryError, RelativePath, RootId};
use mediaflow_core::organization::coordinator::{
    OrganizationFileOperationHandler, OrganizationNfoHandler, OrganizationPlanningHandler,
};
use mediaflow_core::organization::executor::{ExecutionOutcome, OrganizationExecutor};
use mediaflow_core::organization::fs::{
    AppliedFile, CompensationOutcome, FileLocator, FileOperationError, FileOperationSpec,
    NfoOperationSpec, ObservedFile, OrganizationFs, ProcessingStopToken, TargetCapabilitySnapshot,
    VerifiedOperation,
};
use mediaflow_core::organization::journal_store::{JournalStatus, JournalStore, LocalNfoStatus};
use mediaflow_core::organization::model::{
    ConfirmedNfoMetadata, OrganizationNamingPattern, OrganizationNfoPolicy, OrganizationOperation,
    OrganizationRuleInput, OrganizationTargetKind,
};
use mediaflow_core::organization::plan_service::{
    OrganizationPlanService, OrganizationPlanningPort,
};
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::organization::planner::{PlanningIdentity, PlanningInput};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use mediaflow_core::shared::error::{AppError, ErrorCode};
use mediaflow_core::tasks::processing::model::{ProcessingCheckpoint, ProcessingReason};
use mediaflow_core::tasks::processing::model::{ProcessingStage, ProcessingStatus};
use mediaflow_core::tasks::processing::store::ProcessingStore;
use mediaflow_core::tasks::processing::worker::{
    ProcessingHandlerOutcome, ProcessingStageHandler, ProcessingStopToken as WorkerStopToken,
    ProcessingWorker,
};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[test]
fn organization_processing_contract_exposes_stable_checkpoints_and_reasons() {
    assert_eq!(
        ProcessingCheckpoint::ExecutionAuthorized.as_str(),
        "execution-authorized"
    );
    assert_eq!(ProcessingCheckpoint::NfoFailed.as_str(), "nfo-failed");
    assert_eq!(
        ProcessingCheckpoint::CatalogCommitted.as_str(),
        "catalog-committed"
    );
    assert_eq!(
        ProcessingReason::OrganizationIoTemporary.as_str(),
        "organization.io-temporary"
    );
    assert_eq!(
        ProcessingReason::OrganizationNfoFailed.as_str(),
        "organization.nfo-failed"
    );
}

struct FailOnceHandler {
    first: AtomicBool,
}

#[async_trait]
impl ProcessingStageHandler for FailOnceHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Identification
    }

    async fn run(
        &self,
        _lease: &mediaflow_core::tasks::processing::model::ProcessingLease,
        _stop: WorkerStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        if self.first.swap(false, Ordering::SeqCst) {
            Err(AppError::new(ErrorCode::Internal, "injected task failure"))
        } else {
            Ok(ProcessingHandlerOutcome::IdentificationComplete {
                reason: ProcessingReason::IdentificationConfirmedTitleYear,
            })
        }
    }
}

#[tokio::test]
async fn one_task_handler_failure_does_not_prevent_the_next_task_from_converging() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let first_revision =
        common::seed_stable_revision(db.pool(), inbox, b"first.mkv", vec![1]).await;
    let second_revision =
        common::seed_stable_revision(db.pool(), inbox, b"second.mkv", vec![2]).await;
    let tasks = ProcessingStore::new(db.pool().clone());
    let first = tasks
        .ensure_revision(first_revision, 91_000_001)
        .await
        .unwrap();
    let second = tasks
        .ensure_revision(second_revision, 91_000_002)
        .await
        .unwrap();
    let worker = ProcessingWorker::new(
        "organization-failure-isolation".to_owned(),
        tasks.clone(),
        vec![Arc::new(FailOnceHandler {
            first: AtomicBool::new(true),
        })],
        Arc::new(ManualTaskClock::new(100_000_000)),
    )
    .unwrap();

    assert_eq!(
        worker.run_once().await.unwrap_err().code(),
        ErrorCode::Internal
    );
    assert_eq!(worker.run_once().await.unwrap(), Some(second.id));
    let converged = tasks.get(account, second.id).await.unwrap();
    assert_eq!(converged.status, ProcessingStatus::Queued);
    assert_eq!(converged.stage, ProcessingStage::Planning);
    assert_eq!(
        tasks.get(account, first.id).await.unwrap().status,
        ProcessingStatus::Running
    );
}

#[derive(Clone)]
struct FixedPlanningInput(PlanningInput);

#[async_trait]
impl OrganizationPlanningPort for FixedPlanningInput {
    async fn load(
        &self,
        _account_id: uuid::Uuid,
        _task_id: uuid::Uuid,
    ) -> Result<PlanningInput, AppError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn no_rule_plan_pauses_and_plan_scoped_one_time_authorization_resumes_execution() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let mut input = movie_planning_input(&fixture);
    input.target.automatic = false;
    input.target.rules.clear();
    let port: Arc<dyn OrganizationPlanningPort> = Arc::new(FixedPlanningInput(input));
    let service = OrganizationPlanService::new(port, fixture.db.pool().clone());
    let paused_plan = service
        .ensure_current(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(
        paused_plan.authorization,
        mediaflow_core::organization::planner::PlanAuthorization::Paused
    );
    let tasks = ProcessingStore::new(fixture.db.pool().clone());
    let clock = Arc::new(ManualTaskClock::new(93_000_000));
    let handler = OrganizationPlanningHandler::new(
        fixture.account_id,
        service.clone(),
        tasks.clone(),
        clock.clone(),
    );
    handler
        .run(&fixture.lease, WorkerStopToken::default())
        .await
        .unwrap();
    let paused = tasks
        .get(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(paused.status, ProcessingStatus::Paused);
    assert_eq!(paused.checkpoint, ProcessingCheckpoint::PlanningPaused);
    assert_eq!(paused.organization_plan_id, Some(paused_plan.id));

    tasks
        .retry(
            fixture.account_id,
            paused.id,
            "retry-authorized-plan",
            94_000_000,
        )
        .await
        .unwrap();
    let authorized = service
        .authorize_once(
            fixture.account_id,
            paused.id,
            paused_plan.version,
            "authorize-current-plan-once",
        )
        .await
        .unwrap();
    assert_eq!(
        authorized.authorization,
        mediaflow_core::organization::planner::PlanAuthorization::OneTime
    );
    let lease = tasks
        .claim_next(
            "organization-planning",
            &[ProcessingStage::Planning],
            95_000_000,
        )
        .await
        .unwrap()
        .unwrap();
    clock.set(96_000_000);
    handler
        .run(&lease, WorkerStopToken::default())
        .await
        .unwrap();
    let queued = tasks.get(fixture.account_id, paused.id).await.unwrap();
    assert_eq!(queued.status, ProcessingStatus::Queued);
    assert_eq!(queued.stage, ProcessingStage::FileOperation);
    assert_eq!(queued.checkpoint, ProcessingCheckpoint::ExecutionAuthorized);
    assert_eq!(queued.organization_plan_id, Some(paused_plan.id));
}

#[tokio::test]
async fn ambiguous_generic_grouping_is_persisted_as_a_planning_pause() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let mut input = movie_planning_input(&fixture);
    input.identity = PlanningIdentity::GenericVideo {
        title: "Unsorted clip".to_owned(),
        group: None,
        sequence: None,
    };
    input.target.kind = OrganizationTargetKind::GenericVideo;
    input.target.naming_pattern = OrganizationNamingPattern::GenericNumbered;
    input.target.rules = vec![OrganizationRuleInput {
        media_kind: OrganizationTargetKind::GenericVideo,
        inbox_directory_id: Some(input.source_inbox_id),
        explicit_tag: None,
        enabled: true,
    }];
    let service = OrganizationPlanService::new(
        Arc::new(FixedPlanningInput(input)),
        fixture.db.pool().clone(),
    );
    let tasks = ProcessingStore::new(fixture.db.pool().clone());
    OrganizationPlanningHandler::new(
        fixture.account_id,
        service,
        tasks.clone(),
        Arc::new(ManualTaskClock::new(93_000_000)),
    )
    .run(&fixture.lease, WorkerStopToken::default())
    .await
    .unwrap();
    let paused = tasks
        .get(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(paused.status, ProcessingStatus::Paused);
    assert_eq!(paused.stage, ProcessingStage::Planning);
    assert_eq!(paused.checkpoint, ProcessingCheckpoint::PlanningPaused);
    assert_eq!(
        paused.reason,
        Some(ProcessingReason::OrganizationPlanPaused)
    );
}

fn movie_planning_input(fixture: &common::organization::OrganizationFixture) -> PlanningInput {
    PlanningInput {
        task_id: fixture.lease.task.id,
        file_revision_id: fixture.lease.task.file_revision_id,
        selected_identity_id: fixture.plan.draft.selected_identity_id,
        source: fixture.plan.draft.source.clone(),
        source_inbox_id: fixture.lease.task.inbox_directory_id,
        source_writable: false,
        source_unchanged: true,
        destination_exists: false,
        same_filesystem: true,
        explicit_tags: BTreeSet::default(),
        one_time_authorized: false,
        identity: PlanningIdentity::Movie {
            title: "Arrival".to_owned(),
            year: Some(2016),
            version_label: None,
        },
        nfo_metadata: ConfirmedNfoMetadata::default(),
        target: fixture.plan.draft.target.clone(),
    }
}

#[tokio::test]
async fn cancellation_after_file_verification_keeps_result_and_stops_at_the_safe_boundary() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let tasks = ProcessingStore::new(fixture.db.pool().clone());
    tasks
        .finish_organization_plan(&fixture.lease, fixture.plan.id, true, 93_000_000)
        .await
        .unwrap();
    let file_lease = tasks
        .claim_next(
            "organization-cancel",
            &[ProcessingStage::FileOperation],
            94_000_000,
        )
        .await
        .unwrap()
        .unwrap();
    let journals = JournalStore::new(fixture.db.pool().clone());
    let executor = OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        journals.clone(),
        fixture.fs.clone(),
        Arc::new(ManualTaskClock::new(95_000_000)),
    );
    let result = match executor
        .run_file_stage(&file_lease, ProcessingStopToken::default())
        .await
        .unwrap()
    {
        ExecutionOutcome::Completed(result) => result,
        outcome => panic!("unexpected execution outcome: {outcome:?}"),
    };
    tasks
        .request_cancel(
            fixture.account_id,
            file_lease.task.id,
            "cancel-after-file-verified",
            96_000_000,
        )
        .await
        .unwrap();

    let cancelled = tasks
        .finish_organization_file(&file_lease, result.id, false, 97_000_000)
        .await
        .unwrap();
    assert_eq!(cancelled.status, ProcessingStatus::Cancelled);
    assert_eq!(cancelled.stage, ProcessingStage::FileOperation);
    assert_eq!(cancelled.checkpoint, ProcessingCheckpoint::Cancelled);
    assert_eq!(cancelled.organization_result_id, Some(result.id));
    assert_eq!(
        journals
            .for_task(fixture.account_id, file_lease.task.id)
            .await
            .unwrap()[0]
            .status,
        JournalStatus::Verified
    );
}

#[tokio::test]
async fn nfo_failure_is_partial_success_and_manual_retry_never_repeats_the_file_copy() {
    let fixture = organization_fixture_with_nfo(
        OrganizationOperation::Copy,
        OrganizationNfoPolicy::GenerateMissing,
    )
    .await;
    let tasks = ProcessingStore::new(fixture.db.pool().clone());
    tasks
        .finish_organization_plan(&fixture.lease, fixture.plan.id, true, 93_000_000)
        .await
        .unwrap();
    let counts = Arc::new(FailureCounts::default());
    let fs: Arc<dyn OrganizationFs> = Arc::new(FailFirstNfoFs {
        inner: fixture.fs.clone(),
        counts: counts.clone(),
        fail_copy: AtomicBool::new(false),
        fail_nfo: AtomicBool::new(true),
    });
    let clock = Arc::new(ManualTaskClock::new(100_000_000));
    let executor = OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fs,
        clock.clone(),
    );
    let handlers: Vec<Arc<dyn ProcessingStageHandler>> = vec![
        Arc::new(OrganizationFileOperationHandler::new(
            executor.clone(),
            tasks.clone(),
            clock.clone(),
        )),
        Arc::new(OrganizationNfoHandler::new(
            executor,
            tasks.clone(),
            clock.clone(),
        )),
    ];
    let worker = ProcessingWorker::new(
        "organization-nfo".to_owned(),
        tasks.clone(),
        handlers,
        clock.clone(),
    )
    .unwrap();

    worker.run_once().await.unwrap();
    worker.run_once().await.unwrap();
    let partial = tasks
        .get(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(partial.status, ProcessingStatus::PartialSuccess);
    assert_eq!(partial.stage, ProcessingStage::Nfo);
    assert_eq!(partial.checkpoint, ProcessingCheckpoint::NfoFailed);
    assert_eq!(counts.file_copies.load(Ordering::SeqCst), 1);
    assert_eq!(counts.nfo_writes.load(Ordering::SeqCst), 1);

    tasks
        .retry(
            fixture.account_id,
            partial.id,
            "retry-only-nfo",
            101_000_000,
        )
        .await
        .unwrap();
    clock.set(102_000_000);
    worker.run_once().await.unwrap();
    let ready = tasks.get(fixture.account_id, partial.id).await.unwrap();
    assert_eq!(ready.status, ProcessingStatus::Queued);
    assert_eq!(ready.stage, ProcessingStage::Completion);
    assert_eq!(ready.checkpoint, ProcessingCheckpoint::LocalResultPrepared);
    assert_eq!(counts.file_copies.load(Ordering::SeqCst), 1);
    assert_eq!(counts.nfo_writes.load(Ordering::SeqCst), 2);
    let result = JournalStore::new(fixture.db.pool().clone())
        .result_for_task(fixture.account_id, partial.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.nfo_status, LocalNfoStatus::Generated);
}

#[tokio::test]
async fn temporary_file_failure_recovers_from_the_single_executing_journal() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let tasks = ProcessingStore::new(fixture.db.pool().clone());
    tasks
        .finish_organization_plan(&fixture.lease, fixture.plan.id, true, 93_000_000)
        .await
        .unwrap();
    let counts = Arc::new(FailureCounts::default());
    let fs: Arc<dyn OrganizationFs> = Arc::new(FailFirstNfoFs {
        inner: fixture.fs.clone(),
        counts: counts.clone(),
        fail_copy: AtomicBool::new(true),
        fail_nfo: AtomicBool::new(false),
    });
    let clock = Arc::new(ManualTaskClock::new(100_000_000));
    let executor = OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fs,
        clock.clone(),
    );
    let worker = ProcessingWorker::new(
        "organization-file-recovery".to_owned(),
        tasks.clone(),
        vec![Arc::new(OrganizationFileOperationHandler::new(
            executor,
            tasks.clone(),
            clock.clone(),
        ))],
        clock.clone(),
    )
    .unwrap();

    worker.run_once().await.unwrap();
    let paused = tasks
        .get(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(paused.status, ProcessingStatus::Paused);
    assert_eq!(
        paused.checkpoint,
        ProcessingCheckpoint::FileOperationExecuting
    );
    assert_eq!(counts.file_copies.load(Ordering::SeqCst), 1);

    clock.set(131_000_000);
    worker.run_once().await.unwrap();
    let ready = tasks.get(fixture.account_id, paused.id).await.unwrap();
    assert_eq!(ready.status, ProcessingStatus::Queued);
    assert_eq!(ready.stage, ProcessingStage::Completion);
    assert_eq!(counts.file_copies.load(Ordering::SeqCst), 2);
    let journals = JournalStore::new(fixture.db.pool().clone())
        .for_task(fixture.account_id, paused.id)
        .await
        .unwrap();
    assert_eq!(journals.len(), 1);
    assert_eq!(journals[0].status, JournalStatus::Verified);
}

#[derive(Default)]
struct FailureCounts {
    file_copies: AtomicUsize,
    nfo_writes: AtomicUsize,
}

struct FailFirstNfoFs {
    inner: Arc<mediaflow_core::platform::capability_fs::OsCapabilityFs>,
    counts: Arc<FailureCounts>,
    fail_copy: AtomicBool,
    fail_nfo: AtomicBool,
}

impl OrganizationFs for FailFirstNfoFs {
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
        self.counts.file_copies.fetch_add(1, Ordering::SeqCst);
        if self.fail_copy.swap(false, Ordering::SeqCst) {
            Err(FileOperationError::IoTemporary)
        } else {
            self.inner.copy_no_clobber(operation, stop)
        }
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

    fn write_new_nfo(
        &self,
        operation: &NfoOperationSpec,
        bytes: &[u8],
    ) -> Result<AppliedFile, FileOperationError> {
        self.counts.nfo_writes.fetch_add(1, Ordering::SeqCst);
        if self.fail_nfo.swap(false, Ordering::SeqCst) {
            Err(FileOperationError::IoTemporary)
        } else {
            self.inner.write_new_nfo(operation, bytes)
        }
    }

    fn remove_verified_source(
        &self,
        operation_id: uuid::Uuid,
        source: &ObservedFile,
        expected_sha256: &[u8; 32],
    ) -> Result<(), FileOperationError> {
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
