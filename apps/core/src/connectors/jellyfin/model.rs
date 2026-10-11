//! Jellyfin 连接的公开投影与配置命令。

use crate::connectors::model::SecretString;
use serde::Serialize;

use crate::connectors::model::{IntegrationFailureCode, IntegrationHealth};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Jellyfin 连接的脱敏投影；不回传凭据。
pub struct JellyfinIntegrationView {
    /// 此端点固定返回 `jellyfin`。
    pub kind: &'static str,
    /// 加密凭据列是否已经写入。
    pub configured: bool,
    /// 管理员配置的服务基址（公开信息，用于打开首页）。
    pub base_url: Option<String>,
    /// 管理员可读的服务显示名。
    pub display_name: Option<String>,
    /// 最近一次连接测试观察到的服务器版本。
    pub server_version: Option<String>,
    /// 单调递增的乐观并发版本；零表示从未存在配置记录。
    pub config_version: i64,
    /// 最近一次安全健康分类。
    pub health: IntegrationHealth,
    /// 可安全展示的稳定失败码。
    pub failure_code: Option<IntegrationFailureCode>,
    /// 最近一次权威检查的 UTC RFC3339 时间。
    pub checked_at: Option<String>,
}

/// 保存或测试一份 Jellyfin 连接所需的候选输入。
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct JellyfinConfigCommand {
    /// 候选服务基址（`http://` 或 `https://` 绝对地址）。
    pub base_url: String,
    /// 管理员可读显示名。
    pub display_name: Option<String>,
    /// Jellyfin API 凭据（`X-Emby-Token`）。
    pub api_token: SecretString,
}

impl std::fmt::Debug for JellyfinConfigCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JellyfinConfigCommand")
            .field("base_url", &self.base_url)
            .field("display_name", &self.display_name)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
/// 不落盘的连接测试结果。
pub struct JellyfinConnectionTestResult {
    /// Ping 与已认证信息请求是否全部成功。
    pub reachable: bool,
    /// 稳定健康分类。
    pub health: IntegrationHealth,
    /// 稳定失败码；成功时为 `None`。
    pub failure_code: Option<IntegrationFailureCode>,
    /// 观察到的服务器版本（脱敏投影）。
    pub server_version: Option<String>,
    /// UTC RFC3339 检查时间。
    pub checked_at: String,
}
