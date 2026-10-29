//! Durable optional delivery state; database transactions never span network requests.
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use sqlx::{Row as _, SqlitePool};
use uuid::Uuid;

use crate::shared::error::{AppError, ErrorCode};

/// Fixed redacted notification payload. The delivery identity remains stable across retries.
#[derive(Clone, Debug, Serialize)]
pub struct DeliveryPayload {
    /// Stable receiver-side idempotency identity.
    pub delivery_id: String,
    /// Existing notification category.
    pub kind: String,
    /// Bounded administrator-readable title.
    pub title: String,
    /// Bounded redacted body, never a file path or credential.
    pub body: String,
    /// Optional processing task reference.
    pub task_id: Option<Uuid>,
}

/// One exclusively leased delivery and its frozen configuration version.
pub struct DeliveryLease {
    /// Fixed payload.
    pub payload: DeliveryPayload,
    /// Target URL; deliberately omitted from Debug and logs.
    pub target_url: String,
    pub(crate) webhook_id: Vec<u8>,
    pub(crate) config_version: i64,
    pub(crate) token: Uuid,
    pub(crate) attempt: i64,
}

/// Redacted transport disposition used for retry and independent webhook health.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryOutcome {
    /// Receiver accepted the notification.
    Delivered,
    /// Network, rate-limit, or server failure permits bounded retry.
    Retryable,
    /// Receiver rejected credentials; automatic retries stop.
    Unauthorized,
    /// Invalid destination or terminal response; automatic retries stop.
    Rejected,
}

#[derive(Clone)]
/// `SQLite` durable optional delivery store.
pub struct DeliveryStore {
    pool: SqlitePool,
}

