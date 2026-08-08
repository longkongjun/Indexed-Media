use sqlx::SqlitePool;

use crate::shared::error::{AppError, ErrorCode};
use crate::shared::id::new_id;

/// 按稳定的决策 ID 恰好记录一次人工决定审计操作。
///
/// 消费者回执与安全审计行在同一立即事务中提交；若提交前失败，两者都会回滚，后续重放可重新写入；
/// 提交成功后的重放不会产生重复回执或审计行。
///
/// # Errors
///
/// 无法提交回执或审计事务时返回 [`AppError`]。
pub async fn record_manual_decision_once(
    pool: &SqlitePool,
    decision_id: uuid::Uuid,
    now_us: i64,
) -> Result<(), AppError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.map_err(internal)?;
    let receipt = sqlx::query(
        "INSERT INTO platform_event_consumer_receipts(consumer,decision_id,consumed_at_us)
         VALUES ('audit',?,?) ON CONFLICT(consumer,decision_id) DO NOTHING",
    )
    .bind(decision_id.as_bytes().as_slice())
    .bind(now_us)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;
    if receipt.rows_affected() == 1 {
        sqlx::query(
            "INSERT INTO platform_audit_events
             (id,occurred_at_us,category,action,outcome,subject_id,safe_details_json)
             VALUES (?,?,'identification','identification.manual-decision.accepted',
                     'success',?,'{}')",
        )
        .bind(new_id().as_bytes().as_slice())
        .bind(now_us)
        .bind(decision_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    }
    tx.commit().await.map_err(internal)
}

/// 追加一条不可变的身份/安全审计事件，并可选提供安全的主体标识符。
///
/// 此行会获得生成的 `UUIDv7` 和当前 UTC 微秒时间戳。`action`、`outcome` 和 `subject_id`
/// 会按原样持久化；调用方不得传入密钥或主机路径。
///
/// # Errors
///
/// 若 `SQLite` 无法插入审计行，则返回 [`AppError`]。出错时不会暴露部分写入的行。
pub async fn record(
    pool: &SqlitePool,
    action: &str,
    outcome: &str,
    subject_id: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query("INSERT INTO platform_audit_events (id, occurred_at_us, category, action, outcome, subject_id, safe_details_json) VALUES (?, ?, 'identity', ?, ?, ?, '{}')")
        .bind(new_id().as_bytes().as_slice())
        .bind(chrono::Utc::now().timestamp_micros())
        .bind(action)
        .bind(outcome)
        .bind(subject_id)
        .execute(pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
    Ok(())
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
