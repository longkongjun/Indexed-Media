use std::path::Path;

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::platform::outbox::OutboxNotifier;
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, PageRequest};

use super::model::{CreateDownloadTaskCommand, DownloadTaskFilter, DownloadTaskView};
use super::task_store::DownloadTaskStore;

#[derive(Clone)]
/// 协调手工下载任务幂等验收与脱敏查询。
pub struct DownloadTaskService {
    store: DownloadTaskStore,
}

impl DownloadTaskService {
    /// 打开任务服务并固定当前实例密钥。
    ///
    /// # Errors
    ///
    /// 实例密钥不安全或不可用时返回配置错误。
    pub fn open(pool: SqlitePool, config_dir: &Path) -> Result<Self, AppError> {
        Self::open_with_notifier(pool, config_dir, OutboxNotifier::new())
    }

    /// 使用共享 outbox 通知器打开任务服务。
    ///
    /// # Errors
    ///
    /// 实例密钥不安全或不可用时返回配置错误。
    pub fn open_with_notifier(
        pool: SqlitePool,
        config_dir: &Path,
        notifier: OutboxNotifier,
    ) -> Result<Self, AppError> {
        Ok(Self {
            store: DownloadTaskStore::open_with_notifier(pool, config_dir, notifier)?,
        })
    }

    /// 幂等验收一项手工下载任务。
    ///
    /// # Errors
    ///
    /// 输入、连接、幂等关系、加密或数据库失败时返回稳定应用错误。
    pub async fn create(
        &self,
        command: CreateDownloadTaskCommand,
        idempotency_key: &str,
    ) -> Result<DownloadTaskView, AppError> {
        self.store
            .accept(
                command,
                idempotency_key,
                chrono::Utc::now().timestamp_micros(),
            )
            .await
    }

    /// 返回一页脱敏下载任务。
    ///
    /// # Errors
    ///
    /// 过滤、游标、数据库或持久行无效时返回稳定应用错误。
    pub async fn list(
        &self,
        filter: &DownloadTaskFilter,
        page: &PageRequest,
    ) -> Result<CursorPage<DownloadTaskView>, AppError> {
        self.store.list(filter, page).await
    }

    /// 返回一项脱敏下载任务。
    ///
    /// # Errors
    ///
    /// 任务不存在、数据库或持久行无效时返回稳定应用错误。
    pub async fn get(&self, id: Uuid) -> Result<DownloadTaskView, AppError> {
        self.store
            .get(id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "download task not found"))
    }
}
