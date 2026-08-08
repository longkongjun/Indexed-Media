use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use uuid::Uuid;

use crate::connectors::model::{IntegrationHealth, SecretString};
use crate::platform::secrets::{
    InstanceKey, SealedSecret, SecretAad, SecretBytes, SecretCipher as _,
};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, PageRequest};

use super::model::{
    AutomationFailureCode, AutomationSource, AutomationSourceInput, AutomationSourceKind,
    SECRET_SCHEMA_VERSION, ValidatedAutomationSourceInput, WebhookAction,
};

type SourceRow = (
    Vec<u8>,
    String,
    String,
    i64,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    i64,
    String,
    Option<i64>,
    Option<String>,
    i64,
    i64,
);

type SecretRow = (String, i64, Option<i64>, Option<Vec<u8>>, Option<Vec<u8>>);

const SOURCE_COLUMNS: &str =
    "id,kind,display_name,enabled,downloader_connection_id,inbox_directory_id,
     endpoint_summary,poll_interval_seconds,allowed_actions_json,secret_fingerprint,
     config_version,health,checked_at_us,failure_code,projection_version,updated_at_us";

#[derive(Deserialize, Serialize)]
struct SourceCursor {
    version: u8,
    updated_at_us: i64,
    id: Uuid,
}

#[derive(Deserialize, Serialize)]
struct CursorEnvelope {
    payload: String,
    checksum: String,
}

#[derive(Clone)]
/// Encrypted, versioned `SQLite` boundary for automation source configuration.
pub struct AutomationSourceStore {
    pool: SqlitePool,
    key: Arc<InstanceKey>,
}

/// Idempotent webhook rotation result with a short-lived decrypted response secret.
pub struct WebhookRotationResult {
    /// Updated redacted source record.
    pub source: AutomationSource,
    /// Secret returned only to the current service response.
    pub secret: SecretBytes,
}

impl std::fmt::Debug for WebhookRotationResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebhookRotationResult")
            .field("source", &self.source)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

impl AutomationSourceStore {
    /// Open the store and pin the instance encryption key.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the instance key boundary is unsafe.
    pub fn open(pool: SqlitePool, config_dir: &Path) -> Result<Self, AppError> {
        let key = InstanceKey::load_or_create(config_dir)
            .map(Arc::new)
            .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
        Ok(Self { pool, key })
    }

