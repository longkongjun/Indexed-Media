use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::identity::model::{AccountSummary, AuthenticatedSession, DeviceAccessMode};
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
            access_mode: DeviceAccessMode::Administrator,
            account: AccountSummary {
                id: Uuid::from_slice(&account_bytes)
                    .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
                administrator_name: display_name,
            },
            csrf_sha256: Some(csrf_sha256),
            principal: crate::identity::model::Principal::Session { session_id },
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

    // 上面是 impl 结束前追加注释错位，重新包一个 impl 块。
}

impl IdentityStore {
    /// 列出全部家庭成员。
    ///
    /// # Errors
    ///
    /// 数据库失败时返回稳定错误。
    pub async fn list_family_members(
        &self,
    ) -> Result<Vec<crate::identity::model::FamilyMemberView>, AppError> {
        let rows = sqlx::query(
            "SELECT id,display_name,created_at_us FROM family_members
             WHERE status='active' ORDER BY created_at_us",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        rows.iter()
            .map(|row| {
                let id: Vec<u8> = row.get("id");
                let created_us: i64 = row.get("created_at_us");
                Ok(crate::identity::model::FamilyMemberView {
                    id: Uuid::from_slice(&id)
                        .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
                    display_name: row.get("display_name"),
                    created_at: chrono::TimeZone::timestamp_micros(&chrono::Utc, created_us)
                        .single()
                        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    /// 创建一名家庭成员；显示名规范化后唯一。
    ///
    /// # Errors
    ///
    /// 名称无效、名称冲突或数据库失败时返回稳定错误。
    pub async fn insert_family_member(
        &self,
        display_name: &str,
        password_phc: &str,
        now_us: i64,
    ) -> Result<crate::identity::model::FamilyMemberView, AppError> {
        let id = Uuid::now_v7();
        let normalized = display_name.trim().to_lowercase();
        let trimmed = display_name.trim();
        if normalized.is_empty() || normalized.len() > 100 {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "member name is invalid",
            ));
        }
        let rows = sqlx::query(
            "INSERT INTO family_members (id,normalized_name,display_name,password_phc,status,created_at_us,updated_at_us)
             VALUES (?,?,?,?,'active',?,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(&normalized)
        .bind(trimmed)
        .bind(password_phc)
        .bind(now_us)
        .bind(now_us)
        .execute(&self.pool)
        .await
        .map_err(|_| AppError::new(ErrorCode::ResourceConflict, "member name is taken"))?;
        if rows.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::ResourceConflict,
                "member name is taken",
            ));
        }
        Ok(crate::identity::model::FamilyMemberView {
            id,
            display_name: trimmed.to_owned(),
            created_at: chrono::TimeZone::timestamp_micros(&chrono::Utc, now_us)
                .single()
                .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
                .unwrap_or_default(),
        })
    }

    /// 删除（停用）一名家庭成员。
    ///
    /// # Errors
    ///
    /// 目标不存在或数据库失败时返回稳定错误。
    pub async fn delete_family_member(&self, id: Uuid, now_us: i64) -> Result<(), AppError> {
        let rows = sqlx::query(
            "UPDATE family_members SET status='disabled',updated_at_us=? WHERE id=? AND status='active'",
        )
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        if rows.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::NotFound,
                "family member not found",
            ));
        }
        Ok(())
    }
}

/// 设备行中需要暴露给管理面或认证结果的结构化材料。
pub struct DeviceRecord {
    /// 服务端实际执行的持久化访问能力。
    pub access_mode: DeviceAccessMode,
    /// 设备稳定 UUID。
    pub id: Uuid,
    /// 拥有设备的账户 UUID。
    pub account_id: Uuid,
    /// enrollment 时提供的显示名称。
    pub display_name: String,
    /// 令牌绝对过期时间（微秒）。
    pub absolute_expires_at_us: i64,
    /// 创建时间（微秒）。
    pub created_at_us: i64,
    /// 最近一次认证时间（微秒）；从未使用为 `None`。
    pub last_used_at_us: Option<i64>,
    /// 撤销时间（微秒）；未撤销为 `None`。
    pub revoked_at_us: Option<i64>,
}

impl IdentityStore {
    #[allow(clippy::too_many_arguments)]
    /// 插入新设备行；仅存令牌 SHA-256 摘要。
    ///
    /// # Errors
    ///
    /// `SQLite` 拒绝插入时返回 [`AppError`]。
    pub async fn insert_device(
        &self,
        access_mode: DeviceAccessMode,
        id: Uuid,
        account_id: Uuid,
        display_name: &str,
        token_hash: &[u8],
        credential_version: i64,
        absolute_expires_at_us: i64,
        now_us: i64,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO identity_devices
             (id,account_id,display_name,token_sha256,credential_version,absolute_expires_at_us,created_at_us,access_mode)
             VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(display_name)
        .bind(token_hash)
        .bind(credential_version)
        .bind(absolute_expires_at_us)
        .bind(now_us)
        .bind(access_mode.as_str())
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        Ok(())
    }

