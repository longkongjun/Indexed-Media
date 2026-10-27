//! 通知中心：应用内通知的唯一必达通道与可选出站 Webhook 配置。
//!
//! 通知在领域事务内写入（与状态转换原子提交）；Webhook 是附加投递面，配置、健康与
//! 测试独立维护，其失败绝不影响本地任务或应用内投递。

use sqlx::Row;
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use crate::shared::error::{AppError, ErrorCode};

/// 一条已持久化的应用内通知投影。
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NotificationView {
    /// 通知稳定 ID。
    pub id: Uuid,
    /// 稳定通知类别。
    pub kind: &'static str,
    /// 管理员可读标题。
    pub title: String,
    /// 脱敏正文（不含路径/凭据）。
    pub body: String,
    /// 可选关联任务。
    pub task_id: Option<Uuid>,
    /// 创建时间 UTC RFC3339。
    pub created_at: String,
    /// 已读时间；未读为 `None`。
    pub read_at: Option<String>,
}

/// 通知类别与任务关联的构造约束。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationKind {
    /// 任务等待人工审核。
    ReviewWaiting,
    /// 整理流程安全暂停。
    OrganizationPaused,
    /// 整理结果已回滚。
    OrganizationRolledBack,
    /// 系统级通知。
    SystemNotice,
}

impl NotificationKind {
    /// 线上稳定类别值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReviewWaiting => "review.waiting",
            Self::OrganizationPaused => "organization.paused",
            Self::OrganizationRolledBack => "organization.rolled-back",
            Self::SystemNotice => "system.notice",
        }
    }
}

/// 应用内通知存储。
#[derive(Clone)]
pub struct NotificationStore {
    pool: SqlitePool,
}

impl NotificationStore {
    /// 创建存储。
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 在既有事务内写入一条通知（与领域状态转换原子提交）。
    ///
    /// # Errors
    ///
    /// 输入越界或写入失败时返回稳定错误。
    pub async fn insert_in_transaction(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        kind: NotificationKind,
        title: &str,
        body: &str,
        task_id: Option<Uuid>,
        now_us: i64,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::now_v7();
        let task_bound = matches!(
            kind,
            NotificationKind::ReviewWaiting
                | NotificationKind::OrganizationPaused
                | NotificationKind::OrganizationRolledBack
        );
        if (task_bound != task_id.is_some())
            || title.is_empty()
            || title.len() > 200
            || body.is_empty()
            || body.len() > 2000
        {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "notification payload is invalid",
            ));
        }
        sqlx::query(
            "INSERT INTO notifications (id,kind,title,body,task_id,created_at_us,read_at_us)
             VALUES (?,?,?,?,?,?,NULL)",
        )
        .bind(id.as_bytes().to_vec())
        .bind(kind.as_str())
        .bind(title)
        .bind(body)
        .bind(task_id.map(|id| id.as_bytes().to_vec()))
        .bind(now_us)
        .execute(&mut **transaction)
        .await
        .map_err(database)?;
        Ok(id)
    }

    /// 最新在前的通知分页；`unread_only` 过滤未读，时间/ID边界与排序一致。
    ///
    /// # Errors
    ///
    /// 数据库失败时返回稳定错误。
    pub async fn list(
        &self,
        unread_only: bool,
        before_us: Option<i64>,
        before_id: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<NotificationView>, Option<String>), AppError> {
        let limit = limit.clamp(1, 100);
        let rows = sqlx::query(
            "SELECT id,kind,title,body,task_id,created_at_us,read_at_us
             FROM notifications
             WHERE (?1 = 0 OR read_at_us IS NULL)
               AND (?2 IS NULL OR created_at_us < ?2 OR (created_at_us = ?2 AND id < ?3))
             ORDER BY created_at_us DESC, id DESC LIMIT ?4",
        )
        .bind(i64::from(unread_only))
        .bind(before_us)
        .bind(before_id.map(|id| id.as_bytes().to_vec()))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(database)?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(decode(&row)?);
        }
        let next_cursor = if i64::try_from(items.len()).unwrap_or(i64::MAX) < limit {
            None
        } else {
            items
                .last()
                .map(|item| format!("notifications:{}:{}", item.created_at, item.id))
        };
        Ok((items, next_cursor))
    }

    /// 标记单条已读（幂等）。
    ///
    /// # Errors
    ///
    /// 通知不存在或数据库失败。
    pub async fn mark_read(&self, id: Uuid, now_us: i64) -> Result<(), AppError> {
        let rows =
            sqlx::query("UPDATE notifications SET read_at_us=COALESCE(read_at_us,?) WHERE id=?")
                .bind(now_us)
                .bind(id.as_bytes().to_vec())
                .execute(&self.pool)
                .await
                .map_err(database)?;
        if rows.rows_affected() != 1 {
            return Err(AppError::new(ErrorCode::NotFound, "notification not found"));
        }
        Ok(())
    }

    /// 全部标记已读（幂等）；返回受影响行数。
    ///
    /// # Errors
    ///
    /// 数据库失败。
    pub async fn mark_all_read(&self, now_us: i64) -> Result<u64, AppError> {
        let rows = sqlx::query(
            "UPDATE notifications SET read_at_us=COALESCE(read_at_us,?) WHERE read_at_us IS NULL",
        )
        .bind(now_us)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        Ok(rows.rows_affected())
    }

    /// 未读计数。
    ///
    /// # Errors
    ///
    /// 数据库失败。
    pub async fn unread_count(&self) -> Result<i64, AppError> {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notifications WHERE read_at_us IS NULL")
            .fetch_one(&self.pool)
            .await
            .map_err(database)
    }
}

fn decode(row: &sqlx::sqlite::SqliteRow) -> Result<NotificationView, AppError> {
    let kind: String = row.try_get("kind").map_err(database)?;
    let kind = match kind.as_str() {
        "review.waiting" => "review.waiting",
        "organization.paused" => "organization.paused",
        "organization.rolled-back" => "organization.rolled-back",
        "system.notice" => "system.notice",
        _ => return Err(bad_state()),
    };
    let task_id: Option<Vec<u8>> = row.try_get("task_id").map_err(database)?;
    let created_us: i64 = row.try_get("created_at_us").map_err(database)?;
    let read_us: Option<i64> = row.try_get("read_at_us").map_err(database)?;
    let time = |us: i64| {
        chrono::TimeZone::timestamp_micros(&chrono::Utc, us)
            .single()
            .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
            .unwrap_or_default()
    };
    Ok(NotificationView {
        id: Uuid::from_slice(
            row.try_get::<Vec<u8>, _>("id")
                .map_err(database)?
                .as_slice(),
        )
        .map_err(bad_uuid)?,
        kind,
        title: row.try_get("title").map_err(database)?,
        body: row.try_get("body").map_err(database)?,
        task_id: task_id.and_then(|bytes| Uuid::from_slice(&bytes).ok()),
        created_at: time(created_us),
        read_at: read_us.map(time),
    })
}

fn bad_uuid(error: uuid::Error) -> AppError {
    AppError::with_source(ErrorCode::DatabaseInvalid, error)
}

fn bad_state() -> AppError {
    AppError::new(ErrorCode::DatabaseInvalid, "stored notification is invalid")
}

fn database(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
