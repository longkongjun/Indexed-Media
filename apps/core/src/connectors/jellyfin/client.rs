//! Jellyfin 连接测试的 HTTP 适配器与可注入测试端口。

use async_trait::async_trait;

use crate::connectors::jellyfin::model::JellyfinConnectionTestResult;
use crate::connectors::model::{IntegrationFailureCode, IntegrationHealth};

/// 一次 Jellyfin 连接测试的端口；生产实现走有界 HTTP，测试注入内存实现。
#[async_trait]
pub trait JellyfinConnectionTester: Send + Sync {
    /// 以候选地址与凭据执行不落盘测试。
    ///
    /// # Errors
    ///
    /// 上游不可达、凭据被拒或响应异常时返回稳定失败码。
    async fn test(
        &self,
        base_url: &str,
        token: &[u8],
    ) -> Result<JellyfinProbe, IntegrationFailureCode>;
}

/// 连接测试观察到的服务器事实。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JellyfinProbe {
    /// `/System/Info` 报告的服务器版本（脱敏投影）。
    pub server_version: String,
}

/// 有界生产测试器：`GET /System/Ping` + `GET /System/Info`（`X-Emby-Token`）。
pub struct HttpJellyfinTester {
    client: reqwest::Client,
}

impl HttpJellyfinTester {
    #[must_use]
    pub fn production() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[async_trait]
impl JellyfinConnectionTester for HttpJellyfinTester {
    async fn test(
        &self,
        base_url: &str,
        token: &[u8],
    ) -> Result<JellyfinProbe, IntegrationFailureCode> {
        let base =
            reqwest::Url::parse(base_url).map_err(|_| IntegrationFailureCode::InvalidResponse)?;
        let info_url = base
            .join("System/Info")
            .map_err(|_| IntegrationFailureCode::InvalidResponse)?;
        let response = self
            .client
            .get(info_url)
            .header("X-Emby-Token", String::from_utf8_lossy(token).as_bytes())
            .send()
            .await
            .map_err(|_| IntegrationFailureCode::Unavailable)?;
        match response.status().as_u16() {
            200 => {
                let body: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|_| IntegrationFailureCode::InvalidResponse)?;
                let version = body
                    .get("Version")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                if version.is_empty() {
                    return Err(IntegrationFailureCode::InvalidResponse);
                }
                Ok(JellyfinProbe {
                    server_version: version.chars().take(64).collect(),
                })
            }
            401 | 403 => Err(IntegrationFailureCode::Unauthorized),
            _ => Err(IntegrationFailureCode::Unavailable),
        }
    }
}

/// 把端口结果映射为公开测试投影。
#[must_use]
pub fn probe_to_result(
    probe: Result<JellyfinProbe, IntegrationFailureCode>,
    now: chrono::DateTime<chrono::Utc>,
) -> JellyfinConnectionTestResult {
    match probe {
        Ok(found) => JellyfinConnectionTestResult {
            reachable: true,
            health: IntegrationHealth::Healthy,
            failure_code: None,
            server_version: Some(found.server_version),
            checked_at: now.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        },
        Err(code) => JellyfinConnectionTestResult {
            reachable: matches!(code, IntegrationFailureCode::Unauthorized),
            health: if matches!(code, IntegrationFailureCode::Unauthorized) {
                IntegrationHealth::Unauthorized
            } else {
                IntegrationHealth::Unavailable
            },
            failure_code: Some(code),
            server_version: None,
            checked_at: now.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        },
    }
}
