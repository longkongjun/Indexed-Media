//! One-at-a-time optional notification delivery and startup lifecycle.
use super::delivery_store::{DeliveryOutcome, DeliveryStore};
use super::transport::{PublicHttpsTransport, WebhookTransport};
use crate::shared::error::AppError;
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Clone)]
/// Durable optional worker; external failure never changes the originating task.
pub struct NotificationDeliveryWorker {
    store: DeliveryStore,
    transport: Arc<dyn WebhookTransport>,
}

impl NotificationDeliveryWorker {
    /// Bind explicit storage and transport for controlled boundary checks.
    #[must_use]
    pub fn new(pool: SqlitePool, transport: Arc<dyn WebhookTransport>) -> Self {
        Self {
            store: DeliveryStore::new(pool),
            transport,
        }
    }

    /// Claim and attempt at most one due notification; returns whether work was found.
    ///
    /// # Errors
    /// Returns durable storage failures; transport dispositions are persisted independently.
    pub async fn run_once(&self, now: i64) -> Result<bool, AppError> {
        let Some(lease) = self.store.claim(now).await? else {
            return Ok(false);
        };
        let started = std::time::Instant::now();
        let outcome = if self.store.configuration_current(&lease).await? {
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.transport.deliver(&lease.target_url, &lease.payload),
            )
            .await
            .unwrap_or(DeliveryOutcome::Retryable)
        } else {
            DeliveryOutcome::Rejected
        };
        let elapsed = i64::try_from(started.elapsed().as_micros()).unwrap_or(i64::MAX);
        self.store
            .finish(&lease, outcome, now.saturating_add(elapsed))
            .await?;
        Ok(true)
    }

    /// Start a bounded independent worker loop. The owner aborts and joins it on shutdown.
    #[must_use]
    pub fn start(pool: SqlitePool) -> tokio::task::JoinHandle<()> {
        let worker = Self::new(pool, Arc::new(PublicHttpsTransport));
        tokio::spawn(async move {
            loop {
                match worker.run_once(chrono::Utc::now().timestamp_micros()).await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => eprintln!(
                        "notification.delivery_checkpoint_failed: {}",
                        error.code().as_str()
                    ),
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        })
    }
}
