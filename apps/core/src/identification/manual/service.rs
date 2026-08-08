use sqlx::SqlitePool;

use crate::identification::manual::store::ManualDecisionStore;
use crate::platform::audit;
use crate::platform::outbox::OutboxNotifier;
use crate::shared::error::AppError;
use crate::tasks::processing::service::ProcessingTaskService;

#[derive(Clone)]
/// 将已接受识别决定协调到任务与审计消费者投影。
pub struct DecisionCoordinator {
    decisions: ManualDecisionStore,
    processing: ProcessingTaskService,
    pool: SqlitePool,
}

impl DecisionCoordinator {
    #[must_use]
    /// 使用共享通知器创建协调器，使任务状态事件与决定回执复用同一唤醒链路。
    pub fn new(pool: SqlitePool, notifier: OutboxNotifier) -> Self {
        Self {
            decisions: ManualDecisionStore::new_with_notifier(pool.clone(), notifier.clone()),
            processing: ProcessingTaskService::new_with_notifier(pool.clone(), notifier),
            pool,
        }
    }

    /// 按稳定顺序应用并确认至多 `limit` 项已接受人工决定。
    ///
    /// 任务与审计消费者分别保存自己的 decision-ID 持久回执；若在最终识别标记前崩溃，
    /// 该派发仍可安全重放且不会重复产生消费者副作用。
    ///
    /// # Errors
    ///
    /// 有界读取、任务应用、审计写入或派发标记中的首个失败会直接返回。
    pub async fn dispatch_once(&self, limit: u32, now_us: i64) -> Result<u64, AppError> {
        let pending = self.decisions.pending_dispatches(limit).await?;
        for item in &pending {
            self.processing
                .apply_manual_decision(item.account_id, &item.dispatch, now_us)
                .await?;
            audit::record_manual_decision_once(&self.pool, item.dispatch.decision_id(), now_us)
                .await?;
            self.decisions
                .mark_applied(item.dispatch.decision_id(), now_us)
                .await?;
        }
        Ok(pending.len() as u64)
    }
}
