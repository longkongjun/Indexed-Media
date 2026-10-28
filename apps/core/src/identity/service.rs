use std::path::{Path, PathBuf};

use async_trait::async_trait;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::identity::model::{
    AccountSummary, AuthenticatedSession, BootstrapCommand, DeviceView, LoginCommand, NewSession,
};
use crate::identity::store::{DeviceRecord, IdentityStore};
use crate::platform::password::{PasswordEngine, configured_time_cost};
use crate::platform::{audit, random};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::id::new_id;

/// 设备令牌原始形式的前缀；仅用于日志辨识与跨通道防呆，不参与摘要。
pub const DEVICE_TOKEN_PREFIX: &str = "mfdev_";
/// 一次性配对码有效期（微秒）。
const PAIRING_CODE_TTL_US: i64 = 10 * 60 * 1_000_000;
/// 配对码熵（5 字节 = 8 个 base32 字符 = 40 bit）。
const PAIRING_CODE_BYTES: usize = 5;
/// RFC 4648 base32 字母表；无 `0`/`1`，人工输入歧义有限。
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

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

    /// 列出家庭成员。
    ///
    /// # Errors
    ///
    /// 数据库失败时返回稳定错误。
    pub async fn list_family_members(
        &self,
    ) -> Result<Vec<crate::identity::model::FamilyMemberView>, AppError> {
        self.store.list_family_members().await
    }

    /// 创建一名家庭成员（Argon2id 哈希后落库）。
    ///
    /// # Errors
    ///
    /// 名称或密码无效、名称冲突或数据库失败时返回稳定错误。
    pub async fn create_family_member(
        &self,
        display_name: &str,
        password: &str,
    ) -> Result<crate::identity::model::FamilyMemberView, AppError> {
        let phc = self.passwords.hash(password.to_owned()).await?;
        self.store
            .insert_family_member(display_name, &phc, chrono::Utc::now().timestamp_micros())
            .await
    }

    /// 删除（停用）一名家庭成员。
    ///
    /// # Errors
    ///
    /// 目标不存在或数据库失败时返回稳定错误。
    pub async fn delete_family_member(&self, member_id: Uuid) -> Result<(), AppError> {
        self.store
            .delete_family_member(member_id, chrono::Utc::now().timestamp_micros())
            .await
    }

    /// 使用显式密码引擎创建服务，主要用于经校准的运行时/测试。
    #[must_use]
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

/// enrollment 成功结果；原始设备令牌仅向 HTTP 层返回一次。
///
/// 仅持久化 `raw_token` 的 SHA-256 摘要；`Debug` 隐藏该值。
pub struct NewDevice {
    /// 新注册设备的公开表示。
    pub device: DeviceView,
    /// `Authorization: Bearer` 使用的原始令牌（`mfdev_` 前缀）。
    pub raw_token: String,
}

impl std::fmt::Debug for NewDevice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NewDevice")
            .field("device", &self.device)
            .field("raw_token", &"[REDACTED]")
            .finish()
    }
}

impl IdentityService {
    /// 为管理员签发一次性配对码（10 分钟有效、仅存摘要），并尽力清理过期码行。
    ///
    /// # Errors
    ///
    /// 安全随机数失败或数据库失败时返回 [`AppError`]。
    pub async fn issue_pairing_code(&self, account_id: Uuid) -> Result<(String, i64), AppError> {
        self.issue_pairing_code_with_access(
            account_id,
            crate::identity::model::DeviceAccessMode::Administrator,
        )
        .await
    }

