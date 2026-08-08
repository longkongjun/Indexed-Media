use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::identity::service::IdentityService;
use crate::platform::audit;
use crate::platform::db::Db;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::request_security::{CsrfGuard, SessionGuard, validate_same_origin};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::PageRequest;

use super::model::{
    CreateDownloadTaskCommand, DownloadTaskFilter, DownloadTaskStatus, DownloadTaskView,
};
use super::task_service::DownloadTaskService;

#[derive(Clone)]
struct DownloadTaskHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    tasks: Option<DownloadTaskService>,
}

impl DownloadTaskHttpState {
    async fn ensure_ready(&self) -> Result<(), AppError> {
        if !self.config.readiness_issues().is_empty() {
            return Err(not_ready());
        }
        match tokio::time::timeout(
            std::time::Duration::from_millis(250),
            sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(self.db.pool()),
        )
        .await
        {
            Ok(Ok(1)) => Ok(()),
            _ => Err(not_ready()),
        }
    }

    async fn guard_get(&self, headers: &HeaderMap) -> Result<(), AppError> {
        self.ensure_ready().await?;
        SessionGuard::authenticate(headers, &self.identity).await?;
        Ok(())
    }

    async fn guard_write(&self, headers: &HeaderMap) -> Result<(), AppError> {
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
        Ok(())
    }

    fn service(&self) -> Result<&DownloadTaskService, AppError> {
        self.tasks.as_ref().ok_or_else(not_ready)
    }
}

/// 构建手工下载任务的认证、同源与 CSRF 防护路由。
pub fn router(config: AppConfig, db: &Db) -> Router {
    router_with_notifier(config, db, OutboxNotifier::new())
}

/// 使用共享 outbox 通知器构建手工下载任务路由。
pub fn router_with_notifier(config: AppConfig, db: &Db, notifier: OutboxNotifier) -> Router {
    let tasks =
        DownloadTaskService::open_with_notifier(db.pool().clone(), &config.config_dir, notifier)
            .ok();
    Router::new()
        .route("/api/v1/download-tasks", get(list_tasks).post(create_task))
        .route("/api/v1/download-tasks/{downloadTaskId}", get(get_task))
        .with_state(Arc::new(DownloadTaskHttpState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            config,
            db: db.clone(),
            tasks,
        }))
}

async fn create_task(
    State(state): State<Arc<DownloadTaskHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<CreateDownloadTaskCommand>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    state.guard_write(&headers).await?;
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| validation("missing download task idempotency key"))?;
    let Json(command) = payload.map_err(|_| validation("invalid download task JSON"))?;
    let result = state.service()?.create(command, idempotency_key).await?;
    audit::record(
        state.db.pool(),
        "download-task.create",
        "success",
        Some(&result.id.to_string()),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

async fn list_tasks(
    State(state): State<Arc<DownloadTaskHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    state.guard_get(&headers).await?;
    let (filter, page) = list_query(&uri)?;
    Ok(Json(state.service()?.list(&filter, &page).await?))
}

async fn get_task(
    State(state): State<Arc<DownloadTaskHttpState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<DownloadTaskView>, AppError> {
    state.guard_get(&headers).await?;
    Ok(Json(state.service()?.get(id).await?))
}

fn list_query(uri: &Uri) -> Result<(DownloadTaskFilter, PageRequest), AppError> {
    let mut filter = DownloadTaskFilter::default();
    let mut cursor = None;
    let mut limit = None;
    for (key, value) in url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "connection_id" if filter.connection_id.is_none() => {
                filter.connection_id = Some(
                    value
                        .parse()
                        .map_err(|_| validation("invalid download task connection filter"))?,
                );
            }
            "status" if filter.status.is_none() => {
                filter.status = Some(
                    DownloadTaskStatus::parse(&value)
                        .ok_or_else(|| validation("invalid download task status filter"))?,
                );
            }
            "q" if filter.query.is_none() => filter.query = Some(value.into_owned()),
            "cursor" if cursor.is_none() => cursor = Some(value.into_owned()),
            "limit" if limit.is_none() => {
                limit = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| validation("invalid download task page limit"))?,
                );
            }
            _ => return Err(validation("invalid download task list query")),
        }
    }
    let page =
        PageRequest::new(cursor, limit).map_err(|_| validation("invalid download task page"))?;
    Ok((filter, page))
}

fn validation(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn not_ready() -> AppError {
    AppError::new(ErrorCode::NotReady, "download task routes are not ready")
}
