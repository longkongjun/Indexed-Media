use std::path::Path;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::SqlitePool;
use subtle::ConstantTimeEq as _;
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::connectors::downloader::model::CreateDownloadTaskCommand;
use crate::connectors::model::SecretString;
use crate::platform::db::Db;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::secrets::{InstanceKey, IntegrationKind, SecretAad, SecretCipher as _};
use crate::shared::error::{AppError, ErrorCode};

use super::event_store::write_event;
use super::model::{AutomationSource, AutomationSourceKind, WebhookAction};
use super::source_store::AutomationSourceStore;

const MAX_WEBHOOK_BODY_BYTES: usize = 64 * 1024;
const SIGNATURE_WINDOW_SECONDS: u64 = 300;
const PAYLOAD_SCHEMA_VERSION: u16 = 1;

#[derive(Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum SourceWebhookEventRequest {
    #[serde(rename = "download.create")]
    DownloadCreate {
        downloader_connection_id: Uuid,
        source: SecretString,
        display_name: String,
    },
    #[serde(rename = "inbox.reconcile")]
    InboxReconcile { inbox_directory_id: Uuid },
}

impl std::fmt::Debug for SourceWebhookEventRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DownloadCreate {
                downloader_connection_id,
                display_name,
                ..
            } => formatter
                .debug_struct("DownloadCreate")
                .field("downloader_connection_id", downloader_connection_id)
                .field("source", &"[REDACTED]")
                .field("display_name", display_name)
                .finish(),
            Self::InboxReconcile { inbox_directory_id } => formatter
                .debug_struct("InboxReconcile")
                .field("inbox_directory_id", inbox_directory_id)
                .finish(),
        }
    }
}

#[derive(Clone, Copy)]
enum ValidatedAction {
    CreateDownload { downloader_connection_id: Uuid },
    ReconcileInbox { inbox_directory_id: Uuid },
}

impl ValidatedAction {
    const fn action(self) -> WebhookAction {
        match self {
            Self::CreateDownload { .. } => WebhookAction::DownloadCreate,
            Self::ReconcileInbox { .. } => WebhookAction::InboxReconcile,
        }
    }

