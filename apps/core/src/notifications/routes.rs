//! 通知中心的管理员路由：应用内通知列表/已读与出站 Webhook 配置。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::identity::service::IdentityService;
use crate::notifications::store::NotificationStore;
use crate::notifications::webhook::WebhookStore;
use crate::platform::db::Db;
use crate::platform::request_security::SessionGuard;
use crate::shared::error::{AppError, ErrorCode};

#[derive(Clone)]
struct NotificationState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    notifications: NotificationStore,
    webhooks: WebhookStore,
}

#[derive(Deserialize)]
struct ListQuery {
    unread_only: Option<bool>,
    cursor: Option<String>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct MarkReadBody {
    notification_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct WebhookInput {
    target_url: String,
    display_name: Option<String>,
    enabled: bool,
}

/// 组装通知中心管理员路由。
pub fn router(config: AppConfig, db: &Db) -> Router {
    Router::new()
        .route("/api/v1/notifications", get(list_notifications))
        .route("/api/v1/notifications/read-markers", post(mark_read))
        .route(
            "/api/v1/notification-webhooks",
            get(list_webhooks).post(create_webhook),
        )
        .route(
            "/api/v1/notification-webhooks/{webhookId}",
            delete(remove_webhook),
        )
        .route(
            "/api/v1/notification-webhooks/connection-tests",
            post(test_webhook),
        )
        .with_state(Arc::new(NotificationState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            notifications: NotificationStore::new(db.pool().clone()),
            webhooks: WebhookStore::new(db.pool().clone()),
            config,
            db: db.clone(),
        }))
}

impl NotificationState {
    async fn guard_get(&self, headers: &HeaderMap) -> Result<(), AppError> {
        self.ensure_ready().await?;
        SessionGuard::authenticate(headers, &self.identity).await?;
        Ok(())
    }

    async fn guard_write(&self, headers: &HeaderMap) -> Result<(), AppError> {
        self.ensure_ready().await?;
        SessionGuard::authenticate_without_sliding(headers, &self.identity).await?;
        crate::platform::request_security::validate_same_origin(
            headers,
            &self.config.public_origin,
        )?;
        crate::platform::request_security::CsrfGuard::validate(
            headers,
            &SessionGuard::authenticate_without_sliding(headers, &self.identity).await?,
        )?;
        Ok(())
    }

    async fn ensure_ready(&self) -> Result<(), AppError> {
        if !self.config.readiness_issues().is_empty()
            || !matches!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(250),
                    sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(self.db.pool()),
                )
                .await,
                Ok(Ok(1))
            )
        {
            return Err(AppError::new(
                ErrorCode::NotReady,
                "business routes are closed",
            ));
        }
        Ok(())
    }
}

async fn list_notifications(
    State(state): State<Arc<NotificationState>>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    state.guard_get(&headers).await?;
    let before_us = query
        .cursor
        .as_deref()
        .and_then(|cursor| cursor.strip_prefix("notifications:"))
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_micros());
    let (items, next_cursor) = state
        .notifications
        .list(
            query.unread_only.unwrap_or(false),
            before_us,
            query.limit.unwrap_or(50),
        )
        .await?;
    let unread_count = state.notifications.unread_count().await?;
    Ok(Json(serde_json::json!({
        "items": items,
        "next_cursor": next_cursor,
        "unread_count": unread_count,
    })))
}

async fn mark_read(
    State(state): State<Arc<NotificationState>>,
    headers: HeaderMap,
    body: Option<Json<MarkReadBody>>,
) -> Result<Json<Value>, AppError> {
    state.guard_write(&headers).await?;
    let now_us = chrono::Utc::now().timestamp_micros();
    let affected = match body.map(|Json(body)| body) {
        Some(MarkReadBody {
            notification_id: Some(id),
        }) => {
            state.notifications.mark_read(id, now_us).await?;
            1_u64
        }
        _ => state.notifications.mark_all_read(now_us).await?,
    };
    Ok(Json(serde_json::json!({ "marked_read": affected })))
}

async fn list_webhooks(
    State(state): State<Arc<NotificationState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    state.guard_get(&headers).await?;
    Ok(Json(
        serde_json::json!({ "items": state.webhooks.list().await? }),
    ))
}

async fn create_webhook(
    State(state): State<Arc<NotificationState>>,
    headers: HeaderMap,
    Json(input): Json<WebhookInput>,
) -> Result<Json<Value>, AppError> {
    state.guard_write(&headers).await?;
    let version = state
        .webhooks
        .save(
            Uuid::now_v7(),
            0,
            &input.target_url,
            input.display_name.as_deref(),
            input.enabled,
            chrono::Utc::now().timestamp_micros(),
        )
        .await?;
    Ok(Json(serde_json::json!({ "config_version": version })))
}

async fn remove_webhook(
    State(state): State<Arc<NotificationState>>,
    headers: HeaderMap,
    axum::extract::Path(webhook_id): axum::extract::Path<Uuid>,
) -> Result<Response, AppError> {
    state.guard_write(&headers).await?;
    let version = if_match(&headers)?;
    state.webhooks.delete(webhook_id, version).await?;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

async fn test_webhook(
    State(state): State<Arc<NotificationState>>,
    headers: HeaderMap,
    Json(input): Json<WebhookInput>,
) -> Result<Json<Value>, AppError> {
    state.guard_write(&headers).await?;
    // 探测端点与保存路径同校验（SA-3-001）：拒绝 http:// 内网目标被当作 SSRF 探测原语。
    if !input.target_url.starts_with("https://") || input.target_url.len() > 2048 {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "webhook target must be an https URL",
        ));
    }
    crate::notifications::webhook::test_delivery(&input.target_url).await?;
    Ok(Json(serde_json::json!({ "delivered": true })))
}

fn if_match(headers: &HeaderMap) -> Result<i64, AppError> {
    use axum::http::header;
    let value = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::new(ErrorCode::ValidationFailed, "If-Match is required"))?;
    if value.is_empty() || value.len() > 19 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "invalid If-Match",
        ));
    }
    value
        .parse()
        .map_err(|_| AppError::new(ErrorCode::ValidationFailed, "invalid If-Match"))
}
