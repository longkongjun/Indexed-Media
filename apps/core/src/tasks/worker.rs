use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

use crate::discovery::observations::{FileObservation, ObservationSink, ScanEntryError};
use crate::discovery::scanner::{ScanStopToken, Scanner};
use crate::discovery::service::DiscoveryService;
use crate::platform::task_runtime::TaskClock;
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::model::{ScanCounts, ScanLease};
use crate::tasks::store::{LEASE_RENEW_INTERVAL_US, TaskStore};

/// 每次认领一个任务、维护其租约、扫描收件箱并提交终态。
pub struct ScanWorker {
    id: String,
    store: TaskStore,
    discovery: DiscoveryService,
    scanner: Arc<dyn Scanner>,
    clock: Arc<dyn TaskClock>,
    renewal_interval: std::time::Duration,
}

impl ScanWorker {
    #[must_use]
    /// 使用标准的 10 秒租约续期间隔创建工作器。
    ///
    /// `id` 会作为租约所有者持久化，应唯一标识此运行时实例。
    pub fn new(
        id: String,
        store: TaskStore,
        discovery: DiscoveryService,
        scanner: Arc<dyn Scanner>,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        Self {
            id,
            store,
            discovery,
            scanner,
            clock,
            renewal_interval: std::time::Duration::from_micros(
                u64::try_from(LEASE_RENEW_INTERVAL_US).unwrap_or(10_000_000),
            ),
        }
    }

    #[must_use]
    /// 覆盖工作器续期其 30 秒租约的频率。
    ///
    /// 此项用于确定性测试；零时长会在 Tokio 于 [`Self::run_once`] 中创建间隔时触发 panic。
    pub fn with_renewal_interval(mut self, renewal_interval: std::time::Duration) -> Self {
        self.renewal_interval = renewal_interval;
        self
    }

    /// 认领并完整处理下一个符合条件的任务；队列为空时返回 `None`。
    ///
    /// 根目录/预检/扫描失败通常会成为已提交的失败任务并返回其 ID。取消会在数据库批次边界被观察到。
    /// 心跳会在扫描过程中续期乐观租约，每次终态转换都会原子发出其发件箱事件。
    ///
    /// # Errors
    ///
    /// 认领、续期、观测持久化、取消查询或终态提交失败时返回 [`AppError`]。租约丢失会直接返回，因为另一个
    /// 工作器可能拥有任务。已提交的批次保持持久化。
    pub async fn run_once(&self) -> Result<Option<Uuid>, AppError> {
        let Some(lease) = self.store.claim_next(&self.id, self.clock.now_us()).await? else {
            return Ok(None);
        };
        let task_id = lease.task.id;
        let source = match self
            .discovery
            .open_scan_source(lease.task.inbox_directory_id)
            .await
        {
            Ok(source) => source,
            Err(error) => {
                self.store
                    .finish_failed_with_root_error(
                        &lease,
                        error.code().as_str(),
                        self.clock.now_us(),
                    )
                    .await?;
                return Ok(Some(task_id));
            }
        };
        let shared_lease = Arc::new(Mutex::new(lease));
        let cancelled = Arc::new(AtomicBool::new(false));
        let sink = Arc::new(StoreObservationSink {
            store: self.store.clone(),
            lease: Arc::clone(&shared_lease),
            clock: Arc::clone(&self.clock),
            cancelled: Arc::clone(&cancelled),
        });
        let scan_stop = ScanStopToken::default();
        let (heartbeat_stop_sender, heartbeat_stop_receiver) = watch::channel(false);
        let scan = self.scanner.scan(source, sink, scan_stop.clone());
        let heartbeat = renewal_loop(
            self.store.clone(),
            Arc::clone(&shared_lease),
            Arc::clone(&self.clock),
            heartbeat_stop_receiver,
            self.renewal_interval,
        );
        tokio::pin!(scan);
        tokio::pin!(heartbeat);
        let scan_result = tokio::select! {
            scan_result = &mut scan => {
                let _ = heartbeat_stop_sender.send(true);
                heartbeat.await?;
                scan_result
            }
            heartbeat_result = &mut heartbeat => {
                scan_stop.stop();
                let _ = scan.await;
                return heartbeat_result.map(|()| Some(task_id));
            }
        };
        let mut lease = shared_lease.lock().await.clone();
        match scan_result {
            Ok(summary) => {
                lease.task.counts.visited_directories = summary.visited_directories;
                lease.task.counts.skipped_entries = summary.skipped_entries;
                if cancelled.load(Ordering::Acquire) || self.store.cancel_requested(task_id).await?
                {
                    self.store
                        .finish_cancelled(&lease, self.clock.now_us())
                        .await?;
                } else {
                    self.store
                        .finish_success(&lease, lease.task.counts, self.clock.now_us())
                        .await?;
                }
            }
            Err(error) => {
                if is_fatal_scan_boundary(error.code()) {
                    self.store
                        .finish_failed_with_root_error(
                            &lease,
                            error.code().as_str(),
                            self.clock.now_us(),
                        )
                        .await?;
                } else if cancelled.load(Ordering::Acquire)
                    || self.store.cancel_requested(task_id).await?
                {
                    self.store
                        .finish_cancelled(&lease, self.clock.now_us())
                        .await?;
                } else if error.code() == ErrorCode::TaskLeaseLost {
                    return Err(error);
                } else {
                    self.store
                        .finish_failed_with_root_error(
                            &lease,
                            error.code().as_str(),
                            self.clock.now_us(),
                        )
                        .await?;
                }
            }
        }
        Ok(Some(task_id))
    }

    /// 在注入的时钟时间将每个过期运行任务重新排入恢复尝试。
    ///
    /// 仅在完整遍历成功时返回回收数量。每个任务转换独立提交；一个或多个回收后的错误不会返回计数，且先前的恢复
    /// 转换仍会保留提交。
    ///
    /// # Errors
    ///
    /// 首个回收事务失败时返回 [`AppError`]。
    pub async fn reclaim_expired(&self) -> Result<u64, AppError> {
        self.store.reclaim_expired(self.clock.now_us()).await
    }
}

struct StoreObservationSink {
    store: TaskStore,
    lease: Arc<Mutex<ScanLease>>,
    clock: Arc<dyn TaskClock>,
    cancelled: Arc<AtomicBool>,
}

#[async_trait]
impl ObservationSink for StoreObservationSink {
    async fn write_batch(
        &self,
        files: Vec<FileObservation>,
        errors: Vec<ScanEntryError>,
        progress: ScanCounts,
    ) -> Result<(), AppError> {
        let mut lease = self.lease.lock().await;
        if self.store.cancel_requested(lease.task.id).await? {
            self.cancelled.store(true, Ordering::Release);
            return Err(AppError::new(
                ErrorCode::TaskInvalidState,
                "scan cancellation reached a database batch boundary",
            ));
        }
        *lease = self
            .store
            .record_observations(&lease, &files, &errors, progress, self.clock.now_us())
            .await?;
        Ok(())
    }
}

async fn renewal_loop(
    store: TaskStore,
    lease: Arc<Mutex<ScanLease>>,
    clock: Arc<dyn TaskClock>,
    mut stop: watch::Receiver<bool>,
    renewal_interval: std::time::Duration,
) -> Result<(), AppError> {
    let mut interval = tokio::time::interval(renewal_interval);
    interval.tick().await;
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let mut lease = lease.lock().await;
                *lease = store.renew(&lease, clock.now_us()).await?;
            }
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

fn is_fatal_scan_boundary(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::RootUnavailable | ErrorCode::PathInvalid | ErrorCode::PathEscape
    )
}
