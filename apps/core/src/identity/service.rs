use std::path::{Path, PathBuf};

use async_trait::async_trait;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::identity::model::{
    AccountSummary, AuthenticatedSession, BootstrapCommand, LoginCommand, NewSession,
};
use crate::identity::store::IdentityStore;
use crate::platform::password::{PasswordEngine, configured_time_cost};
use crate::platform::{audit, random};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::id::new_id;

#[async_trait]
/// 涉及安全性的管理员引导和会话操作。
pub trait IdentityUseCases: Send + Sync {
    /// 创建唯一管理员账户，并消费文件支持的引导秘密值。
    ///
    /// 成功调用会提交账户、追加成功审计记录，并尽力移除秘密文件。秘密值不匹配会追加拒绝审计记录。
    /// 账户插入会在成功审计插入前提交；若审计插入失败，即使账户已存在，此方法仍返回错误，且不会移除秘密文件。
    ///
    /// # Errors
    ///
    /// 引导已完成、秘密源或权限无效、秘密值不匹配、名称/密码策略不通过、密码哈希失败或任意必需
    /// 数据库/审计操作失败时返回 [`AppError`]。
    async fn bootstrap(&self, command: BootstrapCommand) -> Result<AccountSummary, AppError>;
    /// 验证管理员凭据并持久化新的有期限会话。
    ///
    /// 登录失败会更新持久化来源/候选限流并追加拒绝审计行；成功会清除限流、仅存储令牌摘要并追加成功审计行。
    /// 限流变更和会话插入会在随后的审计插入前独立提交。后续审计失败不会回滚这些变更。特别是，成功审计失败时，
    /// 虽然会话行已存在，该方法仍会返回错误，且不会泄露原始 Bearer 或 CSRF 令牌。
    ///
    /// # Errors
    ///
    /// 存在有效限流、凭据无效、密码/令牌生成失败或任意必需数据库/审计操作失败时返回 [`AppError`]。
    async fn create_session(&self, command: LoginCommand) -> Result<NewSession, AppError>;
    /// 认证原始会话令牌，并将其空闲截止时间滑动至绝对截止时间以内。
    ///
    /// # Errors
    ///
    /// 令牌未知、已过期、已撤销、绑定到过期凭据，或无法读取/更新会话/账户行时返回 [`AppError`]。
    async fn authenticate(&self, raw_token: &str) -> Result<AuthenticatedSession, AppError>;
    /// 将会话作为退出登录撤销，并追加成功审计记录。
    ///
    /// 会话已撤销或不存在时，撤销操作具有幂等性。撤销更新会在审计插入前提交，因此审计失败会返回错误，
    /// 但会话仍保持撤销状态。
    ///
    /// # Errors
    ///
    /// 数据库更新或审计插入失败时返回 [`AppError`]。
    async fn revoke_session(&self, session_id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
/// 协调身份存储、密码引擎、引导秘密值源和审计日志。
pub struct IdentityService {
    store: IdentityStore,
    passwords: PasswordEngine,
    config_dir: PathBuf,
}

impl IdentityService {
    #[must_use]
    /// 使用配置的 Argon2 时间成本和默认密码引擎创建服务。
    ///
    /// 此方法读取 `MEDIAFLOW_ARGON2_TIME_COST`；无效或缺失值会使用安全默认值。
    pub fn new(pool: sqlx::SqlitePool, config_dir: PathBuf) -> Self {
        Self::new_with_password_engine(
            pool,
            config_dir,
            PasswordEngine::new(
                2,
                configured_time_cost(std::env::var("MEDIAFLOW_ARGON2_TIME_COST").ok().as_deref()),
            ),
        )
    }

    #[must_use]
    /// 使用显式密码引擎创建服务，主要用于经校准的运行时/测试。
    pub fn new_with_password_engine(
        pool: sqlx::SqlitePool,
        config_dir: PathBuf,
        passwords: PasswordEngine,
    ) -> Self {
        Self {
            store: IdentityStore::new(pool),
            passwords,
            config_dir,
        }
    }

    /// 返回数据库是否不含管理员账户。
    ///
    /// # Errors
    ///
    /// 无法查询身份表时返回 [`AppError`]。
    pub async fn requires_initialization(&self) -> Result<bool, AppError> {
        self.store.requires_initialization().await
    }

    /// 生成新的 CSRF 令牌，请求存储替换摘要，并返回原始令牌。
    ///
    /// 对缺失或已撤销的会话，存储替换会成功地不执行操作，且返回的令牌不会持久化；调用者必须在轮换前认证会话。
    ///
    /// # Errors
    ///
    /// 安全随机数失败或无法持久化会话更新时返回 [`AppError`]。
    pub async fn rotate_csrf(&self, session_id: Uuid) -> Result<String, AppError> {
        let token = random::random_token()?;
        self.store
            .replace_csrf(session_id, &random::sha256(token.as_bytes()))
            .await?;
        Ok(token)
    }

    /// 认证原始会话令牌，而不变更其空闲截止时间或最后使用时间。
    ///
    /// # Errors
    ///
    /// 会话无效/已过期，或无法安全读取其行时返回 [`AppError`]。
    pub async fn authenticate_without_sliding(
        &self,
        raw_token: &str,
    ) -> Result<AuthenticatedSession, AppError> {
        self.store
            .authenticate(
                &random::sha256(raw_token.as_bytes()),
                chrono::Utc::now().timestamp_micros(),
                false,
            )
            .await
    }

    /// 为 `action` 追加拒绝审计行，但不附加目标标识符。
    ///
    /// # Errors
    ///
    /// 无法持久化审计行时返回 [`AppError`]。
    pub async fn audit_denial(&self, action: &str) -> Result<(), AppError> {
        audit::record(self.store.pool(), action, "denied", None).await
    }
}

#[async_trait]
impl IdentityUseCases for IdentityService {
    async fn bootstrap(&self, command: BootstrapCommand) -> Result<AccountSummary, AppError> {
        if !self.store.requires_initialization().await? {
            return Err(AppError::new(
                ErrorCode::BootstrapAlreadyCompleted,
                "account already exists",
            ));
        }
        let (expected, file) = read_bootstrap_secret(&self.config_dir)?;
        if random::sha256(expected.as_bytes())
            .ct_eq(&random::sha256(command.bootstrap_secret.as_bytes()))
            .unwrap_u8()
            != 1
        {
            audit::record(self.store.pool(), "bootstrap", "denied", None).await?;
            return Err(AppError::new(
                ErrorCode::BootstrapInvalidSecret,
                "bootstrap secret mismatch",
            ));
        }
        let display = command.administrator_name.trim();
        if display.is_empty() || command.password.chars().count() < 12 {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "administrator name or password policy invalid",
            ));
        }
        let normalized = normalize_name(display);
        let phc = self.passwords.hash(command.password).await?;
        let now = chrono::Utc::now().timestamp_micros();
        let account = self
            .store
            .insert_account(new_id(), &normalized, display, &phc, now)
            .await?;
        audit::record(
            self.store.pool(),
            "bootstrap",
            "success",
            Some(&account.id.to_string()),
        )
        .await?;
        if let Some(path) = file {
            let _ = std::fs::remove_file(path);
        }
        Ok(account)
    }

