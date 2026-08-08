use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use uuid::Uuid;

use crate::connectors::downloader::model::CreateDownloadTaskCommand;
use crate::connectors::model::SecretString;
use crate::platform::outbox::{OutboxNotifier, OutboxWriter};
use crate::platform::secrets::{
    InstanceKey, IntegrationKind, SealedSecret, SecretAad, SecretBytes, SecretCipher as _,
};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, PageRequest};
use crate::tasks::events::{AUTOMATION_EVENT_CHANGED, AutomationEventChangedPayload};

use super::model::{
    AutomationAction, AutomationEventFilter, AutomationEventStatus, AutomationEventView,
    AutomationFailureCode,
};
use super::rss::client::FeedClientError;
use super::rss::poller::{FeedPollCommit, FeedPollCommitPort};

type EventRow = (
    Vec<u8>,
    Vec<u8>,
    String,
    String,
    String,
    Option<String>,
    Option<Vec<u8>>,
    Option<i64>,
    Option<String>,
    i64,
    Option<i64>,
    i64,
    i64,
    i64,
);

type LeaseRow = (Vec<u8>, String, i64, Vec<u8>, Vec<u8>, i64);

const EVENT_COLUMNS: &str =
    "id,source_id,source_display_name,action,status,downstream_kind,downstream_id,
     result_count,failure_code,attempt_count,retry_at_us,projection_version,created_at_us,updated_at_us";

#[derive(Deserialize, Serialize)]
struct EventCursor {
    version: u8,
    filter_digest: String,
    updated_at_us: i64,
    id: Uuid,
}

#[derive(Deserialize, Serialize)]
struct CursorEnvelope {
    payload: String,
    checksum: String,
}

#[derive(Serialize)]
struct RssDownloadPayload<'a> {
    kind: &'static str,
    downloader_connection_id: Uuid,
    source: &'a str,
    display_name: &'a str,
}

#[derive(Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum WirePayload {
    #[serde(rename = "download.create")]
    DownloadCreate {
        downloader_connection_id: Uuid,
        source: SecretString,
        display_name: String,
    },
    #[serde(rename = "inbox.reconcile")]
    InboxReconcile { inbox_directory_id: Uuid },
}

/// Decrypted short-lived fixed action payload.
pub enum AutomationEventPayload {
    /// Create one managed download.
    CreateDownload {
        /// Target connection.
        connection_id: Uuid,
        /// Validated magnet or HTTPS torrent source.
        source: SecretBytes,
        /// Bounded display name.
        display_name: String,
    },
    /// Reconcile one registered inbox.
    ReconcileInbox {
        /// Target inbox.
        inbox_id: Uuid,
    },
}

impl AutomationEventPayload {
    #[must_use]
    pub const fn connection_id(&self) -> Option<Uuid> {
        match self {
            Self::CreateDownload { connection_id, .. } => Some(*connection_id),
            Self::ReconcileInbox { .. } => None,
        }
    }

    #[must_use]
    pub fn source(&self) -> Option<&[u8]> {
        match self {
            Self::CreateDownload { source, .. } => Some(source.expose()),
            Self::ReconcileInbox { .. } => None,
        }
    }

    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        match self {
            Self::CreateDownload { display_name, .. } => Some(display_name),
            Self::ReconcileInbox { .. } => None,
        }
    }

    #[must_use]
    pub const fn inbox_id(&self) -> Option<Uuid> {
        match self {
            Self::CreateDownload { .. } => None,
            Self::ReconcileInbox { inbox_id } => Some(*inbox_id),
        }
    }
}

impl std::fmt::Debug for AutomationEventPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateDownload {
                connection_id,
                display_name,
                ..
            } => formatter
                .debug_struct("CreateDownload")
                .field("connection_id", connection_id)
                .field("source", &"[REDACTED]")
                .field("display_name", display_name)
                .finish(),
            Self::ReconcileInbox { inbox_id } => formatter
                .debug_struct("ReconcileInbox")
                .field("inbox_id", inbox_id)
                .finish(),
        }
    }
}

/// Claimed event payload and lease identity.
pub struct AutomationEventLease {
    /// Event identifier.
    pub id: Uuid,
    /// Decrypted action payload.
    pub payload: AutomationEventPayload,
    /// Attempt including this claim.
    pub attempt_count: u8,
    lease_token: Uuid,
}

