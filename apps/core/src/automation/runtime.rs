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
    events: super::event_store::AutomationEventStore,
    poller: Arc<super::rss::poller::FeedPoller>,
    notified: Arc<Notify>,
    failures: watch::Sender<Option<ErrorCode>>,
}

impl AutomationRuntime {
    /// Build the production event worker with managed download and inbox-reconcile ports.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when an encrypted store cannot open.
    #[allow(clippy::needless_pass_by_value)]
    pub fn production(
        pool: SqlitePool,
        config_dir: &Path,
        notifier: OutboxNotifier,
    ) -> Result<Self, AppError> {
        let feed_client = super::rss::client::FeedHttpClient::production()
            .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
        Self::with_feed_client(pool, config_dir, notifier, Arc::new(feed_client))
    }

    /// Build the same durable runtime with an explicit bounded feed transport.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when an encrypted store cannot open.
    #[allow(clippy::needless_pass_by_value)]
    pub fn with_feed_client(
        pool: SqlitePool,
        config_dir: &Path,
        notifier: OutboxNotifier,
        feed_client: Arc<dyn super::rss::client::FeedClient>,
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
        let completions =
            DownloadCompletionDispatcher::open(pool.clone(), config_dir, notifier.clone())?;
        let events = super::event_store::AutomationEventStore::open_with_notifier(
            pool.clone(),
            config_dir,
            notifier.clone(),
        )?;
        let poller = Arc::new(super::rss::poller::FeedPoller::new(
            feed_client,
            Arc::new(events.clone()),
        ));
        let (failures, _) = watch::channel(None);
        Ok(Self {
            worker,
            completions,
            events,
            poller,
            notified: Arc::new(Notify::new()),
            failures,
        })
    }

    /// 轮询至多一个到期 RSS 源并原子提交草稿事件。
    ///
    /// # Errors
    ///
    /// 数据库、密钥或上游 feed 故障会记录到源的失败提交后返回；提交本身失败向上传播。
    pub async fn poll_due_rss_source(&self) -> Result<bool, AppError> {
        let now_us = chrono::Utc::now().timestamp_micros();
        let Some(source) = self.events.claim_due_rss_source(now_us, 30_000_000).await? else {
            return Ok(false);
        };
        match self.poller.poll(source).await {
            Ok(result) => {
                let _ = result;
                Ok(true)
            }
            // poller 在 Err 时已通过 commit port 记录失败并释放租约。
            Err(_) => Ok(true),
        }
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

    /// Give completion signals, durable events, and due feeds one bounded turn each.
    ///
    /// # Errors
    ///
    /// Returns persistence or lease errors without treating failed checkpoints as success.
    pub async fn run_once(&self) -> Result<Option<uuid::Uuid>, AppError> {
        let now_us = chrono::Utc::now().timestamp_micros();
        let completion = self.completions.run_once(now_us).await?;
        let event = self.worker.run_once().await?;
        let polled = self.poll_due_rss_source().await?;
        Ok(completion
            .map(|dispatch| dispatch.event_id.unwrap_or(dispatch.download_task_id))
            .or(event)
            .or_else(|| polled.then(uuid::Uuid::nil)))
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