    /// Read one redacted source record.
    ///
    /// # Errors
    ///
    /// Returns an internal error for unavailable or invalid persisted data.
    pub async fn get(&self, id: Uuid) -> Result<Option<AutomationSource>, AppError> {
        let query = format!("SELECT {SOURCE_COLUMNS} FROM automation_sources WHERE id=?");
        sqlx::query_as::<_, SourceRow>(&query)
            .bind(id.as_bytes().as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(database_error)?
            .map(decode_source)
            .transpose()
    }

    /// List redacted sources in a stable cursor page.
    ///
    /// # Errors
    ///
    /// Returns a validation or internal error for an invalid page or database row.
    pub async fn list_page(
        &self,
        page: &PageRequest,
    ) -> Result<CursorPage<AutomationSource>, AppError> {
        if page.limit == 0 || page.limit > crate::shared::page::MAX_PAGE_LIMIT {
            return Err(validation("automation source page limit is outside bounds"));
        }
        let cursor = page.cursor.as_deref().map(decode_cursor).transpose()?;
        let mut query = QueryBuilder::<Sqlite>::new("SELECT ");
        query.push(SOURCE_COLUMNS).push(" FROM automation_sources");
        if let Some(cursor) = &cursor {
            query
                .push(" WHERE (updated_at_us<")
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
            .build_query_as::<SourceRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(database_error)?;
        let has_more = rows.len() > page.limit as usize;
        let items = rows
            .into_iter()
            .take(page.limit as usize)
            .map(decode_source)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if has_more {
            items
                .last()
                .map(|item| {
                    encode_cursor(&SourceCursor {
                        version: 1,
                        updated_at_us: item.updated_at_us,
                        id: item.id,
                    })
                })
                .transpose()?
        } else {
            None
        };
        Ok(CursorPage { items, next_cursor })
    }

    /// Validate references, encrypt the source secret when present, and insert version one.
    ///
    /// # Errors
    ///
    /// Returns stable validation, not-found, encryption, or database errors.
    pub async fn insert(
        &self,
        id: Uuid,
        input: AutomationSourceInput,
        generated_secret: Option<SecretString>,
        now_us: i64,
    ) -> Result<AutomationSource, AppError> {
        let input = input.validate(generated_secret)?;
        self.validate_references(&input).await?;
        self.ensure_completion_mapping_available(&input, None)
            .await?;
        let sealed = self.seal(id, 1, &input)?;
        let fingerprint = secret_fingerprint(&input);
        let allowed_actions = encode_actions(&input.allowed_actions)?;
        let (health, failure_code) = initial_health(&input);
        let (schema, nonce, ciphertext) = sealed_parts(sealed);
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        sqlx::query(
            "INSERT INTO automation_sources
             (id,kind,display_name,enabled,downloader_connection_id,inbox_directory_id,
              endpoint_summary,poll_interval_seconds,allowed_actions_json,secret_fingerprint,
              secret_schema_version,secret_nonce,secret_ciphertext,config_version,health,
              checked_at_us,failure_code,projection_version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,1,?,NULL,?,1,?,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(input.kind.as_str())
        .bind(input.display_name)
        .bind(i64::from(input.enabled))
        .bind(
            input
                .downloader_connection_id
                .map(|value| value.as_bytes().to_vec()),
        )
        .bind(
            input
                .inbox_directory_id
                .map(|value| value.as_bytes().to_vec()),
        )
        .bind(input.endpoint_summary)
        .bind(input.poll_interval_seconds.map(i64::from))
        .bind(allowed_actions)
        .bind(fingerprint)
        .bind(schema)
        .bind(nonce)
        .bind(ciphertext)
        .bind(health_value(health))
        .bind(failure_code.map(AutomationFailureCode::as_str))
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        let next_poll =
            (input.kind == AutomationSourceKind::Rss && input.enabled).then_some(now_us);
        sqlx::query(
            "INSERT INTO automation_source_runtime
             (source_id,next_poll_at_us,cursor_schema_version,cursor_nonce,cursor_ciphertext,
              lease_token,lease_expires_at_us,attempt_count,updated_at_us)
             VALUES (?,?,NULL,NULL,NULL,NULL,NULL,0,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(next_poll)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        self.get(id)
            .await?
            .ok_or_else(|| invalid_database("inserted automation source is missing"))
    }

    /// Replace a source under optimistic concurrency, retaining a webhook secret.
    ///
    /// # Errors
    ///
    /// Returns validation, not-found, conflict, encryption, or database errors.
    pub async fn replace(
        &self,
        id: Uuid,
        expected_version: i64,
        input: AutomationSourceInput,
        now_us: i64,
    ) -> Result<AutomationSource, AppError> {
        if expected_version < 1 {
            return Err(version_conflict());
        }
        let current = self
            .get(id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "automation source not found"))?;
        let input_kind = input.kind();
        if current.kind != input_kind {
            return Err(validation("automation source kind cannot be changed"));
        }
        let retained = if current.kind == AutomationSourceKind::Webhook {
            let secret = self.load_secret(id).await?;
            Some(SecretString::new(
                String::from_utf8(secret.expose().to_vec())
                    .map_err(|_| invalid_database("invalid webhook secret"))?,
            ))
        } else {
            None
        };
        let input = input.validate(retained)?;
        self.validate_references(&input).await?;
        self.ensure_completion_mapping_available(&input, Some(id))
            .await?;
        let next_version = expected_version
            .checked_add(1)
            .ok_or_else(version_conflict)?;
        let sealed = self.seal(id, next_version, &input)?;
        let fingerprint = secret_fingerprint(&input);
        let allowed_actions = encode_actions(&input.allowed_actions)?;
        let (health, failure_code) = initial_health(&input);
        let (schema, nonce, ciphertext) = sealed_parts(sealed);
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let rows = sqlx::query(
            "UPDATE automation_sources SET display_name=?,enabled=?,downloader_connection_id=?,
             inbox_directory_id=?,endpoint_summary=?,poll_interval_seconds=?,allowed_actions_json=?,
             secret_fingerprint=?,secret_schema_version=?,secret_nonce=?,secret_ciphertext=?,
             config_version=?,health=?,checked_at_us=NULL,failure_code=?,
             projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND config_version=? AND kind=?",
        )
        .bind(input.display_name)
        .bind(i64::from(input.enabled))
        .bind(
            input
                .downloader_connection_id
                .map(|value| value.as_bytes().to_vec()),
        )
        .bind(
            input
                .inbox_directory_id
                .map(|value| value.as_bytes().to_vec()),
        )
        .bind(input.endpoint_summary)
        .bind(input.poll_interval_seconds.map(i64::from))
        .bind(allowed_actions)
        .bind(fingerprint)
        .bind(schema)
        .bind(nonce)
        .bind(ciphertext)
        .bind(next_version)
        .bind(health_value(health))
        .bind(failure_code.map(AutomationFailureCode::as_str))
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(expected_version)
        .bind(input.kind.as_str())
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(version_conflict());
        }
        let next_poll =
            (input.kind == AutomationSourceKind::Rss && input.enabled).then_some(now_us);
        sqlx::query(
            "UPDATE automation_source_runtime SET next_poll_at_us=?,cursor_schema_version=NULL,
             cursor_nonce=NULL,cursor_ciphertext=NULL,lease_token=NULL,lease_expires_at_us=NULL,
             attempt_count=0,updated_at_us=? WHERE source_id=?",
        )
        .bind(next_poll)
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        self.get(id)
            .await?
            .ok_or_else(|| invalid_database("replaced automation source is missing"))
    }

    /// Delete a disabled source under optimistic concurrency.
    ///
    /// # Errors
    ///
    /// Returns not-found, in-use, version-conflict, or database errors.
    pub async fn delete(&self, id: Uuid, expected_version: i64) -> Result<(), AppError> {
        if expected_version < 1 {
            return Err(version_conflict());
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let state = sqlx::query_as::<_, (i64, i64)>(
            "SELECT enabled,config_version FROM automation_sources WHERE id=?",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "automation source not found"))?;
        if state.1 != expected_version {
            return Err(version_conflict());
        }
        if state.0 != 0 {
            return Err(AppError::new(
                ErrorCode::ResourceConflict,
                "automation source must be disabled before deletion",
            ));
        }
        let active_events = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM automation_events
             WHERE source_id=? AND status IN ('pending','running','retry-wait')",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        if active_events != 0 {
            return Err(AppError::new(
                ErrorCode::ResourceConflict,
                "automation source has active events",
            ));
        }
        let rows = sqlx::query("DELETE FROM automation_sources WHERE id=? AND config_version=?")
            .bind(id.as_bytes().as_slice())
            .bind(expected_version)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(version_conflict());
        }
        tx.commit().await.map_err(database_error)
    }

    /// Rotate a webhook key under optimistic concurrency and an idempotency key.
    ///
    /// Exact retries return the same encrypted receipt; a key rebound to another expected version
    /// returns a request conflict.
    ///
    /// # Errors
    ///
    /// Returns validation, not-found, version, idempotency, encryption, or persistence errors.
    #[allow(clippy::too_many_lines)]
    pub async fn rotate_webhook_secret(
        &self,
        id: Uuid,
        expected_version: i64,
        idempotency_key: &str,
        generated_secret: SecretString,
        now_us: i64,
    ) -> Result<WebhookRotationResult, AppError> {
        if expected_version < 1
            || !(1..=255).contains(&idempotency_key.len())
            || idempotency_key.chars().any(char::is_control)
            || !(32..=128).contains(&generated_secret.len())
        {
            return Err(validation("invalid webhook secret rotation"));
        }
        let key_digest: [u8; 32] = Sha256::digest(idempotency_key.as_bytes()).into();
        let request_digest = rotation_request_digest(id, expected_version);
        let secret = generated_secret.into_secret_bytes();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let receipt = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, i64, i64, Vec<u8>, Vec<u8>)>(
            "SELECT id,request_sha256,source_config_version,secret_schema_version,
                    secret_nonce,secret_ciphertext
             FROM automation_webhook_rotation_receipts
             WHERE source_id=? AND idempotency_key_sha256=?",
        )
        .bind(id.as_bytes().as_slice())
        .bind(key_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        if let Some(receipt) = receipt {
            if receipt.1.as_slice() != request_digest {
                return Err(AppError::new(
                    ErrorCode::RequestConflict,
                    "webhook rotation key is bound to another request",
                ));
            }
            let receipt_id = Uuid::from_slice(&receipt.0).map_err(invalid_database)?;
            let schema = u16::try_from(receipt.3)
                .map_err(|_| invalid_database("invalid rotation schema"))?;
            let nonce: [u8; 24] = receipt
                .4
                .try_into()
                .map_err(|_| invalid_database("invalid rotation nonce"))?;
            let sealed = SealedSecret::from_parts(schema, nonce, receipt.5)
                .map_err(|error| invalid_database(error.to_string()))?;
            let replay_secret = self
                .key
                .open(
                    &SecretAad::new(
                        receipt_id,
                        crate::platform::secrets::IntegrationKind::AutomationWebhookRotationReceipt,
                        receipt.2,
                        schema,
                    ),
                    &sealed,
                )
                .map_err(secret_error)?;
            tx.commit().await.map_err(database_error)?;
            let source = self
                .get(id)
                .await?
                .ok_or_else(|| invalid_database("rotated webhook source is missing"))?;
            return Ok(WebhookRotationResult {
                source,
                secret: replay_secret,
            });
        }
        let current = sqlx::query_as::<_, (i64, i64)>(
            "SELECT enabled,config_version FROM automation_sources
             WHERE id=? AND kind='webhook'",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "webhook source not found"))?;
        if current.1 != expected_version {
            return Err(version_conflict());
        }
        let next_version = expected_version
            .checked_add(1)
            .ok_or_else(version_conflict)?;
        let source_sealed = self
            .key
            .seal(
                &SecretAad::new(
                    id,
                    crate::platform::secrets::IntegrationKind::AutomationWebhook,
                    next_version,
                    SECRET_SCHEMA_VERSION,
                ),
                secret.expose(),
            )
            .map_err(secret_error)?;
        let digest = Sha256::digest(secret.expose());
        let fingerprint = hex::encode(&digest[digest.len() - 4..]);
        let (health, failure_code) = if current.0 == 1 {
            (IntegrationHealth::Healthy, None)
        } else {
            (
                IntegrationHealth::Degraded,
                Some(AutomationFailureCode::SourceDisabled),
            )
        };
        let rows = sqlx::query(
            "UPDATE automation_sources SET secret_fingerprint=?,secret_schema_version=?,
             secret_nonce=?,secret_ciphertext=?,config_version=?,health=?,checked_at_us=NULL,
             failure_code=?,projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND kind='webhook' AND config_version=?",
        )
        .bind(fingerprint)
        .bind(i64::from(source_sealed.schema_version()))
        .bind(source_sealed.nonce().as_slice())
        .bind(source_sealed.ciphertext())
        .bind(next_version)
        .bind(health_value(health))
        .bind(failure_code.map(AutomationFailureCode::as_str))
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(version_conflict());
        }
        let receipt_id = Uuid::now_v7();
        let receipt_sealed = self
            .key
            .seal(
                &SecretAad::new(
                    receipt_id,
                    crate::platform::secrets::IntegrationKind::AutomationWebhookRotationReceipt,
                    next_version,
                    SECRET_SCHEMA_VERSION,
                ),
                secret.expose(),
            )
            .map_err(secret_error)?;
        sqlx::query(
            "INSERT INTO automation_webhook_rotation_receipts
             (id,source_id,idempotency_key_sha256,request_sha256,source_config_version,
              secret_schema_version,secret_nonce,secret_ciphertext,created_at_us)
             VALUES (?,?,?,?,?,?,?,?,?)",
        )
        .bind(receipt_id.as_bytes().as_slice())
        .bind(id.as_bytes().as_slice())
        .bind(key_digest.as_slice())
        .bind(request_digest.as_slice())
        .bind(next_version)
        .bind(i64::from(receipt_sealed.schema_version()))
        .bind(receipt_sealed.nonce().as_slice())
        .bind(receipt_sealed.ciphertext())
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        let source = self
            .get(id)
            .await?
            .ok_or_else(|| invalid_database("rotated webhook source is missing"))?;
        Ok(WebhookRotationResult { source, secret })
    }

    /// Decrypt a short-lived RSS URL or webhook key.
    ///
    /// # Errors
    ///
    /// Returns not-configured, not-found, authentication, or invalid-row errors.
    pub async fn load_secret(&self, id: Uuid) -> Result<SecretBytes, AppError> {
        let row = sqlx::query_as::<_, SecretRow>(
            "SELECT kind,config_version,secret_schema_version,secret_nonce,secret_ciphertext
             FROM automation_sources WHERE id=?",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "automation source not found"))?;
        let kind = AutomationSourceKind::parse(&row.0)
            .ok_or_else(|| invalid_database("invalid automation source kind"))?;
        let integration_kind = kind.integration_kind().ok_or_else(|| {
            AppError::new(ErrorCode::IntegrationNotConfigured, "source has no secret")
        })?;
        let schema = u16::try_from(
            row.2
                .ok_or_else(|| invalid_database("missing automation source secret schema"))?,
        )
        .map_err(|_| invalid_database("invalid automation source secret schema"))?;
        let nonce: [u8; 24] = row
            .3
            .ok_or_else(|| invalid_database("missing automation source nonce"))?
            .try_into()
            .map_err(|_| invalid_database("invalid automation source nonce"))?;
        let sealed = SealedSecret::from_parts(
            schema,
            nonce,
            row.4
                .ok_or_else(|| invalid_database("missing automation source ciphertext"))?,
        )
        .map_err(|error| invalid_database(error.to_string()))?;
        self.key
            .open(
                &SecretAad::new(id, integration_kind, row.1, schema),
                &sealed,
            )
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
    }

    pub(crate) async fn validate_references(
        &self,
        input: &ValidatedAutomationSourceInput,
    ) -> Result<(), AppError> {
        if let Some(id) = input.downloader_connection_id {
            let exists = sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM downloader_connections WHERE id=?)",
            )
            .bind(id.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .map_err(database_error)?;
            if exists != 1 {
                return Err(AppError::new(
                    ErrorCode::NotFound,
                    "downloader connection not found",
                ));
            }
        }
        if let Some(id) = input.inbox_directory_id {
            let exists = sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM discovery_inbox_directories WHERE id=?)",
            )
            .bind(id.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .map_err(database_error)?;
            if exists != 1 {
                return Err(AppError::new(
                    ErrorCode::NotFound,
                    "inbox directory not found",
                ));
            }
        }
        Ok(())
    }

    async fn ensure_completion_mapping_available(
        &self,
        input: &ValidatedAutomationSourceInput,
        excluding_id: Option<Uuid>,
    ) -> Result<(), AppError> {
        if input.kind != AutomationSourceKind::DownloadCompletion || !input.enabled {
            return Ok(());
        }
        let connection_id = input
            .downloader_connection_id
            .ok_or_else(|| validation("completion mapping is missing its connection"))?;
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM automation_sources
             WHERE kind='download-completion' AND enabled=1 AND downloader_connection_id=?
               AND (? IS NULL OR id!=?))",
        )
        .bind(connection_id.as_bytes().as_slice())
        .bind(excluding_id.map(|id| id.as_bytes().to_vec()))
        .bind(excluding_id.map(|id| id.as_bytes().to_vec()))
        .fetch_one(&self.pool)
        .await
        .map_err(database_error)?;
        if exists == 1 {
            return Err(AppError::new(
                ErrorCode::ResourceConflict,
                "downloader connection already has an enabled completion mapping",
            ));
        }
        Ok(())
    }

    fn seal(
        &self,
        id: Uuid,
        version: i64,
        input: &ValidatedAutomationSourceInput,
    ) -> Result<Option<SealedSecret>, AppError> {
        let Some(secret) = &input.secret else {
            return Ok(None);
        };
        let kind = input
            .kind
            .integration_kind()
            .ok_or_else(|| validation("automation source cannot contain a secret"))?;
        self.key
            .seal(
                &SecretAad::new(id, kind, version, SECRET_SCHEMA_VERSION),
                secret.expose(),
            )
            .map(Some)
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
    }
}

