use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use tokio::sync::{Notify, watch};

use crate::bootstrap::config::AppConfig;
use crate::discovery::capability::DeploymentRootSet;
use crate::discovery::scanner::BoundedScanner;
use crate::discovery::service::DiscoveryService;
use crate::identification::manual::service::DecisionCoordinator;
use crate::platform::capability_fs::OsCapabilityFs;
use crate::platform::db::Db;
use crate::platform::outbox::OutboxNotifier;
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::processing::service::ProcessingTaskService;
use crate::tasks::processing::worker::{ProcessingStageHandler, ProcessingWorker};
use crate::tasks::store::TaskStore;
use crate::tasks::worker::ScanWorker;

/// 同时租用的每文件处理任务的默认最大数量。
pub const PROCESSING_TASK_CONCURRENCY: usize = 20;

/// 注入租约和状态转换决策的 UTC 微秒时钟。
pub trait TaskClock: Send + Sync {
    /// 返回当前逻辑 UTC 微秒时间戳。
    fn now_us(&self) -> i64;
}

#[derive(Clone, Copy, Debug, Default)]
/// 由系统 UTC 墙上时钟支持的生产任务时钟。
pub struct SystemTaskClock;

impl TaskClock for SystemTaskClock {
    fn now_us(&self) -> i64 {
        chrono::Utc::now().timestamp_micros()
    }
}

#[derive(Debug)]
/// 用于确定性租约/恢复测试的可原子调整任务时钟。
pub struct ManualTaskClock {
    now_us: AtomicI64,
}

impl ManualTaskClock {
    #[must_use]
    /// 以提供的逻辑 UTC 微秒值创建时钟。
    pub const fn new(now_us: i64) -> Self {
        Self {
            now_us: AtomicI64::new(now_us),
        }
    }

    /// 替换后续读取使用的逻辑时间戳。
    pub fn set(&self, now_us: i64) {
        self.now_us.store(now_us, Ordering::SeqCst);
    }

    /// 原子地增加 `delta_us`；负增量会使时钟倒退。
    pub fn advance(&self, delta_us: i64) {
        self.now_us.fetch_add(delta_us, Ordering::SeqCst);
    }
}

