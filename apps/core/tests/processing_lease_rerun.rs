mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::task_runtime::{ManualTaskClock, ProcessingTaskRuntime, TaskClock};
use mediaflow_core::shared::error::{AppError, ErrorCode};
use mediaflow_core::tasks::processing::model::{
    ProcessingLease, ProcessingReason, ProcessingStage,
};
use mediaflow_core::tasks::processing::store::{PROCESSING_LEASE_DURATION_US, ProcessingStore};
use mediaflow_core::tasks::processing::worker::{
    ProcessingHandlerOutcome, ProcessingStageHandler, ProcessingStopToken, ProcessingWorker,
};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use uuid::Uuid;

const T0: i64 = 100_000_000;

#[derive(Debug)]
struct TaskRow {
    status: String,
    stage: String,
    version: i64,
    lease_owner: Option<String>,
    lease_expires_at_us: Option<i64>,
    attempt_count: i64,
    next_retry_at_us: Option<i64>,
    reason: Option<String>,
}

#[derive(Debug)]
struct AttemptRow {
    ordinal: i64,
    reason: String,
    status: String,
    failure_code: Option<String>,
    started_at_us: Option<i64>,
    finished_at_us: Option<i64>,
}

async fn task_row(pool: &sqlx::SqlitePool, task_id: Uuid) -> TaskRow {
    let row = sqlx::query(
        "SELECT status,stage,version,lease_owner,lease_expires_at_us,attempt_count,
                next_retry_at_us,reason
         FROM tasks_processing_tasks WHERE id=?",
    )
    .bind(task_id.as_bytes().as_slice())
    .fetch_one(pool)
    .await
    .unwrap();
    TaskRow {
        status: row.get("status"),
        stage: row.get("stage"),
        version: row.get("version"),
        lease_owner: row.get("lease_owner"),
        lease_expires_at_us: row.get("lease_expires_at_us"),
        attempt_count: row.get("attempt_count"),
        next_retry_at_us: row.get("next_retry_at_us"),
        reason: row.get("reason"),
    }
}

async fn attempt_rows(pool: &sqlx::SqlitePool, task_id: Uuid) -> Vec<AttemptRow> {
    sqlx::query(
        "SELECT ordinal,reason,status,failure_code,started_at_us,finished_at_us
         FROM tasks_processing_attempts WHERE task_id=? ORDER BY ordinal",
    )
    .bind(task_id.as_bytes().as_slice())
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| AttemptRow {
        ordinal: row.get("ordinal"),
        reason: row.get("reason"),
        status: row.get("status"),
        failure_code: row.get("failure_code"),
        started_at_us: row.get("started_at_us"),
        finished_at_us: row.get("finished_at_us"),
    })
    .collect()
}

async fn log_timeline(label: &str, pool: &sqlx::SqlitePool, task_id: Uuid, now_us: i64) {
    let task = task_row(pool, task_id).await;
    eprintln!("[timeline] {label} now_us={now_us} task={task:?}");
    for attempt in attempt_rows(pool, task_id).await {
        eprintln!("[timeline]   {attempt:?}");
    }
}

