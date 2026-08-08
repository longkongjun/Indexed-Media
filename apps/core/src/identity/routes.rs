use crate::bootstrap::config::AppConfig;
use crate::identity::IdentityUseCases;
use crate::identity::model::{
    BootstrapCommand, BootstrapResponse, BootstrapStatusResponse, LoginCommand, LoginRequest,
    SessionResponse,
};
use crate::identity::service::IdentityService;
use crate::platform::db::Db;
use crate::platform::request_security::{
    CsrfGuard, SessionGuard, source_key, validate_same_origin,
};
use crate::shared::error::{AppError, ErrorCode};
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Extension, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Clone)]
struct IdentityHttpState {
    config: AppConfig,
    db: Db,
    service: IdentityService,
}

impl IdentityHttpState {
    async fn ensure_ready(&self) -> Result<(), AppError> {
        if !self.config.readiness_issues().is_empty() {
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
}

/// 构建系统引导和管理员会话 HTTP 路由。
///
/// 路由器强制执行启动就绪状态、来源/CSRF 规则及安全会话 Cookie，并将凭据/会话状态变更委托给
/// [`IdentityService`]。
pub fn router(config: AppConfig, db: &Db) -> Router {
    let service = IdentityService::new(db.pool().clone(), config.config_dir.clone());
    Router::new()
        .route("/api/v1/system/bootstrap-status", get(bootstrap_status))
        .route("/api/v1/system/bootstrap", post(bootstrap))
        .route("/api/v1/sessions", post(create_session))
        .route("/api/v1/session", get(get_session).delete(delete_session))
        .with_state(Arc::new(IdentityHttpState {
            config,
            db: db.clone(),
            service,
        }))
}
async fn bootstrap_status(
    State(state): State<Arc<IdentityHttpState>>,
) -> Result<Json<BootstrapStatusResponse>, AppError> {
    state.ensure_ready().await?;
    Ok(Json(BootstrapStatusResponse {
        requires_initialization: state.service.requires_initialization().await?,
        version: "v1",
    }))
}
async fn bootstrap(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<BootstrapCommand>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    state.ensure_ready().await?;
    if let Err(error) = validate_same_origin(&headers, &state.config.public_origin) {
        state.service.audit_denial("origin").await?;
        return Err(error);
    }
    let Json(command) =
        payload.map_err(|_| AppError::new(ErrorCode::ValidationFailed, "invalid JSON request"))?;
    let account = state.service.bootstrap(command).await?;
    Ok((
        StatusCode::CREATED,
        Json(BootstrapResponse {
            account,
            version: "v1",
        }),
    ))
}
async fn create_session(
    State(state): State<Arc<IdentityHttpState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    payload: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    state.ensure_ready().await?;
    if let Err(error) = validate_same_origin(&headers, &state.config.public_origin) {
        state.service.audit_denial("origin").await?;
        return Err(error);
    }
    let Json(request) =
        payload.map_err(|_| AppError::new(ErrorCode::ValidationFailed, "invalid JSON request"))?;
    let session = state
        .service
        .create_session(LoginCommand {
            administrator_name: request.administrator_name,
            password: request.password,
            source_key: source_key(
                &headers,
                peer.map(|value| value.0.0),
                &state.config.trusted_proxy_cidrs,
            ),
        })
        .await?;
    let mut response = (
        StatusCode::CREATED,
        Json(SessionResponse {
            account: session.account,
            csrf_token: session.csrf_token,
            version: "v1",
        }),
    )
        .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(&session.raw_token))
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
    );
    Ok(response)
}
async fn get_session(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
) -> Result<Json<SessionResponse>, AppError> {
    state.ensure_ready().await?;
    let session = SessionGuard::authenticate(&headers, &state.service).await?;
    let csrf_token = state.service.rotate_csrf(session.session_id).await?;
    Ok(Json(SessionResponse {
        account: session.account,
        csrf_token,
        version: "v1",
    }))
}
async fn delete_session(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    state.ensure_ready().await?;
    let session = SessionGuard::authenticate_without_sliding(&headers, &state.service).await?;
    if let Err(error) = validate_same_origin(&headers, &state.config.public_origin) {
        state.service.audit_denial("origin").await?;
        return Err(error);
    }
    if let Err(error) = CsrfGuard::validate(&headers, &session) {
        state.service.audit_denial("csrf").await?;
        return Err(error);
    }
    state.service.revoke_session(session.session_id).await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "__Host-mediaflow_session=; Max-Age=0; Secure; HttpOnly; SameSite=Strict; Path=/",
        ),
    );
    Ok(response)
}
fn session_cookie(token: &str) -> String {
    format!("__Host-mediaflow_session={token}; Secure; HttpOnly; SameSite=Strict; Path=/")
}
