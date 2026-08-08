use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::automation::model::AutomationFailureCode;
use crate::automation::worker::{AutomationActionError, InboxReconcilePort, InboxReconcileResult};
use crate::platform::outbox::OutboxNotifier;
use crate::shared::error::{AppError, ErrorCode};

use super::coordinator::{ReconcileSchedule, ScanReason, schedule_reconcile_in_tx};

type RequestRow = (
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    String,
    Option<i64>,
    Option<i64>,
    i64,
    i64,
);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// Durable registered-inbox reconciliation request status.
pub enum ReconcileRequestStatus {
    Accepted,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// Redacted internal reconciliation request projection.
pub struct ReconcileRequestView {
    pub id: Uuid,
    pub inbox_directory_id: Uuid,
    pub scan_task_id: Uuid,
    pub status: ReconcileRequestStatus,
    pub observed_files: Option<i64>,
    pub errors: Option<i64>,
    pub created_at_us: i64,
    pub updated_at_us: i64,
}

#[derive(Clone)]
/// Idempotent request-key boundary for full reconciliation of registered inboxes.
pub struct ReconcileRequestService {
    pool: SqlitePool,
    notifier: OutboxNotifier,
}

impl ReconcileRequestService {
    #[must_use]
    /// Bind reconciliation requests to the shared task outbox notifier.
    pub fn new(pool: SqlitePool, notifier: OutboxNotifier) -> Self {
        Self { pool, notifier }
    }

    /// Accept or replay one request without accepting a path from the caller.
    ///
    /// # Errors
    ///
    /// Returns validation, conflict, inbox availability, configuration, or persistence errors.
    pub async fn accept(
        &self,
        inbox_id: Uuid,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<ReconcileRequestView, AppError> {
        validate_key(idempotency_key)?;
        let key_digest = digest(&[
            b"mediaflow.discovery-reconcile.key.v1\0",
            idempotency_key.as_bytes(),
        ]);
        let request_digest = digest(&[
            b"mediaflow.discovery-reconcile.request.v1\0",
            inbox_id.as_bytes(),
        ]);
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        if let Some(row) = sqlx::query_as::<_, RequestRow>(
            "SELECT id,request_sha256,inbox_directory_id,scan_task_id,status,observed_files,errors,
                    created_at_us,updated_at_us FROM discovery_reconcile_requests
             WHERE idempotency_key_sha256=?",
        )
        .bind(key_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        {
            if row.1.as_slice() != request_digest || decode_uuid(&row.2)? != inbox_id {
                return Err(AppError::new(
                    ErrorCode::RequestConflict,
                    "reconcile request key is bound to another inbox",
                ));
            }
            let id = decode_uuid(&row.0)?;
            refresh_terminal(&mut tx, id, now_us).await?;
            let view = fetch_view(&mut tx, id).await?;
            tx.commit().await.map_err(internal)?;
            return Ok(view);
        }
        let health = sqlx::query_scalar::<_, String>(
            "SELECT health FROM discovery_inbox_directories WHERE id=?",
        )
        .bind(inbox_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "inbox directory not found"))?;
        if health != "available" {
            return Err(AppError::new(
                ErrorCode::RootUnavailable,
                "inbox directory is unavailable",
            ));
        }
        let scheduled =
            schedule_reconcile_in_tx(&mut tx, inbox_id, ScanReason::WatchRecovery, now_us).await?;
        let (task_id, created) = match scheduled {
            ReconcileSchedule::Created(id) => (id, true),
            ReconcileSchedule::Existing(id) => (id, false),
            ReconcileSchedule::Unavailable => {
                return Err(AppError::new(
                    ErrorCode::ConfigInvalid,
                    "reconcile task account is unavailable",
                ));
            }
        };
        let request_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO discovery_reconcile_requests
             (id,idempotency_key_sha256,request_sha256,inbox_directory_id,scan_task_id,status,
              observed_files,errors,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,'accepted',NULL,NULL,?,?)",
        )
        .bind(request_id.as_bytes().as_slice())
        .bind(key_digest.as_slice())
        .bind(request_digest.as_slice())
        .bind(inbox_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        let view = fetch_view(&mut tx, request_id).await?;
        tx.commit().await.map_err(internal)?;
        if created {
            self.notifier.notify_after_commit();
        }
        Ok(view)
    }
}

#[async_trait]
impl InboxReconcilePort for ReconcileRequestService {
    async fn reconcile_inbox(
        &self,
        inbox_id: Uuid,
        idempotency_key: &str,
    ) -> Result<InboxReconcileResult, AutomationActionError> {
        self.accept(
            inbox_id,
            idempotency_key,
            chrono::Utc::now().timestamp_micros(),
        )
        .await
        .map(|request| InboxReconcileResult {
            request_id: request.id,
            result_count: request.observed_files.unwrap_or(0),
        })
        .map_err(|error| match error.code() {
            ErrorCode::RequestConflict | ErrorCode::ResourceConflict => {
                AutomationActionError::new(AutomationFailureCode::DownstreamConflict, false)
            }
            ErrorCode::ValidationFailed => {
                AutomationActionError::new(AutomationFailureCode::PayloadInvalid, false)
            }
            ErrorCode::NotFound => {
                AutomationActionError::new(AutomationFailureCode::IntegrationNotConfigured, true)
            }
            _ => AutomationActionError::unavailable(),
        })
    }
}

async fn refresh_terminal(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    request_id: Uuid,
    now_us: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE discovery_reconcile_requests SET
         status=CASE WHEN task.status IN ('completed','partial-success')
                     THEN 'completed' ELSE 'failed' END,
         observed_files=task.observed_files,errors=task.errors,updated_at_us=?
         FROM tasks_scan_tasks task
         WHERE discovery_reconcile_requests.id=?
           AND discovery_reconcile_requests.status='accepted'
           AND task.id=discovery_reconcile_requests.scan_task_id
           AND task.status NOT IN ('queued','running')",
    )
    .bind(now_us)
    .bind(request_id.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(internal)
}

async fn fetch_view(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: Uuid,
) -> Result<ReconcileRequestView, AppError> {
    let row = sqlx::query_as::<_, RequestRow>(
        "SELECT id,request_sha256,inbox_directory_id,scan_task_id,status,observed_files,errors,
                created_at_us,updated_at_us FROM discovery_reconcile_requests WHERE id=?",
    )
    .bind(id.as_bytes().as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(ReconcileRequestView {
        id: decode_uuid(&row.0)?,
        inbox_directory_id: decode_uuid(&row.2)?,
        scan_task_id: decode_uuid(&row.3)?,
        status: match row.4.as_str() {
            "accepted" => ReconcileRequestStatus::Accepted,
            "completed" => ReconcileRequestStatus::Completed,
            "failed" => ReconcileRequestStatus::Failed,
            _ => return Err(invalid("invalid reconcile request status")),
        },
        observed_files: row.5,
        errors: row.6,
        created_at_us: row.7,
        updated_at_us: row.8,
    })
}

fn validate_key(value: &str) -> Result<(), AppError> {
    if !(1..=255).contains(&value.len()) || value.chars().any(char::is_control) {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "invalid reconcile request key",
        ));
    }
    Ok(())
}

fn digest(parts: &[&[u8]]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part);
    }
    digest.finalize().into()
}

fn decode_uuid(value: &[u8]) -> Result<Uuid, AppError> {
    Uuid::from_slice(value).map_err(|_| invalid("invalid reconcile request UUID"))
}

fn internal(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn invalid(message: &'static str) -> AppError {
    AppError::new(ErrorCode::Internal, message)
}