/// 反例 A：阶段处理器返回确定性的非租约错误。
///
/// 期望：失败必须以真实错误码记录在当前 attempt 上并把任务转入可恢复状态；任务不得停留在
/// `running` 等待租约到期，再被 `claim_next` 标记为 `task.lease-lost` 并以 `lease-recovery`
/// attempt 无上限重跑。
#[tokio::test]
async fn handler_failure_must_not_be_recorded_as_lease_loss_and_rerun_by_expiry() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"failing.mkv", vec![31]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, T0 - 1).await.unwrap();
    let clock = Arc::new(ManualTaskClock::new(T0));
    let calls = Arc::new(AtomicUsize::new(0));
    let handlers: Vec<Arc<dyn ProcessingStageHandler>> = vec![Arc::new(FailingHandler {
        calls: Arc::clone(&calls),
    })];
    let runtime = ProcessingTaskRuntime::new_with_concurrency(
        db.pool().clone(),
        OutboxNotifier::new(),
        &handlers,
        clock.clone(),
        2,
    )
    .unwrap();
    runtime.prepare().await.unwrap();
    let mut failures = runtime.subscribe_failures();
    let handle = runtime.start();

    tokio::time::timeout(Duration::from_secs(5), failures.changed())
        .await
        .expect("first handler failure is published")
        .unwrap();
    let first_failure = failures.borrow().clone().unwrap();
    log_timeline("A.after-first-failure", db.pool(), task.id, clock.now_us()).await;
    let after_first = task_row(db.pool(), task.id).await;
    let first_attempts = attempt_rows(db.pool(), task.id).await;

    for cycle in 2..=3u32 {
        let current = task_row(db.pool(), task.id).await;
        match current.next_retry_at_us {
            Some(next_retry_at_us) if current.status == "paused" => clock.set(next_retry_at_us),
            _ => clock.advance(PROCESSING_LEASE_DURATION_US),
        }
        tokio::time::timeout(Duration::from_secs(5), failures.changed())
            .await
            .expect("handler failure is published again")
            .unwrap();
        log_timeline(
            &format!("A.after-cycle-{cycle}"),
            db.pool(),
            task.id,
            clock.now_us(),
        )
        .await;
    }
    handle.abort();
    let final_attempts = attempt_rows(db.pool(), task.id).await;

    assert_eq!(first_failure.code, ErrorCode::Internal);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_ne!(
        after_first.status, "running",
        "handler failure left the task running until lease expiry: {after_first:?}"
    );
    assert_eq!(after_first.status, "paused");
    assert!(
        after_first.lease_owner.is_none() && after_first.lease_expires_at_us.is_none(),
        "paused task must release its lease: {after_first:?}"
    );
    assert!(after_first.reason.is_none(), "{after_first:?}");
    assert_eq!(after_first.version, 3);
    assert_eq!(
        after_first.next_retry_at_us,
        Some(T0 + PROCESSING_LEASE_DURATION_US)
    );
    assert_eq!(first_attempts[0].status, "failed");
    assert_eq!(
        first_attempts[0].failure_code.as_deref(),
        Some(ErrorCode::Internal.as_str()),
        "attempt did not record the real failure: {first_attempts:?}"
    );
    assert!(
        final_attempts
            .iter()
            .all(|attempt| attempt.failure_code.as_deref() != Some("task.lease-lost")),
        "handler failures were attributed to lease loss: {final_attempts:?}"
    );
    assert!(
        final_attempts
            .iter()
            .all(|attempt| attempt.reason != "lease-recovery"),
        "handler failures produced lease-recovery attempts: {final_attempts:?}"
    );
}

/// 反例 B：处理器执行期间逻辑时钟越过租约到期，但没有任何其他 worker 接管该任务。
///
/// 期望：任务 `version` 与 `lease_owner` 仍属于当前 worker，终态提交必须成功；不得因为
/// `lease_expires_at_us` 已过而拒绝提交、把任务留在 `running` 并在下一轮回收重跑。
#[tokio::test]
async fn late_completion_without_takeover_must_commit_instead_of_rerunning() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"late.mkv", vec![32]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, T0 - 1).await.unwrap();
    let clock = Arc::new(ManualTaskClock::new(T0));
    let calls = Arc::new(AtomicUsize::new(0));
    let handlers: Vec<Arc<dyn ProcessingStageHandler>> = vec![Arc::new(OutliveLeaseHandler {
        clock: Arc::clone(&clock),
        calls: Arc::clone(&calls),
        pool: db.pool().clone(),
    })];
    let runtime = ProcessingTaskRuntime::new_with_concurrency(
        db.pool().clone(),
        OutboxNotifier::new(),
        &handlers,
        clock.clone(),
        2,
    )
    .unwrap();
    runtime.prepare().await.unwrap();
    let handle = runtime.start();

    // 持续重跑会让调用次数迅速超过 1；正确实现下在整个观察窗口内只会执行一次。
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
    while calls.load(Ordering::SeqCst) < 3 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    handle.abort();
    log_timeline("B.final", db.pool(), task.id, clock.now_us()).await;
    let final_task = task_row(db.pool(), task.id).await;
    let attempts = attempt_rows(db.pool(), task.id).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "handler was re-run after a late but still exclusive completion: {attempts:?}"
    );
    assert_eq!(final_task.status, "queued", "{final_task:?}");
    assert_eq!(final_task.stage, "planning", "{final_task:?}");
    assert_eq!(final_task.attempt_count, 1);
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt.failure_code.as_deref() != Some("task.lease-lost")),
        "{attempts:?}"
    );
}

