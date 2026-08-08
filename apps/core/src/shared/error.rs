use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 失败 Core 操作面向客户端的稳定分类。
///
/// [`Self::as_str`] 是线上的错误码。多个内部分类会有意归并为 `internal.error`，
/// 以免泄露配置、数据库、备份和诊断详情。
pub enum ErrorCode {
    /// 账户已存在后仍请求初始管理员创建。
    BootstrapAlreadyCompleted,
    /// 引导凭据与本地引导密钥不匹配。
    BootstrapInvalidSecret,
    /// 管理员名称或密码认证失败。
    InvalidCredentials,
    /// 来自该来源的登录尝试被暂时限流。
    RateLimited,
    /// 提供的会话令牌缺失、已过期、已撤销或不可用。
    SessionExpired,
    /// 状态变更请求缺少绑定到会话的 CSRF 证明。
    CsrfInvalid,
    /// 状态变更请求不匹配已配置的公开源。
    OriginUntrusted,
    /// 请求的部署根目录标识符尚未配置。
    RootNotFound,
    /// 已配置的部署根目录无法打开或身份已改变。
    RootUnavailable,
    /// 调用方提供的路径或根目录标识符不符合允许的语法。
    PathInvalid,
    /// 解析请求路径会离开其已配置的部署根目录。
    PathEscape,
    /// 能力遍历在禁止链接的位置遇到了符号链接。
    PathSymlinkForbidden,
    /// 新收件箱与现有收件箱相等、包含现有收件箱或被其包含。
    InboxOverlap,
    /// 请求的收件箱目录不存在，无法执行该操作。
    InboxNotFound,
    /// 已认证账户无权查看请求的扫描任务。
    TaskNotFound,
    /// 请求的任务状态转换在当前状态下无效。
    TaskInvalidState,
    /// 工作器在失去租约/版本声明后尝试修改任务。
    TaskLeaseLost,
    /// 乐观并发配置版本不再匹配当前持久投影。
    ConfigVersionConflict,
    /// TMDB 尚未配置可解密凭据。
    IntegrationNotConfigured,
    /// TMDB 拒绝了候选或持久凭据。
    IntegrationUnauthorized,
    /// TMDB 要求调用方稍后重试。
    IntegrationRateLimited,
    /// 元数据 provider 当前不可用，但任务可恢复。
    ProviderUnavailable,
    /// 幂等键已绑定到不同的请求参数。
    RequestConflict,
    /// 整理目标被配置到只读部署根。
    OrganizationRootReadOnly,
    /// 整理目标与收件箱或其他目标重叠。
    OrganizationTargetOverlap,
    /// 整理目标目录当前无法通过能力重新验证。
    OrganizationTargetUnavailable,
    /// 资源仍被活动任务引用，当前不能删除。
    ResourceConflict,
    /// An automation source is disabled for new event acceptance.
    AutomationSourceDisabled,
    /// Inbound webhook authentication failed without disclosing the reason.
    AutomationSignatureInvalid,
    /// A webhook nonce was already accepted.
    AutomationReplay,
    /// A signed action is outside the source's fixed allow-list.
    AutomationActionInvalid,
    /// A signed fixed-action payload is invalid.
    AutomationPayloadInvalid,
    /// A signed request body exceeds the fixed byte budget.
    PayloadTooLarge,
    /// 请求语法或调用方提供的值未通过校验。
    ValidationFailed,
    /// 必需的 `SQLite` 备份或其验证失败。
    BackupFailed,
    /// 启动配置格式错误或不安全。
    ConfigInvalid,
    /// 无法安全打开、验证或迁移 `SQLite` 数据库。
    DatabaseInvalid,
    /// 请求恢复到已包含数据的配置目录。
    RestoreTargetNotEmpty,
    /// 未找到非领域专属的 HTTP 资源。
    NotFound,
    /// 启动安全或依赖健康状态使业务路由保持关闭。
    NotReady,
    /// 仅可作为通用错误暴露的意外内部失败。
    Internal,
}