    async fn create_session(&self, command: LoginCommand) -> Result<NewSession, AppError> {
        let normalized = normalize_name(&command.administrator_name);
        let candidate = random::domain_digest(b"login-candidate", normalized.as_bytes());
        let source = random::domain_digest(b"login-source", command.source_key.as_bytes());
        let now = chrono::Utc::now().timestamp_micros();
        if let Some(retry) = self.store.throttle_state(&candidate, &source, now).await? {
            audit::record(self.store.pool(), "login", "rate_limited", None).await?;
            return Err(rate_limited(retry, now));
        }
        let account = self.store.account_credential(&normalized).await?;
        let phc = match &account {
            Some(account) => account.password_phc.clone(),
            None => self.passwords.dummy_phc(),
        };
        let valid = self.passwords.verify(command.password, phc).await?;
        let Some(account) = account.filter(|_| valid) else {
            let retry = self.store.record_failure(&candidate, &source, now).await?;
            audit::record(self.store.pool(), "login", "denied", None).await?;
            if let Some(until) = retry {
                audit::record(self.store.pool(), "login", "rate_limited", None).await?;
                return Err(rate_limited(until, now));
            }
            return Err(AppError::new(
                ErrorCode::InvalidCredentials,
                "credential verification failed",
            ));
        };
        self.store.clear_throttle(&candidate, &source).await?;
        let raw_token = random::random_token()?;
        let csrf_token = random::random_token()?;
        let id = new_id();
        let absolute = now + 7 * 24 * 60 * 60 * 1_000_000_i64;
        let idle = (now + 30 * 60 * 1_000_000_i64).min(absolute);
        self.store
            .insert_session(
                id,
                account.id,
                &random::sha256(raw_token.as_bytes()),
                &random::sha256(csrf_token.as_bytes()),
                idle,
                absolute,
                account.credential_version,
                now,
            )
            .await?;
        audit::record(
            self.store.pool(),
            "login",
            "success",
            Some(&account.id.to_string()),
        )
        .await?;
        Ok(NewSession {
            id,
            raw_token,
            csrf_token,
            account: AccountSummary {
                id: account.id,
                administrator_name: account.display_name,
            },
        })
    }

