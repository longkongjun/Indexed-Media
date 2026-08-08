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

use super::model::{
    AutomationSourceConnectionTestResult, AutomationSourceCreateResult, AutomationSourceInput,
    AutomationSourceView,
};
use super::service::AutomationSourceService;

#[derive(Clone)]
struct AutomationHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    automation: Option<AutomationSourceService>,
}

impl AutomationHttpState {
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

    fn service(&self) -> Result<&AutomationSourceService, AppError> {
        self.automation.as_ref().ok_or_else(not_ready)
    }
}

/// Build authenticated automation source management routes.
pub fn router(config: AppConfig, db: &Db) -> Router {
    let automation = AutomationSourceService::open(db.pool().clone(), &config.config_dir).ok();
    Router::new()
        .route(
            "/api/v1/automation-sources",
            get(list_sources).post(create_source),
        )
        .route(
            "/api/v1/automation-sources/connection-tests",
            post(test_source),
        )
        .route(
            "/api/v1/automation-sources/{automationSourceId}",
            get(get_source).put(replace_source).delete(delete_source),
        )
        .route(
            "/api/v1/automation-sources/{automationSourceId}/secret-rotations",
            post(rotate_webhook_secret),
        )
        .with_state(Arc::new(AutomationHttpState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            config,
            db: db.clone(),
            automation,
        }))
}

async fn list_sources(
    State(state): State<Arc<AutomationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    state.guard_get(&headers).await?;
    Ok(Json(state.service()?.list(&list_query(&uri)?).await?))
}

async fn get_source(
    State(state): State<Arc<AutomationHttpState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<AutomationSourceView>, AppError> {
    state.guard_get(&headers).await?;
    Ok(Json(state.service()?.get(id).await?))
}

async fn test_source(
    State(state): State<Arc<AutomationHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<AutomationSourceInput>, JsonRejection>,
) -> Result<Json<AutomationSourceConnectionTestResult>, AppError> {
    state.guard_write(&headers).await?;
    let Json(input) = payload.map_err(|_| validation("invalid automation source JSON"))?;
    let result = state.service()?.test_candidate(input).await?;
    audit::record(state.db.pool(), "automation.source-test", "success", None).await?;
    Ok(Json(result))
}

async fn create_source(
    State(state): State<Arc<AutomationHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<AutomationSourceInput>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    state.guard_write(&headers).await?;
    let Json(input) = payload.map_err(|_| validation("invalid automation source JSON"))?;
    let result = state.service()?.create(input).await?;
    let id = match &result {
        AutomationSourceCreateResult::Source(source) => source.id,
        AutomationSourceCreateResult::Webhook(receipt) => receipt.source.id,
    };
    audit::record(
        state.db.pool(),
        "automation.source-create",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn replace_source(
    State(state): State<Arc<AutomationHttpState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    payload: Result<Json<AutomationSourceInput>, JsonRejection>,
) -> Result<Json<AutomationSourceView>, AppError> {
    state.guard_write(&headers).await?;
    let expected = if_match(&headers)?;
    let Json(input) = payload.map_err(|_| validation("invalid automation source JSON"))?;
    let result = state.service()?.replace(id, expected, input).await?;
    audit::record(
        state.db.pool(),
        "automation.source-replace",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok(Json(result))
}

async fn delete_source(
    State(state): State<Arc<AutomationHttpState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    state.guard_write(&headers).await?;
    state.service()?.delete(id, if_match(&headers)?).await?;
    audit::record(
        state.db.pool(),
        "automation.source-delete",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn rotate_webhook_secret(
    State(state): State<Arc<AutomationHttpState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<super::model::WebhookSecretReceipt>, AppError> {
    state.guard_write(&headers).await?;
    let result = state
        .service()?
        .rotate_webhook_secret(id, if_match(&headers)?, idempotency_key(&headers)?)
        .await?;
    audit::record(
        state.db.pool(),
        "automation.webhook-secret-rotate",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok(Json(result))
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
                        .map_err(|_| validation("invalid automation source page limit"))?,
                );
            }
            _ => return Err(validation("invalid automation source list query")),
        }
    }
    PageRequest::new(cursor, limit).map_err(|_| validation("invalid automation source page"))
}

fn if_match(headers: &HeaderMap) -> Result<i64, AppError> {
    let value = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| validation("missing automation source version"))?;
    if value.is_empty() || value.len() > 19 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(validation("invalid automation source version"));
    }
    value
        .parse()
        .map_err(|_| validation("invalid automation source version"))
}

fn idempotency_key(headers: &HeaderMap) -> Result<&str, AppError> {
    let value = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| validation("missing webhook rotation idempotency key"))?;
    if !(1..=255).contains(&value.len()) || value.chars().any(char::is_control) {
        return Err(validation("invalid webhook rotation idempotency key"));
    }
    Ok(value)
}

fn validation(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn not_ready() -> AppError {
    AppError::new(ErrorCode::NotReady, "automation routes are not ready")
}
