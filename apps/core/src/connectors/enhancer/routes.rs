use std::sync::Arc;

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, header};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::bootstrap::config::AppConfig;
use crate::identity::service::IdentityService;
use crate::platform::audit;
use crate::platform::db::Db;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::request_security::{CsrfGuard, SessionGuard, validate_same_origin};
use crate::shared::error::{AppError, ErrorCode};

use super::model::{EnhancerConfigInput, EnhancerConfigView, EnhancerConnectionTestResult};
use super::ollama::OllamaClient;
use super::port::IdentificationEnhancer;
use super::service::EnhancerService;

#[derive(Clone)]
struct EnhancerHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    service: EnhancerService,
}

impl EnhancerHttpState {
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
}

/// Build production routes using the one fixed local Ollama adapter.
pub fn router(config: AppConfig, db: &Db, notifier: OutboxNotifier) -> Router {
    let adapter = OllamaClient::production().map_or_else(
        |_| Arc::new(UnavailableEnhancer) as Arc<dyn IdentificationEnhancer>,
        |client| Arc::new(client) as Arc<dyn IdentificationEnhancer>,
    );
    router_with_dyn_adapter(config, db, adapter, notifier)
}

/// Build routes around an injected bounded adapter.
pub fn router_with_adapter<T>(config: AppConfig, db: &Db, adapter: Arc<T>) -> Router
where
    T: IdentificationEnhancer + 'static,
{
    let adapter: Arc<dyn IdentificationEnhancer> = adapter;
    router_with_dyn_adapter(config, db, adapter, OutboxNotifier::new())
}

fn router_with_dyn_adapter(
    config: AppConfig,
    db: &Db,
    adapter: Arc<dyn IdentificationEnhancer>,
    notifier: OutboxNotifier,
) -> Router {
    Router::new()
        .route(
            "/api/v1/identification-enhancer",
            get(get_config).put(put_config),
        )
        .route(
            "/api/v1/identification-enhancer/connection-tests",
            post(test_connection),
        )
        .with_state(Arc::new(EnhancerHttpState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            service: EnhancerService::new(db.pool().clone(), adapter, notifier),
            config,
            db: db.clone(),
        }))
}

async fn get_config(
    State(state): State<Arc<EnhancerHttpState>>,
    headers: HeaderMap,
) -> Result<Json<EnhancerConfigView>, AppError> {
    state.guard_get(&headers).await?;
    Ok(Json(state.service.get().await?))
}

async fn test_connection(
    State(state): State<Arc<EnhancerHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<EnhancerConfigInput>, JsonRejection>,
) -> Result<Json<EnhancerConnectionTestResult>, AppError> {
    state.guard_write(&headers).await?;
    let Json(input) = payload.map_err(|_| validation())?;
    let result = state.service.test(input).await?;
    audit::record(
        state.db.pool(),
        "identification-enhancer.connection-test",
        "success",
        None,
    )
    .await?;
    Ok(Json(result))
}

async fn put_config(
    State(state): State<Arc<EnhancerHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<EnhancerConfigInput>, JsonRejection>,
) -> Result<Json<EnhancerConfigView>, AppError> {
    state.guard_write(&headers).await?;
    let expected = if_match(&headers)?;
    let Json(input) = payload.map_err(|_| validation())?;
    let result = state.service.replace(expected, input).await?;
    audit::record(
        state.db.pool(),
        "identification-enhancer.config-save",
        "success",
        None,
    )
    .await?;
    Ok(Json(result))
}

fn if_match(headers: &HeaderMap) -> Result<i64, AppError> {
    let value = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(validation)?;
    if value.is_empty() || value.len() > 19 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(validation());
    }
    value.parse().map_err(|_| validation())
}

fn validation() -> AppError {
    AppError::new(
        ErrorCode::ValidationFailed,
        "invalid identification enhancer request",
    )
}

fn not_ready() -> AppError {
    AppError::new(ErrorCode::NotReady, "enhancer routes are not ready")
}

struct UnavailableEnhancer;

#[async_trait::async_trait]
impl IdentificationEnhancer for UnavailableEnhancer {
    async fn probe(
        &self,
        _endpoint: super::port::EnhancerEndpoint<'_>,
    ) -> Result<super::port::EnhancerProbe, super::port::EnhancerError> {
        Err(super::port::EnhancerError::Unavailable)
    }

    async fn enhance(
        &self,
        _endpoint: super::port::EnhancerEndpoint<'_>,
        _input: &super::model::EnhancementInput,
    ) -> Result<super::model::EnhancementHints, super::port::EnhancerError> {
        Err(super::port::EnhancerError::Unavailable)
    }
}