impl ErrorCode {
    #[must_use]
    /// 返回在 HTTP 错误封装中序列化的稳定错误码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BootstrapAlreadyCompleted => "bootstrap.already_completed",
            Self::BootstrapInvalidSecret => "bootstrap.invalid_secret",
            Self::InvalidCredentials => "auth.invalid_credentials",
            Self::RateLimited => "auth.rate_limited",
            Self::SessionExpired => "session.expired",
            Self::CsrfInvalid => "csrf.invalid",
            Self::OriginUntrusted => "origin.untrusted",
            Self::RootNotFound => "root.not_found",
            Self::RootUnavailable => "root.unavailable",
            Self::PathInvalid => "path.invalid",
            Self::PathEscape => "path.escape",
            Self::PathSymlinkForbidden => "path.symlink_forbidden",
            Self::InboxOverlap => "inbox.overlap",
            Self::InboxNotFound => "inbox.not_found",
            Self::TaskNotFound => "task.not_found",
            Self::TaskInvalidState => "task.invalid_state",
            Self::TaskLeaseLost => "task.lease_lost",
            Self::ConfigVersionConflict | Self::RequestConflict => "request.conflict",
            Self::OrganizationRootReadOnly => "organization.root-read-only",
            Self::OrganizationTargetOverlap => "organization.target-overlap",
            Self::OrganizationTargetUnavailable => "organization.target-unavailable",
            Self::ResourceConflict => "resource.conflict",
            Self::AutomationSourceDisabled => "automation.source-disabled",
            Self::AutomationSignatureInvalid => "automation.signature-invalid",
            Self::AutomationReplay => "automation.replay",
            Self::AutomationActionInvalid => "automation.action-invalid",
            Self::AutomationPayloadInvalid | Self::PayloadTooLarge => "automation.payload-invalid",
            Self::IntegrationNotConfigured => "integration.not_configured",
            Self::IntegrationUnauthorized => "integration.unauthorized",
            Self::IntegrationRateLimited => "integration.rate_limited",
            Self::ProviderUnavailable => "provider.unavailable",
            Self::ValidationFailed => "validation.failed",
            Self::NotFound
            | Self::NotReady
            | Self::BackupFailed
            | Self::ConfigInvalid
            | Self::DatabaseInvalid
            | Self::RestoreTargetNotEmpty
            | Self::Internal => "internal.error",
        }
    }
}

/// 具有私有诊断信息和可选公开详情的边界安全应用错误。
///
/// `Display`、`Debug` 与 HTTP 转换绝不暴露传给 [`Self::new`] 或 [`Self::with_source`] 的诊断信息。
/// [`ErrorCode`] 选择状态和安全消息；只有通过 [`Self::with_detail`] 添加的值会作为结构化详情序列化。
pub struct AppError {
    code: ErrorCode,
    _diagnostic: String,
    details: BTreeMap<String, Value>,
}

impl AppError {
    /// 使用稳定分类与私有诊断消息创建错误。
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            _diagnostic: message.into(),
            details: BTreeMap::new(),
        }
    }

    #[must_use]
    /// 添加或替换面向客户端的结构化详情并返回更新后的错误。
    ///
    /// 调用方只能传入可安全序列化到进程外的值。
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    /// 创建私有诊断信息为所提供源错误文本的错误。
    pub fn with_source(code: ErrorCode, source: impl std::error::Error) -> Self {
        Self::new(code, source.to_string())
    }

    #[must_use]
    /// 返回用于状态与线上错误码映射的稳定分类。
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    #[must_use]
    /// 返回该错误分类对应的固定最终用户消息。
    ///
    /// 结果绝不包含私有诊断或源错误文本。
    pub fn safe_message(&self) -> &'static str {
        match self.code {
            ErrorCode::BootstrapAlreadyCompleted => "The system has already been initialized.",
            ErrorCode::BootstrapInvalidSecret => "The bootstrap credentials are invalid.",
            ErrorCode::InvalidCredentials => "The administrator name or password is invalid.",
            ErrorCode::RateLimited => "Too many login attempts. Try again later.",
            ErrorCode::SessionExpired => "The session has expired.",
            ErrorCode::CsrfInvalid => "The request security token is invalid.",
            ErrorCode::OriginUntrusted => "The request origin is not trusted.",
            ErrorCode::RootNotFound => "The deployment root was not found.",
            ErrorCode::RootUnavailable => "The deployment root is unavailable.",
            ErrorCode::PathInvalid => "The relative path is invalid.",
            ErrorCode::PathEscape => "The relative path escapes the deployment root.",
            ErrorCode::PathSymlinkForbidden => "Symbolic links are not allowed.",
            ErrorCode::InboxOverlap => "The inbox directory overlaps an existing directory.",
            ErrorCode::InboxNotFound => "The inbox directory was not found.",
            ErrorCode::TaskNotFound => "The scan task was not found.",
            ErrorCode::TaskInvalidState => "The scan task is not in a valid state for this action.",
            ErrorCode::TaskLeaseLost => "The scan task lease is no longer valid.",
            ErrorCode::ConfigVersionConflict => "The resource version has changed.",
            ErrorCode::IntegrationNotConfigured => "The integration is not configured.",
            ErrorCode::IntegrationUnauthorized => "The integration credentials were rejected.",
            ErrorCode::IntegrationRateLimited => {
                "The integration is rate limited. Try again later."
            }
            ErrorCode::ProviderUnavailable => {
                "The metadata provider is unavailable. Try again later."
            }
            ErrorCode::RequestConflict => {
                "The idempotency key is already bound to another request."
            }
            ErrorCode::OrganizationRootReadOnly => "The organization target root is read-only.",
            ErrorCode::OrganizationTargetOverlap => {
                "The organization target overlaps existing configuration."
            }
            ErrorCode::OrganizationTargetUnavailable => "The organization target is unavailable.",
            ErrorCode::ResourceConflict => "The resource is still in use.",
            ErrorCode::AutomationSourceDisabled => "The automation source is disabled.",
            ErrorCode::AutomationSignatureInvalid => "The webhook signature is invalid.",
            ErrorCode::AutomationReplay => "The webhook request has already been accepted.",
            ErrorCode::AutomationActionInvalid => "The automation action is not allowed.",
            ErrorCode::AutomationPayloadInvalid | ErrorCode::PayloadTooLarge => {
                "The automation payload is invalid."
            }
            ErrorCode::ValidationFailed => "The request is invalid.",
            ErrorCode::ConfigInvalid => "The server configuration is invalid.",
            ErrorCode::DatabaseInvalid => "The database could not be verified.",
            ErrorCode::BackupFailed => "The database backup could not be completed.",
            ErrorCode::RestoreTargetNotEmpty => "The restore target must be empty.",
            ErrorCode::NotFound => "The requested resource was not found.",
            ErrorCode::NotReady => "The service is not ready.",
            ErrorCode::Internal => "The request could not be completed.",
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.safe_message())
    }
}