/// 反例 C：续租循环遇到瞬时数据库不可用（连接池获取超时），租约本身并未过期或被接管。
///
/// 期望：瞬时错误不得终止心跳并停止处理器；连接恢复后续租继续，处理器正常完成并提交。
#[tokio::test]
async fn transient_renewal_failure_must_not_abandon_a_live_handler() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"transient.mkv", vec![33]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, T0 - 1).await.unwrap();
    // worker 使用单连接、短获取超时的独立连接池，测试据此确定性地制造一次瞬时 PoolTimedOut。
    let worker_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_millis(200))
        .connect_with(
            SqliteConnectOptions::new()
                .filename(fixture.database_path())
                .journal_mode(SqliteJournalMode::Wal)
                .foreign_keys(true)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await
        .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(AtomicBool::new(false));
    let stopped = Arc::new(AtomicBool::new(false));
    let worker = ProcessingWorker::new(
        "transient-worker".to_owned(),
        ProcessingStore::new(worker_pool.clone()),
        vec![Arc::new(ReleasableHandler {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
            stopped: Arc::clone(&stopped),
        })],
        Arc::new(ManualTaskClock::new(T0)),
    )
    .unwrap()
    .with_renewal_interval(Duration::from_millis(5));

    let running = tokio::spawn(async move { worker.run_once().await });
    started.notified().await;
    let held = worker_pool.acquire().await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    drop(held);
    tokio::time::sleep(Duration::from_millis(50)).await;
    release.store(true, Ordering::SeqCst);
    let result = tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap();
    log_timeline("C.final", db.pool(), task.id, T0).await;

    assert!(
        !stopped.load(Ordering::SeqCst),
        "handler was stopped by a transient renewal failure: {result:?}"
    );
    assert_eq!(result.unwrap(), Some(task.id));
    let final_task = task_row(db.pool(), task.id).await;
    assert_eq!(final_task.status, "queued", "{final_task:?}");
    assert_eq!(final_task.stage, "planning", "{final_task:?}");
}

