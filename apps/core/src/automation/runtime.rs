use std::path::Path;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::{Notify, watch};

use crate::connectors::downloader::task_service::DownloadTaskService;
use crate::discovery::reconcile_service::ReconcileRequestService;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::task_runtime::SystemTaskClock;
use crate::shared::error::{AppError, ErrorCode};

use super::completion::DownloadCompletionDispatcher;
use super::worker::AutomationWorker;

#[derive(Clone)]
/// Startup recovery, bounded drain, and idle wakeup for durable automation events.
pub struct AutomationRuntime {
    worker: AutomationWorker,
    completions: DownloadCompletionDispatcher,
    notified: Arc<Notify>,
    failures: watch::Sender<Option<ErrorCode>>,
}

impl AutomationRuntime {
    /// Build the production event worker with managed download and inbox-reconcile ports.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when an encrypted store cannot open.
    pub fn production(
        pool: SqlitePool,
        config_dir: &Path,
        notifier: OutboxNotifier,
    ) -> Result<Self, AppError> {
        let downloads =
            DownloadTaskService::open_with_notifier(pool.clone(), config_dir, notifier.clone())?;
        let reconciles = ReconcileRequestService::new(pool.clone(), notifier.clone());
        let worker = AutomationWorker::open_with_notifier(
            pool.clone(),
            config_dir,
            Arc::new(downloads),
            Arc::new(reconciles),
            Arc::new(SystemTaskClock),
            notifier.clone(),
        )?;
        let completions = DownloadCompletionDispatcher::open(pool, config_dir, notifier)?;
        let (failures, _) = watch::channel(None);
        Ok(Self {
            worker,
            completions,
            notified: Arc::new(Notify::new()),
            failures,
        })
    }

    /// Reclaim all expired event leases before serving traffic.
    ///
    /// # Errors
    ///
    /// Returns a persistence error.
    pub async fn prepare(&self) -> Result<u64, AppError> {
        let now_us = chrono::Utc::now().timestamp_micros();
        let events = self.worker.prepare().await?;
        let completions = self.completions.prepare(now_us).await?;
        Ok(events.saturating_add(completions))
    }

    /// Execute at most one due event.
    ///
    /// # Errors
    ///
    /// Returns worker persistence or lease errors.
    pub async fn run_once(&self) -> Result<Option<uuid::Uuid>, AppError> {
        let now_us = chrono::Utc::now().timestamp_micros();
        if let Some(dispatch) = self.completions.run_once(now_us).await? {
            return Ok(dispatch.event_id.or(Some(dispatch.download_task_id)));
        }
        self.worker.run_once().await
    }

    /// Wake an idle runtime after a new event commit.
    pub fn notify(&self) {
        self.notified.notify_one();
    }

    #[must_use]
    /// Subscribe to the latest stable loop failure code.
    pub fn subscribe_failures(&self) -> watch::Receiver<Option<ErrorCode>> {
        self.failures.subscribe()
    }

    #[must_use]
    /// Start the persistent bounded event worker loop.
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                loop {
                    match self.run_once().await {
                        Ok(Some(_)) => {}
                        Ok(None) => break,
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
}