impl TaskClock for ManualTaskClock {
    fn now_us(&self) -> i64 {
        self.now_us.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
/// 排空排队扫描、空闲时轮询并发布运行时失败的后台循环。
pub struct TaskRuntime {
    worker: Arc<ScanWorker>,
    notified: Arc<Notify>,
    poll_interval: std::time::Duration,
    failures: watch::Sender<Option<TaskRuntimeFailure>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 向就绪状态/监控使用者公开的最新工作器循环失败。
pub struct TaskRuntimeFailure {
    /// 工作器错误的稳定分类。
    pub code: ErrorCode,
    /// 运行时观测到该失败时的 UTC 微秒。
    pub occurred_at_us: i64,
}

/// 与打开全部根目录能力的适配器配对的部署根目录声明。
///
/// 构造仅限于 [`TaskRuntime::validate_roots`]。
pub struct ValidatedTaskRoots {
    roots: DeploymentRootSet,
    fs: Arc<dyn crate::discovery::capability::CapabilityFs>,
}

impl TaskRuntime {
    #[must_use]
    /// 使用 250 ms 空闲轮询间隔包装已构造的工作器。
    pub fn new(worker: ScanWorker) -> Self {
        let (failures, _) = watch::channel(None);
        Self {
            worker: Arc::new(worker),
            notified: Arc::new(Notify::new()),
            poll_interval: std::time::Duration::from_millis(250),
            failures,
        }
    }

    /// 加载部署根目录配置并打开每个声明的根目录能力。
    ///
    /// # Errors
    ///
    /// 根目录配置无效，或任一根目录无法在配置的运行模式下打开、识别、枚举或获准使用时，返回 [`AppError`]。
    pub fn validate_roots(config: &AppConfig) -> Result<ValidatedTaskRoots, AppError> {
        let roots = DeploymentRootSet::load(&config.deployment_roots_file, config.mode)?;
        let fs = OsCapabilityFs::open(roots.declarations(), config.mode)?;
        Ok(ValidatedTaskRoots {
            roots,
            fs: Arc::new(fs),
        })
    }

    #[must_use]
    /// 根据先前验证的根目录和本地通知器构建生产扫描运行时。
    pub fn from_validated(config: &AppConfig, db: &Db, validated: ValidatedTaskRoots) -> Self {
        Self::from_validated_with_notifier(config, db, validated, OutboxNotifier::new())
    }

    #[must_use]
    /// 构建与事务性任务事件共享 `notifier` 的生产扫描运行时。
    ///
    /// 运行时记录 deployment-roots 指纹；若配置文件在验证后发生变化，就会停止打开扫描源。
    pub fn from_validated_with_notifier(
        config: &AppConfig,
        db: &Db,
        validated: ValidatedTaskRoots,
        notifier: OutboxNotifier,
    ) -> Self {
        let fingerprint = validated.roots.fingerprint();
        let discovery =
            DiscoveryService::new(validated.roots.view_map(), validated.fs, db.pool().clone())
                .with_config_guard(config.deployment_roots_file.clone(), fingerprint);
        Self::new(ScanWorker::new(
            format!("core-{}", uuid::Uuid::now_v7()),
            TaskStore::new_with_notifier(db.pool().clone(), notifier),
            discovery,
            Arc::new(BoundedScanner::default()),
            Arc::new(SystemTaskClock),
        ))
    }

    /// 一步完成已配置根目录验证和生产运行时构建。
    ///
    /// # Errors
    ///
    /// 返回与 [`Self::validate_roots`] 相同的配置/能力错误。
    pub fn from_app(config: &AppConfig, db: &Db) -> Result<Self, AppError> {
        let validated = Self::validate_roots(config)?;
        Ok(Self::from_validated(config, db, validated))
    }

    /// 在循环开始前重新排队每个已过期的运行中任务，并将其先前尝试标记为失败。
    ///
    /// 完整恢复过程成功后返回已处理的过期任务数。每个任务状态转换独立提交；若后续恢复失败，
    /// 此方法返回不带计数的错误，而先前的恢复提交仍保持可见。
    ///
    /// # Errors
    ///
    /// 若回收事务性任务状态/outbox 事件失败，则返回 [`AppError`]。
    pub async fn prepare(&self) -> Result<u64, AppError> {
        self.worker.reclaim_expired().await
    }

    /// 唤醒一个空闲运行时循环，使新排队工作无需等待轮询即可被认领。
    pub fn notify(&self) {
        self.notified.notify_one();
    }

    #[must_use]
    /// 订阅最新工作器失败；初始值为 `None`。
    pub fn subscribe_failures(&self) -> watch::Receiver<Option<TaskRuntimeFailure>> {
        self.failures.subscribe()
    }

    #[must_use]
    /// 启动不终止的工作器循环并返回其 Tokio join 句柄。
    ///
    /// 循环会排空可用任务，在错误或空队列时发布失败后暂停，并在 [`Self::notify`] 或 250 ms 轮询后恢复。
    /// 丢弃句柄不会取消任务；关闭时调用方必须显式中止它。
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                loop {
                    match self.worker.run_once().await {
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(error) => {
                            self.failures.send_replace(Some(TaskRuntimeFailure {
                                code: error.code(),
                                occurred_at_us: chrono::Utc::now().timestamp_micros(),
                            }));
                            break;
                        }
                    }
                }
                tokio::select! {
                    () = self.notified.notified() => {}
                    () = tokio::time::sleep(self.poll_interval) => {}
                }
            }
        })
    }
}

/// 负责持久请求物化、租约恢复与有界处理工作器的运行时。
pub struct ProcessingTaskRuntime {
    service: ProcessingTaskService,
    decision_coordinator: DecisionCoordinator,
    workers: Vec<Arc<ProcessingWorker>>,
    clock: Arc<dyn TaskClock>,
    notified: Arc<Notify>,
    poll_interval: std::time::Duration,
    failures: watch::Sender<Option<TaskRuntimeFailure>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 处理器开始领取任务前完成的启动准备摘要。
pub struct ProcessingPrepareSummary {
    /// 从待处理稳定修订版本物化或复用的任务数量。
    pub ensured_tasks: u64,
    /// 启动时从过期运行租约恢复到可领取状态的任务数量。
    pub recovered_leases: u64,
}

impl ProcessingTaskRuntime {
    /// 构建默认包含 20 个工作器的处理运行时。
    ///
    /// # Errors
    ///
    /// 处理器包含重复阶段注册时返回配置错误。
    pub fn new(
        pool: sqlx::SqlitePool,
        notifier: OutboxNotifier,
        handlers: &[Arc<dyn ProcessingStageHandler>],
        clock: Arc<dyn TaskClock>,
    ) -> Result<Self, AppError> {
        Self::new_with_concurrency(pool, notifier, handlers, clock, PROCESSING_TASK_CONCURRENCY)
    }