/// 回归：租约真正到期且被另一个 worker 接管后，旧 worker 的结果必须被拒绝，不得覆盖新 attempt。
///
/// 持有方侧谓词去掉到期条件后，排他性完全由 `version`/`lease_owner` 承担；本测试钉住接管仍然
/// 使旧 worker 的终态提交返回 `task-lease-lost`。
#[tokio::test]
async fn stale_worker_result_is_rejected_after_a_real_takeover() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"takeover.mkv", vec![34]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, T0 - 1).await.unwrap();
    let clock = Arc::new(ManualTaskClock::new(T0));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(AtomicBool::new(false));
    let stopped = Arc::new(AtomicBool::new(false));
    // 旧 worker 保持默认 10s 续租间隔：测试窗口内不会续租，接管条件可确定性成立。
    let stale = ProcessingWorker::new(
        "stale-worker".to_owned(),
        store.clone(),
        vec![Arc::new(ReleasableHandler {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
            stopped: Arc::clone(&stopped),
        })],
        clock.clone(),
    )
    .unwrap();
    let stale_run = tokio::spawn(async move { stale.run_once().await });
    started.notified().await;
    let before = task_row(db.pool(), task.id).await;
    assert_eq!(before.lease_owner.as_deref(), Some("stale-worker"));

    clock.advance(PROCESSING_LEASE_DURATION_US);
    let fresh = ProcessingWorker::new(
        "fresh-worker".to_owned(),
        store.clone(),
        vec![Arc::new(ImmediateCompleteHandler)],
        clock.clone(),
    )
    .unwrap();
    assert_eq!(fresh.run_once().await.unwrap(), Some(task.id));
    let taken = task_row(db.pool(), task.id).await;
    log_timeline("D.after-takeover", db.pool(), task.id, clock.now_us()).await;
    assert_eq!(taken.status, "queued");
    assert_eq!(taken.stage, "planning");
    assert_eq!(taken.attempt_count, 2);

    release.store(true, Ordering::SeqCst);
    let stale_result = tokio::time::timeout(Duration::from_secs(3), stale_run)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stale_result.unwrap_err().code(), ErrorCode::TaskLeaseLost);
    let final_task = task_row(db.pool(), task.id).await;
    assert_eq!(
        final_task.version, taken.version,
        "stale worker mutated the task"
    );
    let attempts = attempt_rows(db.pool(), task.id).await;
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(attempts[0].failure_code.as_deref(), Some("task.lease-lost"));
    assert_eq!(attempts[1].ordinal, 2);
    assert_eq!(attempts[1].reason, "lease-recovery");
}

/// 回归：进程重启时 `prepare()` 仍把死亡 worker 的 attempt 归因为 `task.lease-lost` 并创建
/// `lease-recovery` attempt；这是租约到期机制的本职场景，不受本次修复影响。
#[tokio::test]
async fn restart_recovery_still_attributes_dead_owner_attempts_to_lease_loss() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"restart.mkv", vec![35]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, T0 - 1).await.unwrap();
    let clock = Arc::new(ManualTaskClock::new(T0));
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
    let handlers: Vec<Arc<dyn ProcessingStageHandler>> = Vec::new();
    let runtime = ProcessingTaskRuntime::new_with_concurrency(
        db.pool().clone(),
        OutboxNotifier::new(),
        &handlers,
        clock.clone(),
        1,
    )
    .unwrap();

    let summary = runtime.prepare().await.unwrap();
    log_timeline(
        "E.after-restart-prepare",
        db.pool(),
        task.id,
        clock.now_us(),
    )
    .await;
    assert_eq!(summary.recovered_leases, 1);
    let recovered = task_row(db.pool(), task.id).await;
    assert_eq!(recovered.status, "queued");
    assert_eq!(recovered.stage, "identification");
    assert_eq!(recovered.attempt_count, 2);
    assert!(recovered.lease_owner.is_none());
    let attempts = attempt_rows(db.pool(), task.id).await;
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(attempts[0].failure_code.as_deref(), Some("task.lease-lost"));
    assert_eq!(attempts[0].started_at_us, Some(T0));
    assert_eq!(attempts[0].finished_at_us, Some(lease.expires_at_us));
    assert_eq!(attempts[1].reason, "lease-recovery");
    assert_eq!(attempts[1].status, "queued");
}