impl std::fmt::Debug for AutomationEventLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AutomationEventLease")
            .field("id", &self.id)
            .field("payload", &self.payload)
            .field("attempt_count", &self.attempt_count)
            .field("lease_token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone)]
/// Durable encrypted automation event state machine store.
pub struct AutomationEventStore {
    pool: SqlitePool,
    key: Arc<InstanceKey>,
    notifier: OutboxNotifier,
}

impl AutomationEventStore {
    /// Open the event store with the current instance key.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the key boundary is unsafe.
    pub fn open(pool: SqlitePool, config_dir: &Path) -> Result<Self, AppError> {
        Self::open_with_notifier(pool, config_dir, OutboxNotifier::new())
    }

    /// Open the event store with a shared post-commit outbox notifier.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the key boundary is unsafe.
    pub fn open_with_notifier(
        pool: SqlitePool,
        config_dir: &Path,
        notifier: OutboxNotifier,
    ) -> Result<Self, AppError> {
        let key = InstanceKey::load_or_create(config_dir)
            .map(Arc::new)
            .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
        Ok(Self {
            pool,
            key,
            notifier,
        })
    }

    /// Read one redacted event.
    ///
    /// # Errors
    ///
    /// Returns an internal error for unavailable or invalid persisted state.
    pub async fn get(&self, id: Uuid) -> Result<Option<AutomationEventView>, AppError> {
        let query = format!("SELECT {EVENT_COLUMNS} FROM automation_events WHERE id=?");
        sqlx::query_as::<_, EventRow>(&query)
            .bind(id.as_bytes().as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(database_error)?
            .map(decode_view)
            .transpose()
    }

    /// List redacted events in stable update order.
    ///
    /// # Errors
    ///
    /// Returns validation or persistence errors.
    pub async fn list_page(
        &self,
        page: &PageRequest,
    ) -> Result<CursorPage<AutomationEventView>, AppError> {
        self.list_filtered_page(page, AutomationEventFilter::default())
            .await
    }

    /// List redacted events with a cursor bound to the selected filters.
    ///
    /// # Errors
    ///
    /// Returns validation or persistence errors.
    pub async fn list_filtered_page(
        &self,
        page: &PageRequest,
        filter: AutomationEventFilter,
    ) -> Result<CursorPage<AutomationEventView>, AppError> {
        if page.limit == 0 || page.limit > crate::shared::page::MAX_PAGE_LIMIT {
            return Err(validation("automation event page limit is outside bounds"));
        }
        let filter_digest = event_filter_digest(filter);
        let cursor = page.cursor.as_deref().map(decode_cursor).transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.filter_digest != filter_digest)
        {
            return Err(validation("automation event cursor filter changed"));
        }
        let mut query = QueryBuilder::<Sqlite>::new("SELECT ");
        query.push(EVENT_COLUMNS).push(" FROM automation_events");
        let mut has_where = false;
        if let Some(source_id) = filter.source_id {
            push_where(&mut query, &mut has_where);
            query
                .push("source_id=")
                .push_bind(source_id.as_bytes().to_vec());
        }
        if let Some(status) = filter.status {
            push_where(&mut query, &mut has_where);
            query.push("status=").push_bind(status.as_str());
        }
        if let Some(action) = filter.action {
            push_where(&mut query, &mut has_where);
            query.push("action=").push_bind(action.as_str());
        }
        if let Some(cursor) = &cursor {
            push_where(&mut query, &mut has_where);
            query
                .push("(updated_at_us<")
                .push_bind(cursor.updated_at_us)
                .push(" OR (updated_at_us=")
                .push_bind(cursor.updated_at_us)
                .push(" AND id<")
                .push_bind(cursor.id.as_bytes().to_vec())
                .push("))");
        }
        query
            .push(" ORDER BY updated_at_us DESC,id DESC LIMIT ")
            .push_bind(i64::from(page.limit) + 1);
        let rows = query
            .build_query_as::<EventRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(database_error)?;
        let has_more = rows.len() > page.limit as usize;
        let items = rows
            .into_iter()
            .take(page.limit as usize)
            .map(decode_view)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if has_more {
            items
                .last()
                .map(|item| {
                    encode_cursor(&EventCursor {
                        version: 1,
                        filter_digest: filter_digest.clone(),
                        updated_at_us: parse_time(&item.updated_at)?,
                        id: item.id,
                    })
                })
                .transpose()?
        } else {
            None
        };
        Ok(CursorPage { items, next_cursor })
    }

    /// Claim one due event with a bounded lease.
    ///
    /// # Errors
    ///
    /// Returns validation, persistence, or payload authentication errors.
    pub async fn claim_one(
        &self,
        now_us: i64,
        lease_duration_us: i64,
    ) -> Result<Option<AutomationEventLease>, AppError> {
        if lease_duration_us <= 0 {
            return Err(validation("invalid automation lease duration"));
        }
        let lease_token = Uuid::now_v7();
        let lease_expires = now_us
            .checked_add(lease_duration_us)
            .ok_or_else(|| validation("invalid automation lease deadline"))?;
        for _ in 0..100 {
            let mut tx = self
                .pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .map_err(database_error)?;
            let row = sqlx::query_as::<_, LeaseRow>(
                "SELECT id,action,payload_schema_version,payload_nonce,payload_ciphertext,attempt_count
                 FROM automation_events WHERE attempt_count<10
                   AND (status='pending' OR (status='retry-wait' AND retry_at_us<=?))
                 ORDER BY created_at_us,id LIMIT 1",
            )
            .bind(now_us)
            .fetch_optional(&mut *tx)
            .await
            .map_err(database_error)?;
            let Some(row) = row else {
                tx.commit().await.map_err(database_error)?;
                return Ok(None);
            };
            let id = Uuid::from_slice(&row.0).map_err(invalid_database)?;
            let Ok(payload) = self.decrypt_payload(id, &row.1, row.2, row.3, row.4) else {
                let failure = if AutomationAction::parse(&row.1).is_none() {
                    AutomationFailureCode::ActionInvalid
                } else {
                    AutomationFailureCode::PayloadInvalid
                };
                sqlx::query(
                    "UPDATE automation_events SET status='failed',attempt_count=attempt_count+1,
                     failure_code=?,retry_at_us=NULL,lease_token=NULL,lease_expires_at_us=NULL,
                     projection_version=projection_version+1,updated_at_us=? WHERE id=?
                     AND (status='pending' OR status='retry-wait')",
                )
                .bind(failure.as_str())
                .bind(now_us)
                .bind(&row.0)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
                if AutomationAction::parse(&row.1).is_some() {
                    write_event(&mut tx, id, now_us).await?;
                }
                tx.commit().await.map_err(database_error)?;
                self.notifier.notify_after_commit();
                continue;
            };
            let rows = sqlx::query(
                "UPDATE automation_events SET status='running',attempt_count=attempt_count+1,
                 retry_at_us=NULL,lease_token=?,lease_expires_at_us=?,projection_version=projection_version+1,
                 updated_at_us=? WHERE id=? AND (status='pending' OR status='retry-wait')",
            )
            .bind(lease_token.as_bytes().as_slice())
            .bind(lease_expires)
            .bind(now_us)
            .bind(&row.0)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
            if rows.rows_affected() != 1 {
                return Err(AppError::new(
                    ErrorCode::TaskLeaseLost,
                    "automation lease lost",
                ));
            }
            write_event(&mut tx, id, now_us).await?;
            tx.commit().await.map_err(database_error)?;
            self.notifier.notify_after_commit();
            return Ok(Some(AutomationEventLease {
                id,
                payload,
                attempt_count: u8::try_from(row.5 + 1).map_err(invalid_database)?,
                lease_token,
            }));
        }
        Ok(None)
    }

    /// Complete one leased event with a stable downstream relation.
    ///
    /// # Errors
    ///
    /// Returns lease-lost or persistence errors.
    pub async fn complete(
        &self,
        lease: &AutomationEventLease,
        downstream_kind: &str,
        downstream_id: Uuid,
        result_count: i64,
        now_us: i64,
    ) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let rows = sqlx::query(
            "UPDATE automation_events SET status='completed',downstream_kind=?,downstream_id=?,
             result_count=?,failure_code=NULL,lease_token=NULL,lease_expires_at_us=NULL,
             projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND status='running' AND lease_token=?",
        )
        .bind(downstream_kind)
        .bind(downstream_id.as_bytes().as_slice())
        .bind(result_count)
        .bind(now_us)
        .bind(lease.id.as_bytes().as_slice())
        .bind(lease.lease_token.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::TaskLeaseLost,
                "automation lease lost",
            ));
        }
        write_event(&mut tx, lease.id, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        Ok(())
    }

    /// Commit a recoverable or terminal action failure for one lease.
    ///
    /// # Errors
    ///
    /// Returns lease-lost or persistence errors.
    pub async fn fail(
        &self,
        lease: &AutomationEventLease,
        code: AutomationFailureCode,
        recoverable: bool,
        now_us: i64,
    ) -> Result<(), AppError> {
        let retry = recoverable && lease.attempt_count < 10;
        let status = if retry { "retry-wait" } else { "failed" };
        let retry_at = retry.then_some(now_us.saturating_add(backoff_us(lease.attempt_count)));
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let rows = sqlx::query(
            "UPDATE automation_events SET status=?,failure_code=?,retry_at_us=?,lease_token=NULL,
             lease_expires_at_us=NULL,projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND status='running' AND lease_token=?",
        )
        .bind(status)
        .bind(code.as_str())
        .bind(retry_at)
        .bind(now_us)
        .bind(lease.id.as_bytes().as_slice())
        .bind(lease.lease_token.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::TaskLeaseLost,
                "automation lease lost",
            ));
        }
        write_event(&mut tx, lease.id, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        Ok(())
    }

    /// Requeue or terminally fail expired running leases without external calls.
    ///
    /// # Errors
    ///
    /// Returns a persistence error.
    pub async fn reclaim_expired(&self, now_us: i64) -> Result<u64, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let ids = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT id FROM automation_events WHERE status='running' AND lease_expires_at_us<=?
             ORDER BY lease_expires_at_us,id LIMIT 1000",
        )
        .bind(now_us)
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        for raw_id in &ids {
            sqlx::query(
                "UPDATE automation_events SET
                 status=CASE WHEN attempt_count>=10 THEN 'failed' ELSE 'retry-wait' END,
                 failure_code='integration.unavailable',
                 retry_at_us=CASE WHEN attempt_count>=10 THEN NULL ELSE ? END,
                 lease_token=NULL,lease_expires_at_us=NULL,
                 projection_version=projection_version+1,updated_at_us=?
                 WHERE id=? AND status='running' AND lease_expires_at_us<=?",
            )
            .bind(now_us)
            .bind(now_us)
            .bind(raw_id)
            .bind(now_us)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
            let id = Uuid::from_slice(raw_id).map_err(invalid_database)?;
            write_event(&mut tx, id, now_us).await?;
        }
        tx.commit().await.map_err(database_error)?;
        if !ids.is_empty() {
            self.notifier.notify_after_commit();
        }
        u64::try_from(ids.len()).map_err(invalid_database)
    }

    /// Idempotently cancel a not-yet-completed event.
    ///
    /// # Errors
    ///
    /// Returns validation, not-found, state, conflict, or persistence errors.
    pub async fn cancel(
        &self,
        id: Uuid,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<AutomationEventView, AppError> {
        self.command(id, "cancel", idempotency_key, now_us).await
    }

    /// Idempotently retry a failed event.
    ///
    /// # Errors
    ///
    /// Returns validation, not-found, state, conflict, or persistence errors.
    pub async fn retry(
        &self,
        id: Uuid,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<AutomationEventView, AppError> {
        self.command(id, "retry", idempotency_key, now_us).await
    }

    async fn command(
        &self,
        id: Uuid,
        operation: &'static str,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<AutomationEventView, AppError> {
        validate_idempotency(idempotency_key)?;
        let key_digest: [u8; 32] = Sha256::digest(idempotency_key.as_bytes()).into();
        let request_digest = command_digest(id, operation);
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let receipt = sqlx::query_as::<_, (String, Vec<u8>)>(
            "SELECT operation,request_sha256 FROM automation_event_command_receipts
             WHERE event_id=? AND idempotency_key_sha256=?",
        )
        .bind(id.as_bytes().as_slice())
        .bind(key_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        if let Some((existing_operation, existing_request)) = receipt {
            if existing_operation != operation || existing_request.as_slice() != request_digest {
                return Err(AppError::new(
                    ErrorCode::RequestConflict,
                    "automation command idempotency conflict",
                ));
            }
            tx.commit().await.map_err(database_error)?;
            return self
                .get(id)
                .await?
                .ok_or_else(|| not_found("automation event not found"));
        }
        let result = match operation {
            "cancel" => {
                sqlx::query(
                    "UPDATE automation_events SET status='cancelled',retry_at_us=NULL,
                     lease_token=NULL,lease_expires_at_us=NULL,projection_version=projection_version+1,
                     updated_at_us=? WHERE id=? AND downstream_id IS NULL
                     AND status IN ('pending','retry-wait')",
                )
                .bind(now_us)
                .bind(id.as_bytes().as_slice())
                .execute(&mut *tx)
                .await
                .map_err(database_error)?
            }
            "retry" => {
                sqlx::query(
                    "UPDATE automation_events SET status='pending',attempt_count=0,retry_at_us=NULL,
                     failure_code=NULL,projection_version=projection_version+1,updated_at_us=?
                     WHERE id=? AND status='failed'",
                )
                .bind(now_us)
                .bind(id.as_bytes().as_slice())
                .execute(&mut *tx)
                .await
                .map_err(database_error)?
            }
            _ => return Err(validation("invalid automation event command")),
        };
        if result.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::TaskInvalidState,
                "automation event command is invalid for current state",
            ));
        }
        write_event(&mut tx, id, now_us).await?;
        let projection = sqlx::query_scalar::<_, i64>(
            "SELECT projection_version FROM automation_events WHERE id=?",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO automation_event_command_receipts
             (event_id,operation,idempotency_key_sha256,request_sha256,
              resulting_projection_version,created_at_us) VALUES (?,?,?,?,?,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(operation)
        .bind(key_digest.as_slice())
        .bind(request_digest.as_slice())
        .bind(projection)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        self.get(id)
            .await?
            .ok_or_else(|| not_found("automation event not found"))
    }

    fn decrypt_payload(
        &self,
        id: Uuid,
        action: &str,
        schema: i64,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Result<AutomationEventPayload, AppError> {
        let schema = u16::try_from(schema).map_err(invalid_database)?;
        let nonce: [u8; 24] = nonce
            .try_into()
            .map_err(|_| invalid_database("invalid event nonce"))?;
        let sealed = SealedSecret::from_parts(schema, nonce, ciphertext)
            .map_err(|error| invalid_database(error.to_string()))?;
        let plaintext = self
            .key
            .open(
                &SecretAad::new(id, IntegrationKind::AutomationEventPayload, 1, schema),
                &sealed,
            )
            .map_err(secret_error)?;
        let wire: WirePayload = serde_json::from_slice(plaintext.expose())
            .map_err(|_| invalid_database("invalid event payload"))?;
        match (AutomationAction::parse(action), wire) {
            (
                Some(AutomationAction::CreateDownload),
                WirePayload::DownloadCreate {
                    downloader_connection_id,
                    source,
                    display_name,
                },
            ) => {
                let command = CreateDownloadTaskCommand {
                    connection_id: downloader_connection_id,
                    source,
                    display_name,
                }
                .validate()
                .map_err(|_| invalid_database("invalid download event payload"))?;
                Ok(AutomationEventPayload::CreateDownload {
                    connection_id: command.connection_id,
                    source: command.source,
                    display_name: command.display_name,
                })
            }
            (
                Some(AutomationAction::ReconcileInbox),
                WirePayload::InboxReconcile { inbox_directory_id },
            ) => Ok(AutomationEventPayload::ReconcileInbox {
                inbox_id: inbox_directory_id,
            }),
            _ => Err(invalid_database("event action and payload disagree")),
        }
    }
}

#[async_trait]
impl FeedPollCommitPort for AutomationEventStore {
    async fn commit(
        &self,
        source_id: Uuid,
        expected_config_version: i64,
        commit: FeedPollCommit,
    ) -> Result<usize, FeedClientError> {
        self.commit_rss_poll(source_id, expected_config_version, commit)
            .await
            .map_err(|error| match error.code() {
                ErrorCode::RequestConflict | ErrorCode::ValidationFailed => {
                    FeedClientError::InvalidResponse
                }
                _ => FeedClientError::Unavailable,
            })
    }

    async fn commit_failure(
        &self,
        source_id: Uuid,
        expected_config_version: i64,
        failure: FeedClientError,
    ) -> Result<(), FeedClientError> {
        let (health, code) = match failure {
            FeedClientError::Unauthorized => ("unauthorized", "integration.unauthorized"),
            FeedClientError::RateLimited => ("rate-limited", "integration.rate-limited"),
            FeedClientError::Unavailable => ("unavailable", "integration.unavailable"),
            FeedClientError::Timeout => ("unavailable", "provider.timeout"),
            FeedClientError::ResponseTooLarge => ("degraded", "provider.response-too-large"),
            FeedClientError::InvalidResponse => ("degraded", "provider.invalid-response"),
        };
        let now = chrono::Utc::now().timestamp_micros();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|_| FeedClientError::Unavailable)?;
        let attempt = sqlx::query_scalar::<_, i64>(
            "SELECT runtime.attempt_count FROM automation_source_runtime runtime
             JOIN automation_sources source ON source.id=runtime.source_id
             WHERE source.id=? AND source.kind='rss' AND source.enabled=1
               AND source.config_version=?",
        )
        .bind(source_id.as_bytes().as_slice())
        .bind(expected_config_version)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| FeedClientError::Unavailable)?
        .ok_or(FeedClientError::InvalidResponse)?;
        let next_attempt = (attempt + 1).min(10);
        let exponent = u32::try_from(next_attempt.saturating_sub(1).min(10))
            .map_err(|_| FeedClientError::Unavailable)?;
        let delay_seconds = 60_i64.saturating_mul(1_i64 << exponent).min(86_400);
        sqlx::query(
            "UPDATE automation_source_runtime SET next_poll_at_us=?,attempt_count=?,
             lease_token=NULL,lease_expires_at_us=NULL,updated_at_us=? WHERE source_id=?",
        )
        .bind(now.saturating_add(delay_seconds.saturating_mul(1_000_000)))
        .bind(next_attempt)
        .bind(now)
        .bind(source_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(|_| FeedClientError::Unavailable)?;
        let rows = sqlx::query(
            "UPDATE automation_sources SET health=?,failure_code=?,checked_at_us=?,
             projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND kind='rss' AND config_version=?",
        )
        .bind(health)
        .bind(code)
        .bind(now)
        .bind(now)
        .bind(source_id.as_bytes().as_slice())
        .bind(expected_config_version)
        .execute(&mut *tx)
        .await
        .map_err(|_| FeedClientError::Unavailable)?;
        if rows.rows_affected() != 1 {
            return Err(FeedClientError::InvalidResponse);
        }
        tx.commit()
            .await
            .map_err(|_| FeedClientError::Unavailable)?;
        Ok(())
    }
}

impl AutomationEventStore {
    #[allow(clippy::too_many_lines)]
    async fn commit_rss_poll(
        &self,
        source_id: Uuid,
        expected_config_version: i64,
        commit: FeedPollCommit,
    ) -> Result<usize, AppError> {
        let now = chrono::Utc::now().timestamp_micros();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let source = sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT display_name,enabled,poll_interval_seconds FROM automation_sources
             WHERE id=? AND kind='rss' AND config_version=?",
        )
        .bind(source_id.as_bytes().as_slice())
        .bind(expected_config_version)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| validation("RSS source version changed"))?;
        if source.1 != 1 {
            return Err(AppError::new(
                ErrorCode::AutomationSourceDisabled,
                "RSS source is disabled",
            ));
        }
        let mut accepted = 0_usize;
        for draft in commit.drafts {
            let source_text = std::str::from_utf8(draft.source().expose())
                .map_err(|_| validation("invalid RSS download source"))?;
            let mut payload = serde_json::to_vec(&RssDownloadPayload {
                kind: "download.create",
                downloader_connection_id: draft.downloader_connection_id,
                source: source_text,
                display_name: &draft.display_name,
            })
            .map_err(invalid_database)?;
            let event_id = Uuid::now_v7();
            let sealed = self
                .key
                .seal(
                    &SecretAad::new(event_id, IntegrationKind::AutomationEventPayload, 1, 1),
                    &payload,
                )
                .map_err(secret_error)?;
            let request_digest: [u8; 32] = Sha256::digest(&payload).into();
            payload.fill(0);
            let result = sqlx::query(
                "INSERT INTO automation_events
                 (id,source_id,source_display_name,source_config_version,action,dedup_key_sha256,
                  request_sha256,payload_schema_version,payload_nonce,payload_ciphertext,status,
                  downstream_kind,downstream_id,result_count,failure_code,attempt_count,retry_at_us,
                  lease_token,lease_expires_at_us,projection_version,created_at_us,updated_at_us)
                 VALUES (?,?,?,?, 'create-download',?,?,?,?,?,'pending',NULL,NULL,NULL,NULL,0,
                         NULL,NULL,NULL,1,?,?)
                 ON CONFLICT(source_id,dedup_key_sha256) DO NOTHING",
            )
            .bind(event_id.as_bytes().as_slice())
            .bind(source_id.as_bytes().as_slice())
            .bind(&source.0)
            .bind(expected_config_version)
            .bind(draft.dedup_key.as_slice())
            .bind(request_digest.as_slice())
            .bind(i64::from(sealed.schema_version()))
            .bind(sealed.nonce().as_slice())
            .bind(sealed.ciphertext())
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
            if result.rows_affected() == 1 {
                write_event(&mut tx, event_id, now).await?;
            } else {
                let existing = sqlx::query_as::<_, (String, Vec<u8>)>(
                    "SELECT action,request_sha256 FROM automation_events
                     WHERE source_id=? AND dedup_key_sha256=?",
                )
                .bind(source_id.as_bytes().as_slice())
                .bind(draft.dedup_key.as_slice())
                .fetch_one(&mut *tx)
                .await
                .map_err(database_error)?;
                if existing.0 != AutomationAction::CreateDownload.as_str()
                    || existing.1.as_slice() != request_digest
                {
                    return Err(AppError::new(
                        ErrorCode::RequestConflict,
                        "automation event dedup key conflicts with existing payload",
                    ));
                }
            }
            accepted += usize::try_from(result.rows_affected()).map_err(invalid_database)?;
        }
        let (cursor_schema, cursor_nonce, cursor_ciphertext) = match commit.cursor {
            Some(cursor) => {
                let mut bytes = serde_json::to_vec(&cursor).map_err(invalid_database)?;
                let sealed = self
                    .key
                    .seal(
                        &SecretAad::new(
                            source_id,
                            IntegrationKind::AutomationRssCursor,
                            expected_config_version,
                            1,
                        ),
                        &bytes,
                    )
                    .map_err(secret_error)?;
                bytes.fill(0);
                (
                    Some(i64::from(sealed.schema_version())),
                    Some(sealed.nonce().to_vec()),
                    Some(sealed.ciphertext().to_vec()),
                )
            }
            None => (None, None, None),
        };
        let next_poll = now.saturating_add(source.2.saturating_mul(1_000_000));
        sqlx::query(
            "UPDATE automation_source_runtime SET next_poll_at_us=?,cursor_schema_version=?,
             cursor_nonce=?,cursor_ciphertext=?,lease_token=NULL,lease_expires_at_us=NULL,
             attempt_count=0,updated_at_us=? WHERE source_id=?",
        )
        .bind(next_poll)
        .bind(cursor_schema)
        .bind(cursor_nonce)
        .bind(cursor_ciphertext)
        .bind(now)
        .bind(source_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "UPDATE automation_sources SET health='healthy',failure_code=NULL,checked_at_us=?,
             projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND config_version=?",
        )
        .bind(now)
        .bind(now)
        .bind(source_id.as_bytes().as_slice())
        .bind(expected_config_version)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        if accepted > 0 {
            self.notifier.notify_after_commit();
        }
        Ok(accepted)
    }
}