impl std::fmt::Debug for AppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppError")
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}

impl std::error::Error for AppError {}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
    request_id: Uuid,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    details: BTreeMap<String, Value>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self.code {
            ErrorCode::BootstrapInvalidSecret
            | ErrorCode::InvalidCredentials
            | ErrorCode::SessionExpired
            | ErrorCode::IntegrationUnauthorized
            | ErrorCode::AutomationSignatureInvalid => StatusCode::UNAUTHORIZED,
            ErrorCode::BootstrapAlreadyCompleted
            | ErrorCode::RootUnavailable
            | ErrorCode::InboxOverlap
            | ErrorCode::TaskInvalidState
            | ErrorCode::TaskLeaseLost
            | ErrorCode::ConfigVersionConflict
            | ErrorCode::IntegrationNotConfigured
            | ErrorCode::RequestConflict
            | ErrorCode::OrganizationTargetOverlap
            | ErrorCode::ResourceConflict => StatusCode::CONFLICT,
            ErrorCode::AutomationSourceDisabled | ErrorCode::AutomationReplay => {
                StatusCode::CONFLICT
            }
            ErrorCode::RateLimited | ErrorCode::IntegrationRateLimited => {
                StatusCode::TOO_MANY_REQUESTS
            }
            ErrorCode::CsrfInvalid | ErrorCode::OriginUntrusted => StatusCode::FORBIDDEN,
            ErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::ValidationFailed
            | ErrorCode::OrganizationRootReadOnly
            | ErrorCode::PathInvalid
            | ErrorCode::PathEscape
            | ErrorCode::PathSymlinkForbidden => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::AutomationActionInvalid | ErrorCode::AutomationPayloadInvalid => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            ErrorCode::NotFound
            | ErrorCode::RootNotFound
            | ErrorCode::InboxNotFound
            | ErrorCode::TaskNotFound => StatusCode::NOT_FOUND,
            ErrorCode::NotReady
            | ErrorCode::ProviderUnavailable
            | ErrorCode::OrganizationTargetUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code: self.code.as_str(),
                    message: self.safe_message(),
                    request_id: Uuid::now_v7(),
                    details: self.details,
                },
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{AppError, ErrorCode};

    #[test]
    fn default_display_and_debug_do_not_leak_internal_diagnostics() {
        let sentinel = "/private/nas/secret.sqlite SELECT password_hash";
        let error = AppError::new(ErrorCode::DatabaseInvalid, sentinel);

        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
        assert!(error.to_string().contains("database could not be verified"));
    }
}
