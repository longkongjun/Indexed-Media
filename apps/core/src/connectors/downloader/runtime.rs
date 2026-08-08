use std::path::PathBuf;
use std::sync::Arc;

use sqlx::{Row as _, SqlitePool};
use tokio::sync::{Notify, watch};

use crate::platform::outbox::OutboxNotifier;
use crate::platform::task_runtime::{SystemTaskClock, TaskClock};
use crate::shared::error::{AppError, ErrorCode};

use super::connection_service::DownloaderRegistry;
use super::connection_store::DownloaderConnectionStore;
use super::task_store::DownloadTaskStore;
use super::worker::DownloadTaskWorker;

#[derive(Clone)]
/// 下载任务 worker 的启动恢复、空闲唤醒与持续后台循环。
pub struct DownloadTaskRuntime {
    pool: SqlitePool,
    notifier: OutboxNotifier,
    registry: DownloaderRegistry,
    clock: Arc<dyn TaskClock>,
    worker_id: String,
    notified: Arc<Notify>,
    failures: watch::Sender<Option<ErrorCode>>,
}

impl DownloadTaskRuntime {
    #[must_use]
    /// 绑定数据库、共享 outbox 通知器、内置适配器和时钟。
    pub fn new(
        pool: SqlitePool,
        notifier: OutboxNotifier,
        registry: DownloaderRegistry,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        let (failures, _) = watch::channel(None);
        Self {
            pool,
            notifier,
            registry,
            clock,
            worker_id: format!("download-core-{}", uuid::Uuid::now_v7()),
            notified: Arc::new(Notify::new()),
            failures,
        }
    }

    /// 构建两个生产适配器与系统时钟的下载任务运行时。
    ///
    /// # Errors
    ///
    /// 底层 HTTP 客户端无法安全构建时返回启动配置错误。
    pub fn production(pool: SqlitePool, notifier: OutboxNotifier) -> Result<Self, AppError> {
        Ok(Self::new(
            pool,
            notifier,
            DownloaderRegistry::production()?,
            Arc::new(SystemTaskClock),
        ))
    }

    /// 处理最早连接的一批到期任务；队列为空时返回零。
    ///
    /// # Errors
    ///
    /// 数据库路径、实例密钥、租约、外部状态提交或 outbox 原子提交失败时返回稳定应用错误。
    pub async fn run_once(&self) -> Result<usize, AppError> {
        self.worker().await?.run_once().await
    }

    /// 重新排队连接配置已经变化的认证/版本阻塞任务。
    ///
    /// # Errors
    ///
    /// 数据库路径、实例密钥、数据库或 outbox 无效时返回稳定应用错误。
    pub async fn prepare(&self) -> Result<u64, AppError> {
        self.worker().await?.prepare().await
    }

    /// 唤醒空闲下载任务循环。
    pub fn notify(&self) {
        self.notified.notify_one();
    }

    #[must_use]
    /// 订阅后台循环最近一次稳定失败码。
    pub fn subscribe_failures(&self) -> watch::Receiver<Option<ErrorCode>> {
        self.failures.subscribe()
    }

    #[must_use]
    /// 启动不终止的有界下载任务循环。
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                loop {
                    match self.run_once().await {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(error) => {
                            self.failures.send_replace(Some(error.code()));
                            break;
                        }
                    }
                }
                tokio::select! {
                    () = self.notified.notified() => {}
                    () = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
                }
            }
        })
    }

    async fn worker(&self) -> Result<DownloadTaskWorker, AppError> {
        let config_dir = database_config_dir(&self.pool).await?;
        Ok(DownloadTaskWorker::new(
            self.worker_id.clone(),
            DownloadTaskStore::open_with_notifier(
                self.pool.clone(),
                &config_dir,
                self.notifier.clone(),
            )?,
            DownloaderConnectionStore::open(self.pool.clone(), &config_dir)?,
            self.registry.clone(),
            self.clock.clone(),
        ))
    }
}

async fn database_config_dir(pool: &SqlitePool) -> Result<PathBuf, AppError> {
    let rows = sqlx::query("PRAGMA database_list")
        .fetch_all(pool)
        .await
        .map_err(internal)?;
    let path = rows
        .iter()
        .find(|row| row.try_get::<String, _>("name").ok().as_deref() == Some("main"))
        .and_then(|row| row.try_get::<String, _>("file").ok())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new(ErrorCode::ConfigInvalid, "database path is unavailable"))?;
    path.parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| AppError::new(ErrorCode::ConfigInvalid, "database parent is unavailable"))
}

fn internal(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
