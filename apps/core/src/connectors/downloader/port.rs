use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::model::{DownloaderCapabilities, LoadedDownloaderCredentials};
use crate::platform::secrets::SecretBytes;

#[derive(Clone, Copy)]
/// 一次协议调用所需的脱敏地址与短生命周期凭据。
pub struct DownloaderEndpoint<'a> {
    /// 规范化 HTTP(S) 基地址。
    pub base_url: &'a str,
    /// 仅在调用期间借用的解密凭据。
    pub credentials: &'a LoadedDownloaderCredentials,
}

impl std::fmt::Debug for DownloaderEndpoint<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DownloaderEndpoint")
            .field("base_url", &self.base_url)
            .field("credentials", &"[REDACTED]")
            .finish()
    }
}

/// 新增远端任务所需的含密下载源和公开 correlation tag。
pub struct AddDownloadRequest<'a> {
    /// `magnet:` 或 HTTPS torrent URL 的秘密字节。
    pub source: &'a SecretBytes,
    /// 由本地任务 ID 稳定派生的非秘密 tag。
    pub correlation_tag: &'a str,
}

impl std::fmt::Debug for AddDownloadRequest<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AddDownloadRequest")
            .field("source", &"[REDACTED]")
            .field("correlation_tag", &self.correlation_tag)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一批 `MediaFlow` 自有远端任务或一个待恢复 correlation tag 查询。
pub struct RemoteDownloadQuery {
    /// 已持久关联的远端 torrent hash；最多 100 项。
    pub remote_ids: Vec<String>,
    /// 尚未关联时使用的唯一 correlation tag。
    pub correlation_tag: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 下载器返回的稳定 torrent hash 引用。
pub struct RemoteDownloadRef {
    /// 跨重启稳定的远端 torrent hash。
    pub remote_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 产品无关的远端 torrent 状态。
pub enum RemoteDownloadStatus {
    /// 等待元数据、校验、分配或排队。
    Queued,
    /// 正在下载。
    Downloading,
    /// 下载尚未完成且被暂停。
    Paused,
    /// 已完成下载并可能正在做种。
    Completed,
    /// 下载器明确报告不可恢复错误。
    Failed,
    /// 新版本返回了尚未识别的安全状态。
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一项有界、脱敏的远端下载快照。
pub struct RemoteDownloadSnapshot {
    /// 稳定 torrent hash。
    pub remote_id: String,
    /// 产品无关状态。
    pub status: RemoteDownloadStatus,
    /// `0..=10000` 的完成度基点。
    pub progress_basis_points: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
/// 不携带 URL、凭据、Cookie 或上游正文的下载器协议失败。
pub enum DownloadSourceError {
    /// 下载器拒绝凭据。
    #[error("downloader credentials were rejected")]
    Unauthorized,
    /// 下载器对请求限流。
    #[error("downloader rate limited the request")]
    RateLimited,
    /// 下载器协议版本不受支持。
    #[error("downloader protocol version is unsupported")]
    UnsupportedVersion,
    /// 下载器暂时不可用。
    #[error("downloader is temporarily unavailable")]
    Unavailable,
    /// 有界请求超时。
    #[error("downloader request timed out")]
    Timeout,
    /// 响应超过字节预算。
    #[error("downloader response is too large")]
    ResponseTooLarge,
    /// 响应不符合协议。
    #[error("downloader response is invalid")]
    InvalidResponse,
    /// correlation 查找返回多个远端任务。
    #[error("downloader correlation is ambiguous")]
    CorrelationAmbiguous,
    /// 已关联远端任务不存在。
    #[error("remote download is missing")]
    RemoteMissing,
}

/// qBittorrent 与 Transmission 适配器共同实现的最小能力端口。
#[async_trait]
pub trait DownloadSource: Send + Sync {
    /// 探测产品、协议版本和 Change 1 所需能力。
    async fn probe(
        &self,
        endpoint: DownloaderEndpoint<'_>,
    ) -> Result<DownloaderCapabilities, DownloadSourceError>;

    /// 新增一项远端下载，并返回或恢复其稳定 hash。
    async fn add(
        &self,
        endpoint: DownloaderEndpoint<'_>,
        request: AddDownloadRequest<'_>,
    ) -> Result<RemoteDownloadRef, DownloadSourceError>;

    /// 只获取显式远端 hash 或 correlation tag 对应的有界快照。
    async fn fetch(
        &self,
        endpoint: DownloaderEndpoint<'_>,
        query: RemoteDownloadQuery,
    ) -> Result<Vec<RemoteDownloadSnapshot>, DownloadSourceError>;
}
