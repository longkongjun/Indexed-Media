use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Deserialize)]
/// 用于创建唯一管理员账户的一次性凭据。
///
/// 自定义的 `Debug` 实现会隐藏两个秘密值。服务要求管理员名称非空、密码至少 12 个字符，且引导秘密值必须匹配。
pub struct BootstrapCommand {
    /// 通过本地引导秘密值源提供的秘密值。
    pub bootstrap_secret: String,
    /// 管理员显示名称；匹配前会执行去除空白和转小写的规范化。
    pub administrator_name: String,
    /// 由密码引擎使用的管理员明文密码。
    pub password: String,
}

#[derive(Clone, Deserialize)]
/// 会话创建端点接受的 JSON 凭据。
///
/// 其 `Debug` 输出会隐藏 `password`；明文仅保留至完成登录验证。
pub struct LoginRequest {
    /// 管理员名称；身份服务会在查询前将其规范化。
    pub administrator_name: String,
    /// 待验证的明文密码。
    pub password: String,
}

#[derive(Clone)]
/// 登录凭据及用于持久化限流、保护隐私的来源信息。
///
/// 自定义的 `Debug` 实现会隐藏密码和来源键。
pub struct LoginCommand {
    /// 待规范化并查询的管理员名称。
    pub administrator_name: String,
    /// 待验证的明文密码。
    pub password: String,
    /// 持久化前会进行域分隔哈希的规范客户端来源字符串。
    pub source_key: String,
}

#[derive(Clone, Debug, Serialize)]
/// 唯一管理员账户的公开身份；绝不包含凭据材料。
pub struct AccountSummary {
    /// 稳定的账户 UUID。
    pub id: Uuid,
    /// 引导时选择的、已去除空白的原始显示名称。
    pub administrator_name: String,
}

#[derive(Clone, Debug, Serialize)]
/// 一次性管理员创建成功响应。
pub struct BootstrapResponse {
    /// 新近持久化的管理员账户。
    pub account: AccountSummary,
    /// 响应架构版本，当前为 `"v1"`。
    pub version: &'static str,
}

#[derive(Clone, Debug, Serialize)]
/// 与安全会话 Cookie 一同返回的已认证会话元数据。
pub struct SessionResponse {
    /// 由会话认证的管理员。
    pub account: AccountSummary,
    /// 客户端必须在变更状态请求中回显的原始会话绑定 CSRF 令牌。
    pub csrf_token: String,
    /// 响应架构版本，当前为 `"v1"`。
    pub version: &'static str,
}

#[derive(Clone, Debug, Serialize)]
/// 指示本地安装是否仍需创建管理员账户。
pub struct BootstrapStatusResponse {
    /// 仅在 `SQLite` 中尚无身份账户时为 `true`。
    pub requires_initialization: bool,
    /// 响应架构版本，当前为 `"v1"`。
    pub version: &'static str,
}

#[derive(Clone)]
/// 新近持久化的会话，以及仅向 HTTP 层返回一次的原始凭据。
///
/// 仅持久化 `raw_token` 和 `csrf_token` 的 SHA-256 摘要；`Debug` 会隐藏两个值。
pub struct NewSession {
    /// 已持久化会话行的稳定标识符。
    pub id: Uuid,
    /// 放入安全、仅 HTTP 会话 Cookie 的 Bearer 令牌。
    pub raw_token: String,
    /// 在响应体中返回的 CSRF 令牌。
    pub csrf_token: String,
    /// 由会话认证的管理员。
    pub account: AccountSummary,
}

#[derive(Clone, Debug)]
/// 请求凭据的来源通道；cookie 会话与设备令牌互不可替换。
pub enum Principal {
    /// 浏览器 `__Host-mediaflow_session` cookie 认证的会话。
    Session {
        /// 用于 CSRF 轮换和撤销的会话行标识符。
        session_id: Uuid,
    },
    /// `Authorization: Bearer mfdev_…` 认证的设备（M5 第二批）。
    Device {
        /// 用于自轮换与自撤销的设备行标识符。
        device_id: Uuid,
    },
}

#[derive(Clone, Debug)]
/// 与其活跃管理员账户关联的有效、未撤销的已认证主体。
///
/// 会话通道携带 CSRF 摘要；设备令牌通道按"令牌即写证明"豁免同源/CSRF（见
/// `platform::request_security`），因此没有 CSRF 摘要。
pub struct AuthenticatedSession {
    /// 拥有该凭据的活跃管理员。
    pub account: AccountSummary,
    /// 会话通道的 32 字节 CSRF 摘要；设备通道为 `None`。
    pub csrf_sha256: Option<[u8; 32]>,
    /// 认证主体通道与行标识符。
    pub principal: Principal,
}