fn sealed_parts(sealed: Option<SealedSecret>) -> (Option<i64>, Option<Vec<u8>>, Option<Vec<u8>>) {
    sealed.map_or((None, None, None), |sealed| {
        (
            Some(i64::from(sealed.schema_version())),
            Some(sealed.nonce().to_vec()),
            Some(sealed.ciphertext().to_vec()),
        )
    })
}

fn initial_health(
    input: &ValidatedAutomationSourceInput,
) -> (IntegrationHealth, Option<AutomationFailureCode>) {
    if !input.enabled {
        return (
            IntegrationHealth::Degraded,
            Some(AutomationFailureCode::SourceDisabled),
        );
    }
    if input.kind == AutomationSourceKind::Rss {
        return (
            IntegrationHealth::Degraded,
            Some(AutomationFailureCode::IntegrationUnavailable),
        );
    }
    (IntegrationHealth::Healthy, None)
}

fn secret_fingerprint(input: &ValidatedAutomationSourceInput) -> Option<String> {
    if input.kind != AutomationSourceKind::Webhook {
        return None;
    }
    input.secret.as_ref().map(|secret| {
        let digest = Sha256::digest(secret.expose());
        hex::encode(&digest[digest.len() - 4..])
    })
}

fn encode_actions(actions: &[WebhookAction]) -> Result<Option<String>, AppError> {
    if actions.is_empty() {
        Ok(None)
    } else {
        serde_json::to_string(actions)
            .map(Some)
            .map_err(invalid_database)
    }
}

