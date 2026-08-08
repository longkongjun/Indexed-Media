mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::task_runtime::{ManualTaskClock, ProcessingTaskRuntime, TaskClock};
use mediaflow_core::shared::error::{AppError, ErrorCode};
use mediaflow_core::tasks::processing::model::{
    ProcessingLease, ProcessingReason, ProcessingStage, ProcessingStatus,
};
use mediaflow_core::tasks::processing::store::ProcessingStore;
use mediaflow_core::tasks::processing::worker::{
    ProcessingHandlerOutcome, ProcessingStageHandler, ProcessingStopToken, ProcessingWorker,
};

#[tokio::test]
async fn worker_runs_only_registered_stage_and_stops_after_identification_checkpoint() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"movie.mkv", vec![1]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, 90_000_001).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let worker = ProcessingWorker::new(
        "processing-worker".to_owned(),
        store.clone(),
        vec![Arc::new(CompleteHandler {
            calls: Arc::clone(&calls),
        })],
        Arc::new(ManualTaskClock::new(100_000_000)),
    )
    .unwrap();

    assert_eq!(worker.run_once().await.unwrap(), Some(task.id));
    let stopped = store.get_by_task_id(task.id).await.unwrap();
    assert_eq!(stopped.status, ProcessingStatus::Queued);
    assert_eq!(stopped.stage, ProcessingStage::Planning);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(worker.run_once().await.unwrap(), None);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn worker_does_not_apply_a_second_transition_after_handler_owned_transaction() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"owned.mkv", vec![9]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, 90_000_001).await.unwrap();
    let worker = ProcessingWorker::new(
        "processing-owned-transaction".to_owned(),
        store.clone(),
        vec![Arc::new(CommittedHandler {
            store: store.clone(),
        })],
        Arc::new(ManualTaskClock::new(100_000_000)),
    )
    .unwrap();

    assert_eq!(worker.run_once().await.unwrap(), Some(task.id));
    let committed = store.get_by_task_id(task.id).await.unwrap();
    assert_eq!(committed.status, ProcessingStatus::Queued);
    assert_eq!(committed.stage, ProcessingStage::Planning);
    assert_eq!(committed.attempt_count, 1);
}

#[tokio::test]
async fn renewal_loss_stops_handler_and_prevents_a_stale_terminal_commit() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"movie.mkv", vec![2]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, 90_000_001).await.unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let stopped = Arc::new(AtomicBool::new(false));
    let worker = ProcessingWorker::new(
        "processing-worker".to_owned(),
        store.clone(),
        vec![Arc::new(BlockingHandler {
            started: Arc::clone(&started),
            stopped: Arc::clone(&stopped),
        })],
        Arc::new(ManualTaskClock::new(100_000_000)),
    )
    .unwrap()
    .with_renewal_interval(std::time::Duration::from_millis(5));

    let running = tokio::spawn(async move { worker.run_once().await });
    started.notified().await;
    sqlx::query("UPDATE tasks_processing_tasks SET version=version+1 WHERE id=?")
        .bind(task.id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::TaskLeaseLost);
    assert!(stopped.load(Ordering::SeqCst));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM tasks_processing_tasks WHERE id=?")
            .bind(task.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "running"
    );
}

#[tokio::test]
async fn running_cancel_stops_handler_and_commits_only_the_cancel_checkpoint() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"movie.mkv", vec![3]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, 90_000_001).await.unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let stopped = Arc::new(AtomicBool::new(false));
    let worker = ProcessingWorker::new(
        "processing-worker".to_owned(),
        store.clone(),
        vec![Arc::new(BlockingHandler {
            started: Arc::clone(&started),
            stopped: Arc::clone(&stopped),
        })],
        Arc::new(ManualTaskClock::new(100_000_000)),
    )
    .unwrap()
    .with_renewal_interval(std::time::Duration::from_millis(5));

    let running = tokio::spawn(async move { worker.run_once().await });
    started.notified().await;
    let requested = store
        .request_cancel(account, task.id, "running-cancel", 100_000_001)
        .await
        .unwrap();
    assert_eq!(requested.status, ProcessingStatus::Running);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(1), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Some(task.id)
    );
    assert!(stopped.load(Ordering::SeqCst));
    let cancelled = store.get_by_task_id(task.id).await.unwrap();
    assert_eq!(cancelled.status, ProcessingStatus::Cancelled);
    assert_eq!(cancelled.attempt_count, 1);
}

#[tokio::test]
async fn runtime_prepare_materializes_pending_requests_before_recovering_expired_leases() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    common::seed_stable_revision(db.pool(), inbox, b"movie.mkv", vec![4]).await;
    let clock = Arc::new(ManualTaskClock::new(100_000_000));
    let handlers: Vec<Arc<dyn ProcessingStageHandler>> = Vec::new();
    let runtime = ProcessingTaskRuntime::new_with_concurrency(
        db.pool().clone(),
        OutboxNotifier::new(),
        &handlers,
        clock.clone(),
        1,
    )
    .unwrap();
    let first = runtime.prepare().await.unwrap();
    assert_eq!(first.ensured_tasks, 1);
    assert_eq!(first.recovered_leases, 0);

    let store = ProcessingStore::new(db.pool().clone());
    let lease = store
        .claim_next(
            "dead-worker",
            &[ProcessingStage::Identification],
            clock.now_us(),
        )
        .await
        .unwrap()
        .unwrap();
    clock.set(lease.expires_at_us);
    let second = runtime.prepare().await.unwrap();
    assert_eq!(second.ensured_tasks, 0);
    assert_eq!(second.recovered_leases, 1);
}

struct CompleteHandler {
    calls: Arc<AtomicUsize>,
}

struct CommittedHandler {
    store: ProcessingStore,
}

#[async_trait]
impl ProcessingStageHandler for CommittedHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Identification
    }

    async fn run(
        &self,
        lease: &ProcessingLease,
        _stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        self.store
            .finish_identification_complete(
                lease,
                ProcessingReason::IdentificationConfirmedTitleYear,
                100_000_001,
            )
            .await?;
        Ok(ProcessingHandlerOutcome::Committed)
    }
}

#[async_trait]
impl ProcessingStageHandler for CompleteHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Identification
    }

    async fn run(
        &self,
        _lease: &ProcessingLease,
        _stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ProcessingHandlerOutcome::IdentificationComplete {
            reason: ProcessingReason::IdentificationConfirmedTitleYear,
        })
    }
}

struct BlockingHandler {
    started: Arc<tokio::sync::Notify>,
    stopped: Arc<AtomicBool>,
}

#[async_trait]
impl ProcessingStageHandler for BlockingHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Identification
    }

    async fn run(
        &self,
        _lease: &ProcessingLease,
        stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        self.started.notify_one();
        while !stop.is_stopped() {
            tokio::task::yield_now().await;
        }
        self.stopped.store(true, Ordering::SeqCst);
        Ok(ProcessingHandlerOutcome::IdentificationComplete {
            reason: ProcessingReason::IdentificationConfirmedTitleYear,
        })
    }
}

#[allow(dead_code)]
fn _clock_is_object_safe(clock: &dyn TaskClock) -> i64 {
    clock.now_us()
}
