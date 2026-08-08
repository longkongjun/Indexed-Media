use std::sync::Arc;

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::bootstrap::config::AppConfig;
use crate::connectors::model::{TmdbConfigCommand, TmdbConnectionTestResult, TmdbIntegrationView};
use crate::connectors::service::{
    ConnectorService, TmdbConnectionTester, UnavailableTmdbConnectionTester,
};
use crate::connectors::tmdb::TmdbClient;
use crate::identity::service::IdentityService;
use crate::platform::audit;
use crate::platform::db::Db;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::request_security::{CsrfGuard, SessionGuard, validate_same_origin};
use crate::shared::error::{AppError, ErrorCode};

#[derive(Clone)]
struct ConnectorHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    connectors: ConnectorService,
}

impl ConnectorHttpState {
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
}

/// 使用固定来源生产测试器构建已认证 TMDB 配置路由。
pub fn router(config: AppConfig, db: &Db) -> Router {
    router_with_notifier(config, db, OutboxNotifier::new())
}

/// 构建生产 TMDB 路由，并与 SSE 读取器共享提交后唤醒。
pub fn router_with_notifier(config: AppConfig, db: &Db, notifier: OutboxNotifier) -> Router {
    let tester: Arc<dyn TmdbConnectionTester> = match TmdbClient::production() {
        Ok(client) => Arc::new(client),
        Err(_) => Arc::new(UnavailableTmdbConnectionTester),
    };
    router_with_dyn_tester(config, db, tester, notifier)
}

/// 使用注入的有界连接测试器构建已认证 TMDB 配置路由。
pub fn router_with_tester<T>(config: AppConfig, db: &Db, tester: Arc<T>) -> Router
where
    T: TmdbConnectionTester + 'static,
{
    let tester: Arc<dyn TmdbConnectionTester> = tester;
    router_with_dyn_tester(config, db, tester, OutboxNotifier::new())
}

fn router_with_dyn_tester(
    config: AppConfig,
    db: &Db,
    tester: Arc<dyn TmdbConnectionTester>,
    notifier: OutboxNotifier,
) -> Router {
    let identity = IdentityService::new(db.pool().clone(), config.config_dir.clone());
    let connectors = ConnectorService::new_with_notifier(
        db.pool().clone(),
        &config.config_dir,
        tester,
        notifier,
    );
    Router::new()
        .route(
            "/api/v1/integrations/tmdb",
            get(get_tmdb).put(put_tmdb).delete(delete_tmdb),
        )
        .route(
            "/api/v1/integrations/tmdb/connection-tests",
            post(test_tmdb),
        )
        .with_state(Arc::new(ConnectorHttpState {
            config,
            db: db.clone(),
            identity,
            connectors,
        }))
}

async fn get_tmdb(
    State(state): State<Arc<ConnectorHttpState>>,
    headers: HeaderMap,
) -> Result<Json<TmdbIntegrationView>, AppError> {
    state.guard_get(&headers).await?;
    Ok(Json(state.connectors.get_tmdb().await?))
}

async fn test_tmdb(
    State(state): State<Arc<ConnectorHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<TmdbConfigCommand>, JsonRejection>,
) -> Result<Json<TmdbConnectionTestResult>, AppError> {
    state.guard_write(&headers).await?;
    let Json(command) = payload.map_err(|_| validation_error())?;
    let result = state.connectors.test_tmdb(command).await?;
    audit::record(
        state.db.pool(),
        "tmdb.connection-test",
        "success",
        Some("tmdb"),
    )
    .await?;
    Ok(Json(result))
}

async fn put_tmdb(
    State(state): State<Arc<ConnectorHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<TmdbConfigCommand>, JsonRejection>,
) -> Result<Json<TmdbIntegrationView>, AppError> {
    state.guard_write(&headers).await?;
    let expected = if_match(&headers)?;
    let Json(command) = payload.map_err(|_| validation_error())?;
    let result = state.connectors.put_tmdb(command, expected).await?;
    audit::record(state.db.pool(), "tmdb.config-save", "success", Some("tmdb")).await?;
    Ok(Json(result))
}

async fn delete_tmdb(
    State(state): State<Arc<ConnectorHttpState>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    state.guard_write(&headers).await?;
    state.connectors.delete_tmdb(if_match(&headers)?).await?;
    audit::record(
        state.db.pool(),
        "tmdb.config-delete",
        "success",
        Some("tmdb"),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn if_match(headers: &HeaderMap) -> Result<i64, AppError> {
    let value = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(validation_error)?;
    if value.is_empty() || value.len() > 19 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(validation_error());
    }
    value.parse().map_err(|_| validation_error())
}

fn validation_error() -> AppError {
    AppError::new(ErrorCode::ValidationFailed, "invalid tmdb request")
}