    /// 以令牌摘要认证设备并更新 `last_used_at_us`（60 秒节流，避免高频认证/SSE 周期
    /// 重认证造成写放大）。
    ///
    /// 设备令牌无空闲过期：仅检查未撤销、未到绝对过期、账户活跃且凭据版本一致
    /// （管理员改密后设备需重新配对）。
    ///
    /// # Errors
    ///
    /// 无匹配的有效设备时返回 [`ErrorCode::SessionExpired`]；查询失败或持久化字节格式
    /// 错误时返回内部 [`AppError`]。
    pub async fn authenticate_device(
        &self,
        token_hash: &[u8],
        now_us: i64,
    ) -> Result<crate::identity::model::AuthenticatedSession, AppError> {
        const TOUCH_INTERVAL_US: i64 = 60 * 1_000_000;
        let row = sqlx::query(
            "UPDATE identity_devices
             SET last_used_at_us=CASE
                 WHEN last_used_at_us IS NULL OR ?-last_used_at_us>=? THEN ?
                 ELSE last_used_at_us END
             WHERE token_sha256=? AND revoked_at_us IS NULL AND absolute_expires_at_us>?
               AND EXISTS (
                 SELECT 1 FROM identity_accounts a
                 WHERE a.id=identity_devices.account_id AND a.status='active'
                   AND a.credential_version=identity_devices.credential_version
               )
             RETURNING id,account_id,access_mode",
        )
        .bind(now_us)
        .bind(TOUCH_INTERVAL_US)
        .bind(now_us)
        .bind(token_hash)
        .bind(now_us)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?
        .ok_or_else(|| {
            AppError::new(ErrorCode::SessionExpired, "device token is no longer valid")
        })?;
        session_from_device_row(&self.pool, row).await
    }

