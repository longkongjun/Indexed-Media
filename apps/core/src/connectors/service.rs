use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::connectors::model::{
    EpisodeIdentity, IntegrationFailureCode, IntegrationHealth, MetadataCandidate, ProviderError,
    ProviderMediaKind, TmdbConfigCommand, TmdbConnectionTestResult, TmdbExternalIdRequest,
    TmdbIntegrationView, TmdbSearchRequest,
};
use crate::connectors::store::{ConnectorStore, TmdbIntegrationRecord};
use crate::platform::outbox::OutboxNotifier;
use crate::platform::secrets::{
    InstanceKey, IntegrationKind, SecretAad, SecretBytes, SecretCipher as _, SecretError,
};
use crate::shared::error::{AppError, ErrorCode};

const SECRET_SCHEMA_VERSION: u16 = 1;
const DEFAULT_LOCALE: &str = "zh-CN";

#[async_trait]
/// 供确定性识别流程使用的类型化元数据提供方端口。
pub trait MetadataProvider: Send + Sync {
    /// 在文本搜索前解析有界显式外部 ID。
    ///
    /// # Errors
    ///
    /// 外部 ID 或 locale 无效、凭据被拒绝、网络/超时/限流失败、响应越界或无效，以及缓存
    /// 读写或解码失败时返回 [`ProviderError`]。
    async fn find_external(
        &self,
        token: &SecretBytes,
        request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError>;

    /// 搜索有界电影或剧集候选。
    ///
    /// # Errors
    ///
    /// 标题、年份、locale 或 region 无效，凭据被拒绝，网络/超时/限流失败，响应越界或无效，
    /// 以及缓存读写或解码失败时返回 [`ProviderError`]。
    async fn search(
        &self,
        token: &SecretBytes,
        request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError>;

    /// 获取并合并首选、原始语言和英文详情字段。
    ///
    /// # Errors
    ///
    /// 提供方 ID 或 locale 无效、凭据被拒绝、网络/超时/限流失败、响应越界或无效，以及缓存
    /// 读写或解码失败时返回 [`ProviderError`]。
    async fn details(
        &self,
        token: &SecretBytes,
        media_kind: ProviderMediaKind,
        provider_id: i64,
        locale: &str,
    ) -> Result<MetadataCandidate, ProviderError>;

    /// 核验剧集中每个请求的季/集身份均存在。
    ///
    /// # Errors
    ///
    /// 剧集 ID、locale 或季/集引用无效、重复或超出数量边界，目标集数不存在，凭据被拒绝，
    /// 或发生网络、超时、限流、响应边界及响应校验失败时返回 [`ProviderError`]。
    async fn verify_episodes(
        &self,
        token: &SecretBytes,
        series_id: i64,
        episodes: &[(u16, u16)],
        locale: &str,
    ) -> Result<Vec<EpisodeIdentity>, ProviderError>;
}

#[async_trait]
/// 可注入的有界 TMDB 连接探测器。
pub trait TmdbConnectionTester: Send + Sync {
    /// 测试一个候选凭据且不持久化。
    ///
    /// # Errors
    ///
    /// 调用方须在调用前完成 locale 与 region 校验；Token 无法构造凭据、凭据被提供方拒绝，
    /// 或提供方请求发生网络、超时、限流、响应边界及响应校验失败时返回 [`ProviderError`]。
    async fn test(
        &self,
        token: &SecretBytes,
        locale: &str,
        region: Option<&str>,
    ) -> Result<(), ProviderError>;
}

pub(crate) struct UnavailableTmdbConnectionTester;

#[async_trait]
impl TmdbConnectionTester for UnavailableTmdbConnectionTester {
    async fn test(
        &self,
        _token: &SecretBytes,
        _locale: &str,
        _region: Option<&str>,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::TemporarilyUnavailable {
            retry_at_us: chrono::Utc::now().timestamp_micros() + 60_000_000,
        })
    }
}

#[derive(Clone)]
/// 协调脱敏 TMDB 配置、认证加密与不落盘连接探测。
pub struct ConnectorService {
    store: ConnectorStore,
    key: Result<Arc<InstanceKey>, SecretError>,
    tester: Arc<dyn TmdbConnectionTester>,
}

impl ConnectorService {
    #[must_use]
    /// 使用给定连接测试器创建服务，并立即尝试加载或首次创建实例密钥；失败会保存在服务内，
    /// 由 [`Self::ensure_key_ready`] 或后续凭据操作返回。
    pub fn new(pool: SqlitePool, config_dir: &Path, tester: Arc<dyn TmdbConnectionTester>) -> Self {
        Self::new_with_notifier(pool, config_dir, tester, OutboxNotifier::new())
    }

