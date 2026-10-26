use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

use crate::platform::task_runtime::TaskClock;
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::processing::model::{ProcessingLease, ProcessingReason, ProcessingStage};
use crate::tasks::processing::store::{
    PROCESSING_LEASE_DURATION_US, PROCESSING_LEASE_RENEW_INTERVAL_US, ProcessingStore,
};

#[derive(Clone, Default)]
/// 供长时间运行阶段处理器使用的协作式停止信号。
pub struct ProcessingStopToken {
    stopped: Arc<AtomicBool>,
}

impl ProcessingStopToken {
    /// 请求处理器在下一个副作用或检查点前停止。
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    #[must_use]
    /// 返回租约丢失或取消是否已请求停止。
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 阶段处理结果；`Committed` 表示处理器已自行提交，其他变体由 worker 转为原子任务状态变更。
pub enum ProcessingHandlerOutcome {
    /// 处理器已自行原子提交任务状态与阶段副作用，worker 不再为该结果提交阶段状态转换。
    Committed,
    /// 身份已确认，可把任务推进到后续阶段。
    IdentificationComplete {
        /// 描述确认依据的稳定脱敏原因。
        reason: ProcessingReason,
    },
    /// 识别需要人工复核，任务应进入等待确认状态。
    WaitingConfirmation {
        /// 描述需要复核原因的稳定脱敏分类。
        reason: ProcessingReason,
    },
    /// 暂时性依赖阻塞处理，任务应暂停到指定时间。
    Paused {
        /// 描述依赖阻塞的稳定脱敏原因。
        reason: ProcessingReason,
        /// 最早允许重试的 Unix epoch 微秒时间。
        next_retry_at_us: i64,
    },
}

#[async_trait]
/// 一个已注册能力阶段；实现可自行提交并返回 `Committed`，也可把状态转换交给 worker。
pub trait ProcessingStageHandler: Send + Sync {
    /// 返回该处理器唯一负责的能力阶段。
    fn stage(&self) -> ProcessingStage;

    /// 在给定租约权限和协作停止信号下执行一次有界阶段处理。
    ///
    /// 返回 `Committed` 时，处理器已自行提交任务状态与阶段副作用；返回其他 outcome 时，由
    /// worker 提交对应状态转换。暂时性依赖受阻可用 `Paused` 表达。
    ///
    /// # Errors
    ///
    /// 租约权限失效、处理器执行失败或处理器自行提交失败时返回 [`AppError`]。收到停止信号后，
    /// 实现必须停止继续产生副作用。
    async fn run(
        &self,
        lease: &ProcessingLease,
        stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError>;
}

/// 每次领取一个已注册阶段，并用可续租租约保护其提交权限。
pub struct ProcessingWorker {
    id: String,
    store: ProcessingStore,
    handlers: HashMap<ProcessingStage, Arc<dyn ProcessingStageHandler>>,
    clock: Arc<dyn TaskClock>,
    renewal_interval: std::time::Duration,
}

impl ProcessingWorker {
    /// 构建 worker，并拒绝重复的阶段注册。
    ///
    /// # Errors
    ///
    /// 多个处理器负责同一阶段时返回配置错误。
    pub fn new(
        id: String,
        store: ProcessingStore,
        handlers: Vec<Arc<dyn ProcessingStageHandler>>,
        clock: Arc<dyn TaskClock>,
    ) -> Result<Self, AppError> {
        let mut indexed = HashMap::with_capacity(handlers.len());
        for handler in handlers {
            if indexed.insert(handler.stage(), handler).is_some() {
                return Err(AppError::new(
                    ErrorCode::ConfigInvalid,
                    "duplicate processing stage handler",
                ));
            }
        }
        Ok(Self {
            id,
            store,
            handlers: indexed,
            clock,
            renewal_interval: std::time::Duration::from_micros(
                u64::try_from(PROCESSING_LEASE_RENEW_INTERVAL_US).unwrap_or(10_000_000),
            ),
        })
    }

    #[must_use]
    /// 为确定性测试覆盖心跳周期。
    pub fn with_renewal_interval(mut self, interval: std::time::Duration) -> Self {
        self.renewal_interval = interval;
        self
    }

