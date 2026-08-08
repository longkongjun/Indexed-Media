use std::sync::Arc;

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

use super::event_store::AutomationEventStore;
use super::model::{
    AutomationAction, AutomationEventFilter, AutomationEventStatus, AutomationEventView,
};

#[derive(Clone)]
struct EventHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    events: Option<AutomationEventStore>,
}

impl EventHttpState {
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

    fn store(&self) -> Result<&AutomationEventStore, AppError> {
        self.events.as_ref().ok_or_else(not_ready)
    }
}

/// Build authenticated event query, retry, and cancellation routes.
pub fn router(config: AppConfig, db: &Db) -> Router {
    router_with_notifier(config, db, OutboxNotifier::new())
}

/// Build event routes with the shared post-commit outbox notifier.
pub fn router_with_notifier(config: AppConfig, db: &Db, notifier: OutboxNotifier) -> Router {
    let events =
        AutomationEventStore::open_with_notifier(db.pool().clone(), &config.config_dir, notifier)
            .ok();
    Router::new()
        .route("/api/v1/automation-events", get(list_events))
        .route(
            "/api/v1/automation-events/{automationEventId}",
            get(get_event),
        )
        .route(
            "/api/v1/automation-events/{automationEventId}/retries",
            axum::routing::post(retry_event),
        )
        .route(
            "/api/v1/automation-events/{automationEventId}/cancellations",
            axum::routing::post(cancel_event),
        )
        .with_state(Arc::new(EventHttpState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            config,
            db: db.clone(),
            events,
        }))
}

async fn list_events(
    State(state): State<Arc<EventHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    state.guard_get(&headers).await?;
    let (page, filter) = list_query(&uri)?;
    Ok(Json(
        state.store()?.list_filtered_page(&page, filter).await?,
    ))
}

async fn get_event(
    State(state): State<Arc<EventHttpState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<AutomationEventView>, AppError> {
    state.guard_get(&headers).await?;
    Ok(Json(
        state
            .store()?
            .get(id)
            .await?
            .ok_or_else(|| not_found("automation event not found"))?,
    ))
}

async fn retry_event(
    State(state): State<Arc<EventHttpState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    state.guard_write(&headers).await?;
    let event = state
        .store()?
        .retry(
            id,
            idempotency_key(&headers)?,
            chrono::Utc::now().timestamp_micros(),
        )
        .await?;
    audit::record(
        state.db.pool(),
        "automation.event-retry",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(event)))
}

async fn cancel_event(
    State(state): State<Arc<EventHttpState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    state.guard_write(&headers).await?;
    let event = state
        .store()?
        .cancel(
            id,
            idempotency_key(&headers)?,
            chrono::Utc::now().timestamp_micros(),
        )
        .await?;
    audit::record(
        state.db.pool(),
        "automation.event-cancel",
        "success",
        Some(&id.to_string()),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(event)))
}

fn list_query(uri: &Uri) -> Result<(PageRequest, AutomationEventFilter), AppError> {
    let mut cursor = None;
    let mut limit = None;
    let mut source_id = None;
    let mut status = None;
    let mut action = None;
    for (key, value) in url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "cursor" if cursor.is_none() => cursor = Some(value.into_owned()),
            "limit" if limit.is_none() => {
                limit = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| validation("invalid automation event page limit"))?,
                );
            }
            "source_id" if source_id.is_none() => {
                source_id = Some(
                    value
                        .parse::<Uuid>()
                        .map_err(|_| validation("invalid automation event source filter"))?,
                );
            }
            "status" if status.is_none() => {
                status = Some(
                    AutomationEventStatus::parse(&value)
                        .ok_or_else(|| validation("invalid automation event status filter"))?,
                );
            }
            "action" if action.is_none() => {
                action = Some(
                    AutomationAction::parse(&value)
                        .ok_or_else(|| validation("invalid automation event action filter"))?,
                );
            }
            _ => return Err(validation("invalid automation event list query")),
        }
    }
    let page =
        PageRequest::new(cursor, limit).map_err(|_| validation("invalid automation event page"))?;
    Ok((
        page,
        AutomationEventFilter {
            source_id,
            status,
            action,
        },
    ))
}

fn idempotency_key(headers: &HeaderMap) -> Result<&str, AppError> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| (1..=255).contains(&value.len()))
        .ok_or_else(|| validation("invalid automation event idempotency key"))
}

fn validation(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn not_found(message: &'static str) -> AppError {
    AppError::new(ErrorCode::NotFound, message)
}

fn not_ready() -> AppError {
    AppError::new(ErrorCode::NotReady, "automation event routes are not ready")
}