    /// 签发具有持久化权限上限的一次性配对码。
    ///
    /// # Errors
    /// 随机数生成或持久化失败时返回错误。
    pub async fn issue_pairing_code_with_access(
        &self,
        account_id: Uuid,
        access_mode: crate::identity::model::DeviceAccessMode,
    ) -> Result<(String, i64), AppError> {
        let mut bytes = [0_u8; PAIRING_CODE_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
        let code = encode_pairing_code(bytes);
        let now = chrono::Utc::now().timestamp_micros();
        let expires = now + PAIRING_CODE_TTL_US;
        self.store
            .insert_pairing_code(
                new_id(),
                account_id,
                &random::sha256(code.as_bytes()),
                now,
                expires,
                access_mode,
            )
            .await?;
        audit::record(
            self.store.pool(),
            "device-pairing-code",
            "success",
            Some(&account_id.to_string()),
        )
        .await?;
        let _ = sqlx::query("DELETE FROM identity_pairing_codes WHERE expires_at_us<?")
            .bind(now)
            .execute(self.store.pool())
            .await;
        Ok((code, expires))
    }

    /// 以一次性配对码完成设备 enrollment，签发设备令牌。
    ///
    /// 错码按来源限流（15 分钟窗 5 次失败锁 5 分钟，与登录限流同表同策略）。
    ///
    /// # Errors
    ///
    /// 限流激活、配对码未知/过期/已使用、设备名无效或数据库失败时返回 [`AppError`]。
    pub async fn enroll_with_pairing_code(
        &self,
        pairing_code: &str,
        device_name: &str,
        source_key: &str,
    ) -> Result<NewDevice, AppError> {
        self.enroll_with_pairing_code_access(
            pairing_code,
            device_name,
            source_key,
            crate::identity::model::DeviceAccessMode::Administrator,
        )
        .await
    }

    /// 按明确访问能力配对设备。
    ///
    /// # Errors
    /// 返回配对、限流和持久化错误。
    pub async fn enroll_with_pairing_code_access(
        &self,
        pairing_code: &str,
        device_name: &str,
        source_key: &str,
        access_mode: crate::identity::model::DeviceAccessMode,
    ) -> Result<NewDevice, AppError> {
        let candidate = random::domain_digest(b"device-enrollment", source_key.as_bytes());
        let source = random::domain_digest(b"device-enrollment-source", b"pairing-code");
        let now = chrono::Utc::now().timestamp_micros();
        if let Some(retry) = self.store.throttle_state(&candidate, &source, now).await? {
            audit::record(self.store.pool(), "device-enrollment", "rate_limited", None).await?;
            return Err(rate_limited(retry, now));
        }
        let normalized = normalize_pairing_code(pairing_code);
        let account_id = match self
            .store
            .consume_pairing_code(&random::sha256(normalized.as_bytes()), now, access_mode)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = audit::record(self.store.pool(), "device-enrollment", "denied", None).await;
                if let Some(retry) = self.store.record_failure(&candidate, &source, now).await? {
                    audit::record(self.store.pool(), "device-enrollment", "rate_limited", None)
                        .await?;
                    return Err(rate_limited(retry, now));
                }
                return Err(error);
            }
        };
        self.store.clear_throttle(&candidate, &source).await?;
        self.enroll(account_id, device_name, access_mode).await
    }

    /// 以管理员凭据完成设备 enrollment（移动端登录页凭据模式），签发设备令牌。
    ///
    /// 与 Web 登录共用候选项/来源限流与 dummy-hash 防时序侧信道。
    ///
    /// # Errors
    ///
    /// 限流激活、凭据无效、设备名无效或数据库失败时返回 [`AppError`]。
    pub async fn enroll_with_credentials(
        &self,
        administrator_name: &str,
        password: &str,
        device_name: &str,
        source_key: &str,
    ) -> Result<NewDevice, AppError> {
        self.enroll_with_credentials_access(
            administrator_name,
            password,
            device_name,
            source_key,
            crate::identity::model::DeviceAccessMode::Administrator,
        )
        .await
    }

    /// 按明确访问能力使用管理员凭据注册设备。
    ///
    /// # Errors
    /// 返回认证、限流和持久化错误。
    pub async fn enroll_with_credentials_access(
        &self,
        administrator_name: &str,
        password: &str,
        device_name: &str,
        source_key: &str,
        access_mode: crate::identity::model::DeviceAccessMode,
    ) -> Result<NewDevice, AppError> {
        let normalized = normalize_name(administrator_name);
        let candidate = random::domain_digest(b"login-candidate", normalized.as_bytes());
        let source = random::domain_digest(b"login-source", source_key.as_bytes());
        let now = chrono::Utc::now().timestamp_micros();
        if let Some(retry) = self.store.throttle_state(&candidate, &source, now).await? {
            audit::record(self.store.pool(), "device-enrollment", "rate_limited", None).await?;
            return Err(rate_limited(retry, now));
        }
        let account = self.store.account_credential(&normalized).await?;
        let phc = match &account {
            Some(account) => account.password_phc.clone(),
            None => self.passwords.dummy_phc(),
        };
        let valid = self.passwords.verify(password.to_owned(), phc).await?;
        let Some(account) = account.filter(|_| valid) else {
            let _ = audit::record(self.store.pool(), "device-enrollment", "denied", None).await;
            if let Some(retry) = self.store.record_failure(&candidate, &source, now).await? {
                audit::record(self.store.pool(), "device-enrollment", "rate_limited", None).await?;
                return Err(rate_limited(retry, now));
            }
            return Err(AppError::new(
                ErrorCode::InvalidCredentials,
                "credential verification failed",
            ));
        };
        self.store.clear_throttle(&candidate, &source).await?;
        self.enroll(account.id, device_name, access_mode).await
    }

    /// 认证原始设备令牌（仅摘要查表），并按 60 秒节流更新 `last_used_at`。
    ///
    /// # Errors
    ///
    /// 令牌未知、已撤销、已过期或账户凭据版本已变化时返回 [`ErrorCode::SessionExpired`]。
    pub async fn authenticate_device(
        &self,
        raw_token: &str,
    ) -> Result<AuthenticatedSession, AppError> {
        self.store
            .authenticate_device(
                &random::sha256(raw_token.as_bytes()),
                chrono::Utc::now().timestamp_micros(),
            )
            .await
    }

    /// 自轮换设备令牌：同事务原子替换摘要，旧令牌立即失效。
    ///
    /// # Errors
    ///
    /// 设备缺失、已撤销或已过期时返回 [`ErrorCode::SessionExpired`]；安全随机数或数据库
    /// 失败返回 [`AppError`]。
    pub async fn rotate_device_token(&self, device_id: Uuid) -> Result<(String, i64), AppError> {
        let (raw, digest) = fresh_device_token()?;
        let expires = device_absolute_expiry();
        self.store
            .rotate_device_token(device_id, &digest, expires)
            .await?;
        audit::record(
            self.store.pool(),
            "device-rotation",
            "success",
            Some(&device_id.to_string()),
        )
        .await?;
        Ok((raw, expires))
    }

    /// 撤销设备令牌并记录审计；幂等。
    ///
    /// # Errors
    ///
    /// 数据库失败时返回 [`AppError`]。
    pub async fn revoke_device(&self, device_id: Uuid, reason: &str) -> Result<(), AppError> {
        self.store
            .revoke_device(device_id, chrono::Utc::now().timestamp_micros(), reason)
            .await?;
        audit::record(
            self.store.pool(),
            "device-revocation",
            "success",
            Some(&device_id.to_string()),
        )
        .await
    }

    /// 读取单个设备的公开表示。
    ///
    /// # Errors
    ///
    /// 设备不存在或数据库失败时返回 [`AppError`]。
    pub async fn device(&self, device_id: Uuid) -> Result<DeviceView, AppError> {
        let record = self.store.device(device_id).await?;
        Ok(device_view(&record))
    }

    /// 列出全部设备（含已撤销）的公开表示。
    ///
    /// # Errors
    ///
    /// 数据库失败时返回 [`AppError`]。
    pub async fn list_devices(&self) -> Result<Vec<DeviceView>, AppError> {
        let records = self.store.list_devices().await?;
        Ok(records.iter().map(device_view).collect())
    }

    async fn enroll(
        &self,
        account_id: Uuid,
        device_name: &str,
        access_mode: crate::identity::model::DeviceAccessMode,
    ) -> Result<NewDevice, AppError> {
        let name = device_name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "device name is invalid",
            ));
        }
        let credential = self
            .store
            .account_credential_version(account_id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::InvalidCredentials, "account is not active"))?;
        let (raw, digest) = fresh_device_token()?;
        let now = chrono::Utc::now().timestamp_micros();
        let expires = device_absolute_expiry();
        let id = new_id();
        self.store
            .insert_device(
                access_mode,
                id,
                account_id,
                name,
                &digest,
                credential,
                expires,
                now,
            )
            .await?;
        audit::record(
            self.store.pool(),
            "device-enrollment",
            "success",
            Some(&id.to_string()),
        )
        .await?;
        Ok(NewDevice {
            device: DeviceView {
                access_mode,
                id,
                display_name: name.to_owned(),
                created_at: micros_to_rfc3339(now),
                last_used_at: None,
                expires_at: micros_to_rfc3339(expires),
                revoked_at: None,
            },
            raw_token: raw,
        })
    }
}

