use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::shared::error::{AppError, ErrorCode};

/// 新收件箱的最小文件年龄，单位为秒。
pub const DEFAULT_MINIMUM_AGE_SECONDS: i64 = 60;
/// 两次计数观察之间的默认最小间隔，单位为秒。
pub const DEFAULT_STABLE_OBSERVATION_INTERVAL_SECONDS: i64 = 30;
/// 默认完整对账间隔，单位为秒。
pub const DEFAULT_RECONCILE_INTERVAL_SECONDS: i64 = 900;

/// 面向 API 和 revision 快照的版本化发现策略。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiscoveryPolicyView {
    /// 策略所属收件箱。
    pub inbox_directory_id: Uuid,
    /// 文件修改后至少等待的秒数。
    pub minimum_age_seconds: i64,
    /// 两次相同观察计数之间至少等待的秒数。
    pub stable_observation_interval_seconds: i64,
    /// 完整对账周期，单位为秒。
    pub reconcile_interval_seconds: i64,
    /// 是否允许文件系统 watcher 提供提示。
    pub watcher_enabled: bool,
    /// 乐观并发版本，从 1 开始。
    pub config_version: i64,
}

/// 完整替换一个收件箱发现策略的请求。
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutDiscoveryPolicyCommand {
    /// 文件修改后至少等待的秒数。
    pub minimum_age_seconds: i64,
    /// 两次相同观察计数之间至少等待的秒数。
    pub stable_observation_interval_seconds: i64,
    /// 完整对账周期，单位为秒。
    pub reconcile_interval_seconds: i64,
    /// 是否允许文件系统 watcher 提供提示。
    pub watcher_enabled: bool,
}