fn decode_source(row: SourceRow) -> Result<AutomationSource, AppError> {
    Ok(AutomationSource {
        id: decode_uuid(&row.0)?,
        kind: AutomationSourceKind::parse(&row.1)
            .ok_or_else(|| invalid_database("invalid automation source kind"))?,
        display_name: row.2,
        enabled: decode_bool(row.3)?,
        downloader_connection_id: row.4.as_deref().map(decode_uuid).transpose()?,
        inbox_directory_id: row.5.as_deref().map(decode_uuid).transpose()?,
        endpoint_summary: row.6,
        poll_interval_seconds: row
            .7
            .map(|value| u32::try_from(value).map_err(invalid_database))
            .transpose()?,
        allowed_actions: row
            .8
            .map(|value| serde_json::from_str(&value).map_err(invalid_database))
            .transpose()?
            .unwrap_or_default(),
        secret_fingerprint: row.9,
        config_version: row.10,
        health: parse_health(&row.11).ok_or_else(|| invalid_database("invalid health"))?,
        checked_at_us: row.12,
        failure_code: row
            .13
            .map(|value| {
                AutomationFailureCode::parse(&value)
                    .ok_or_else(|| invalid_database("invalid automation failure code"))
            })
            .transpose()?,
        projection_version: row.14,
        updated_at_us: row.15,
    })
}