    #[must_use]
    /// 使用共享通知器创建服务，使配置事务提交后可立即唤醒健康事件交付；构造期间会立即尝试
    /// 加载或首次创建实例密钥，失败会延迟到 [`Self::ensure_key_ready`] 或凭据操作返回。
    pub fn new_with_notifier(
        pool: SqlitePool,
        config_dir: &Path,
        tester: Arc<dyn TmdbConnectionTester>,
        notifier: OutboxNotifier,
    ) -> Self {
        let key = InstanceKey::load_or_create(config_dir).map(Arc::new);
        Self {
            store: ConnectorStore::new_with_notifier(pool, notifier),
            key,
            tester,
        }
    }

    /// 在后台 worker 启动前确认实例密钥已安全加载或创建。
    ///
    /// # Errors
    ///
    /// 密钥存储不可用或权限不安全时返回启动配置错误。
    pub fn ensure_key_ready(&self) -> Result<(), AppError> {
        self.key().map(|_| ())
    }

    /// 读取不含凭据的当前配置与健康投影。
    ///
    /// # Errors
    ///
    /// 持久化连接器记录不可用或无效时返回错误。
    pub async fn get_tmdb(&self) -> Result<TmdbIntegrationView, AppError> {
        self.store
            .load_tmdb()
            .await?
            .map_or_else(|| Ok(unconfigured_view()), project)
    }

    /// 测试候选 Token，不写入数据库记录或加密文件。
    ///
    /// # Errors
    ///
    /// 返回校验或提供方失败，且不暴露候选 Token。
    pub async fn test_tmdb(
        &self,
        command: TmdbConfigCommand,
    ) -> Result<TmdbConnectionTestResult, AppError> {
        validate_command(&command)?;
        let token = command.api_read_access_token.into_secret_bytes();
        let now = chrono::Utc::now();
        self.tester
            .test(&token, &command.locale, command.region.as_deref())
            .await
            .map_err(provider_error)?;
        Ok(TmdbConnectionTestResult {
            reachable: true,
            health: IntegrationHealth::Healthy,
            failure_code: None,
            checked_at: now.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        })
    }

    /// 使用乐观并发加密并保存一份 TMDB 配置。
    ///
    /// # Errors
    ///
    /// 返回校验、密钥、加密、数据库或版本冲突错误。
    pub async fn put_tmdb(
        &self,
        command: TmdbConfigCommand,
        expected_version: i64,
    ) -> Result<TmdbIntegrationView, AppError> {
        validate_command(&command)?;
        if expected_version < 0 {
            return Err(validation_error());
        }
        let current = self.store.load_tmdb().await?;
        let (id, actual_version) = current.as_ref().map_or((Uuid::now_v7(), 0), |value| {
            (value.id, value.config_version)
        });
        if actual_version != expected_version {
            return Err(AppError::new(
                ErrorCode::ConfigVersionConflict,
                "connector config version changed",
            ));
        }
        let next_version = expected_version
            .checked_add(1)
            .ok_or_else(validation_error)?;
        let key = self.key()?;
        let token = command.api_read_access_token.into_secret_bytes();
        let sealed = key
            .seal(
                &SecretAad::new(
                    id,
                    IntegrationKind::Tmdb,
                    next_version,
                    SECRET_SCHEMA_VERSION,
                ),
                token.expose(),
            )
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        self.store
            .save_tmdb(
                id,
                expected_version,
                &command.locale,
                command.region.as_deref(),
                &sealed,
                chrono::Utc::now().timestamp_micros(),
            )
            .await?;
        self.get_tmdb().await
    }

    /// 清除加密凭据列，同时保留单调递增的墓碑版本。
    ///
    /// # Errors
    ///
    /// 返回校验、数据库或乐观并发错误。
    pub async fn delete_tmdb(&self, expected_version: i64) -> Result<(), AppError> {
        if expected_version < 0 {
            return Err(validation_error());
        }
        self.store
            .delete_tmdb(expected_version, chrono::Utc::now().timestamp_micros())
            .await?;
        Ok(())
    }

    /// 为类型化 TMDB 提供方解密已配置凭据。
    ///
    /// # Errors
    ///
    /// 返回未配置、密钥、认证或持久化记录错误。
    pub async fn load_tmdb_credential(
        &self,
    ) -> Result<(SecretBytes, String, Option<String>), AppError> {
        let record = self.store.load_tmdb().await?.ok_or_else(|| {
            AppError::new(
                ErrorCode::IntegrationNotConfigured,
                "tmdb is not configured",
            )
        })?;
        let sealed = record.sealed.as_ref().ok_or_else(|| {
            AppError::new(
                ErrorCode::IntegrationNotConfigured,
                "tmdb is not configured",
            )
        })?;
        let key = self.key()?;
        let token = key
            .open(
                &SecretAad::new(
                    record.id,
                    IntegrationKind::Tmdb,
                    record.config_version,
                    sealed.schema_version(),
                ),
                sealed,
            )
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        Ok((token, record.locale, record.region))
    }