/// 回归：运行时同时在飞的处理器数量不超过配置的 worker 数，且全部任务各自只执行一次。
#[tokio::test]
async fn runtime_keeps_in_flight_handlers_within_configured_concurrency() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let store = ProcessingStore::new(db.pool().clone());
    let mut task_ids = Vec::new();
    for index in 0..5_u8 {
        let revision = common::seed_stable_revision(
            db.pool(),
            inbox,
            format!("concurrent-{index}.mkv").as_bytes(),
            vec![40 + index],
        )
        .await;
        task_ids.push(store.ensure_revision(revision, T0 - 1).await.unwrap().id);
    }
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let handlers: Vec<Arc<dyn ProcessingStageHandler>> = vec![Arc::new(ConcurrencyProbeHandler {
        in_flight: Arc::clone(&in_flight),
        max_seen: Arc::clone(&max_seen),
    })];
    let runtime = ProcessingTaskRuntime::new_with_concurrency(
        db.pool().clone(),
        OutboxNotifier::new(),
        &handlers,
        Arc::new(ManualTaskClock::new(T0)),
        2,
    )
    .unwrap();
    runtime.prepare().await.unwrap();
    let handle = runtime.start();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut done = 0;
        for task_id in &task_ids {
            let row = task_row(db.pool(), *task_id).await;
            if row.status == "queued" && row.stage == "planning" {
                done += 1;
            }
        }
        if done == task_ids.len() || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    handle.abort();

    assert!(
        (1..=2).contains(&max_seen.load(Ordering::SeqCst)),
        "in-flight handlers exceeded concurrency: {}",
        max_seen.load(Ordering::SeqCst)
    );
    for task_id in &task_ids {
        let row = task_row(db.pool(), *task_id).await;
        assert_eq!(row.status, "queued", "{row:?}");
        assert_eq!(row.stage, "planning", "{row:?}");
        assert_eq!(row.attempt_count, 1, "{row:?}");
    }
}

/// 回归：处理器失败导致的暂停任务可由管理员立即重试，并以 `manual-retry` attempt 重新排队。
#[tokio::test]
async fn admin_can_retry_a_task_paused_by_handler_failure() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"retry.mkv", vec![36]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, T0 - 1).await.unwrap();
    let worker = ProcessingWorker::new(
        "failing-worker".to_owned(),
        store.clone(),
        vec![Arc::new(FailingHandler {
            calls: Arc::new(AtomicUsize::new(0)),
        })],
        Arc::new(ManualTaskClock::new(T0)),
    )
    .unwrap();

    assert_eq!(
        worker.run_once().await.unwrap_err().code(),
        ErrorCode::Internal
    );
    let paused = task_row(db.pool(), task.id).await;
    assert_eq!(paused.status, "paused");
    assert_eq!(
        paused.next_retry_at_us,
        Some(T0 + PROCESSING_LEASE_DURATION_US)
    );

    let retried = store
        .retry(account, task.id, "retry-1", T0 + 1)
        .await
        .unwrap();
    log_timeline("G.after-admin-retry", db.pool(), task.id, T0 + 1).await;
    assert_eq!(retried.attempt_count, 2);
    let queued = task_row(db.pool(), task.id).await;
    assert_eq!(queued.status, "queued");
    assert_eq!(queued.stage, "identification");
    assert!(queued.next_retry_at_us.is_none());
    let attempts = attempt_rows(db.pool(), task.id).await;
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(
        attempts[0].failure_code.as_deref(),
        Some(ErrorCode::Internal.as_str())
    );
    assert_eq!(attempts[1].reason, "manual-retry");
    assert_eq!(attempts[1].status, "queued");
}

/// 每次执行都返回确定性的非租约错误，模拟阶段处理器内部失败。
struct FailingHandler {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ProcessingStageHandler for FailingHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Identification
    }

    async fn run(
        &self,
        _lease: &ProcessingLease,
        _stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(AppError::new(
            ErrorCode::Internal,
            "deterministic stage failure",
        ))
    }
}

/// 执行期间把逻辑时钟推过租约到期，再正常返回结果；期间没有其他 worker 接管。
struct OutliveLeaseHandler {
    clock: Arc<ManualTaskClock>,
    calls: Arc<AtomicUsize>,
    pool: sqlx::SqlitePool,
}

#[async_trait]
impl ProcessingStageHandler for OutliveLeaseHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Identification
    }

    async fn run(
        &self,
        lease: &ProcessingLease,
        _stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        log_timeline(
            &format!(
                "B.call#{call} attempt={} version={} expires_at_us={}",
                lease.attempt_id, lease.version, lease.expires_at_us
            ),
            &self.pool,
            lease.task.id,
            self.clock.now_us(),
        )
        .await;
        self.clock.advance(PROCESSING_LEASE_DURATION_US + 1_000_000);
        Ok(ProcessingHandlerOutcome::IdentificationComplete {
            reason: ProcessingReason::IdentificationConfirmedTitleYear,
        })
    }
}