    /// 处理下一个由已注册阶段负责的任务；没有符合条件的任务时返回 `None`。
    ///
    /// `Committed` 结果保留处理器已经完成的提交；其他结果由 worker 提交相应状态转换。
    /// 处理器自身失败而本 worker 仍持有任务时，任务转入带退避的暂停态并在 attempt 上记录
    /// 真实失败码，不会停留在 `running` 等待租约到期后被当作租约丢失回收重跑。
    ///
    /// # Errors
    ///
    /// 租约丢失、取消检查点、数据库或 outbox 失败时返回错误。处理器失败时返回处理器的原始错误
    /// 供运行时发布，但任务已先转入暂停态（仍持有时）或保持原状（已被接管或数据库不可用时）。
    /// 租约丢失时会在返回前停止处理器，且绝不提交其过期结果。
    pub async fn run_once(&self) -> Result<Option<Uuid>, AppError> {
        let stages = self.handlers.keys().copied().collect::<Vec<_>>();
        let Some(lease) = self
            .store
            .claim_next(&self.id, &stages, self.clock.now_us())
            .await?
        else {
            return Ok(None);
        };
        let task_id = lease.task.id;
        let handler = self.handlers.get(&lease.task.stage).ok_or_else(|| {
            AppError::new(ErrorCode::Internal, "claimed unregistered processing stage")
        })?;
        let shared_lease = Arc::new(Mutex::new(lease));
        let stop = ProcessingStopToken::default();
        let (heartbeat_stop_sender, heartbeat_stop_receiver) = watch::channel(false);
        let handler_lease = shared_lease.lock().await.clone();
        let execution = handler.run(&handler_lease, stop.clone());
        let heartbeat = renewal_loop(
            self.store.clone(),
            Arc::clone(&shared_lease),
            Arc::clone(&self.clock),
            stop.clone(),
            heartbeat_stop_receiver,
            self.renewal_interval,
        );
        tokio::pin!(execution);
        tokio::pin!(heartbeat);
        let outcome = tokio::select! {
            outcome = &mut execution => {
                let _ = heartbeat_stop_sender.send(true);
                match heartbeat.await {
                    Ok(HeartbeatExit::Stopped) => outcome,
                    Ok(HeartbeatExit::Cancelled) => {
                        let lease = shared_lease.lock().await.clone();
                        self.store.finish_cancelled(&lease, self.clock.now_us()).await?;
                        return Ok(Some(task_id));
                    }
                    // 续租失败与 handler 完成存在固有竞态：handler 已自行提交（Committed）
                    // 时任务已离开 running，续租条件自然失效；只有未提交的结果才受续租失败保护。
                    Err(renewal_error) => match outcome {
                        Ok(ProcessingHandlerOutcome::Committed) => return Ok(Some(task_id)),
                        Ok(_) | Err(_) => return Err(renewal_error),
                    },
                }
            }
            heartbeat_result = &mut heartbeat => {
                stop.stop();
                let exit = match heartbeat_result {
                    Ok(exit) => exit,
                    Err(error) => {
                        // 同上：handler 已成功提交时续租失败是预期竞态，不覆盖业务结果。
                        match execution.await {
                            Ok(ProcessingHandlerOutcome::Committed) => return Ok(Some(task_id)),
                            Ok(_) | Err(_) => return Err(error),
                        }
                    }
                };
                let _ = execution.await;
                match exit {
                    HeartbeatExit::Cancelled => {
                        let lease = shared_lease.lock().await.clone();
                        self.store.finish_cancelled(&lease, self.clock.now_us()).await?;
                        return Ok(Some(task_id));
                    }
                    HeartbeatExit::Stopped => {
                        return Err(AppError::new(
                            ErrorCode::Internal,
                            "processing heartbeat stopped before handler",
                        ));
                    }
                }
            }
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let lease = shared_lease.lock().await.clone();
                return Err(self.pause_after_handler_failure(&lease, error).await);
            }
        };
        let lease = shared_lease.lock().await.clone();
        self.commit_handler_outcome(task_id, &lease, outcome)
            .await?;
        Ok(Some(task_id))
    }

    /// 处理器失败后，只要本 worker 仍持有任务行，就先把任务转入带退避的暂停态，再返回处理器
    /// 的原始错误交给运行时发布。
    ///
    /// 暂停事务的 `version`/`lease_owner` 守卫保证已被接管的任务不会被改写；守卫失败或数据库
    /// 不可用时任务保持原状，由租约到期回收兜底。两种情况都返回同一个处理器错误。
    async fn pause_after_handler_failure(
        &self,
        lease: &ProcessingLease,
        error: AppError,
    ) -> AppError {
        let now_us = self.clock.now_us();
        let next_retry_at_us =
            now_us.saturating_add(stage_failure_retry_delay_us(lease.task.attempt_count));
        let _ = self
            .store
            .finish_stage_failed(lease, error.code(), next_retry_at_us, now_us)
            .await;
        error
    }