fn push_where(query: &mut QueryBuilder<'_, Sqlite>, has_where: &mut bool) {
    if *has_where {
        query.push(" AND ");
    } else {
        query.push(" WHERE ");
        *has_where = true;
    }
}

fn event_filter_digest(filter: AutomationEventFilter) -> String {
    let mut digest = Sha256::new();
    digest.update(b"mediaflow.automation-event.filter.v1\0");
    if let Some(source_id) = filter.source_id {
        digest.update(source_id.as_bytes());
    }
    digest.update([0]);
    if let Some(status) = filter.status {
        digest.update(status.as_str().as_bytes());
    }
    digest.update([0]);
    if let Some(action) = filter.action {
        digest.update(action.as_str().as_bytes());
    }
    hex::encode(&digest.finalize()[..16])
}

pub(crate) async fn write_event(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    id: Uuid,
    now_us: i64,
) -> Result<(), AppError> {
    let row = sqlx::query_as::<_, (i64, String, String, Option<String>)>(
        "SELECT projection_version,status,action,failure_code FROM automation_events WHERE id=?",
    )
    .bind(id.as_bytes().as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    let status = AutomationEventStatus::parse(&row.1)
        .ok_or_else(|| invalid_database("invalid automation event status"))?;
    let action = AutomationAction::parse(&row.2)
        .ok_or_else(|| invalid_database("invalid automation event action"))?;
    let failure_code = row
        .3
        .as_deref()
        .map(|value| {
            AutomationFailureCode::parse(value)
                .ok_or_else(|| invalid_database("invalid automation event failure"))
        })
        .transpose()?;
    OutboxWriter::write(
        tx,
        AUTOMATION_EVENT_CHANGED,
        id,
        &AutomationEventChangedPayload {
            automation_event_id: id,
            projection_version: row.0,
            status,
            action,
            failure_code,
        },
        now_us,
    )
    .await?;
    Ok(())
}

fn decode_view(row: EventRow) -> Result<AutomationEventView, AppError> {
    let status = AutomationEventStatus::parse(&row.4)
        .ok_or_else(|| invalid_database("invalid automation event status"))?;
    let downstream_id = row.6.as_deref().map(decode_uuid).transpose()?;
    let allowed_actions = match status {
        AutomationEventStatus::Pending | AutomationEventStatus::RetryWait
            if downstream_id.is_none() =>
        {
            vec!["cancel".to_owned()]
        }
        AutomationEventStatus::Failed => vec!["retry".to_owned()],
        _ => Vec::new(),
    };
    Ok(AutomationEventView {
        id: decode_uuid(&row.0)?,
        source_id: decode_uuid(&row.1)?,
        source_display_name: row.2,
        action: AutomationAction::parse(&row.3)
            .ok_or_else(|| invalid_database("invalid automation event action"))?,
        status,
        downstream_kind: row.5,
        downstream_id,
        result_count: row.7,
        failure_code: row
            .8
            .map(|value| {
                AutomationFailureCode::parse(&value)
                    .ok_or_else(|| invalid_database("invalid automation event failure"))
            })
            .transpose()?,
        attempt_count: u8::try_from(row.9).map_err(invalid_database)?,
        retry_at: row.10.map(format_time).transpose()?,
        allowed_actions,
        projection_version: row.11,
        created_at: format_time(row.12)?,
        updated_at: format_time(row.13)?,
    })
}

fn backoff_us(attempt: u8) -> i64 {
    let exponent = u32::from(attempt.saturating_sub(1).min(10));
    1_000_000_i64.saturating_mul(1_i64 << exponent)
}

fn validate_idempotency(value: &str) -> Result<(), AppError> {
    if !(1..=255).contains(&value.len()) || value.chars().any(char::is_control) {
        return Err(validation("invalid automation command idempotency key"));
    }
    Ok(())
}

fn command_digest(id: Uuid, operation: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"mediaflow.automation-command.v1\0");
    digest.update(id.as_bytes());
    digest.update([0]);
    digest.update(operation.as_bytes());
    digest.finalize().into()
}