    /// 读取单个设备行（含已撤销）；管理面与自撤销/自轮换使用。
    ///
    /// # Errors
    ///
    /// 设备不存在或数据库失败时返回 [`AppError`]。
    pub async fn device(&self, device_id: Uuid) -> Result<DeviceRecord, AppError> {
        let row = sqlx::query(
            "SELECT id,account_id,display_name,absolute_expires_at_us,created_at_us,last_used_at_us,revoked_at_us,access_mode
             FROM identity_devices WHERE id=?",
        )
        .bind(device_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "device not found"))?;
        device_record_from_row(&row)
    }

    /// 列出全部设备（含已撤销），按创建时间升序。
    ///
    /// # Errors
    ///
    /// 数据库失败时返回 [`AppError`]。
    pub async fn list_devices(&self) -> Result<Vec<DeviceRecord>, AppError> {
        let rows = sqlx::query(
            "SELECT id,account_id,display_name,absolute_expires_at_us,created_at_us,last_used_at_us,revoked_at_us,access_mode
             FROM identity_devices ORDER BY created_at_us",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        rows.iter().map(device_record_from_row).collect()
    }

    /// 撤销设备令牌（幂等；已撤销的行保持原撤销时间）。
    ///
    /// # Errors
    ///
    /// `SQLite` 无法执行更新时返回 [`AppError`]。
    pub async fn revoke_device(
        &self,
        device_id: Uuid,
        now_us: i64,
        reason: &str,
    ) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE identity_devices SET revoked_at_us=?,revoked_reason=?
             WHERE id=? AND revoked_at_us IS NULL",
        )
        .bind(now_us)
        .bind(reason)
        .bind(device_id.as_bytes().as_slice())
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        Ok(())
    }

    /// 原子替换未撤销、未过期设备的令牌摘要并延长绝对过期。
    ///
    /// # Errors
    ///
    /// 设备缺失、已撤销或已过期时返回 [`ErrorCode::SessionExpired`]；数据库失败返回
    /// 内部 [`AppError`]。
    pub async fn rotate_device_token(
        &self,
        device_id: Uuid,
        new_token_hash: &[u8],
        new_absolute_expires_at_us: i64,
    ) -> Result<(), AppError> {
        let result = sqlx::query(
            "UPDATE identity_devices
             SET token_sha256=?,absolute_expires_at_us=?
             WHERE id=? AND revoked_at_us IS NULL AND absolute_expires_at_us>?",
        )
        .bind(new_token_hash)
        .bind(new_absolute_expires_at_us)
        .bind(device_id.as_bytes().as_slice())
        .bind(chrono::Utc::now().timestamp_micros())
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        if result.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::SessionExpired,
                "device token is no longer valid",
            ));
        }
        Ok(())
    }

    /// 插入一次性配对码；仅存规范化码的 SHA-256 摘要。
    ///
    /// # Errors
    ///
    /// `SQLite` 拒绝插入时返回 [`AppError`]。
    pub async fn insert_pairing_code(
        &self,
        id: Uuid,
        account_id: Uuid,
        code_hash: &[u8],
        now_us: i64,
        expires_at_us: i64,
        access_mode: DeviceAccessMode,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO identity_pairing_codes (id,account_id,code_sha256,created_at_us,expires_at_us,access_mode)
             VALUES (?,?,?,?,?,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(code_hash)
        .bind(now_us)
        .bind(expires_at_us)
        .bind(access_mode.as_str())
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        Ok(())
    }

    /// 原子消费一次性配对码：仅当未使用且未过期时置 `used_at_us` 并返回签发账户。
    ///
    /// # Errors
    ///
    /// 码未知、已使用或已过期时返回 [`ErrorCode::InvalidCredentials`]；数据库失败返回
    /// 内部 [`AppError`]。
    pub async fn consume_pairing_code(
        &self,
        code_hash: &[u8],
        now_us: i64,
        requested_mode: DeviceAccessMode,
    ) -> Result<Uuid, AppError> {
        let row = sqlx::query(
            "UPDATE identity_pairing_codes SET used_at_us=?
             WHERE code_sha256=? AND used_at_us IS NULL AND expires_at_us>?
             AND (access_mode='administrator' OR ?='media-read-only')
             RETURNING account_id",
        )
        .bind(now_us)
        .bind(code_hash)
        .bind(now_us)
        .bind(requested_mode.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        let Some(row) = row else {
            let valid_code: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM identity_pairing_codes
                 WHERE code_sha256=? AND used_at_us IS NULL AND expires_at_us>?)",
            )
            .bind(code_hash)
            .bind(now_us)
            .fetch_one(&self.pool)
            .await
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
            return Err(if valid_code {
                AppError::new(
                    ErrorCode::PermissionDenied,
                    "pairing code cannot grant requested access",
                )
            } else {
                AppError::new(
                    ErrorCode::InvalidCredentials,
                    "pairing code is unknown, expired, or already used",
                )
            });
        };
        let account_bytes: Vec<u8> = row.get("account_id");
        Uuid::from_slice(&account_bytes)
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))
    }

    /// 读取账户当前的凭据版本；账户不存在或非活跃时为 `None`。
    ///
    /// # Errors
    ///
    /// 数据库失败时返回 [`AppError`]。
    pub async fn account_credential_version(
        &self,
        account_id: Uuid,
    ) -> Result<Option<i64>, AppError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT credential_version FROM identity_accounts WHERE id=? AND status='active'",
        )
        .bind(account_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
    }
}

async fn session_from_device_row(
    pool: &SqlitePool,
    row: sqlx::sqlite::SqliteRow,
) -> Result<crate::identity::model::AuthenticatedSession, AppError> {
    let device_bytes: Vec<u8> = row.get("id");
    let account_bytes: Vec<u8> = row.get("account_id");
    let device_id = Uuid::from_slice(&device_bytes)
        .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
    let display_name: String =
        sqlx::query_scalar("SELECT display_name FROM identity_accounts WHERE id=?")
            .bind(&account_bytes)
            .fetch_one(pool)
            .await
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
    Ok(crate::identity::model::AuthenticatedSession {
        access_mode: device_access_mode(row.get::<&str, _>("access_mode"))?,
        account: crate::identity::model::AccountSummary {
            id: Uuid::from_slice(&account_bytes)
                .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
            administrator_name: display_name,
        },
        csrf_sha256: None,
        principal: crate::identity::model::Principal::Device { device_id },
    })
}

fn device_record_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<DeviceRecord, AppError> {
    let id: Vec<u8> = row.get("id");
    let account_id: Vec<u8> = row.get("account_id");
    Ok(DeviceRecord {
        access_mode: device_access_mode(row.get::<&str, _>("access_mode"))?,
        id: Uuid::from_slice(&id)
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
        account_id: Uuid::from_slice(&account_id)
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
        display_name: row.get("display_name"),
        absolute_expires_at_us: row.get("absolute_expires_at_us"),
        created_at_us: row.get("created_at_us"),
        last_used_at_us: row.get("last_used_at_us"),
        revoked_at_us: row.get("revoked_at_us"),
    })
}

fn device_access_mode(value: &str) -> Result<DeviceAccessMode, AppError> {
    match value {
        "administrator" => Ok(DeviceAccessMode::Administrator),
        "media-read-only" => Ok(DeviceAccessMode::MediaReadOnly),
        _ => Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "invalid device access mode",
        )),
    }
}