impl std::fmt::Debug for BootstrapCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BootstrapCommand")
            .field("bootstrap_secret", &"[REDACTED]")
            .field("administrator_name", &self.administrator_name)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for LoginRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoginRequest")
            .field("administrator_name", &self.administrator_name)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for LoginCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoginCommand")
            .field("administrator_name", &self.administrator_name)
            .field("password", &"[REDACTED]")
            .field("source_key", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for NewSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NewSession")
            .field("id", &self.id)
            .field("raw_token", &"[REDACTED]")
            .field("csrf_token", &"[REDACTED]")
            .field("account", &self.account)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{AccountSummary, BootstrapCommand, LoginCommand, LoginRequest, NewSession};

    #[test]
    fn credential_commands_redact_secrets_from_debug_output() {
        let outputs = [
            format!(
                "{:?}",
                BootstrapCommand {
                    bootstrap_secret: "bootstrap-secret-sentinel".to_owned(),
                    administrator_name: "admin".to_owned(),
                    password: "bootstrap-password-sentinel".to_owned(),
                }
            ),
            format!(
                "{:?}",
                LoginRequest {
                    administrator_name: "admin".to_owned(),
                    password: "request-password-sentinel".to_owned(),
                }
            ),
            format!(
                "{:?}",
                LoginCommand {
                    administrator_name: "admin".to_owned(),
                    password: "command-password-sentinel".to_owned(),
                    source_key: "source-address-sentinel".to_owned(),
                }
            ),
        ];
        for output in outputs {
            assert!(output.contains("[REDACTED]"));
            assert!(!output.contains("sentinel"));
        }
    }

    #[test]
    fn new_session_redacts_raw_tokens_from_debug_output() {
        let output = format!(
            "{:?}",
            NewSession {
                id: uuid::Uuid::nil(),
                raw_token: "raw-session-token-sentinel".to_owned(),
                csrf_token: "raw-csrf-token-sentinel".to_owned(),
                account: AccountSummary {
                    id: uuid::Uuid::nil(),
                    administrator_name: "admin".to_owned(),
                },
            }
        );
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("sentinel"));
    }
}

/// 家庭成员的公开身份；绝不包含凭据材料（`MVP-WEB-003`：管理端对成员完全关闭）。
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct FamilyMemberView {
    /// 成员稳定 UUID。
    pub id: uuid::Uuid,
    /// 已去除空白的显示名称。
    pub display_name: String,
    /// 创建时间 UTC RFC3339。
    pub created_at: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
/// 设备 enrollment 端点接受的配对或凭据材料（二选一）。
///
/// 自定义 `Debug` 隐藏 `password`；配对码与密码在验证后即丢弃。
pub struct DeviceEnrollmentRequest {
    /// 一次性配对码（Web 管理页签发）。与服务端规范化规则一致：忽略大小写、`-` 与空格。
    pub pairing_code: Option<String>,
    /// 凭据模式：管理员名称。
    pub administrator_name: Option<String>,
    /// 凭据模式：管理员明文密码。
    pub password: Option<String>,
    /// 设备显示名称（1..=64 字符），进入设备列表。
    pub device_name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
/// 一次性配对码签发结果；`code` 原文仅此一次返回。
pub struct PairingCodeResponse {
    /// 8 字符 base32 配对码（客户端按原样输入，服务端负责规范化）。
    pub pairing_code: String,
    /// 过期时间 UTC RFC3339；过期后不可用于 enrollment。
    pub expires_at: String,
    /// 响应架构版本，当前为 `"v1"`。
    pub version: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
/// 设备的公开表示；绝不包含令牌材料。
pub struct DeviceView {
    /// 设备稳定 UUID。
    pub id: uuid::Uuid,
    /// enrollment 时提供的显示名称。
    pub display_name: String,
    /// 创建时间 UTC RFC3339。
    pub created_at: String,
    /// 最近一次认证时间 UTC RFC3339；从未使用过为 `None`。
    pub last_used_at: Option<String>,
    /// 令牌绝对过期时间 UTC RFC3339。
    pub expires_at: String,
    /// 撤销时间 UTC RFC3339；未撤销为 `None`。
    pub revoked_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
/// enrollment 成功响应；`token` 原文仅此一次返回。
pub struct DeviceEnrollmentResponse {
    /// 新注册设备的公开表示。
    pub device: DeviceView,
    /// `Authorization: Bearer` 使用的原始设备令牌（`mfdev_` 前缀）。
    pub token: String,
    /// 响应架构版本，当前为 `"v1"`。
    pub version: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
/// 设备令牌自轮换响应；新令牌原文仅此一次返回，旧令牌立即失效。
pub struct DeviceTokenRotationResponse {
    /// 替换后的原始设备令牌。
    pub token: String,
    /// 新令牌的绝对过期时间 UTC RFC3339。
    pub expires_at: String,
    /// 响应架构版本，当前为 `"v1"`。
    pub version: &'static str,
}

impl std::fmt::Debug for DeviceEnrollmentRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceEnrollmentRequest")
            .field("pairing_code", &"[REDACTED]")
            .field("administrator_name", &self.administrator_name)
            .field("password", &"[REDACTED]")
            .field("device_name", &self.device_name)
            .finish()
    }
}

#[cfg(test)]
mod device_model_tests {
    use super::DeviceEnrollmentRequest;

    #[test]
    fn enrollment_request_redacts_secrets_from_debug_output() {
        let output = format!(
            "{:?}",
            DeviceEnrollmentRequest {
                pairing_code: Some("pairing-code-sentinel".to_owned()),
                administrator_name: Some("admin".to_owned()),
                password: Some("password-sentinel".to_owned()),
                device_name: "Pixel 9".to_owned(),
            }
        );
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("sentinel"));
    }
}