fn decode_uuid(value: &[u8]) -> Result<Uuid, AppError> {
    Uuid::from_slice(value).map_err(invalid_database)
}

fn format_time(value: i64) -> Result<String, AppError> {
    chrono::DateTime::from_timestamp_micros(value)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .ok_or_else(|| invalid_database("invalid automation event timestamp"))
}

fn parse_time(value: &str) -> Result<i64, AppError> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| time.timestamp_micros())
        .map_err(invalid_database)
}

fn encode_cursor(cursor: &EventCursor) -> Result<String, AppError> {
    let payload = serde_json::to_vec(cursor).map_err(invalid_database)?;
    let envelope = CursorEnvelope {
        payload: URL_SAFE_NO_PAD.encode(&payload),
        checksum: cursor_checksum(&payload),
    };
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).map_err(invalid_database)?);
    if encoded.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(invalid_database("automation event cursor exceeds bounds"));
    }
    Ok(encoded)
}

fn decode_cursor(value: &str) -> Result<EventCursor, AppError> {
    if value.is_empty() || value.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(validation("invalid automation event cursor"));
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorEnvelope>(&bytes).ok())
        .ok_or_else(|| validation("invalid automation event cursor"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(envelope.payload)
        .map_err(|_| validation("invalid automation event cursor"))?;
    if envelope.checksum != cursor_checksum(&payload) {
        return Err(validation("invalid automation event cursor"));
    }
    serde_json::from_slice::<EventCursor>(&payload)
        .ok()
        .filter(|cursor| cursor.version == 1)
        .ok_or_else(|| validation("invalid automation event cursor"))
}

fn cursor_checksum(payload: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"mediaflow.automation-event.cursor.v1\0");
    digest.update(payload);
    hex::encode(&digest.finalize()[..16])
}

fn validation(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn not_found(message: &'static str) -> AppError {
    AppError::new(ErrorCode::NotFound, message)
}

fn database_error(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn secret_error(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn invalid_database(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}