    /// 使用显式有界工作器数量构建运行时，供测试与基准使用。
    ///
    /// # Errors
    ///
    /// 并发数为零、超出预算或处理器重复时返回配置错误。
    pub fn new_with_concurrency(
        pool: sqlx::SqlitePool,
        notifier: OutboxNotifier,
        handlers: &[Arc<dyn ProcessingStageHandler>],
        clock: Arc<dyn TaskClock>,
        concurrency: usize,
    ) -> Result<Self, AppError> {
        if concurrency == 0 || concurrency > PROCESSING_TASK_CONCURRENCY {
            return Err(AppError::new(
                ErrorCode::ConfigInvalid,
                "processing concurrency is outside bounds",
            ));
        }
        let service = ProcessingTaskService::new_with_notifier(pool.clone(), notifier.clone());
        let decision_coordinator = DecisionCoordinator::new(pool, notifier);
        let mut workers = Vec::with_capacity(concurrency);
        let runtime_id = uuid::Uuid::now_v7();
        for ordinal in 0..concurrency {
            workers.push(Arc::new(ProcessingWorker::new(
                format!("processing-{runtime_id}-{ordinal}"),
                service.store().clone(),
                handlers.to_owned(),
                Arc::clone(&clock),
            )?));
        }
        let (failures, _) = watch::channel(None);
        Ok(Self {
            service,
            decision_coordinator,
            workers,
            clock,
            notified: Arc::new(Notify::new()),
            poll_interval: std::time::Duration::from_millis(250),
            failures,
        })
    }

    /// 分批物化全部待处理修订版本、恢复过期租约，并幂等派发已接受的人工决定。
    ///
    /// # Errors
    ///
    /// 返回首个请求物化、租约恢复、人工决定读取或应用、审计回执、派发标记、数据库或 outbox 错误。
    pub async fn prepare(&self) -> Result<ProcessingPrepareSummary, AppError> {
        let mut ensured_tasks = 0_u64;
        loop {
            let ensured = self
                .service
                .ensure_pending(1000, self.clock.now_us())
                .await?;
            ensured_tasks = ensured_tasks.saturating_add(ensured);
            if ensured < 1000 {
                break;
            }
        }
        let recovered_leases = self
            .service
            .store()
            .reclaim_expired(self.clock.now_us())
            .await?;
        loop {
            let dispatched = self
                .decision_coordinator
                .dispatch_once(200, self.clock.now_us())
                .await?;
            if dispatched < 200 {
                break;
            }
        }
        Ok(ProcessingPrepareSummary {
            ensured_tasks,
            recovered_leases,
        })
    }

    /// 在持久请求或事件提交后唤醒空闲处理调度器。
    pub fn notify(&self) {
        self.notified.notify_one();
    }

    #[must_use]
    /// 订阅后台调度器最近一次失败；新接收端会看到当前值，调度循环在发布失败后仍会继续运行。
    pub fn subscribe_failures(&self) -> watch::Receiver<Option<TaskRuntimeFailure>> {
        self.failures.subscribe()
    }

    /// 启动单个调度循环，每轮最多派发 20 个拥有独立租约的工作器。
    #[must_use]
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let now_us = self.clock.now_us();
                let mut did_work = false;
                match self.decision_coordinator.dispatch_once(200, now_us).await {
                    Ok(dispatched) => did_work = dispatched > 0,
                    Err(error) => self.publish_failure(error.code()),
                }
                if let Err(error) = self.service.ensure_pending(1000, now_us).await {
                    self.publish_failure(error.code());
                } else {
                    let mut runs = tokio::task::JoinSet::new();
                    for worker in &self.workers {
                        let worker = Arc::clone(worker);
                        runs.spawn(async move { worker.run_once().await });
                    }
                    while let Some(result) = runs.join_next().await {
                        match result {
                            Ok(Ok(Some(_))) => did_work = true,
                            Ok(Ok(None)) => {}
                            Ok(Err(error)) => self.publish_failure(error.code()),
                            Err(_) => self.publish_failure(ErrorCode::Internal),
                        }
                    }
                }
                if !did_work {
                    tokio::select! {
                        () = self.notified.notified() => {}
                        () = tokio::time::sleep(self.poll_interval) => {}
                    }
                }
            }
        })
    }

    fn publish_failure(&self, code: ErrorCode) {
        self.failures.send_replace(Some(TaskRuntimeFailure {
            code,
            occurred_at_us: chrono::Utc::now().timestamp_micros(),
        }));
    }
}