/// 让出执行权直到被释放或被停止，并记录是否收到停止信号。
struct ReleasableHandler {
    started: Arc<tokio::sync::Notify>,
    release: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}

#[async_trait]
impl ProcessingStageHandler for ReleasableHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Identification
    }

    async fn run(
        &self,
        _lease: &ProcessingLease,
        stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        self.started.notify_one();
        loop {
            if stop.is_stopped() {
                self.stopped.store(true, Ordering::SeqCst);
                break;
            }
            if self.release.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        Ok(ProcessingHandlerOutcome::IdentificationComplete {
            reason: ProcessingReason::IdentificationConfirmedTitleYear,
        })
    }
}

/// 立即完成识别阶段，用于模拟接管后的新 worker。
struct ImmediateCompleteHandler;

#[async_trait]
impl ProcessingStageHandler for ImmediateCompleteHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Identification
    }

    async fn run(
        &self,
        _lease: &ProcessingLease,
        _stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        Ok(ProcessingHandlerOutcome::IdentificationComplete {
            reason: ProcessingReason::IdentificationConfirmedTitleYear,
        })
    }
}

/// 记录同时在飞的处理器峰值。
struct ConcurrencyProbeHandler {
    in_flight: Arc<AtomicUsize>,
    max_seen: Arc<AtomicUsize>,
}

#[async_trait]
impl ProcessingStageHandler for ConcurrencyProbeHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Identification
    }

    async fn run(
        &self,
        _lease: &ProcessingLease,
        _stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_seen.fetch_max(current, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(ProcessingHandlerOutcome::IdentificationComplete {
            reason: ProcessingReason::IdentificationConfirmedTitleYear,
        })
    }
}

struct CancelThenFail {
    store: ProcessingStore,
    account: Uuid,
}
#[async_trait]
impl ProcessingStageHandler for CancelThenFail {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Identification
    }
    async fn run(
        &self,
        lease: &ProcessingLease,
        _stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        self.store
            .request_cancel(self.account, lease.task.id, "review-cancel", 100_000_001)
            .await?;
        Err(AppError::new(
            ErrorCode::Internal,
            "handler failed after accepted cancel",
        ))
    }
}
#[tokio::test]
async fn accepted_cancel_is_not_erased_by_handler_failure() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision =
        common::seed_stable_revision(db.pool(), inbox, b"review-cancel.mkv", vec![99]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store.ensure_revision(revision, 99_999_999).await.unwrap();
    let worker = ProcessingWorker::new(
        "review".into(),
        store.clone(),
        vec![Arc::new(CancelThenFail {
            store: store.clone(),
            account,
        })],
        Arc::new(ManualTaskClock::new(100_000_000)),
    )
    .unwrap();
    assert_eq!(
        worker.run_once().await.unwrap_err().code(),
        ErrorCode::Internal
    );
    let status: String = sqlx::query_scalar("SELECT status FROM tasks_processing_tasks WHERE id=?")
        .bind(task.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        status, "cancelled",
        "accepted cancellation must not become an automatically retrying pause"
    );
    let cancelled = task_row(db.pool(), task.id).await;
    assert!(cancelled.next_retry_at_us.is_none());
    let attempts = attempt_rows(db.pool(), task.id).await;
    assert_eq!(attempts.last().unwrap().status, "cancelled");
    assert!(attempts.last().unwrap().failure_code.is_none());
    assert!(
        store
            .claim_next(
                "review-later",
                &[ProcessingStage::Identification],
                T0 + 7_200_000_000
            )
            .await
            .unwrap()
            .is_none(),
        "accepted cancel must not become an automatic retry later"
    );
}
