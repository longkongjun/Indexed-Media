use async_trait::async_trait;
use uuid::Uuid;

use crate::platform::outbox::OutboxNotifier;
use crate::shared::error::AppError;
use crate::shared::page::{CursorPage, PageRequest};
use crate::tasks::model::{DiscoveredFileView, NewScanTask, ScanErrorView, ScanTaskView};
use crate::tasks::store::TaskStore;

#[async_trait]
/// 账户范围的扫描任务创建、控制和结果查询。
pub trait ScanUseCases: Send + Sync {
    /// 原子创建初始排队任务、尝试、扫描批次、幂等绑定和事件。
    ///
    /// 为相同账户/收件箱复用相同键会返回原始任务。
    ///
    /// # Errors
    ///
    /// 幂等键无效/冲突、收件箱不可用或任意事务/发件箱失败时返回 [`AppError`]。
    async fn create(
        &self,
        account_id: Uuid,
        inbox_directory_id: Uuid,
        idempotency_key: String,
    ) -> Result<ScanTaskView, AppError>;
    /// 仅在任务属于 `account_id` 时加载一个任务。
    ///
    /// # Errors
    ///
    /// 任务不存在/不属于该账户，或存储数据/查询失败时返回 [`AppError`]。
    async fn get(&self, account_id: Uuid, task_id: Uuid) -> Result<ScanTaskView, AppError>;
    /// 返回 `account_id` 所拥有任务的快照稳定分页。
    ///
    /// # Errors
    ///
    /// 游标无效/跨账户，或查询/解码失败时返回 [`AppError`]。
    async fn list(
        &self,
        account_id: Uuid,
        page: PageRequest,
    ) -> Result<CursorPage<ScanTaskView>, AppError>;
    /// 为符合条件的终态任务原子排入新的手动尝试。
    ///
    /// 幂等绑定和状态变更事件会随转换一同提交。
    ///
    /// # Errors
    ///
    /// 键无效/冲突、任务缺失、状态不允许或任意事务/发件箱失败时返回 [`AppError`]。
    async fn retry(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: String,
    ) -> Result<ScanTaskView, AppError>;
    /// 幂等请求取消拥有的任务，并发出产生的状态事件。
    ///
    /// 排队任务会立即变为已取消；运行任务会设置一个在批次边界观察的标志。
    ///
    /// # Errors
    ///
    /// 幂等性无效/冲突、任务缺失、状态不允许或事务/发件箱失败时返回 [`AppError`]。
    async fn cancel(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: String,
    ) -> Result<ScanTaskView, AppError>;
    /// 返回拥有任务发现的文件的快照稳定分页。
    ///
    /// # Errors
    ///
    /// 任务不存在/不属于该账户、游标针对其他查询，或持久化数据/查询失败时返回 [`AppError`]。
    async fn list_files(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        page: PageRequest,
    ) -> Result<CursorPage<DiscoveredFileView>, AppError>;
    /// 返回拥有任务当前尝试错误的快照稳定分页。
    ///
    /// # Errors
    ///
    /// 任务不存在/不属于该账户、游标针对其他查询，或持久化数据/查询失败时返回 [`AppError`]。
    async fn list_errors(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        page: PageRequest,
    ) -> Result<CursorPage<ScanErrorView>, AppError>;
}

#[derive(Clone)]
/// 提供 UTC 时间戳并委托给 [`TaskStore`] 的轻量用例门面。
pub struct ScanService {
    store: TaskStore,
}

impl ScanService {
    #[must_use]
    /// 创建带进程本地发件箱通知器的服务。
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self::new_with_notifier(pool, OutboxNotifier::new())
    }

    #[must_use]
    /// 创建任务变更提交后会唤醒 `notifier` 订阅者的服务。
    pub fn new_with_notifier(pool: sqlx::SqlitePool, notifier: OutboxNotifier) -> Self {
        Self {
            store: TaskStore::new_with_notifier(pool, notifier),
        }
    }

    #[must_use]
    /// 借用底层事务存储，供工作器/运行时集成使用。
    pub fn store(&self) -> &TaskStore {
        &self.store
    }
}

#[async_trait]
impl ScanUseCases for ScanService {
    async fn create(
        &self,
        account_id: Uuid,
        inbox_directory_id: Uuid,
        idempotency_key: String,
    ) -> Result<ScanTaskView, AppError> {
        self.store
            .create(NewScanTask {
                account_id,
                inbox_directory_id,
                idempotency_key,
                now_us: chrono::Utc::now().timestamp_micros(),
            })
            .await
    }

    async fn get(&self, account_id: Uuid, task_id: Uuid) -> Result<ScanTaskView, AppError> {
        self.store.get(account_id, task_id).await
    }

    async fn list(
        &self,
        account_id: Uuid,
        page: PageRequest,
    ) -> Result<CursorPage<ScanTaskView>, AppError> {
        self.store.list_tasks(account_id, &page).await
    }

    async fn retry(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: String,
    ) -> Result<ScanTaskView, AppError> {
        self.store
            .retry(
                account_id,
                task_id,
                &idempotency_key,
                chrono::Utc::now().timestamp_micros(),
            )
            .await
    }

    async fn cancel(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: String,
    ) -> Result<ScanTaskView, AppError> {
        self.store
            .request_cancel(
                account_id,
                task_id,
                &idempotency_key,
                chrono::Utc::now().timestamp_micros(),
            )
            .await
    }

    async fn list_files(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        page: PageRequest,
    ) -> Result<CursorPage<DiscoveredFileView>, AppError> {
        self.store.list_files(account_id, task_id, &page).await
    }

    async fn list_errors(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        page: PageRequest,
    ) -> Result<CursorPage<ScanErrorView>, AppError> {
        self.store.list_errors(account_id, task_id, &page).await
    }
}