fn fresh_device_token() -> Result<(String, [u8; 32]), AppError> {
    let raw = format!("{DEVICE_TOKEN_PREFIX}{}", random::random_token()?);
    let digest = random::sha256(raw.as_bytes());
    Ok((raw, digest))
}

fn device_absolute_expiry() -> i64 {
    let days = std::env::var("MEDIAFLOW_DEVICE_TOKEN_DAYS")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|days| (1..=3650).contains(days))
        .unwrap_or(180);
    chrono::Utc::now().timestamp_micros() + days * 24 * 60 * 60 * 1_000_000
}

fn micros_to_rfc3339(micros: i64) -> String {
    chrono::TimeZone::timestamp_micros(&chrono::Utc, micros)
        .single()
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .unwrap_or_default()
}

fn normalize_pairing_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| *c != '-' && *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn encode_pairing_code(bytes: [u8; PAIRING_CODE_BYTES]) -> String {
    let mut code = String::with_capacity(8);
    let mut buffer = 0_u64;
    let mut bits = 0_u32;
    for &byte in &bytes {
        buffer = (buffer << 8) | u64::from(byte);
        bits += 8;
        while bits >= 5 {
            let index = ((buffer >> (bits - 5)) & 0x1f) as usize;
            code.push(BASE32_ALPHABET[index] as char);
            bits -= 5;
        }
    }
    code
}

