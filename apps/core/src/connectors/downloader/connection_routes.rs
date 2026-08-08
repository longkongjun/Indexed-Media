use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::identity::service::IdentityService;
use crate::platform::audit;
use crate::platform::db::Db;
use crate::platform::request_security::{CsrfGuard, SessionGuard, validate_same_origin};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::PageRequest;

use super::connection_service::{DownloaderConnectionService, DownloaderRegistry};
use super::model::{
    DownloaderConnectionInput, DownloaderConnectionTestResult, DownloaderConnectionView,
};

#[derive(Clone)]
struct DownloaderHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    downloaders: Option<DownloaderConnectionService>,
}

impl DownloaderHttpState {
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

    fn service(&self) -> Result<&DownloaderConnectionService, AppError> {
        self.downloaders.as_ref().ok_or_else(not_ready)
    }
}

/// 使用两个内置生产适配器构建下载器连接路由。
pub fn router(config: AppConfig, db: &Db) -> Router {
    let registry = DownloaderRegistry::production().ok();
    router_with_optional_registry(config, db, registry)
}

/// 使用注入注册表构建下载器连接路由。
pub fn router_with_registry(config: AppConfig, db: &Db, registry: DownloaderRegistry) -> Router {
    router_with_optional_registry(config, db, Some(registry))
}

fn router_with_optional_registry(
    config: AppConfig,
    db: &Db,
    registry: Option<DownloaderRegistry>,
) -> Router {
    let downloaders = registry.and_then(|registry| {
        DownloaderConnectionService::open(db.pool().clone(), &config.config_dir, registry).ok()
    });
    Router::new()
        .route(
            "/api/v1/downloader-connections",
            get(list_connections).post(create_connection),
        )
        .route(
            "/api/v1/downloader-connections/connection-tests",
            post(test_connection),
        )
        .route(
            "/api/v1/downloader-connections/{downloaderConnectionId}",
            get(get_connection)
                .put(replace_connection)
                .delete(delete_connection),
        )
        .with_state(Arc::new(DownloaderHttpState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            config,
            db: db.clone(),
            downloaders,
        }))
}

async fn list_connections(
    State(state): State<Arc<DownloaderHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    state.guard_get(&headers).await?;
    let page = list_query(&uri)?;
    Ok(Json(state.service()?.list(&page).await?))
}

async fn get_connection(
    State(state): State<Arc<DownloaderHttpState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<DownloaderConnectionView>, AppError> {
    state.guard_get(&headers).await?;
    Ok(Json(state.service()?.get(id).await?))
}

async fn test_connection(
    State(state): State<Arc<DownloaderHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<DownloaderConnectionInput>, JsonRejection>,
) -> Result<Json<DownloaderConnectionTestResult>, AppError> {
    state.guard_write(&headers).await?;
    let Json(input) = payload.map_err(|_| validation("invalid downloader connection JSON"))?;
    let result = state.service()?.test(input).await?;
    audit::record(
        state.db.pool(),
        "downloader.connection-test",
        "success",
        None,
    )
    .await?;
    Ok(Json(result))
}

async fn create_connection(
    State(state): State<Arc<DownloaderHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<DownloaderConnectionInput>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    state.guard_write(&headers).await?;
    let Json(input) = payload.map_err(|_| validation("invalid downloader connection JSON"))?;
    let result = state.service()?.create(input).await?;
    audit::record(
        state.db.pool(),
        "downloader.connection-create",
        "success",
        Some(&result.id.to_string()),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn replace_connection(
    State(state): State<Arc<DownloaderHttpState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    payload: Result<Json<DownloaderConnectionInput>, JsonRejection>,
) -> Result<Json<DownloaderConnectionView>, AppError> {
    state.guard_write(&headers).await?;
    let expected = if_match(&headers)?;
    let Json(input) = payload.map_err(|_| validation("invalid downloader connection JSON"))?;
    let result = state.service()?.replace(id, expected, input).await?;
    audit::record(
        state.db.pool(),
        "downloader.connection-replace",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok(Json(result))
}

async fn delete_connection(
    State(state): State<Arc<DownloaderHttpState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    state.guard_write(&headers).await?;
    state.service()?.delete(id, if_match(&headers)?).await?;
    audit::record(
        state.db.pool(),
        "downloader.connection-delete",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn list_query(uri: &Uri) -> Result<PageRequest, AppError> {
    let mut cursor = None;
    let mut limit = None;
    for (key, value) in url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "cursor" if cursor.is_none() => cursor = Some(value.into_owned()),
            "limit" if limit.is_none() => {
                limit = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| validation("invalid downloader page limit"))?,
                );
            }
            _ => return Err(validation("invalid downloader list query")),
        }
    }
    PageRequest::new(cursor, limit).map_err(|_| validation("invalid downloader page"))
}

fn if_match(headers: &HeaderMap) -> Result<i64, AppError> {
    let value = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| validation("missing downloader configuration version"))?;
    if value.is_empty() || value.len() > 19 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(validation("invalid downloader configuration version"));
    }
    value
        .parse()
        .map_err(|_| validation("invalid downloader configuration version"))
}

fn validation(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn not_ready() -> AppError {
    AppError::new(ErrorCode::NotReady, "downloader routes are not ready")
}
