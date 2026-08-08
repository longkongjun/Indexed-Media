use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::connectors::model::{IntegrationHealth, SecretString};
use crate::platform::secrets::{IntegrationKind, SecretBytes};
use crate::shared::error::{AppError, ErrorCode};

pub(crate) const SECRET_SCHEMA_VERSION: u16 = 1;
pub(crate) const TASK_SOURCE_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
/// 编译期内置的下载器产品类型。
pub enum DownloaderKind {
    /// qBittorrent `WebUI` API v2。
    Qbittorrent,
    /// Transmission RPC。
    Transmission,
}

impl DownloaderKind {
    #[must_use]
    /// 返回 `SQLite` 与线协议使用的稳定值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Qbittorrent => "qbittorrent",
            Self::Transmission => "transmission",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "qbittorrent" => Some(Self::Qbittorrent),
            "transmission" => Some(Self::Transmission),
            _ => None,
        }
    }

    pub(crate) const fn integration_kind(self) -> IntegrationKind {
        match self {
            Self::Qbittorrent => IntegrationKind::Qbittorrent,
            Self::Transmission => IntegrationKind::Transmission,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 下载器连接和任务共用的稳定脱敏失败分类。
pub enum DownloaderFailureCode {
    #[serde(rename = "integration.not-configured")]
    /// 连接配置不存在。
    IntegrationNotConfigured,
    #[serde(rename = "integration.unauthorized")]
    /// 下载器拒绝凭据。
    IntegrationUnauthorized,
    #[serde(rename = "integration.rate-limited")]
    /// 下载器要求延迟请求。
    IntegrationRateLimited,
    #[serde(rename = "integration.unavailable")]
    /// 下载器暂时不可用。
    IntegrationUnavailable,
    #[serde(rename = "integration.unsupported-version")]
    /// 下载器协议版本低于兼容基线。
    UnsupportedVersion,
    #[serde(rename = "provider.timeout")]
    /// 有界请求超时。
    ProviderTimeout,
    #[serde(rename = "provider.response-too-large")]
    /// 响应超过本地字节预算。
    ResponseTooLarge,
    #[serde(rename = "provider.invalid-response")]
    /// 响应不符合已知协议。
    InvalidResponse,
    #[serde(rename = "download.correlation-ambiguous")]
    /// 恢复关联时匹配到多个远端任务。
    CorrelationAmbiguous,
    #[serde(rename = "download.remote-missing")]
    /// 已关联远端任务不存在。
    RemoteMissing,
}

impl DownloaderFailureCode {
    #[must_use]
    /// 返回 `SQLite` 与线协议使用的稳定值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IntegrationNotConfigured => "integration.not-configured",
            Self::IntegrationUnauthorized => "integration.unauthorized",
            Self::IntegrationRateLimited => "integration.rate-limited",
            Self::IntegrationUnavailable => "integration.unavailable",
            Self::UnsupportedVersion => "integration.unsupported-version",
            Self::ProviderTimeout => "provider.timeout",
            Self::ResponseTooLarge => "provider.response-too-large",
            Self::InvalidResponse => "provider.invalid-response",
            Self::CorrelationAmbiguous => "download.correlation-ambiguous",
            Self::RemoteMissing => "download.remote-missing",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "integration.not-configured" => Some(Self::IntegrationNotConfigured),
            "integration.unauthorized" => Some(Self::IntegrationUnauthorized),
            "integration.rate-limited" => Some(Self::IntegrationRateLimited),
            "integration.unavailable" => Some(Self::IntegrationUnavailable),
            "integration.unsupported-version" => Some(Self::UnsupportedVersion),
            "provider.timeout" => Some(Self::ProviderTimeout),
            "provider.response-too-large" => Some(Self::ResponseTooLarge),
            "provider.invalid-response" => Some(Self::InvalidResponse),
            "download.correlation-ambiguous" => Some(Self::CorrelationAmbiguous),
            "download.remote-missing" => Some(Self::RemoteMissing),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 下载任务的本地持久状态。
pub enum DownloadTaskStatus {
    /// 已验收，等待 worker。
    Queued,
    /// 正在创建或恢复远端任务关联。
    Submitting,
    /// 已关联，正在同步远端进度。
    Monitoring,
    /// 可恢复失败后等待重试。
    RetryWait,
    /// 远端下载已完成。
    Completed,
    /// 下载任务不可恢复地失败。
    Failed,
}

impl DownloadTaskStatus {
    #[must_use]
    /// 返回 `SQLite` 与线协议共用的稳定值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Submitting => "submitting",
            Self::Monitoring => "monitoring",
            Self::RetryWait => "retry-wait",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "submitting" => Some(Self::Submitting),
            "monitoring" => Some(Self::Monitoring),
            "retry-wait" => Some(Self::RetryWait),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
/// 创建手工下载任务的含密命令。
pub struct CreateDownloadTaskCommand {
    /// 目标下载器连接。
    pub connection_id: Uuid,
    /// magnet 或 HTTPS torrent URL；只在加密边界短暂解密。
    pub source: SecretString,
    /// 安全的用户可见任务名。
    pub display_name: String,
}

impl std::fmt::Debug for CreateDownloadTaskCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CreateDownloadTaskCommand")
            .field("connection_id", &self.connection_id)
            .field("source", &"[REDACTED]")
            .field("display_name", &self.display_name)
            .finish()
    }
}

pub(crate) struct ValidatedDownloadTaskCommand {
    pub connection_id: Uuid,
    pub source: SecretBytes,
    pub display_name: String,
}

impl CreateDownloadTaskCommand {
    pub(crate) fn validate(self) -> Result<ValidatedDownloadTaskCommand, AppError> {
        let display_name = self.display_name.trim().to_owned();
        if !(1..=512).contains(&display_name.chars().count())
            || display_name.chars().any(char::is_control)
            || !(1..=8192).contains(&self.source.len())
        {
            return Err(task_validation_error());
        }
        let source = self.source.into_secret_bytes();
        let text = std::str::from_utf8(source.expose()).map_err(|_| task_validation_error())?;
        let parsed = Url::parse(text).map_err(|_| task_validation_error())?;
        let valid = match parsed.scheme() {
            "magnet" => parsed.query_pairs().any(|(key, value)| {
                key == "xt" && value.starts_with("urn:btih:") && value.len() > 9
            }),
            "https" => {
                parsed.host_str().is_some()
                    && parsed.username().is_empty()
                    && parsed.password().is_none()
                    && parsed.fragment().is_none()
            }
            _ => false,
        };
        if !valid {
            return Err(task_validation_error());
        }
        Ok(ValidatedDownloadTaskCommand {
            connection_id: self.connection_id,
            source,
            display_name,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 下载任务列表的稳定可组合过滤条件。
pub struct DownloadTaskFilter {
    /// 只返回指定连接的任务。
    pub connection_id: Option<Uuid>,
    /// 只返回指定本地状态的任务。
    pub status: Option<DownloadTaskStatus>,
    /// 对显示名执行大小写不敏感包含匹配。
    pub query: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 不含下载源、密文、幂等键或租约信息的下载任务投影。
pub struct DownloadTaskView {
    /// 本地稳定 UUID。
    pub id: Uuid,
    /// 目标下载器连接 UUID。
    pub connection_id: Uuid,
    /// 创建时快照的连接显示名。
    pub connection_display_name: String,
    /// 用户可见任务名。
    pub display_name: String,
    /// 本地持久状态。
    pub status: DownloadTaskStatus,
    /// 已关联时的产品无关远端状态。
    pub remote_status: Option<String>,
    /// 0 到 10000 的完成度基点。
    pub progress_basis_points: u16,
    /// 失败时的稳定脱敏分类。
    pub failure_code: Option<DownloaderFailureCode>,
    /// 可恢复失败的 RFC3339 重试时间。
    pub retry_at: Option<String>,
    /// 是否已经持久关联远端 hash。
    pub linked: bool,
    /// 单调递增的投影版本。
    pub projection_version: i64,
    /// RFC3339 创建时间。
    pub created_at: String,
    /// RFC3339 更新时间。
    pub updated_at: String,
}

/// worker 在一个有界租约中处理下载任务所需的私有快照。
pub struct DownloadTaskLease {
    /// 被声明的任务 ID。
    pub task_id: Uuid,
    /// 目标连接 ID。
    pub connection_id: Uuid,
    /// 解密后的短生命周期下载源。
    pub source: SecretBytes,
    /// 已有关联时的稳定远端 hash。
    pub remote_id: Option<String>,
    /// 用于崩溃恢复的公开稳定 tag。
    pub correlation_tag: String,
    /// 包含当前声明在内的持久尝试次数。
    pub attempt_count: u32,
}

impl std::fmt::Debug for DownloadTaskLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DownloadTaskLease")
            .field("task_id", &self.task_id)
            .field("connection_id", &self.connection_id)
            .field("source", &"[REDACTED]")
            .field("remote_id", &self.remote_id)
            .field("correlation_tag", &self.correlation_tag)
            .field("attempt_count", &self.attempt_count)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 最近一次协议协商得到的产品无关能力。
pub struct DownloaderCapabilities {
    /// 能否新增 magnet 或 HTTPS torrent URL。
    pub manual_add: bool,
    /// 能否读取 `MediaFlow` 自有远端任务状态。
    pub task_monitoring: bool,
    /// 下载器产品版本。
    pub product_version: String,
    /// Web API 或 RPC 语义版本。
    pub api_version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
/// 保存连接或执行候选连接测试时使用的含密输入。
pub struct DownloaderConnectionInput {
    /// 内置下载器类型。
    pub kind: DownloaderKind,
    /// 管理界面显示名。
    pub display_name: String,
    /// 不含 userinfo、query 或 fragment 的 HTTP(S) 基地址。
    pub base_url: String,
    /// 下载器用户名；允许为空。
    pub username: SecretString,
    /// 下载器密码；允许为空。
    pub password: SecretString,
    /// 是否允许 worker 调用该连接。
    pub enabled: bool,
}

impl std::fmt::Debug for DownloaderConnectionInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DownloaderConnectionInput")
            .field("kind", &self.kind)
            .field("display_name", &self.display_name)
            .field("base_url", &self.base_url)
            .field("username", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .field("enabled", &self.enabled)
            .finish()
    }
}

pub(crate) struct ValidatedDownloaderConnectionInput {
    pub kind: DownloaderKind,
    pub display_name: String,
    pub base_url: String,
    pub credentials: SecretBytes,
    pub enabled: bool,
}

impl DownloaderConnectionInput {
    pub(crate) fn validate(self) -> Result<ValidatedDownloaderConnectionInput, AppError> {
        let display_name = self.display_name.trim().to_owned();
        let display_len = display_name.chars().count();
        let username_len = self.username.len();
        let password_len = self.password.len();
        if !(1..=120).contains(&display_len) || username_len > 256 || password_len > 4096 {
            return Err(validation_error());
        }
        let parsed = Url::parse(&self.base_url).map_err(|_| validation_error())?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.cannot_be_a_base()
        {
            return Err(validation_error());
        }
        let mut base_url = parsed.to_string();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        if base_url.is_empty() || base_url.len() > 2048 {
            return Err(validation_error());
        }
        let username = self.username.into_secret_bytes();
        let password = self.password.into_secret_bytes();
        let mut encoded = Vec::with_capacity(8 + username_len + password_len);
        encoded.extend_from_slice(
            &u32::try_from(username_len)
                .map_err(|_| validation_error())?
                .to_be_bytes(),
        );
        encoded.extend_from_slice(username.expose());
        encoded.extend_from_slice(
            &u32::try_from(password_len)
                .map_err(|_| validation_error())?
                .to_be_bytes(),
        );
        encoded.extend_from_slice(password.expose());
        Ok(ValidatedDownloaderConnectionInput {
            kind: self.kind,
            display_name,
            base_url,
            credentials: SecretBytes::new(encoded),
            enabled: self.enabled,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 不含凭据、Cookie 或上游正文的连接持久投影。
pub struct DownloaderConnection {
    /// 本地稳定 UUID。
    pub id: Uuid,
    /// 内置下载器类型。
    pub kind: DownloaderKind,
    /// 管理界面显示名。
    pub display_name: String,
    /// 规范化基地址。
    pub base_url: String,
    /// 是否允许自动调用。
    pub enabled: bool,
    /// 单调递增配置版本。
    pub config_version: i64,
    /// 最近一次协商能力。
    pub capabilities: Option<DownloaderCapabilities>,
    /// 最近一次脱敏健康分类。
    pub health: IntegrationHealth,
    /// 最近一次稳定失败分类。
    pub failure_code: Option<DownloaderFailureCode>,
    /// 最近一次探测时间，Unix epoch 微秒。
    pub checked_at_us: Option<i64>,
    /// 最近一次配置或探测更新时间，Unix epoch 微秒。
    pub updated_at_us: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// HTTP API 返回的 RFC3339 脱敏连接投影。
pub struct DownloaderConnectionView {
    /// 本地稳定 UUID。
    pub id: Uuid,
    /// 内置下载器类型。
    pub kind: DownloaderKind,
    /// 管理界面显示名。
    pub display_name: String,
    /// 规范化基地址。
    pub base_url: String,
    /// 是否允许自动调用。
    pub enabled: bool,
    /// 单调递增配置版本。
    pub config_version: i64,
    /// 最近一次协商能力。
    pub capabilities: Option<DownloaderCapabilities>,
    /// 最近一次脱敏健康分类。
    pub health: IntegrationHealth,
    /// 最近一次检查的 RFC3339 时间。
    pub checked_at: Option<String>,
    /// 最近一次稳定失败分类。
    pub failure_code: Option<DownloaderFailureCode>,
    /// 最近一次配置或探测更新的 RFC3339 时间。
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 不落盘候选连接测试的公开结果。
pub struct DownloaderConnectionTestResult {
    /// 身份验证和必要能力协商是否成功。
    pub reachable: bool,
    /// 脱敏健康分类。
    pub health: IntegrationHealth,
    /// 成功时的能力投影。
    pub capabilities: Option<DownloaderCapabilities>,
    /// 失败时的稳定分类。
    pub failure_code: Option<DownloaderFailureCode>,
    /// 检查完成的 RFC3339 时间。
    pub checked_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一次下载器探测可原子提交的公开投影。
pub struct DownloaderConnectionProbe {
    /// 探测后的健康状态。
    pub health: IntegrationHealth,
    /// 失败时的稳定分类。
    pub failure_code: Option<DownloaderFailureCode>,
    /// 成功协商后的能力；失败时可以为空。
    pub capabilities: Option<DownloaderCapabilities>,
}

/// 解密后只允许借用到协议适配器边界的凭据。
pub struct LoadedDownloaderCredentials {
    username: SecretBytes,
    password: SecretBytes,
}

impl LoadedDownloaderCredentials {
    #[must_use]
    /// 从已受保护的短生命周期秘密字节构造协议调用凭据。
    pub const fn new(username: SecretBytes, password: SecretBytes) -> Self {
        Self { username, password }
    }

    #[must_use]
    /// 借用用户名秘密字节。
    pub const fn username(&self) -> &SecretBytes {
        &self.username
    }

    #[must_use]
    /// 借用密码秘密字节。
    pub const fn password(&self) -> &SecretBytes {
        &self.password
    }

    pub(crate) fn decode(encoded: &[u8]) -> Result<Self, AppError> {
        let (username, rest) = take_field(encoded)?;
        let (password, trailing) = take_field(rest)?;
        if !trailing.is_empty() || username.len() > 256 || password.len() > 4096 {
            return Err(invalid_secret_record());
        }
        Ok(Self::new(
            SecretBytes::new(username.to_vec()),
            SecretBytes::new(password.to_vec()),
        ))
    }
}

impl std::fmt::Debug for LoadedDownloaderCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoadedDownloaderCredentials")
            .field("username", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .finish()
    }
}

fn take_field(encoded: &[u8]) -> Result<(&[u8], &[u8]), AppError> {
    let prefix: [u8; 4] = encoded
        .get(..4)
        .ok_or_else(invalid_secret_record)?
        .try_into()
        .map_err(|_| invalid_secret_record())?;
    let length =
        usize::try_from(u32::from_be_bytes(prefix)).map_err(|_| invalid_secret_record())?;
    let end = 4_usize
        .checked_add(length)
        .ok_or_else(invalid_secret_record)?;
    let field = encoded.get(4..end).ok_or_else(invalid_secret_record)?;
    let rest = encoded.get(end..).ok_or_else(invalid_secret_record)?;
    Ok((field, rest))
}

fn validation_error() -> AppError {
    AppError::new(ErrorCode::ValidationFailed, "invalid downloader connection")
}

fn task_validation_error() -> AppError {
    AppError::new(ErrorCode::ValidationFailed, "invalid download task")
}

fn invalid_secret_record() -> AppError {
    AppError::new(ErrorCode::Internal, "invalid downloader credential record")
}
