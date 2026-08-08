use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest as _, Sha256};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::platform::outbox::OutboxNotifier;
use crate::platform::secrets::{InstanceKey, IntegrationKind, SecretAad, SecretCipher as _};
use crate::shared::error::{AppError, ErrorCode};

use super::event_store::write_event;

const LEASE_DURATION_US: i64 = 30_000_000;

type SignalRow = (Vec<u8>, Vec<u8>, i64, Vec<u8>);
type MappingRow = (Vec<u8>, String, i64, Vec<u8>);

#[derive(Serialize)]
struct ReconcilePayload {
    kind: &'static str,
    inbox_directory_id: Uuid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Safe result of dispatching one internal completion signal.
pub struct CompletionDispatch {
    pub download_task_id: Uuid,
    pub event_id: Option<Uuid>,
}

#[derive(Clone)]
/// Converts durable download completion signals into fixed reconcile events.
pub struct DownloadCompletionDispatcher {
    pool: SqlitePool,
    key: Arc<InstanceKey>,
    notifier: OutboxNotifier,
}

impl DownloadCompletionDispatcher {
    /// Open the encrypted completion dispatcher.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the instance key cannot be opened safely.
    pub fn open(
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

    /// Recover up to 1000 expired dispatcher leases without external calls.
    ///
    /// # Errors
    ///
    /// Returns a persistence error.
    pub async fn prepare(&self, now_us: i64) -> Result<u64, AppError> {
        let result = sqlx::query(
            "UPDATE download_completion_signals SET
             status=CASE WHEN attempt_count>=10 THEN 'failed' ELSE 'pending' END,
             outcome=CASE WHEN attempt_count>=10 THEN 'dispatch-failed' ELSE NULL END,
             lease_token=NULL,lease_expires_at_us=NULL,updated_at_us=?
             WHERE download_task_id IN (
               SELECT download_task_id FROM download_completion_signals
               WHERE status='running' AND lease_expires_at_us<=?
               ORDER BY lease_expires_at_us,download_task_id LIMIT 1000
             )",
        )
        .bind(now_us)
        .bind(now_us)
        .execute(&self.pool)
        .await
        .map_err(internal)?;
        Ok(result.rows_affected())
    }

    /// Claim and atomically dispatch at most one pending completion signal.
    ///
    /// # Errors
    ///
    /// Returns encryption, state, or persistence errors.
    pub async fn run_once(&self, now_us: i64) -> Result<Option<CompletionDispatch>, AppError> {
        let Some(signal) = self.claim_one(now_us).await? else {
            return Ok(None);
        };
        self.dispatch(signal, now_us).await.map(Some)
    }

    async fn claim_one(&self, now_us: i64) -> Result<Option<SignalRow>, AppError> {
        let token = Uuid::now_v7();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let row = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, i64)>(
            "SELECT download_task_id,downloader_connection_id,completion_projection_version
             FROM download_completion_signals WHERE status='pending' AND attempt_count<10
             ORDER BY created_at_us,download_task_id LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?;
        let Some((task_id, connection_id, completion_version)) = row else {
            tx.commit().await.map_err(internal)?;
            return Ok(None);
        };
        sqlx::query(
            "UPDATE download_completion_signals SET status='running',attempt_count=attempt_count+1,
             lease_token=?,lease_expires_at_us=?,updated_at_us=?
             WHERE download_task_id=? AND status='pending'",
        )
        .bind(token.as_bytes().as_slice())
        .bind(now_us.saturating_add(LEASE_DURATION_US))
        .bind(now_us)
        .bind(&task_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(Some((
            task_id,
            connection_id,
            completion_version,
            token.as_bytes().to_vec(),
        )))
    }

    async fn dispatch(
        &self,
        signal: SignalRow,
        now_us: i64,
    ) -> Result<CompletionDispatch, AppError> {
        let task_id = decode_uuid(&signal.0)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let mappings = sqlx::query_as::<_, MappingRow>(
            "SELECT id,display_name,config_version,inbox_directory_id
             FROM automation_sources WHERE kind='download-completion' AND enabled=1
               AND downloader_connection_id=? ORDER BY id LIMIT 2",
        )
        .bind(&signal.1)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal)?;
        let Some(mapping) = mappings.first() else {
            complete_signal(&mut tx, &signal, "no-mapping", None, now_us).await?;
            tx.commit().await.map_err(internal)?;
            return Ok(CompletionDispatch {
                download_task_id: task_id,
                event_id: None,
            });
        };
        if mappings.len() != 1 {
            return Err(invalid("multiple enabled completion mappings"));
        }
        let source_id = decode_uuid(&mapping.0)?;
        let inbox_id = decode_uuid(&mapping.3)?;
        let event_id = Uuid::now_v7();
        let mut payload = serde_json::to_vec(&ReconcilePayload {
            kind: "inbox.reconcile",
            inbox_directory_id: inbox_id,
        })
        .map_err(invalid_source)?;
        let sealed = self
            .key
            .seal(
                &SecretAad::new(event_id, IntegrationKind::AutomationEventPayload, 1, 1),
                &payload,
            )
            .map_err(secret)?;
        let request_digest: [u8; 32] = Sha256::digest(&payload).into();
        payload.fill(0);
        let dedup_digest = digest(&[
            b"mediaflow.download-completion.event.v1\0",
            task_id.as_bytes(),
            &signal.2.to_be_bytes(),
        ]);
        sqlx::query(
            "INSERT INTO automation_events
             (id,source_id,source_display_name,source_config_version,action,dedup_key_sha256,
              request_sha256,payload_schema_version,payload_nonce,payload_ciphertext,status,
              downstream_kind,downstream_id,result_count,failure_code,attempt_count,retry_at_us,
              lease_token,lease_expires_at_us,projection_version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,'reconcile-inbox',?,?,?,?,?,'pending',NULL,NULL,NULL,NULL,0,
                     NULL,NULL,NULL,1,?,?)",
        )
        .bind(event_id.as_bytes().as_slice())
        .bind(source_id.as_bytes().as_slice())
        .bind(&mapping.1)
        .bind(mapping.2)
        .bind(dedup_digest.as_slice())
        .bind(request_digest.as_slice())
        .bind(i64::from(sealed.schema_version()))
        .bind(sealed.nonce().as_slice())
        .bind(sealed.ciphertext())
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        write_event(&mut tx, event_id, now_us).await?;
        complete_signal(&mut tx, &signal, "event-created", Some(event_id), now_us).await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        Ok(CompletionDispatch {
            download_task_id: task_id,
            event_id: Some(event_id),
        })
    }
}

async fn complete_signal(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    signal: &SignalRow,
    outcome: &str,
    event_id: Option<Uuid>,
    now_us: i64,
) -> Result<(), AppError> {
    let rows = sqlx::query(
        "UPDATE download_completion_signals SET status='completed',outcome=?,automation_event_id=?,
         lease_token=NULL,lease_expires_at_us=NULL,updated_at_us=?
         WHERE download_task_id=? AND status='running' AND lease_token=?",
    )
    .bind(outcome)
    .bind(event_id.map(|id| id.as_bytes().to_vec()))
    .bind(now_us)
    .bind(&signal.0)
    .bind(&signal.3)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    if rows.rows_affected() != 1 {
        return Err(AppError::new(
            ErrorCode::TaskLeaseLost,
            "download completion lease lost",
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
    Uuid::from_slice(value).map_err(|_| invalid("invalid completion UUID"))
}

fn internal(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn secret(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn invalid_source(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}

fn invalid(message: &'static str) -> AppError {
    AppError::new(ErrorCode::Internal, message)
}
