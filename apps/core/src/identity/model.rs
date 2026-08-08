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
/// 与其活跃管理员账户关联的有效、未撤销会话。
pub struct AuthenticatedSession {
    /// 用于轮换和撤销的标识符。
    pub session_id: Uuid,
    /// 拥有该会话的活跃管理员。
    pub account: AccountSummary,
    /// 用于常量时间请求验证的、已持久化 32 字节 CSRF 摘要。
    pub csrf_sha256: [u8; 32],
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
