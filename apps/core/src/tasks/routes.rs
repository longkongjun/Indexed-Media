#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::discovery::service::DiscoveryService;
use crate::identity::model::AuthenticatedSession;
use crate::identity::service::IdentityService;
use crate::platform::audit;
use crate::platform::db::Db;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::request_security::{CsrfGuard, SessionGuard, validate_same_origin};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::PageRequest;
use crate::tasks::service::{ScanService, ScanUseCases};

#[derive(Clone)]
struct ScanHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    discovery: DiscoveryService,
    scans: ScanService,
}

impl ScanHttpState {
    async fn ensure_ready(&self) -> Result<(), AppError> {
        if !self.config.readiness_issues().is_empty() || !self.discovery.configuration_is_current()
        {
            return Err(AppError::new(
                ErrorCode::NotReady,
                "business routes are closed",
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
                "business routes are closed",
            )),
        }
    }

    async fn guard_get(&self, headers: &HeaderMap) -> Result<AuthenticatedSession, AppError> {
        self.ensure_ready().await?;
        SessionGuard::authenticate(headers, &self.identity).await
    }

    async fn guard_post(&self, headers: &HeaderMap) -> Result<AuthenticatedSession, AppError> {
        self.ensure_ready().await?;
        let session = SessionGuard::authenticate_without_sliding(headers, &self.identity).await?;
        if let Err(error) = validate_same_origin(headers, &self.config.public_origin) {
            self.identity.audit_denial("origin").await?;
            return Err(error);
        }
        if let Err(error) = CsrfGuard::validate(headers, &session) {
            self.identity.audit_denial("csrf").await?;
            return Err(error);
        }
        Ok(session)
    }
}

/// 使用进程本地发件箱通知器构建已认证扫描任务路由。
pub fn router(config: AppConfig, db: &Db, discovery: DiscoveryService) -> Router {
    router_with_notifier(config, db, discovery, OutboxNotifier::new())
}

/// 构建状态变更提交后会唤醒 `notifier` 订阅者的扫描任务路由。
///
/// 读取需要有效管理员会话；变更还需要同源、CSRF 和 `Idempotency-Key` 请求头。创建、重试和取消成功的请求
/// 会追加审计记录。
pub fn router_with_notifier(
    config: AppConfig,
    db: &Db,
    discovery: DiscoveryService,
    notifier: OutboxNotifier,
) -> Router {
    Router::new()
        .route(
            "/api/v1/inbox-directories/{inboxDirectoryId}/scan-tasks",
            post(create_scan),
        )
        .route("/api/v1/scan-tasks", get(list_tasks))
        .route("/api/v1/scan-tasks/{scanTaskId}", get(get_task))
        .route("/api/v1/scan-tasks/{scanTaskId}/attempts", post(retry_task))
        .route("/api/v1/scan-tasks/{scanTaskId}/cancel", post(cancel_task))
        .route("/api/v1/scan-tasks/{scanTaskId}/files", get(list_files))
        .route("/api/v1/scan-tasks/{scanTaskId}/errors", get(list_errors))
        .with_state(Arc::new(ScanHttpState {
            config: config.clone(),
            db: db.clone(),
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            discovery,
            scans: ScanService::new_with_notifier(db.pool().clone(), notifier),
        }))
}

async fn create_scan(
    State(state): State<Arc<ScanHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_post(&headers).await?;
    let inbox_id = id_between(
        &uri,
        "/api/v1/inbox-directories/",
        "/scan-tasks",
        "inbox directory",
    )?;
    let key = idempotency_key(&headers)?;
    let result = state
        .scans
        .create(session.account.id, inbox_id, key)
        .await?;
    audit::record(
        state.db.pool(),
        "scan.create",
        "success",
        Some(&result.id.to_string()),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

async fn list_tasks(
    State(state): State<Arc<ScanHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    Ok(Json(
        state.scans.list(session.account.id, page(&uri)?).await?,
    ))
}

async fn get_task(
    State(state): State<Arc<ScanHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    let id = task_id(&uri, None)?;
    Ok(Json(state.scans.get(session.account.id, id).await?))
}

async fn retry_task(
    State(state): State<Arc<ScanHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_post(&headers).await?;
    let id = task_id(&uri, Some("attempts"))?;
    let result = state
        .scans
        .retry(session.account.id, id, idempotency_key(&headers)?)
        .await?;
    audit::record(
        state.db.pool(),
        "scan.retry",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

async fn cancel_task(
    State(state): State<Arc<ScanHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_post(&headers).await?;
    let id = task_id(&uri, Some("cancel"))?;
    let result = state
        .scans
        .cancel(session.account.id, id, idempotency_key(&headers)?)
        .await?;
    audit::record(
        state.db.pool(),
        "scan.cancel",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

async fn list_files(
    State(state): State<Arc<ScanHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    let id = task_id(&uri, Some("files"))?;
    Ok(Json(
        state
            .scans
            .list_files(session.account.id, id, page(&uri)?)
            .await?,
    ))
}

async fn list_errors(
    State(state): State<Arc<ScanHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    let id = task_id(&uri, Some("errors"))?;
    Ok(Json(
        state
            .scans
            .list_errors(session.account.id, id, page(&uri)?)
            .await?,
    ))
}

fn idempotency_key(headers: &HeaderMap) -> Result<String, AppError> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| AppError::new(ErrorCode::ValidationFailed, "idempotency key missing"))
}

fn task_id(uri: &Uri, suffix: Option<&str>) -> Result<Uuid, AppError> {
    let prefix = "/api/v1/scan-tasks/";
    match suffix {
        Some(suffix) => id_between(uri, prefix, &format!("/{suffix}"), "scan task"),
        None => uri
            .path()
            .strip_prefix(prefix)
            .filter(|value| !value.is_empty() && !value.contains('/'))
            .ok_or_else(|| validation("invalid scan task ID"))
            .and_then(|value| {
                Uuid::parse_str(value).map_err(|_| validation("invalid scan task ID"))
            }),
    }
}

fn id_between(uri: &Uri, prefix: &str, suffix: &str, kind: &str) -> Result<Uuid, AppError> {
    uri.path()
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or_else(|| validation(format!("invalid {kind} ID")))
        .and_then(|value| {
            Uuid::parse_str(value).map_err(|_| validation(format!("invalid {kind} ID")))
        })
}

fn page(uri: &Uri) -> Result<PageRequest, AppError> {
    let mut cursor = None;
    let mut limit = None;
    if let Some(query) = uri.query() {
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            match key.as_ref() {
                "cursor" if cursor.is_none() => cursor = Some(value.into_owned()),
                "limit" if limit.is_none() => {
                    limit = Some(
                        value
                            .parse::<u32>()
                            .map_err(|_| validation("invalid page limit"))?,
                    );
                }
                _ => return Err(validation("invalid page query")),
            }
        }
    }
    PageRequest::new(cursor, limit).map_err(|error| validation(error.to_string()))
}

fn validation(message: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}
