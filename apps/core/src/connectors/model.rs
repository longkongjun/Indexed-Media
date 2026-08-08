use serde::{Deserialize, Deserializer, Serialize};

pub use crate::platform::secrets::IntegrationKind;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 已配置内置集成的脱敏健康状态。
pub enum IntegrationHealth {
    /// 不存在持久化凭据。
    Unconfigured,
    /// 最近一次权威连接检查成功。
    Healthy,
    /// 配置存在，但最近没有通过权威连接检查。
    Degraded,
    /// 无法连接提供方。
    Unavailable,
    /// 提供方拒绝了已配置凭据。
    Unauthorized,
    /// 提供方要求实例稍后重试。
    RateLimited,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 稳定、脱敏的连接器失败码。
pub enum IntegrationFailureCode {
    #[serde(rename = "integration.not-configured")]
    /// 不存在可用凭据。
    NotConfigured,
    #[serde(rename = "integration.unauthorized")]
    /// 提供方拒绝了凭据。
    Unauthorized,
    #[serde(rename = "integration.rate-limited")]
    /// 提供方对请求实施了限流。
    RateLimited,
    #[serde(rename = "integration.unavailable")]
    /// 提供方因未细分的安全原因不可用。
    Unavailable,
    #[serde(rename = "provider.timeout")]
    /// 有界提供方请求超时。
    ProviderTimeout,
    #[serde(rename = "provider.response-too-large")]
    /// 提供方响应超过字节上限。
    ResponseTooLarge,
    #[serde(rename = "provider.invalid-response")]
    /// 提供方响应不符合类型化契约。
    InvalidResponse,
}

impl IntegrationFailureCode {
    #[must_use]
    /// 返回用于 `SQLite` 和线协议的稳定值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotConfigured => "integration.not-configured",
            Self::Unauthorized => "integration.unauthorized",
            Self::RateLimited => "integration.rate-limited",
            Self::Unavailable => "integration.unavailable",
            Self::ProviderTimeout => "provider.timeout",
            Self::ResponseTooLarge => "provider.response-too-large",
            Self::InvalidResponse => "provider.invalid-response",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "integration.not-configured" => Some(Self::NotConfigured),
            "integration.unauthorized" => Some(Self::Unauthorized),
            "integration.rate-limited" => Some(Self::RateLimited),
            "integration.unavailable" => Some(Self::Unavailable),
            "provider.timeout" => Some(Self::ProviderTimeout),
            "provider.response-too-large" => Some(Self::ResponseTooLarge),
            "provider.invalid-response" => Some(Self::InvalidResponse),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// GET/PUT 返回的不含凭据 TMDB 配置与健康投影。
pub struct TmdbIntegrationView {
    /// 此端点固定返回 `tmdb`。
    pub kind: &'static str,
    /// 加密凭据列是否已经写入。
    pub configured: bool,
    /// TMDB 查询使用的首选 locale。
    pub locale: String,
    /// 提供方查询使用的可选 ISO region。
    pub region: Option<String>,
    /// 单调递增的乐观并发版本；零表示从未存在配置记录。
    pub config_version: i64,
    /// 最近一次安全健康分类。
    pub health: IntegrationHealth,
    /// 最近一次健康检查时间；从未检查时为 `None`。
    pub checked_at: Option<String>,
    /// 最近一次稳定、脱敏的失败码。
    pub failure_code: Option<IntegrationFailureCode>,
}

/// 以字节保存、从 `Debug` 隐藏并在析构时清零的秘密输入。
pub struct SecretString(Vec<u8>);

impl SecretString {
    #[must_use]
    /// 从拥有所有权的 UTF-8 字符串创建请求秘密。
    pub fn new(value: String) -> Self {
        Self(value.into_bytes())
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn into_secret_bytes(mut self) -> crate::platform::secrets::SecretBytes {
        crate::platform::secrets::SecretBytes::new(std::mem::take(&mut self.0))
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::new)
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
/// TMDB 保存配置或不落盘连接测试请求。
pub struct TmdbConfigCommand {
    /// 待验证的 API Read Access Token。
    pub api_read_access_token: SecretString,
    /// `ll-RR` 形式的首选 locale。
    pub locale: String,
    /// 可选的两位大写 region。
    pub region: Option<String>,
}

impl std::fmt::Debug for TmdbConfigCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TmdbConfigCommand")
            .field("api_read_access_token", &"[REDACTED]")
            .field("locale", &self.locale)
            .field("region", &self.region)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 不保存候选 TMDB 凭据时得到的脱敏测试结果。
pub struct TmdbConnectionTestResult {
    /// 身份验证和有界提供方请求是否成功。
    pub reachable: bool,
    /// 稳定健康分类。
    pub health: IntegrationHealth,
    /// 稳定失败码；成功时为 `None`。
    pub failure_code: Option<IntegrationFailureCode>,
    /// UTC RFC3339 检查时间。
    pub checked_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
/// 不携带上游正文、URL、Token 或原始诊断文本的提供方失败。
pub enum ProviderError {
    /// 当前未配置提供方凭据。
    #[error("metadata provider is not configured")]
    NotConfigured,
    /// 本地 Token 不是 UTF-8、长度越界或无法构造 Authorization header，或提供方拒绝了凭据。
    #[error("metadata provider credentials are invalid")]
    CredentialsInvalid,
    /// 提供方要求调用方在有界时间后重试。
    #[error("metadata provider rate limited the request")]
    RateLimited {
        /// 最早可重试的 Unix epoch 微秒时间。
        retry_at_us: i64,
    },
    /// 提供方连接/5xx 失败或持久缓存读写失败，并携带有界重试时间。
    #[error("metadata provider is temporarily unavailable")]
    TemporarilyUnavailable {
        /// 最早可重试的 Unix epoch 微秒时间。
        retry_at_us: i64,
    },
    /// 有界请求超时。
    #[error("metadata provider request timed out")]
    Timeout,
    /// 响应超过已配置字节预算。
    #[error("metadata provider response is too large")]
    ResponseTooLarge,
    /// 提供方响应、请求参数、测试相对路径或缓存序列化/解码未通过本地有界校验。
    #[error("metadata provider response is invalid")]
    InvalidResponse,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 类型化元数据提供方端口使用的媒体类别。
pub enum ProviderMediaKind {
    /// 电影长片。
    Movie,
    /// 电视或流媒体剧集。
    Tv,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 有界回退合并后，一个本地化字段的来源。
pub enum FieldLanguageSource {
    /// 管理员请求的首选 locale。
    Preferred,
    /// 作品原始语言 locale。
    Original,
    /// 明确使用 `en-US` 的最终回退。
    English,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 候选来自实时提供方还是持久化缓存层。
pub enum CacheStatus {
    /// 实时提供方响应。
    Miss,
    /// 仍在新鲜期内的持久化缓存结果。
    Fresh,
    /// 仅在提供方暂时失败时使用的过期缓存结果。
    Stale,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 携带精确 locale 与回退来源的值。
pub struct LocalizedField<T> {
    /// 已限制大小的类型化值。
    pub value: T,
    /// 实际产生该值的 locale。
    pub language: String,
    /// 产生该值的回退阶段。
    pub source: FieldLanguageSource,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 稳定 TMDB 身份及其媒体类别。
pub struct CandidateIdentity {
    /// 大于零的 TMDB 数字 ID。
    pub provider_id: i64,
    /// 类型化端点返回的电影或剧集类别。
    pub media_kind: ProviderMediaKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 由提供方核验的季/集身份。
pub struct EpisodeIdentity {
    /// 非负季号。
    pub season: u16,
    /// 大于零的集号。
    pub episode: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 从私有 TMDB DTO 映射得到的有界候选。
pub struct MetadataCandidate {
    /// 稳定提供方身份。
    pub identity: CandidateIdentity,
    /// 带来源标签的有序本地化标题。
    pub titles: Vec<LocalizedField<String>>,
    /// 带来源标签的有序本地化简介。
    pub summaries: Vec<LocalizedField<String>>,
    /// 提供方返回的发行或首播日期。
    pub release_dates: Vec<LocalizedField<chrono::NaiveDate>>,
    /// 作为有界标题证据保留的提供方别名。
    pub aliases: Vec<LocalizedField<String>>,
    /// 剧集候选可选的已核验集数身份。
    pub episodes: Vec<EpisodeIdentity>,
    /// 领域映射 Schema 版本。
    pub provider_version: u16,
    /// 本次返回值使用的缓存层级。
    pub cache_status: CacheStatus,
    /// 提供方或缓存读取的 Unix epoch 微秒时间。
    pub retrieved_at_us: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 有界电影或剧集文本搜索请求。
pub struct TmdbSearchRequest {
    /// 目标端点的媒体类别。
    pub media_kind: ProviderMediaKind,
    /// 已规范化的标题查询。
    pub title: String,
    /// 可选发行或首播年份。
    pub year: Option<u16>,
    /// `ll-RR` 形式的首选 locale。
    pub locale: String,
    /// 可选的两位大写 region。
    pub region: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// TMDB `/find` 支持的外部身份命名空间。
pub enum ExternalIdSource {
    /// `IMDb` 标题 ID。
    Imdb,
    /// `TheTVDB` 剧集/单集 ID。
    Tvdb,
}

impl ExternalIdSource {
    #[must_use]
    /// 返回 TMDB `/find` 固定使用的查询值。
    pub const fn as_tmdb_value(self) -> &'static str {
        match self {
            Self::Imdb => "imdb_id",
            Self::Tvdb => "tvdb_id",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 有界外部 ID 查询请求。
pub struct TmdbExternalIdRequest {
    /// 外部身份命名空间。
    pub source: ExternalIdSource,
    /// 显式外部标识符。
    pub external_id: String,
    /// `ll-RR` 形式的首选 locale。
    pub locale: String,
}