impl DeliveryStore {
    /// Bind an already migrated pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Exclusively claim one due delivery, reclaiming an expired attempt if needed.
    ///
    /// # Errors
    /// Returns persistence or corrupt stored-payload errors.
    pub async fn claim(&self, now: i64) -> Result<Option<DeliveryLease>, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database)?;
        sqlx::query(
            "UPDATE notification_deliveries SET status='cancelled',lease_token=NULL,
             lease_expires_at_us=NULL,failure_code='configuration-changed',updated_at_us=?
             WHERE status IN ('pending','sending','retry-wait') AND NOT EXISTS
             (SELECT 1 FROM notification_webhooks w WHERE w.id=webhook_id
              AND w.enabled=1 AND w.config_version=notification_deliveries.config_version)",
        )
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        sqlx::query(
            "UPDATE notification_deliveries SET status='failed',failure_code='attempts-exhausted',
             lease_token=NULL,lease_expires_at_us=NULL,updated_at_us=?
             WHERE status='sending' AND attempt_count>=10 AND lease_expires_at_us<=?",
        )
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        let row = sqlx::query(
            "SELECT d.id,d.webhook_id,d.config_version,d.attempt_count,w.target_url,
                    n.kind,n.title,n.body,n.task_id
             FROM notification_deliveries d
             JOIN notification_webhooks w ON w.id=d.webhook_id
             JOIN notifications n ON n.id=d.notification_id
             WHERE d.attempt_count<10 AND
              (d.status='pending' OR (d.status='retry-wait' AND d.retry_at_us<=?1)
               OR (d.status='sending' AND d.lease_expires_at_us<=?1))
             ORDER BY d.created_at_us,d.id LIMIT 1",
        )
        .bind(now)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database)?;
        let Some(row) = row else {
            tx.commit().await.map_err(database)?;
            return Ok(None);
        };
        let token = Uuid::now_v7();
        let id: String = row.get("id");
        sqlx::query(
            "UPDATE notification_deliveries SET status='sending',attempt_count=attempt_count+1,
             lease_token=?,lease_expires_at_us=?,retry_at_us=NULL,updated_at_us=? WHERE id=?",
        )
        .bind(token.as_bytes().as_slice())
        .bind(now.saturating_add(30_000_000))
        .bind(now)
        .bind(&id)
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        let task: Option<Vec<u8>> = row.get("task_id");
        let task_id = task
            .map(|bytes| Uuid::from_slice(&bytes))
            .transpose()
            .map_err(|error| AppError::with_source(ErrorCode::DatabaseInvalid, error))?;
        let lease = DeliveryLease {
            payload: DeliveryPayload {
                delivery_id: id,
                kind: row.get("kind"),
                title: row.get("title"),
                body: row.get("body"),
                task_id,
            },
            target_url: row.get("target_url"),
            webhook_id: row.get("webhook_id"),
            config_version: row.get("config_version"),
            token,
            attempt: row.get::<i64, _>("attempt_count") + 1,
        };
        tx.commit().await.map_err(database)?;
        Ok(Some(lease))
    }

    /// Recheck the frozen configuration immediately before starting external I/O.
    ///
    /// # Errors
    /// Returns persistence errors.
    pub async fn configuration_current(&self, lease: &DeliveryLease) -> Result<bool, AppError> {
        let current: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM notification_webhooks w
             JOIN notification_deliveries d ON d.webhook_id=w.id AND d.config_version=w.config_version
             WHERE w.id=? AND w.enabled=1 AND w.config_version=?
               AND d.id=? AND d.status='sending' AND d.lease_token=?",
        ).bind(&lease.webhook_id).bind(lease.config_version)
            .bind(&lease.payload.delivery_id).bind(lease.token.as_bytes().as_slice())
            .fetch_one(&self.pool).await.map_err(database)?;
        Ok(current == 1)
    }

    /// Persist only the current lease result; stale attempts cannot overwrite newer state.
    ///
    /// # Errors
    /// Returns persistence errors.
    pub async fn finish(
        &self,
        lease: &DeliveryLease,
        outcome: DeliveryOutcome,
        now: i64,
    ) -> Result<bool, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database)?;
        let current: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM notification_webhooks WHERE id=? AND enabled=1 AND config_version=?",
        ).bind(&lease.webhook_id).bind(lease.config_version).fetch_one(&mut *tx).await.map_err(database)?;
        let (status, failure, health) = if current == 0 {
            ("cancelled", Some("configuration-changed"), "unavailable")
        } else {
            match outcome {
                DeliveryOutcome::Delivered => ("succeeded", None, "healthy"),
                DeliveryOutcome::Retryable if lease.attempt < 10 => {
                    ("retry-wait", Some("provider-unavailable"), "degraded")
                }
                DeliveryOutcome::Retryable => ("failed", Some("attempts-exhausted"), "unavailable"),
                DeliveryOutcome::Unauthorized => ("failed", Some("unauthorized"), "unauthorized"),
                DeliveryOutcome::Rejected => ("failed", Some("target-rejected"), "unavailable"),
            }
        };
        let retry = (status == "retry-wait")
            .then(|| now.saturating_add(retry_delay(lease.attempt, &lease.payload.delivery_id)));
        let changed = sqlx::query(
            "UPDATE notification_deliveries SET status=?,failure_code=?,retry_at_us=?,lease_token=NULL,
             lease_expires_at_us=NULL,updated_at_us=? WHERE id=? AND status='sending'
             AND lease_token=? AND lease_expires_at_us>?",
        ).bind(status).bind(failure).bind(retry).bind(now).bind(&lease.payload.delivery_id)
            .bind(lease.token.as_bytes().as_slice()).bind(now).execute(&mut *tx).await.map_err(database)?.rows_affected() == 1;
        if changed && current == 1 {
            sqlx::query(
                "UPDATE notification_webhooks SET health=?,last_delivery_us=CASE WHEN ?=1 THEN ? ELSE last_delivery_us END
                 WHERE id=? AND config_version=? AND enabled=1",
            ).bind(health).bind(i64::from(outcome == DeliveryOutcome::Delivered)).bind(now)
                .bind(&lease.webhook_id).bind(lease.config_version).execute(&mut *tx).await.map_err(database)?;
        }
        tx.commit().await.map_err(database)?;
        Ok(changed)
    }
}

fn retry_delay(attempt: i64, id: &str) -> i64 {
    let base =
        5_000_000_i64.saturating_mul(1_i64 << u32::try_from(attempt.clamp(1, 10) - 1).unwrap_or(0));
    let digest = Sha256::digest(format!("{id}:{attempt}"));
    base.saturating_mul(80 + i64::from(digest[0] % 41))
        .saturating_div(100)
        .min(3_600_000_000)
}

fn database(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