    /// 把未自行提交的 handler 结果转换为原子任务状态变更。
    ///
    /// `Committed` 已由处理器提交；取消检查点优先于其余状态转换。
    ///
    /// # Errors
    ///
    /// 取消检查点或状态转换的数据库、outbox 失败时返回错误。
    async fn commit_handler_outcome(
        &self,
        task_id: Uuid,
        lease: &ProcessingLease,
        outcome: ProcessingHandlerOutcome,
    ) -> Result<(), AppError> {
        if outcome == ProcessingHandlerOutcome::Committed {
            return Ok(());
        }
        if self.store.cancel_requested(task_id).await? {
            self.store
                .finish_cancelled(lease, self.clock.now_us())
                .await?;
            return Ok(());
        }
        match outcome {
            ProcessingHandlerOutcome::Committed => unreachable!("committed outcome returned above"),
            ProcessingHandlerOutcome::IdentificationComplete { reason } => {
                self.store
                    .finish_identification_complete(lease, reason, self.clock.now_us())
                    .await?;
            }
            ProcessingHandlerOutcome::WaitingConfirmation { reason } => {
                self.store
                    .finish_waiting_confirmation(lease, reason, self.clock.now_us())
                    .await?;
            }
            ProcessingHandlerOutcome::Paused {
                reason,
                next_retry_at_us,
            } => {
                self.store
                    .finish_paused(lease, reason, next_retry_at_us, self.clock.now_us())
                    .await?;
            }
        }
        Ok(())
    }

    /// 在工作器循环启动前恢复已过期的租约。
    ///
    /// # Errors
    ///
    /// 租约恢复发生无效状态、数据库或 outbox 失败时返回错误。
    pub async fn reclaim_expired(&self) -> Result<u64, AppError> {
        self.store.reclaim_expired(self.clock.now_us()).await
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeartbeatExit {
    Stopped,
    Cancelled,
}

/// 处理器失败后的重试退避：以租约时长起步按 attempt 次数翻倍，上限一小时。
fn stage_failure_retry_delay_us(attempt_count: u64) -> i64 {
    const MAX_DELAY_US: i64 = 3_600_000_000;
    let exponent = attempt_count.saturating_sub(1).min(16);
    PROCESSING_LEASE_DURATION_US
        .saturating_mul(1_i64 << exponent)
        .min(MAX_DELAY_US)
}

async fn renewal_loop(
    store: ProcessingStore,
    lease: Arc<Mutex<ProcessingLease>>,
    clock: Arc<dyn TaskClock>,
    stop: ProcessingStopToken,
    mut stopped: watch::Receiver<bool>,
    renewal_interval: std::time::Duration,
) -> Result<HeartbeatExit, AppError> {
    let mut interval = tokio::time::interval(renewal_interval);
    interval.tick().await;
    loop {
        tokio::select! {
            biased;
            changed = stopped.changed() => {
                if changed.is_err() || *stopped.borrow() {
                    return Ok(HeartbeatExit::Stopped);
                }
            }
            _ = interval.tick() => {
                let mut lease = lease.lock().await;
                // 只有租约确实不再属于本 worker（0 行匹配）才算丢失；连接池超时、SQLITE_BUSY
                // 等瞬时数据库错误保留当前租约并在下一周期重试，避免把活着的处理器判死。
                match store.cancel_requested(lease.task.id).await {
                    Ok(true) => {
                        stop.stop();
                        return Ok(HeartbeatExit::Cancelled);
                    }
                    Ok(false) => {}
                    Err(error) if is_transient(&error) => continue,
                    Err(error) => return Err(error),
                }
                match store.renew(&lease, clock.now_us()).await {
                    Ok(renewed) => *lease = renewed,
                    Err(error) if is_transient(&error) => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
}

fn is_transient(error: &AppError) -> bool {
    !matches!(
        error.code(),
        ErrorCode::TaskLeaseLost | ErrorCode::TaskNotFound
    )
}