    const fn event_action(self) -> &'static str {
        match self {
            Self::CreateDownload { .. } => "create-download",
            Self::ReconcileInbox { .. } => "reconcile-inbox",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum AcceptedStatus {
    Pending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
/// Minimal durable webhook acceptance receipt.
pub struct AutomationEventAccepted {
    /// Stable durable event identifier.
    pub id: Uuid,
    /// Initial event status.
    status: AcceptedStatus,
}

#[derive(Clone)]
struct WebhookEventStore {
    pool: SqlitePool,
    key: Arc<InstanceKey>,
    notifier: OutboxNotifier,
}

impl WebhookEventStore {
    fn open(
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

    async fn is_replay(&self, source_id: Uuid, nonce_digest: &[u8; 32]) -> Result<bool, AppError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM automation_webhook_nonces
             WHERE source_id=? AND nonce_sha256=?)",
        )
        .bind(source_id.as_bytes().as_slice())
        .bind(nonce_digest.as_slice())
        .fetch_one(&self.pool)
        .await
        .map(|value| value == 1)
        .map_err(database_error)
    }

    #[allow(clippy::too_many_arguments)]
    async fn accept(
        &self,
        source: &AutomationSource,
        action: ValidatedAction,
        nonce_digest: [u8; 32],
        body: &[u8],
        signed_at_us: i64,
        now_us: i64,
    ) -> Result<AutomationEventAccepted, AppError> {
        let event_id = Uuid::now_v7();
        let request_digest: [u8; 32] = Sha256::digest(body).into();
        let sealed = self
            .key
            .seal(
                &SecretAad::new(
                    event_id,
                    IntegrationKind::AutomationEventPayload,
                    1,
                    PAYLOAD_SCHEMA_VERSION,
                ),
                body,
            )
            .map_err(secret_error)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let current = sqlx::query_as::<_, (i64, i64, String)>(
            "SELECT enabled,config_version,allowed_actions_json
             FROM automation_sources WHERE id=? AND kind='webhook'",
        )
        .bind(source.id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or_else(signature_error)?;
        if current.1 != source.config_version {
            return Err(signature_error());
        }
        if current.0 == 0 {
            return Err(AppError::new(
                ErrorCode::AutomationSourceDisabled,
                "webhook source is disabled",
            ));
        }
        let allowed: Vec<WebhookAction> =
            serde_json::from_str(&current.2).map_err(invalid_database)?;
        if !allowed.contains(&action.action()) {
            return Err(AppError::new(
                ErrorCode::AutomationActionInvalid,
                "webhook action is not allowed",
            ));
        }
        validate_reference(&mut tx, action).await?;
        let nonce = sqlx::query(
            "INSERT INTO automation_webhook_nonces
             (source_id,nonce_sha256,body_sha256,signed_at_us,expires_at_us,response_event_id,created_at_us)
             VALUES (?,?,?,?,?,?,?) ON CONFLICT(source_id,nonce_sha256) DO NOTHING",
        )
        .bind(source.id.as_bytes().as_slice())
        .bind(nonce_digest.as_slice())
        .bind(request_digest.as_slice())
        .bind(signed_at_us)
        .bind(signed_at_us.saturating_add(300_000_000))
        .bind(event_id.as_bytes().as_slice())
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if nonce.rows_affected() != 1 {
            return Err(replay_error());
        }
        sqlx::query(
            "INSERT INTO automation_events
             (id,source_id,source_display_name,source_config_version,action,dedup_key_sha256,
              request_sha256,payload_schema_version,payload_nonce,payload_ciphertext,status,
              downstream_kind,downstream_id,result_count,failure_code,attempt_count,retry_at_us,
              lease_token,lease_expires_at_us,projection_version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,?,?,?,?,'pending',NULL,NULL,NULL,NULL,0,NULL,NULL,NULL,1,?,?)",
        )
        .bind(event_id.as_bytes().as_slice())
        .bind(source.id.as_bytes().as_slice())
        .bind(&source.display_name)
        .bind(source.config_version)
        .bind(action.event_action())
        .bind(nonce_digest.as_slice())
        .bind(request_digest.as_slice())
        .bind(i64::from(sealed.schema_version()))
        .bind(sealed.nonce().as_slice())
        .bind(sealed.ciphertext())
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        write_event(&mut tx, event_id, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        Ok(AutomationEventAccepted {
            id: event_id,
            status: AcceptedStatus::Pending,
        })
    }
}

#[derive(Clone)]
struct WebhookService {
    sources: AutomationSourceStore,
    events: WebhookEventStore,
}

impl WebhookService {
    fn open(
        pool: SqlitePool,
        config_dir: &Path,
        notifier: OutboxNotifier,
    ) -> Result<Self, AppError> {
        Ok(Self {
            sources: AutomationSourceStore::open(pool.clone(), config_dir)?,
            events: WebhookEventStore::open(pool, config_dir, notifier)?,
        })
    }

    async fn accept(
        &self,
        source_id: Uuid,
        headers: &HeaderMap,
        body: &[u8],
        now_us: i64,
    ) -> Result<AutomationEventAccepted, AppError> {
        let timestamp = required_header(headers, "x-mediaflow-timestamp")?;
        let nonce = required_header(headers, "x-mediaflow-nonce")?;
        let supplied_signature = signature_header(headers)?;
        let signed_seconds = validate_timestamp(timestamp, now_us)?;
        validate_nonce(nonce)?;
        let source = self
            .sources
            .get(source_id)
            .await?
            .filter(|source| source.kind == AutomationSourceKind::Webhook)
            .ok_or_else(signature_error)?;
        let secret = self
            .sources
            .load_secret(source_id)
            .await
            .map_err(|_| signature_error())?;
        let expected = hmac_sha256(
            secret.expose(),
            &[timestamp.as_bytes(), b"\n", nonce.as_bytes(), b"\n", body],
        );
        if !bool::from(expected.ct_eq(&supplied_signature)) {
            return Err(signature_error());
        }
        if !source.enabled {
            return Err(AppError::new(
                ErrorCode::AutomationSourceDisabled,
                "webhook source is disabled",
            ));
        }
        let nonce_digest: [u8; 32] = Sha256::digest(nonce.as_bytes()).into();
        if self.events.is_replay(source_id, &nonce_digest).await? {
            return Err(replay_error());
        }
        let action = validate_payload(body)?;
        if !source.allowed_actions.contains(&action.action()) {
            return Err(AppError::new(
                ErrorCode::AutomationActionInvalid,
                "webhook action is not allowed",
            ));
        }
        self.events
            .accept(
                &source,
                action,
                nonce_digest,
                body,
                signed_seconds.saturating_mul(1_000_000),
                now_us,
            )
            .await
    }
}

#[derive(Clone)]
struct WebhookHttpState {
    config: AppConfig,
    db: Db,
    service: Option<WebhookService>,
}

impl WebhookHttpState {
    async fn ensure_ready(&self) -> Result<(), AppError> {
        if !self.config.readiness_issues().is_empty() {
            return Err(AppError::new(
                ErrorCode::NotReady,
                "webhook route is not ready",
            ));
        }
        match tokio::time::timeout(
            std::time::Duration::from_millis(250),
            sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(self.db.pool()),
        )
        .await
        {
            Ok(Ok(1)) => Ok(()),
            _ => Err(AppError::new(
                ErrorCode::NotReady,
                "webhook route is not ready",
            )),
        }
    }
}

/// Build the session-independent signed webhook ingress route.
pub fn router(config: AppConfig, db: &Db) -> Router {
    router_with_notifier(config, db, OutboxNotifier::new())
}

/// Build the signed webhook ingress route with the shared outbox notifier.
pub fn router_with_notifier(config: AppConfig, db: &Db, notifier: OutboxNotifier) -> Router {
    let service = WebhookService::open(db.pool().clone(), &config.config_dir, notifier).ok();
    Router::new()
        .route(
            "/api/v1/source-webhooks/{automationSourceId}/events",
            post(accept_webhook),
        )
        .with_state(Arc::new(WebhookHttpState {
            config,
            db: db.clone(),
            service,
        }))
}

async fn accept_webhook(
    State(state): State<Arc<WebhookHttpState>>,
    AxumPath(source_id): AxumPath<Uuid>,
    headers: HeaderMap,
    body: Body,
) -> Result<impl IntoResponse, AppError> {
    state.ensure_ready().await?;
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| value != "application/json")
    {
        return Err(payload_error());
    }
    let mut body = to_bytes(body, MAX_WEBHOOK_BODY_BYTES)
        .await
        .map_err(|_| AppError::new(ErrorCode::PayloadTooLarge, "webhook body too large"))?
        .to_vec();
    let result = match &state.service {
        Some(service) => {
            service
                .accept(
                    source_id,
                    &headers,
                    &body,
                    chrono::Utc::now().timestamp_micros(),
                )
                .await
        }
        None => Err(AppError::new(
            ErrorCode::NotReady,
            "webhook route is not ready",
        )),
    };
    body.fill(0);
    Ok((StatusCode::ACCEPTED, Json(result?)))
}

fn validate_payload(body: &[u8]) -> Result<ValidatedAction, AppError> {
    match serde_json::from_slice::<SourceWebhookEventRequest>(body).map_err(|_| payload_error())? {
        SourceWebhookEventRequest::DownloadCreate {
            downloader_connection_id,
            source,
            display_name,
        } => {
            CreateDownloadTaskCommand {
                connection_id: downloader_connection_id,
                source,
                display_name,
            }
            .validate()
            .map_err(|_| payload_error())?;
            Ok(ValidatedAction::CreateDownload {
                downloader_connection_id,
            })
        }
        SourceWebhookEventRequest::InboxReconcile { inbox_directory_id } => {
            Ok(ValidatedAction::ReconcileInbox { inbox_directory_id })
        }
    }
}

async fn validate_reference(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    action: ValidatedAction,
) -> Result<(), AppError> {
    let (table, id) = match action {
        ValidatedAction::CreateDownload {
            downloader_connection_id,
        } => ("downloader_connections", downloader_connection_id),
        ValidatedAction::ReconcileInbox { inbox_directory_id } => {
            ("discovery_inbox_directories", inbox_directory_id)
        }
    };
    let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?)");
    let exists = sqlx::query_scalar::<_, i64>(&query)
        .bind(id.as_bytes().as_slice())
        .fetch_one(&mut **tx)
        .await
        .map_err(database_error)?;
    if exists != 1 {
        return Err(payload_error());
    }
    Ok(())
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, AppError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(signature_error)
}

fn signature_header(headers: &HeaderMap) -> Result<[u8; 32], AppError> {
    let value = required_header(headers, "x-mediaflow-signature")?;
    let hex = value.strip_prefix("sha256=").ok_or_else(signature_error)?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(signature_error());
    }
    let bytes = hex::decode(hex).map_err(|_| signature_error())?;
    bytes.try_into().map_err(|_| signature_error())
}