fn decode_uuid(value: &[u8]) -> Result<Uuid, AppError> {
    Uuid::from_slice(value).map_err(invalid_database)
}

fn decode_bool(value: i64) -> Result<bool, AppError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid_database("invalid SQLite boolean")),
    }
}

fn parse_health(value: &str) -> Option<IntegrationHealth> {
    match value {
        "healthy" => Some(IntegrationHealth::Healthy),
        "degraded" => Some(IntegrationHealth::Degraded),
        "unavailable" => Some(IntegrationHealth::Unavailable),
        "unauthorized" => Some(IntegrationHealth::Unauthorized),
        "rate-limited" => Some(IntegrationHealth::RateLimited),
        _ => None,
    }
}

const fn health_value(value: IntegrationHealth) -> &'static str {
    match value {
        IntegrationHealth::Unconfigured => "unconfigured",
        IntegrationHealth::Healthy => "healthy",
        IntegrationHealth::Degraded => "degraded",
        IntegrationHealth::Unavailable => "unavailable",
        IntegrationHealth::Unauthorized => "unauthorized",
        IntegrationHealth::RateLimited => "rate-limited",
    }
}

fn encode_cursor(cursor: &SourceCursor) -> Result<String, AppError> {
    let payload = serde_json::to_vec(cursor).map_err(invalid_database)?;
    let envelope = CursorEnvelope {
        payload: URL_SAFE_NO_PAD.encode(&payload),
        checksum: cursor_checksum(&payload),
    };
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).map_err(invalid_database)?);
    if encoded.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(invalid_database("automation source cursor exceeds bounds"));
    }
    Ok(encoded)
}

fn decode_cursor(value: &str) -> Result<SourceCursor, AppError> {
    if value.is_empty() || value.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(validation("invalid automation source cursor"));
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorEnvelope>(&bytes).ok())
        .ok_or_else(|| validation("invalid automation source cursor"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(envelope.payload)
        .map_err(|_| validation("invalid automation source cursor"))?;
    if envelope.checksum != cursor_checksum(&payload) {
        return Err(validation("invalid automation source cursor"));
    }
    serde_json::from_slice::<SourceCursor>(&payload)
        .ok()
        .filter(|cursor| cursor.version == 1)
        .ok_or_else(|| validation("invalid automation source cursor"))
}

fn cursor_checksum(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.automation-source.cursor.v1\0");
    hasher.update(payload);
    hex::encode(&hasher.finalize()[..16])
}

fn rotation_request_digest(id: Uuid, expected_version: i64) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"mediaflow.webhook-rotation.v1\0");
    digest.update(id.as_bytes());
    digest.update(expected_version.to_be_bytes());
    digest.finalize().into()
}

fn validation(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn version_conflict() -> AppError {
    AppError::new(
        ErrorCode::ConfigVersionConflict,
        "automation source version changed",
    )
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