    async fn authenticate(&self, raw_token: &str) -> Result<AuthenticatedSession, AppError> {
        self.store
            .authenticate(
                &random::sha256(raw_token.as_bytes()),
                chrono::Utc::now().timestamp_micros(),
                true,
            )
            .await
    }

    async fn revoke_session(&self, session_id: Uuid) -> Result<(), AppError> {
        self.store
            .revoke(session_id, chrono::Utc::now().timestamp_micros())
            .await?;
        audit::record(self.store.pool(), "logout", "success", None).await
    }
}

fn normalize_name(value: &str) -> String {
    value.trim().to_lowercase()
}

fn rate_limited(retry_at_us: i64, now_us: i64) -> AppError {
    let seconds = ((retry_at_us - now_us).max(0) + 999_999) / 1_000_000;
    AppError::new(ErrorCode::RateLimited, "persistent login throttle active")
        .with_detail("retry_after_seconds", seconds)
}

fn read_bootstrap_secret(config_dir: &Path) -> Result<(String, Option<PathBuf>), AppError> {
    let explicit = std::env::var_os("MEDIAFLOW_BOOTSTRAP_SECRET_FILE").map(PathBuf::from);
    select_bootstrap_secret(
        config_dir,
        explicit,
        PathBuf::from("/run/secrets/mediaflow-bootstrap"),
        std::env::var("MEDIAFLOW_BOOTSTRAP_SECRET").ok(),
    )
}

fn select_bootstrap_secret(
    config_dir: &Path,
    explicit: Option<PathBuf>,
    docker: PathBuf,
    env_fallback: Option<String>,
) -> Result<(String, Option<PathBuf>), AppError> {
    let local = config_dir.join("bootstrap.secret");
    for path in explicit.into_iter().chain([docker, local]) {
        if path.is_file() {
            validate_secret_permissions(&path)?;
            let value = std::fs::read_to_string(&path)
                .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
            let secret = value.trim_end_matches(['\r', '\n']).to_owned();
            if secret.is_empty() {
                return Err(AppError::new(
                    ErrorCode::ConfigInvalid,
                    "bootstrap secret file is empty",
                ));
            }
            return Ok((secret, Some(path)));
        }
    }
    env_fallback
        .filter(|value| !value.is_empty())
        .map(|value| (value, None))
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                "bootstrap secret is not configured",
            )
        })
}

#[cfg(unix)]
fn validate_secret_permissions(path: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(AppError::new(
            ErrorCode::ConfigInvalid,
            "bootstrap secret file permissions are unsafe",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_secret_permissions(_path: &Path) -> Result<(), AppError> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::select_bootstrap_secret;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn explicit_secure_file_has_priority_over_environment_fallback() {
        let root = tempfile::tempdir().unwrap();
        let explicit = root.path().join("explicit.secret");
        std::fs::write(&explicit, "file-secret\n").unwrap();
        std::fs::set_permissions(&explicit, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (secret, selected) = select_bootstrap_secret(
            root.path(),
            Some(explicit.clone()),
            root.path().join("missing-docker"),
            Some("environment-secret".to_owned()),
        )
        .unwrap();
        assert_eq!(secret, "file-secret");
        assert_eq!(selected.as_deref(), Some(explicit.as_path()));
    }
}