fn validate_timestamp(value: &str, now_us: i64) -> Result<i64, AppError> {
    if !(10..=16).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(signature_error());
    }
    let signed = value.parse::<i64>().map_err(|_| signature_error())?;
    let now = now_us.div_euclid(1_000_000);
    if now.abs_diff(signed) > SIGNATURE_WINDOW_SECONDS {
        return Err(signature_error());
    }
    Ok(signed)
}

fn validate_nonce(value: &str) -> Result<(), AppError> {
    if !(16..=128).contains(&value.len())
        || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(signature_error());
    }
    Ok(())
}

fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut padded = [0_u8; 64];
    if key.len() > padded.len() {
        padded[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let mut inner_key = padded;
    let mut outer_key = padded;
    for byte in &mut inner_key {
        *byte ^= 0x36;
    }
    for byte in &mut outer_key {
        *byte ^= 0x5c;
    }
    let mut inner = Sha256::new();
    inner.update(inner_key);
    for part in parts {
        inner.update(part);
    }
    let mut outer = Sha256::new();
    outer.update(outer_key);
    outer.update(inner.finalize());
    let result = outer.finalize().into();
    padded.fill(0);
    inner_key.fill(0);
    outer_key.fill(0);
    result
}

fn signature_error() -> AppError {
    AppError::new(
        ErrorCode::AutomationSignatureInvalid,
        "webhook authentication failed",
    )
}

fn replay_error() -> AppError {
    AppError::new(ErrorCode::AutomationReplay, "webhook nonce replayed")
}

fn payload_error() -> AppError {
    AppError::new(
        ErrorCode::AutomationPayloadInvalid,
        "invalid webhook payload",
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