    /// 持久化一次识别提供方查询观察到的脱敏健康分类。历史决策不会改变；仅当公开分类变化时
    /// 发出健康事件。
    ///
    /// # Errors
    ///
    /// 持久化事务或 outbox 失败时返回错误。
    pub async fn record_tmdb_health(
        &self,
        failure: Option<ProviderError>,
        checked_at_us: i64,
    ) -> Result<(), AppError> {
        let (health, failure_code) = match failure {
            None => (IntegrationHealth::Healthy, None),
            Some(ProviderError::NotConfigured) => (
                IntegrationHealth::Unconfigured,
                Some(IntegrationFailureCode::NotConfigured),
            ),
            Some(ProviderError::CredentialsInvalid) => (
                IntegrationHealth::Unauthorized,
                Some(IntegrationFailureCode::Unauthorized),
            ),
            Some(ProviderError::RateLimited { .. }) => (
                IntegrationHealth::RateLimited,
                Some(IntegrationFailureCode::RateLimited),
            ),
            Some(ProviderError::Timeout) => (
                IntegrationHealth::Unavailable,
                Some(IntegrationFailureCode::ProviderTimeout),
            ),
            Some(ProviderError::ResponseTooLarge) => (
                IntegrationHealth::Unavailable,
                Some(IntegrationFailureCode::ResponseTooLarge),
            ),
            Some(ProviderError::InvalidResponse) => (
                IntegrationHealth::Unavailable,
                Some(IntegrationFailureCode::InvalidResponse),
            ),
            Some(ProviderError::TemporarilyUnavailable { .. }) => (
                IntegrationHealth::Unavailable,
                Some(IntegrationFailureCode::Unavailable),
            ),
        };
        self.store
            .update_tmdb_health(health, failure_code, checked_at_us)
            .await
    }

    fn key(&self) -> Result<&InstanceKey, AppError> {
        self.key
            .as_deref()
            .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, *error))
    }
}

fn validate_command(command: &TmdbConfigCommand) -> Result<(), AppError> {
    let token_len = command.api_read_access_token.len();
    if !(16..=4096).contains(&token_len)
        || !valid_locale(&command.locale)
        || command.region.as_deref().is_some_and(|value| {
            value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_uppercase())
        })
    {
        return Err(validation_error());
    }
    Ok(())
}

fn valid_locale(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 5
        && bytes[0..2].iter().all(u8::is_ascii_lowercase)
        && bytes[2] == b'-'
        && bytes[3..5].iter().all(u8::is_ascii_uppercase)
}

fn project(record: TmdbIntegrationRecord) -> Result<TmdbIntegrationView, AppError> {
    Ok(TmdbIntegrationView {
        kind: "tmdb",
        configured: record.sealed.is_some(),
        locale: record.locale,
        region: record.region,
        config_version: record.config_version,
        health: record.health,
        checked_at: record.checked_at_us.map(format_time).transpose()?,
        failure_code: record.failure_code,
    })
}

fn unconfigured_view() -> TmdbIntegrationView {
    TmdbIntegrationView {
        kind: "tmdb",
        configured: false,
        locale: DEFAULT_LOCALE.to_owned(),
        region: None,
        config_version: 0,
        health: IntegrationHealth::Unconfigured,
        checked_at: None,
        failure_code: Some(IntegrationFailureCode::NotConfigured),
    }
}

fn format_time(value: i64) -> Result<String, AppError> {
    chrono::DateTime::from_timestamp_micros(value)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "invalid connector timestamp"))
}

fn provider_error(error: ProviderError) -> AppError {
    let code = match error {
        ProviderError::CredentialsInvalid => ErrorCode::IntegrationUnauthorized,
        ProviderError::RateLimited { .. } => ErrorCode::IntegrationRateLimited,
        ProviderError::NotConfigured
        | ProviderError::TemporarilyUnavailable { .. }
        | ProviderError::Timeout
        | ProviderError::ResponseTooLarge
        | ProviderError::InvalidResponse => ErrorCode::ProviderUnavailable,
    };
    AppError::new(code, "tmdb connection test failed")
}

fn validation_error() -> AppError {
    AppError::new(ErrorCode::ValidationFailed, "invalid tmdb configuration")
}
