use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::identity::model::AuthenticatedSession;
use crate::identity::service::IdentityService;
use crate::organization::model::{OrganizationTargetCommand, OrganizationTargetPreflightCommand};
use crate::organization::target_service::{
    OrganizationTargetPreflight, OrganizationTargetService, OrganizationTargetView,
};
use crate::platform::audit;
use crate::platform::db::Db;
use crate::platform::request_security::{CsrfGuard, SessionGuard, validate_same_origin};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, PageRequest};

#[derive(Clone)]
struct OrganizationHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    targets: OrganizationTargetService,
}

impl OrganizationHttpState {
    async fn ensure_ready(&self) -> Result<(), AppError> {
        if !self.config.readiness_issues().is_empty() || !self.targets.configuration_is_current() {
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

    async fn guard_get(&self, headers: &HeaderMap) -> Result<AuthenticatedSession, AppError> {
        self.ensure_ready().await?;
        SessionGuard::authenticate(headers, &self.identity).await
    }

    async fn guard_write(&self, headers: &HeaderMap) -> Result<AuthenticatedSession, AppError> {
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

/// 构建目标 preflight 与版本化 CRUD 路由。
pub fn router(config: AppConfig, db: &Db, targets: OrganizationTargetService) -> Router {
    Router::new()
        .route(
            "/api/v1/organization-targets",
            get(list_targets).post(create_target),
        )
        .route(
            "/api/v1/organization-targets/preflights",
            post(preflight_target),
        )
        .route(
            "/api/v1/organization-targets/{organizationTargetId}",
            get(get_target).put(replace_target).delete(delete_target),
        )
        .with_state(Arc::new(OrganizationHttpState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            config,
            db: db.clone(),
            targets,
        }))
}

async fn list_targets(
    State(state): State<Arc<OrganizationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<CursorPage<OrganizationTargetView>>, AppError> {
    let session = state.guard_get(&headers).await?;
    let page = list_query(&uri)?;
    Ok(Json(state.targets.list(session.account.id, &page).await?))
}

async fn get_target(
    State(state): State<Arc<OrganizationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<OrganizationTargetView>, AppError> {
    let session = state.guard_get(&headers).await?;
    let id = target_id(&uri)?;
    Ok(Json(state.targets.get(session.account.id, id).await?))
}

async fn preflight_target(
    State(state): State<Arc<OrganizationHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<OrganizationTargetPreflightCommand>, JsonRejection>,
) -> Result<Json<OrganizationTargetPreflight>, AppError> {
    let session = state.guard_write(&headers).await?;
    let Json(command) = payload.map_err(|_| validation("invalid organization preflight JSON"))?;
    let result = state.targets.preflight(session.account.id, command).await?;
    audit::record(
        state.db.pool(),
        "organization.target-preflight",
        "success",
        Some(result.root_id.as_str()),
    )
    .await?;
    Ok(Json(result))
}

async fn create_target(
    State(state): State<Arc<OrganizationHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<OrganizationTargetCommand>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_write(&headers).await?;
    let Json(command) = payload.map_err(|_| validation("invalid organization target JSON"))?;
    let result = state.targets.create(session.account.id, command).await?;
    audit::record(
        state.db.pool(),
        "organization.target-create",
        "success",
        Some(&result.id.to_string()),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn replace_target(
    State(state): State<Arc<OrganizationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    payload: Result<Json<OrganizationTargetCommand>, JsonRejection>,
) -> Result<Json<OrganizationTargetView>, AppError> {
    let session = state.guard_write(&headers).await?;
    let id = target_id(&uri)?;
    let expected_version = if_match(&headers)?;
    let Json(command) = payload.map_err(|_| validation("invalid organization target JSON"))?;
    let result = state
        .targets
        .replace(session.account.id, id, expected_version, command)
        .await?;
    audit::record(
        state.db.pool(),
        "organization.target-replace",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok(Json(result))
}

async fn delete_target(
    State(state): State<Arc<OrganizationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, AppError> {
    let session = state.guard_write(&headers).await?;
    let id = target_id(&uri)?;
    state
        .targets
        .delete(session.account.id, id, if_match(&headers)?)
        .await?;
    audit::record(
        state.db.pool(),
        "organization.target-delete",
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
                        .map_err(|_| validation("invalid organization target page limit"))?,
                );
            }
            _ => return Err(validation("invalid organization target list query")),
        }
    }
    PageRequest::new(cursor, limit)
        .map_err(|_| validation("invalid organization target page request"))
}

fn target_id(uri: &Uri) -> Result<Uuid, AppError> {
    let value = uri
        .path()
        .strip_prefix("/api/v1/organization-targets/")
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or_else(|| validation("invalid organization target ID"))?;
    Uuid::parse_str(value).map_err(|_| validation("invalid organization target ID"))
}

fn if_match(headers: &HeaderMap) -> Result<i64, AppError> {
    let value = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| validation("missing organization target version"))?;
    if value.is_empty() || value.len() > 19 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(validation("invalid organization target version"));
    }
    value
        .parse()
        .map_err(|_| validation("invalid organization target version"))
}

fn validation(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn not_ready() -> AppError {
    AppError::new(ErrorCode::NotReady, "organization routes are not ready")
}
