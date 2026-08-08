use uuid::Uuid;

use crate::platform::outbox::OutboxNotifier;
use crate::shared::error::AppError;
use crate::shared::page::{CursorPage, PageRequest};
use crate::tasks::processing::model::{
    DecisionDispatch, ProcessingTaskFilter, ProcessingTaskPageView, ProcessingTaskView,
};
use crate::tasks::processing::store::ProcessingStore;

#[derive(Clone)]
/// 协调持久化发现请求桥接与处理任务控制。
pub struct ProcessingTaskService {
    store: ProcessingStore,
}

impl ProcessingTaskService {
    #[must_use]
    /// 使用独立通知器创建服务；事件会持久化但不会唤醒共享监听者。
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self::new_with_notifier(pool, OutboxNotifier::new())
    }

    #[must_use]
    /// 使用共享通知器创建服务，使任务事务可立即唤醒 outbox/SSE 交付。
    pub fn new_with_notifier(pool: sqlx::SqlitePool, notifier: OutboxNotifier) -> Self {
        Self {
            store: ProcessingStore::new_with_notifier(pool, notifier),
        }
    }

    #[must_use]
    /// 借用底层事务存储，供运行时组合租约与恢复操作。
    pub fn store(&self) -> &ProcessingStore {
        &self.store
    }

    /// 在 worker 领取前物化至多 `limit` 个待处理稳定 revision 请求。
    ///
    /// # Errors
    ///
    /// 返回底层存储产生的校验、任务状态、数据库或 outbox 错误。
    pub async fn ensure_pending(&self, limit: u32, now_us: i64) -> Result<u64, AppError> {
        let revisions = self.store.pending_revision_ids(limit).await?;
        for revision in &revisions {
            self.store.ensure_revision(*revision, now_us).await?;
        }
        Ok(revisions.len() as u64)
    }

    /// 读取账户拥有的处理任务。
    ///
    /// # Errors
    ///
    /// 任务不属于账户时返回未找到；解码或数据库失败时返回底层错误。
    pub async fn get(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<ProcessingTaskView, AppError> {
        self.store.get(account_id, task_id).await
    }

    /// 返回账户隔离且快照稳定的任务页面。
    ///
    /// # Errors
    ///
    /// 游标无效、解码失败或数据库访问失败时返回错误。
    pub async fn list(
        &self,
        account_id: Uuid,
        page: &PageRequest,
    ) -> Result<CursorPage<ProcessingTaskView>, AppError> {
        self.store.list_tasks(account_id, page).await
    }

    /// 从同一快照返回过滤后的任务中心页面及导航摘要。
    ///
    /// # Errors
    ///
    /// 过滤条件或游标无效、解码失败或数据库访问失败时返回错误。
    pub async fn list_center(
        &self,
        account_id: Uuid,
        filter: &ProcessingTaskFilter,
        page: &PageRequest,
    ) -> Result<ProcessingTaskPageView, AppError> {
        self.store.list_task_center(account_id, filter, page).await
    }

    /// 幂等排队一次人工重试。
    ///
    /// # Errors
    ///
    /// 校验、冲突、未找到、无效状态、数据库或 outbox 失败时返回错误。
    pub async fn retry(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.store
            .retry(account_id, task_id, idempotency_key, now_us)
            .await
    }

    /// 幂等请求取消。
    ///
    /// # Errors
    ///
    /// 校验、冲突、未找到、无效状态、数据库或 outbox 失败时返回错误。
    pub async fn cancel(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.store
            .request_cancel(account_id, task_id, idempotency_key, now_us)
            .await
    }

    /// 在任务本地的幂等边界应用一项有界且已接受的人工决定。
    ///
    /// # Errors
    ///
    /// 所有权或状态错误，以及存储层的原子收据、尝试或 outbox 失败时返回错误。
    pub async fn apply_manual_decision(
        &self,
        account_id: Uuid,
        dispatch: &DecisionDispatch,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.store
            .apply_manual_decision(account_id, dispatch, now_us)
            .await
    }
}
