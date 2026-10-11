//! 可选出站 Webhook：配置存储与不落盘测试。
//!
//! 投递面按 M4 边界保守：HTTPS 目标、有界超时、固定 JSON 载荷（kind/title/body 与
//! 任务引用，不含路径或凭据）；测试不落盘，自动投递随定时触发机制统一接入。

use sqlx::Row;
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use crate::shared::error::{AppError, ErrorCode};

/// 出站 Webhook 的脱敏配置投影。
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct WebhookView {
    /// 配置稳定 ID。
    pub id: Uuid,
    /// 出站目标（公开信息）。
    pub target_url: String,
    /// 管理员可读显示名。
    pub display_name: Option<String>,
    /// 是否启用。
    pub enabled: bool,
    /// 乐观并发版本。
    pub config_version: i64,
    /// 独立健康分类。
    pub health: &'static str,
    /// 最近一次投递时间。
    pub last_delivery_at: Option<String>,
}

/// Webhook 配置存储。
#[derive(Clone)]
pub struct WebhookStore {
    pool: SqlitePool,
}

impl WebhookStore {
    /// 创建存储。
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 保存（插入或按版本更新）一条 Webhook 配置。
    ///
    /// # Errors
    ///
    /// 输入越界、乐观并发或数据库失败。
    pub async fn save(
        &self,
        id: Uuid,
        expected_version: i64,
        target_url: &str,
        display_name: Option<&str>,
        enabled: bool,
        now_us: i64,
    ) -> Result<i64, AppError> {
        if !target_url.starts_with("https://")
            || target_url.len() > 2048
            || display_name.is_some_and(|value| value.is_empty() || value.len() > 120)
        {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "webhook target must be an https URL",
            ));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database)?;
        let rows = if expected_version == 0 {
            sqlx::query(
                "INSERT INTO notification_webhooks
                 (id,target_url,display_name,enabled,config_version,health,last_delivery_us,
                  created_at_us,updated_at_us)
                 VALUES (?,?,?,?,1,'unconfigured',NULL,?,?)",
            )
            .bind(id.as_bytes().to_vec())
            .bind(target_url)
            .bind(display_name)
            .bind(i64::from(enabled))
            .bind(now_us)
            .bind(now_us)
            .execute(&mut *tx)
            .await
            .map_err(database)?
        } else {
            sqlx::query(
                "UPDATE notification_webhooks
                 SET target_url=?,display_name=?,enabled=?,config_version=config_version+1,
                     updated_at_us=?
                 WHERE id=? AND config_version=?",
            )
            .bind(target_url)
            .bind(display_name)
            .bind(i64::from(enabled))
            .bind(now_us)
            .bind(id.as_bytes().to_vec())
            .bind(expected_version)
            .execute(&mut *tx)
            .await
            .map_err(database)?
        };
        if rows.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::ConfigVersionConflict,
                "webhook config version changed",
            ));
        }
        let version = sqlx::query_scalar::<_, i64>(
            "SELECT config_version FROM notification_webhooks WHERE id=?",
        )
        .bind(id.as_bytes().to_vec())
        .fetch_one(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(version)
    }

    /// 删除配置。
    ///
    /// # Errors
    ///
    /// 不存在、版本不匹配或数据库失败。
    pub async fn delete(&self, id: Uuid, expected_version: i64) -> Result<(), AppError> {
        let rows = sqlx::query("DELETE FROM notification_webhooks WHERE id=? AND config_version=?")
            .bind(id.as_bytes().to_vec())
            .bind(expected_version)
            .execute(&self.pool)
            .await
            .map_err(database)?;
        if rows.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::ConfigVersionConflict,
                "webhook config version changed",
            ));
        }
        Ok(())
    }

    /// 全部配置投影。
    ///
    /// # Errors
    ///
    /// 数据库失败。
    pub async fn list(&self) -> Result<Vec<WebhookView>, AppError> {
        let rows = sqlx::query(
            "SELECT id,target_url,display_name,enabled,config_version,health,last_delivery_us
             FROM notification_webhooks ORDER BY created_at_us",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database)?;
        rows.iter().map(decode).collect()
    }
}

fn decode(row: &sqlx::sqlite::SqliteRow) -> Result<WebhookView, AppError> {
    let health: String = row.try_get("health").map_err(database)?;
    let health = match health.as_str() {
        "unconfigured" => "unconfigured",
        "healthy" => "healthy",
        "degraded" => "degraded",
        "unavailable" => "unavailable",
        "unauthorized" => "unauthorized",
        _ => return Err(bad_state()),
    };
    let last_us: Option<i64> = row.try_get("last_delivery_us").map_err(database)?;
    Ok(WebhookView {
        id: Uuid::from_slice(
            row.try_get::<Vec<u8>, _>("id")
                .map_err(database)?
                .as_slice(),
        )
        .map_err(bad_uuid)?,
        target_url: row.try_get("target_url").map_err(database)?,
        display_name: row.try_get("display_name").map_err(database)?,
        enabled: row.try_get::<i64, _>("enabled").map_err(database)? == 1,
        config_version: row.try_get("config_version").map_err(database)?,
        health,
        last_delivery_at: last_us
            .and_then(|us| chrono::TimeZone::timestamp_micros(&chrono::Utc, us).single())
            .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)),
    })
}

/// 对候选目标执行不落盘的出站测试（固定探测载荷）。
///
/// # Errors
///
/// 目标不可达、非 2xx 或响应异常时返回稳定失败码。
pub async fn test_delivery(target_url: &str) -> Result<(), AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|_| AppError::new(ErrorCode::ProviderUnavailable, "webhook client unusable"))?;
    let response = client
        .post(target_url)
        .json(&serde_json::json!({
            "kind": "system.notice",
            "title": "MediaFlow Webhook 测试",
            "body": "这是一条不落盘的出站测试。",
        }))
        .send()
        .await
        .map_err(|_| AppError::new(ErrorCode::ProviderUnavailable, "webhook target unreachable"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorCode::ProviderUnavailable,
            "webhook target rejected the probe",
        ))
    }
}

fn bad_uuid(error: uuid::Error) -> AppError {
    AppError::with_source(ErrorCode::DatabaseInvalid, error)
}

fn bad_state() -> AppError {
    AppError::new(ErrorCode::DatabaseInvalid, "stored webhook is invalid")
}

fn database(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
