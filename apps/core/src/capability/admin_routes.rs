//! 能力活动的管理员只读投影：把 AI 通道事件以 `/api/v1` 管理员面的形式暴露给 Web。
//!
//! 该投影与 AI 协议端点（`/api/capability/v1/*`）相互独立：管理员经会话认证查看脱敏
//! 事件流，不参与能力调用。最新事件在前，`next_cursor` 指向更旧的一页。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use sqlx::Row;
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::identity::model::AuthenticatedSession;
use crate::identity::service::IdentityService;
use crate::platform::db::Db;
use crate::platform::request_security::SessionGuard;
use crate::shared::error::{AppError, ErrorCode};

#[derive(Clone)]
struct CapabilityAdminState {
    config: AppConfig,
    identity: IdentityService,
    pool: SqlitePool,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct CapabilityActivityEvent {
    sequence: i64,
    event_id: Uuid,
    event_type: String,
    occurred_at: String,
    task_id: Option<Uuid>,
    request_id: Option<Uuid>,
    state: Option<String>,
    error_code: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct CapabilityActivityPage {
    items: Vec<CapabilityActivityEvent>,
    next_cursor: Option<String>,
}

#[derive(serde::Deserialize)]
struct ActivityQuery {
    cursor: Option<String>,
    limit: Option<i64>,
}

/// 组装能力活动管理员投影路由。
pub fn router(config: AppConfig, db: &Db) -> Router {
    Router::new()
        .route("/api/v1/capability-events", get(list_activity))
        .with_state(Arc::new(CapabilityAdminState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            pool: db.pool().clone(),
            config,
        }))
}

impl CapabilityAdminState {
    async fn guard(&self, headers: &HeaderMap) -> Result<AuthenticatedSession, AppError> {
        if !self.config.readiness_issues().is_empty()
            || !matches!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(250),
                    sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&self.pool),
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
        SessionGuard::authenticate(headers, &self.identity).await
    }
}

async fn list_activity(
    State(state): State<Arc<CapabilityAdminState>>,
    headers: HeaderMap,
    Query(query): Query<ActivityQuery>,
) -> Result<Response, AppError> {
    let _session = state.guard(&headers).await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let before = query
        .cursor
        .as_deref()
        .and_then(|cursor| cursor.strip_prefix("capability-events:"))
        .and_then(|value| value.parse::<i64>().ok());
    let rows = sqlx::query(
        "SELECT sequence,event_id,event_type,occurred_at_us,task_id,request_id,state,error_json
         FROM capability_events
         WHERE (? IS NULL OR sequence<?)
         ORDER BY sequence DESC LIMIT ?",
    )
    .bind(before)
    .bind(before.unwrap_or(i64::MAX))
    .bind(limit)
    .fetch_all(&state.pool)
    .await
    .map_err(internal)?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let event_id_bytes: Vec<u8> = row.try_get("event_id").map_err(internal)?;
        let error_json: Option<String> = row.try_get("error_json").map_err(internal)?;
        let error_code = error_json.and_then(|json| {
            serde_json::from_str::<serde_json::Value>(&json)
                .ok()
                .and_then(|value| {
                    value
                        .get("error_code")
                        .and_then(|code| code.as_str())
                        .map(str::to_owned)
                })
        });
        let occurred_us: i64 = row.try_get("occurred_at_us").map_err(internal)?;
        items.push(CapabilityActivityEvent {
            sequence: row.try_get("sequence").map_err(internal)?,
            event_id: Uuid::from_slice(&event_id_bytes).map_err(bad_uuid)?,
            event_type: row.try_get("event_type").map_err(internal)?,
            occurred_at: chrono::TimeZone::timestamp_micros(&chrono::Utc, occurred_us)
                .single()
                .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
                .unwrap_or_default(),
            task_id: option_uuid(&row, "task_id")?,
            request_id: option_uuid(&row, "request_id")?,
            state: row.try_get("state").map_err(internal)?,
            error_code,
        });
    }
    let next_cursor = if i64::try_from(items.len()).unwrap_or(i64::MAX) < limit {
        None
    } else {
        items
            .last()
            .map(|event| format!("capability-events:{}", event.sequence))
    };
    Ok(Json(CapabilityActivityPage { items, next_cursor }).into_response())
}

fn option_uuid(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Option<Uuid>, AppError> {
    let bytes: Option<Vec<u8>> = row.try_get(column).map_err(internal)?;
    bytes
        .map(|bytes| Uuid::from_slice(&bytes).map_err(bad_uuid))
        .transpose()
}

fn internal(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn bad_uuid(error: uuid::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