impl PutDiscoveryPolicyCommand {
    fn validate(&self) -> Result<(), AppError> {
        if !(0..=86_400).contains(&self.minimum_age_seconds)
            || !(1..=86_400).contains(&self.stable_observation_interval_seconds)
            || !(60..=604_800).contains(&self.reconcile_interval_seconds)
        {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "discovery policy is outside supported bounds",
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
/// 发现策略的持久化与乐观并发边界。
pub struct DiscoveryPolicyStore {
    pool: sqlx::SqlitePool,
}

impl DiscoveryPolicyStore {
    #[must_use]
    /// 创建绑定到数据库连接池的策略存储。
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    /// 加载策略；对直接创建或升级遗留的收件箱惰性补齐默认值。
    ///
    /// # Errors
    ///
    /// 收件箱不存在时返回 [`ErrorCode::InboxNotFound`]；数据库失败返回内部错误。
    pub async fn get(&self, inbox_id: Uuid) -> Result<DiscoveryPolicyView, AppError> {
        let mut connection = self.pool.acquire().await.map_err(internal)?;
        ensure_default_policy(
            &mut connection,
            inbox_id,
            chrono::Utc::now().timestamp_micros(),
        )
        .await?;
        load_policy(&mut connection, inbox_id).await
    }

    /// 在预期版本匹配时完整替换策略并递增版本。
    ///
    /// # Errors
    ///
    /// 值越界、收件箱不存在、版本冲突或数据库失败时返回 [`AppError`]。
    pub async fn put(
        &self,
        inbox_id: Uuid,
        command: PutDiscoveryPolicyCommand,
        expected_version: i64,
    ) -> Result<DiscoveryPolicyView, AppError> {
        command.validate()?;
        let now_us = chrono::Utc::now().timestamp_micros();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        ensure_default_policy(&mut tx, inbox_id, now_us).await?;
        let updated = sqlx::query(
            "UPDATE discovery_inbox_policies
             SET minimum_age_seconds=?,stable_observation_interval_seconds=?,
                 reconcile_interval_seconds=?,watcher_enabled=?,config_version=config_version+1,
                 updated_at_us=?
             WHERE inbox_directory_id=? AND config_version=?",
        )
        .bind(command.minimum_age_seconds)
        .bind(command.stable_observation_interval_seconds)
        .bind(command.reconcile_interval_seconds)
        .bind(i64::from(command.watcher_enabled))
        .bind(now_us)
        .bind(inbox_id.as_bytes().as_slice())
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        if updated.rows_affected() != 1 {
            tx.rollback().await.map_err(internal)?;
            return Err(AppError::new(
                ErrorCode::ConfigVersionConflict,
                "discovery policy version changed",
            ));
        }
        sqlx::query(
            "UPDATE discovery_watch_states
             SET health=CASE WHEN ?=0 THEN 'disabled'
                             WHEN health='disabled' THEN 'pending' ELSE health END,
                 watcher_active=CASE WHEN ?=0 THEN 0 ELSE watcher_active END,
                 next_reconcile_at_us=?,version=version+1,updated_at_us=?
             WHERE inbox_directory_id=?",
        )
        .bind(i64::from(command.watcher_enabled))
        .bind(i64::from(command.watcher_enabled))
        .bind(now_us.saturating_add(command.reconcile_interval_seconds.saturating_mul(1_000_000)))
        .bind(now_us)
        .bind(inbox_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        let policy = load_policy(&mut tx, inbox_id).await?;
        tx.commit().await.map_err(internal)?;
        Ok(policy)
    }
}

pub(crate) async fn ensure_default_policy(
    connection: &mut sqlx::SqliteConnection,
    inbox_id: Uuid,
    now_us: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO discovery_inbox_policies
         (inbox_directory_id,minimum_age_seconds,stable_observation_interval_seconds,
          reconcile_interval_seconds,watcher_enabled,config_version,created_at_us,updated_at_us)
         SELECT id,?,?,?,?,1,?,? FROM discovery_inbox_directories WHERE id=?
         ON CONFLICT(inbox_directory_id) DO NOTHING",
    )
    .bind(DEFAULT_MINIMUM_AGE_SECONDS)
    .bind(DEFAULT_STABLE_OBSERVATION_INTERVAL_SECONDS)
    .bind(DEFAULT_RECONCILE_INTERVAL_SECONDS)
    .bind(1_i64)
    .bind(now_us)
    .bind(now_us)
    .bind(inbox_id.as_bytes().as_slice())
    .execute(&mut *connection)
    .await
    .map_err(internal)?;
    sqlx::query(
        "INSERT INTO discovery_watch_states
         (inbox_directory_id,health,watcher_active,next_reconcile_at_us,version,created_at_us,updated_at_us)
         SELECT id,'pending',0,?,1,?,? FROM discovery_inbox_directories WHERE id=?
         ON CONFLICT(inbox_directory_id) DO NOTHING",
    )
    .bind(now_us)
    .bind(now_us)
    .bind(now_us)
    .bind(inbox_id.as_bytes().as_slice())
    .execute(&mut *connection)
    .await
    .map_err(internal)?;
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM discovery_inbox_policies WHERE inbox_directory_id=?",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .fetch_one(&mut *connection)
    .await
    .map_err(internal)?;
    if exists == 0 {
        return Err(AppError::new(
            ErrorCode::InboxNotFound,
            "discovery policy inbox not found",
        ));
    }
    Ok(())
}

pub(crate) async fn load_policy(
    connection: &mut sqlx::SqliteConnection,
    inbox_id: Uuid,
) -> Result<DiscoveryPolicyView, AppError> {
    let row = sqlx::query(
        "SELECT minimum_age_seconds,stable_observation_interval_seconds,
                reconcile_interval_seconds,watcher_enabled,config_version
         FROM discovery_inbox_policies WHERE inbox_directory_id=?",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .fetch_optional(&mut *connection)
    .await
    .map_err(internal)?
    .ok_or_else(|| AppError::new(ErrorCode::InboxNotFound, "discovery policy not found"))?;
    Ok(DiscoveryPolicyView {
        inbox_directory_id: inbox_id,
        minimum_age_seconds: row.get("minimum_age_seconds"),
        stable_observation_interval_seconds: row.get("stable_observation_interval_seconds"),
        reconcile_interval_seconds: row.get("reconcile_interval_seconds"),
        watcher_enabled: row.get::<i64, _>("watcher_enabled") != 0,
        config_version: row.get("config_version"),
    })
}

pub(crate) async fn load_or_ensure_policy(
    connection: &mut sqlx::SqliteConnection,
    inbox_id: Uuid,
    now_us: i64,
) -> Result<DiscoveryPolicyView, AppError> {
    match load_policy(connection, inbox_id).await {
        Ok(policy) => Ok(policy),
        Err(error) if error.code() == ErrorCode::InboxNotFound => {
            ensure_default_policy(connection, inbox_id, now_us).await?;
            load_policy(connection, inbox_id).await
        }
        Err(error) => Err(error),
    }
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