fn device_view(record: &DeviceRecord) -> DeviceView {
    DeviceView {
        access_mode: record.access_mode,
        id: record.id,
        display_name: record.display_name.clone(),
        created_at: micros_to_rfc3339(record.created_at_us),
        last_used_at: record.last_used_at_us.map(micros_to_rfc3339),
        expires_at: micros_to_rfc3339(record.absolute_expires_at_us),
        revoked_at: record.revoked_at_us.map(micros_to_rfc3339),
    }
}

#[cfg(test)]
mod device_service_tests {
    use super::{BASE32_ALPHABET, encode_pairing_code, normalize_pairing_code};

    #[test]
    fn pairing_code_is_eight_base32_characters() {
        assert_eq!(
            encode_pairing_code([0x00, 0x00, 0x00, 0x00, 0x00]),
            "AAAAAAAA"
        );
        assert_eq!(
            encode_pairing_code([0xff, 0xff, 0xff, 0xff, 0xff]),
            "77777777"
        );
        assert!(
            encode_pairing_code([0x5a, 0xc3, 0x71, 0x0f, 0xb9])
                .bytes()
                .all(|byte| BASE32_ALPHABET.contains(&byte))
        );
    }

    #[test]
    fn pairing_code_normalization_ignores_case_and_separators() {
        assert_eq!(
            normalize_pairing_code(" abcd-wxyz "),
            normalize_pairing_code("ABCDWXYZ")
        );
    }
}
