use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::identity::model::{AccountSummary, AuthenticatedSession};
use crate::shared::error::{AppError, ErrorCode};

#[derive(Clone)]
/// 单例管理员、会话和登录限流的 `SQLite` 持久化边界。
pub struct IdentityStore {
    pool: SqlitePool,
}

/// 用于在内部验证登录候选项的活跃账户材料。
pub struct AccountCredential {
    /// 稳定的账户标识符。
    pub id: Uuid,
    /// 原始的、已去除空白的管理员显示名称。
    pub display_name: String,
    /// PHC 编码的密码哈希；此值绝不能跨越 API 边界。
    pub password_phc: String,
    /// 复制到会话中的版本，使凭据变更可使旧会话失效。
    pub credential_version: i64,
}

impl IdentityStore {
    #[must_use]
    /// 将身份持久化绑定至 `pool`，但不打开事务。
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    #[must_use]
    /// 借用供身份及相邻审计操作使用的连接池。
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 单例账户表为空时返回 `true`。
    ///
    /// # Errors
    ///
    /// `SQLite` 无法执行计数查询时返回 [`AppError`]。
    pub async fn requires_initialization(&self) -> Result<bool, AppError> {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identity_accounts")
            .fetch_one(&self.pool)
            .await
            .map(|count| count == 0)
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
    }

