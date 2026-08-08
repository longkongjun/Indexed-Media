use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::discovery::DiscoveryUseCases;
use crate::discovery::model::{
    CreateInboxCommand, InboxDirectoryPage, InboxDirectoryView, InboxPreflightView,
    PreflightInboxCommand,
};
use crate::discovery::policy::{DiscoveryPolicyView, PutDiscoveryPolicyCommand};
use crate::discovery::service::DiscoveryService;
use crate::identity::service::IdentityService;
use crate::platform::audit;
use crate::platform::db::Db;
use crate::platform::request_security::{CsrfGuard, SessionGuard, validate_same_origin};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::PageRequest;

#[derive(Clone)]
struct DiscoveryHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    discovery: DiscoveryService,
}

impl DiscoveryHttpState {
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

    async fn guard_get(&self, headers: &HeaderMap) -> Result<(), AppError> {
        self.ensure_ready().await?;
        SessionGuard::authenticate(headers, &self.identity).await?;
        Ok(())
    }

    async fn guard_post(&self, headers: &HeaderMap) -> Result<(), AppError> {
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
}

/// 构建已认证的部署根目录与收件箱目录路由。
///
/// GET 路由需要有效会话。变更路由还会执行同源和 CSRF 检查，并将成功的
/// 预检/创建操作写入审计日志。
pub fn router(config: AppConfig, db: &Db, discovery: DiscoveryService) -> Router {
    let identity = IdentityService::new(db.pool().clone(), config.config_dir.clone());
    Router::new()
        .route("/api/v1/deployment-roots", get(list_roots))
        .route("/api/v1/inbox-directories/preflight", post(preflight_inbox))
        .route(
            "/api/v1/inbox-directories",
            get(list_inboxes).post(create_inbox),
        )
        .route("/api/v1/inbox-directories/{id}", get(get_inbox))
        .route(
            "/api/v1/inbox-directories/{id}/discovery-policy",
            get(get_discovery_policy).put(put_discovery_policy),
        )
        .with_state(Arc::new(DiscoveryHttpState {
            config,
            db: db.clone(),
            identity,
            discovery,
        }))
}

async fn list_roots(
    State(state): State<Arc<DiscoveryHttpState>>,
    headers: HeaderMap,
) -> Result<
    Json<crate::shared::page::CursorPage<crate::discovery::model::DeploymentRootView>>,
    AppError,
> {
    state.guard_get(&headers).await?;
    Ok(Json(crate::shared::page::CursorPage {
        items: state.discovery.list_roots().await?,
        next_cursor: None,
    }))
}

async fn preflight_inbox(
    State(state): State<Arc<DiscoveryHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<PreflightInboxCommand>, JsonRejection>,
) -> Result<Json<InboxPreflightView>, AppError> {
    state.guard_post(&headers).await?;
    let Json(command) =
        payload.map_err(|_| AppError::new(ErrorCode::ValidationFailed, "invalid JSON request"))?;
    let result = state.discovery.preflight(command).await?;
    audit::record(
        state.db.pool(),
        "inbox.preflight",
        "success",
        Some(result.root_id.as_str()),
    )
    .await?;
    Ok(Json(result))
}

async fn list_inboxes(
    State(state): State<Arc<DiscoveryHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<InboxDirectoryPage>, AppError> {
    state.guard_get(&headers).await?;
    let page = PageRequest::new(cursor_from_uri(&uri)?, None)
        .map_err(|error| AppError::new(ErrorCode::ValidationFailed, error.to_string()))?;
    Ok(Json(state.discovery.list_inboxes(page).await?))
}

async fn create_inbox(
    State(state): State<Arc<DiscoveryHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<CreateInboxCommand>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    state.guard_post(&headers).await?;
    let Json(command) =
        payload.map_err(|_| AppError::new(ErrorCode::ValidationFailed, "invalid JSON request"))?;
    let result = state.discovery.create_inbox(command).await?;
    audit::record(
        state.db.pool(),
        "inbox.create",
        "success",
        Some(&result.id.to_string()),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn get_inbox(
    State(state): State<Arc<DiscoveryHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<InboxDirectoryView>, AppError> {
    state.guard_get(&headers).await?;
    let id = inbox_id_from_uri(&uri)?;
    Ok(Json(state.discovery.get_inbox(id).await?))
}

async fn get_discovery_policy(
    State(state): State<Arc<DiscoveryHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<DiscoveryPolicyView>, AppError> {
    state.guard_get(&headers).await?;
    let id = policy_inbox_id_from_uri(&uri)?;
    Ok(Json(state.discovery.get_policy(id).await?))
}

async fn put_discovery_policy(
    State(state): State<Arc<DiscoveryHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    payload: Result<Json<PutDiscoveryPolicyCommand>, JsonRejection>,
) -> Result<Json<DiscoveryPolicyView>, AppError> {
    state.guard_post(&headers).await?;
    let id = policy_inbox_id_from_uri(&uri)?;
    let expected = if_match(&headers)?;
    let Json(command) =
        payload.map_err(|_| AppError::new(ErrorCode::ValidationFailed, "invalid JSON request"))?;
    let result = state.discovery.put_policy(id, command, expected).await?;
    audit::record(
        state.db.pool(),
        "discovery.policy-update",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok(Json(result))
}

fn cursor_from_uri(uri: &Uri) -> Result<Option<String>, AppError> {
    let Some(query) = uri.query() else {
        return Ok(None);
    };
    let mut pairs = url::form_urlencoded::parse(query.as_bytes());
    let Some((key, value)) = pairs.next() else {
        return Err(validation_error("invalid cursor query"));
    };
    if key != "cursor" || pairs.next().is_some() {
        return Err(validation_error("invalid cursor query"));
    }
    Ok(Some(value.into_owned()))
}

fn inbox_id_from_uri(uri: &Uri) -> Result<Uuid, AppError> {
    let value = uri
        .path()
        .strip_prefix("/api/v1/inbox-directories/")
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or_else(|| validation_error("invalid inbox directory ID"))?;
    Uuid::parse_str(value).map_err(|_| validation_error("invalid inbox directory ID"))
}

fn policy_inbox_id_from_uri(uri: &Uri) -> Result<Uuid, AppError> {
    let value = uri
        .path()
        .strip_prefix("/api/v1/inbox-directories/")
        .and_then(|value| value.strip_suffix("/discovery-policy"))
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or_else(|| validation_error("invalid inbox directory ID"))?;
    Uuid::parse_str(value).map_err(|_| validation_error("invalid inbox directory ID"))
}

fn if_match(headers: &HeaderMap) -> Result<i64, AppError> {
    let value = headers
        .get(axum::http::header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| validation_error("invalid discovery policy version"))?;
    if value.is_empty() || value.len() > 19 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(validation_error("invalid discovery policy version"));
    }
    value
        .parse()
        .map_err(|_| validation_error("invalid discovery policy version"))
}

fn validation_error(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}