    /// 在立即事务中插入单例管理员账户。
    ///
    /// 事务会在成功时原子提交，并在插入失败时尝试回滚。
    ///
    /// # Errors
    ///
    /// 单例唯一性冲突时返回 [`ErrorCode::BootstrapAlreadyCompleted`]；获取连接、事务控制或插入失败时返回
    /// [`ErrorCode::Internal`]。
    pub async fn insert_account(
        &self,
        id: Uuid,
        normalized: &str,
        display: &str,
        phc: &str,
        now: i64,
    ) -> Result<AccountSummary, AppError> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        let result = sqlx::query("INSERT INTO identity_accounts (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us) VALUES (1,?,?,?,?,?,?)")
            .bind(id.as_bytes().as_slice()).bind(normalized).bind(display).bind(phc).bind(now).bind(now).execute(&mut *connection).await;
        match result {
            Ok(_) => {
                sqlx::query("COMMIT")
                    .execute(&mut *connection)
                    .await
                    .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
                Ok(AccountSummary {
                    id,
                    administrator_name: display.to_owned(),
                })
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                if error
                    .as_database_error()
                    .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
                {
                    Err(AppError::new(
                        ErrorCode::BootstrapAlreadyCompleted,
                        "singleton account exists",
                    ))
                } else {
                    Err(AppError::with_source(ErrorCode::Internal, error))
                }
            }
        }
    }

    /// 加载规范化管理员名称的活跃凭据材料。
    ///
    /// # Errors
    ///
    /// 查询失败或持久化 UUID 格式错误时返回 [`AppError`]。
    pub async fn account_credential(
        &self,
        normalized: &str,
    ) -> Result<Option<AccountCredential>, AppError> {
        let row = sqlx::query("SELECT id,display_name,password_phc,credential_version FROM identity_accounts WHERE normalized_name=? AND status='active'")
            .bind(normalized).fetch_optional(&self.pool).await.map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        row.map(|row| {
            let id: Vec<u8> = row.get("id");
            Ok(AccountCredential {
                id: Uuid::from_slice(&id)
                    .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
                display_name: row.get("display_name"),
                password_phc: row.get("password_phc"),
                credential_version: row.get("credential_version"),
            })
        })
        .transpose()
    }

    #[allow(clippy::too_many_arguments)]
    /// 使用令牌/CSRF 摘要和微秒过期时间戳持久化会话。
    ///
    /// 原始 Bearer 或 CSRF 令牌不得传给 `token_hash` 或 `csrf_hash`。
    ///
    /// # Errors
    ///
    /// `SQLite` 拒绝或无法执行插入时返回 [`AppError`]。
    pub async fn insert_session(
        &self,
        id: Uuid,
        account_id: Uuid,
        token_hash: &[u8],
        csrf_hash: &[u8],
        idle: i64,
        absolute: i64,
        credential_version: i64,
        now: i64,
    ) -> Result<(), AppError> {
        sqlx::query("INSERT INTO identity_sessions (id,account_id,token_sha256,csrf_sha256,idle_expires_at_us,absolute_expires_at_us,credential_version,last_used_at_us,created_at_us) VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(id.as_bytes().as_slice()).bind(account_id.as_bytes().as_slice()).bind(token_hash).bind(csrf_hash).bind(idle).bind(absolute).bind(credential_version).bind(now).bind(now)
            .execute(&self.pool).await.map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        Ok(())
    }

    /// 加载 `token_hash` 的活跃会话，并可选择将其空闲截止时间滑动 30 分钟。
    ///
    /// 使用 `slide_idle` 时，验证与时间戳更新是同一条 `UPDATE ... RETURNING` 语句。两种模式下账户都必须
    /// 处于活跃状态，且其凭据版本未变化。
    ///
    /// # Errors
    ///
    /// 没有匹配的有效会话时返回 [`ErrorCode::SessionExpired`]。查询失败或持久化 UUID/CSRF 字节格式错误时返回
    /// 内部 [`AppError`]。
    pub async fn authenticate(
        &self,
        token_hash: &[u8],
        now: i64,
        slide_idle: bool,
    ) -> Result<AuthenticatedSession, AppError> {
        let row = if slide_idle {
            sqlx::query(
                "UPDATE identity_sessions
                 SET idle_expires_at_us=MIN(?,absolute_expires_at_us),last_used_at_us=?
                 WHERE token_sha256=? AND revoked_at_us IS NULL
                   AND idle_expires_at_us>? AND absolute_expires_at_us>?
                   AND EXISTS (
                     SELECT 1 FROM identity_accounts a
                     WHERE a.id=identity_sessions.account_id AND a.status='active'
                       AND a.credential_version=identity_sessions.credential_version
                   )
                 RETURNING id,account_id,csrf_sha256",
            )
            .bind(now + 30 * 60 * 1_000_000)
            .bind(now)
            .bind(token_hash)
            .bind(now)
            .bind(now)
            .fetch_optional(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT s.id,s.account_id,s.csrf_sha256
                 FROM identity_sessions s
                 WHERE s.token_sha256=? AND s.revoked_at_us IS NULL
                   AND s.idle_expires_at_us>? AND s.absolute_expires_at_us>?
                   AND EXISTS (
                     SELECT 1 FROM identity_accounts a
                     WHERE a.id=s.account_id AND a.status='active'
                       AND a.credential_version=s.credential_version
                   )",
            )
            .bind(token_hash)
            .bind(now)
            .bind(now)
            .fetch_optional(&self.pool)
            .await
        }
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?
        .ok_or_else(|| AppError::new(ErrorCode::SessionExpired, "session is no longer valid"))?;
        let id_bytes: Vec<u8> = row.get("id");
        let account_bytes: Vec<u8> = row.get("account_id");
        let csrf: Vec<u8> = row.get("csrf_sha256");
        let session_id = Uuid::from_slice(&id_bytes)
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
        let display_name: String =
            sqlx::query_scalar("SELECT display_name FROM identity_accounts WHERE id=?")
                .bind(&account_bytes)
                .fetch_one(&self.pool)
                .await
                .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        let csrf_sha256: [u8; 32] = csrf
            .try_into()
            .map_err(|_| AppError::new(ErrorCode::Internal, "invalid csrf digest"))?;
        Ok(AuthenticatedSession {
            session_id,
            account: AccountSummary {
                id: Uuid::from_slice(&account_bytes)
                    .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
                administrator_name: display_name,
            },
            csrf_sha256,
        })
    }

    /// 在 `now` 将未撤销会话标记为已退出登录。
    ///
    /// 缺失或已撤销的会话会成功地不执行操作。
    ///
    /// # Errors
    ///
    /// `SQLite` 无法执行更新时返回 [`AppError`]。
    pub async fn revoke(&self, session_id: Uuid, now: i64) -> Result<(), AppError> {
        sqlx::query("UPDATE identity_sessions SET revoked_at_us=?,revoked_reason='logout' WHERE id=? AND revoked_at_us IS NULL").bind(now).bind(session_id.as_bytes().as_slice()).execute(&self.pool).await.map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        Ok(())
    }

    /// 替换未撤销会话的 CSRF 摘要。
    ///
    /// 缺失或已撤销的会话会成功地不执行操作；调用者应先认证。
    ///
    /// # Errors
    ///
    /// `SQLite` 无法执行更新时返回 [`AppError`]。
    pub async fn replace_csrf(&self, session_id: Uuid, csrf_hash: &[u8]) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE identity_sessions SET csrf_sha256=? WHERE id=? AND revoked_at_us IS NULL",
        )
        .bind(csrf_hash)
        .bind(session_id.as_bytes().as_slice())
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        Ok(())
    }

    /// 若有效，返回哈希候选项/来源对的未来重试截止时间。
    ///
    /// 已过期或不存在的限流行会产生 `None`，且不会被变更。
    ///
    /// # Errors
    ///
    /// `SQLite` 无法读取限流行时返回 [`AppError`]。
    pub async fn throttle_state(
        &self,
        candidate: &[u8],
        source: &[u8],
        now: i64,
    ) -> Result<Option<i64>, AppError> {
        sqlx::query_scalar::<_, Option<i64>>("SELECT retry_after_us FROM identity_login_throttles WHERE candidate_sha256=? AND source_sha256=?")
            .bind(candidate).bind(source).fetch_optional(&self.pool).await.map(|value| value.flatten().filter(|retry| *retry > now)).map_err(|error| AppError::with_source(ErrorCode::Internal, error))
    }

    /// 在持久化的 15 分钟窗口内记录一次失败登录。
    ///
    /// 失败五次时，该对会锁定五分钟，返回的时间戳是重试截止时间；更低计数返回 `None`。
    ///
    /// # Errors
    ///
    /// 任意限流插入、计数查询或锁定更新失败时返回 [`AppError`]。这三条语句不会封装在调用者可见的事务中。
    pub async fn record_failure(
        &self,
        candidate: &[u8],
        source: &[u8],
        now: i64,
    ) -> Result<Option<i64>, AppError> {
        let window = 15 * 60 * 1_000_000_i64;
        let lock = 5 * 60 * 1_000_000_i64;
        sqlx::query("INSERT INTO identity_login_throttles (candidate_sha256,source_sha256,window_started_at_us,failure_count,retry_after_us,updated_at_us) VALUES (?,?,?,1,NULL,?) ON CONFLICT(candidate_sha256,source_sha256) DO UPDATE SET window_started_at_us=CASE WHEN ?-window_started_at_us>? THEN ? ELSE window_started_at_us END,failure_count=CASE WHEN ?-window_started_at_us>? THEN 1 ELSE failure_count+1 END,updated_at_us=?")
            .bind(candidate).bind(source).bind(now).bind(now).bind(now).bind(window).bind(now).bind(now).bind(window).bind(now).execute(&self.pool).await.map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        let count: i64 = sqlx::query_scalar("SELECT failure_count FROM identity_login_throttles WHERE candidate_sha256=? AND source_sha256=?").bind(candidate).bind(source).fetch_one(&self.pool).await.map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        if count >= 5 {
            let retry = now + lock;
            sqlx::query("UPDATE identity_login_throttles SET retry_after_us=? WHERE candidate_sha256=? AND source_sha256=?").bind(retry).bind(candidate).bind(source).execute(&self.pool).await.map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
            Ok(Some(retry))
        } else {
            Ok(None)
        }
    }

    /// 删除成功认证的候选项/来源对的持久化限流状态。
    ///
    /// # Errors
    ///
    /// `SQLite` 无法执行删除时返回 [`AppError`]。
    pub async fn clear_throttle(&self, candidate: &[u8], source: &[u8]) -> Result<(), AppError> {
        sqlx::query(
            "DELETE FROM identity_login_throttles WHERE candidate_sha256=? AND source_sha256=?",
        )
        .bind(candidate)
        .bind(source)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        Ok(())
    }
}
