use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderName, HeaderValue};
use tokio::sync::Semaphore;
use url::Url;

use crate::connectors::downloader::model::DownloaderCapabilities;
use crate::connectors::downloader::port::{
    AddDownloadRequest, DownloadSource, DownloadSourceError, DownloaderEndpoint,
    RemoteDownloadQuery, RemoteDownloadRef, RemoteDownloadSnapshot,
};

use super::dto::SessionInfo;
use super::mapper::{map_torrent, valid_hash};
use super::{json_rpc, legacy};

const SESSION_HEADER: HeaderName = HeaderName::from_static("x-transmission-session-id");
const RPC_VERSION_HEADER: HeaderName = HeaderName::from_static("x-transmission-rpc-version");

#[derive(Clone, Copy, Debug)]
/// Transmission 客户端的有界 HTTP 与并发设置。
pub struct TransmissionClientOptions {
    /// TCP/TLS 连接期限。
    pub connect_timeout: Duration,
    /// 每次完整请求和流式响应期限。
    pub request_timeout: Duration,
    /// 传输解码后的最大响应字节数。
    pub max_response_bytes: usize,
    /// 同时执行的完整协议操作上限。
    pub max_concurrency: usize,
}

impl Default for TransmissionClientOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(15),
            max_response_bytes: 2 * 1024 * 1024,
            max_concurrency: 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
/// 不包含用户 URL 的 Transmission 客户端构建失败。
pub enum TransmissionClientBuildError {
    /// 至少一个有界客户端选项无效。
    #[error("Transmission client options are invalid")]
    OptionsInvalid,
    /// 无法构建底层 HTTP 客户端。
    #[error("Transmission HTTP client construction failed")]
    ClientBuildFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Transmission 4.0 与 4.1+ 的协议编码选择。
pub enum TransmissionCodec {
    /// RPC semver 5.3.x 的 camelCase legacy envelope。
    Legacy,
    /// RPC semver 6.x 的 `snake_case` JSON-RPC 2.0 envelope。
    JsonRpc2,
}

#[derive(Clone)]
struct SessionId(HeaderValue);

struct NegotiatedSession {
    session_id: Option<SessionId>,
    codec: TransmissionCodec,
    info: SessionInfo,
}

enum RpcReply {
    Success(Vec<u8>),
    Conflict {
        session_id: SessionId,
        version: Option<String>,
    },
}

/// 动态连接地址、禁止重定向且有界的 Transmission RPC 适配器。
#[derive(Clone)]
pub struct TransmissionClient {
    http: reqwest::Client,
    permits: Arc<Semaphore>,
    options: TransmissionClientOptions,
    loopback_only: bool,
}

impl std::fmt::Debug for TransmissionClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransmissionClient")
            .field("options", &self.options)
            .field("loopback_only", &self.loopback_only)
            .finish_non_exhaustive()
    }
}

impl TransmissionClient {
    /// 构建允许已校验 HTTP(S) 家庭网络地址的生产客户端。
    ///
    /// # Errors
    ///
    /// HTTP 客户端无法构建时返回构建错误。
    pub fn production() -> Result<Self, TransmissionClientBuildError> {
        Self::build(TransmissionClientOptions::default(), false)
    }

    /// 构建仅允许纯 HTTP loopback 端点的测试客户端。
    ///
    /// # Errors
    ///
    /// 有界选项无效或 HTTP 客户端无法构建时返回构建错误。
    pub fn for_test_loopback(
        options: TransmissionClientOptions,
    ) -> Result<Self, TransmissionClientBuildError> {
        Self::build(options, true)
    }

    fn build(
        options: TransmissionClientOptions,
        loopback_only: bool,
    ) -> Result<Self, TransmissionClientBuildError> {
        if options.connect_timeout.is_zero()
            || options.request_timeout.is_zero()
            || options.max_response_bytes == 0
            || options.max_response_bytes > 2 * 1024 * 1024
            || options.max_concurrency == 0
            || options.max_concurrency > 4
        {
            return Err(TransmissionClientBuildError::OptionsInvalid);
        }
        let http = reqwest::Client::builder()
            .connect_timeout(options.connect_timeout)
            .timeout(options.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .user_agent("MediaFlow/0.1")
            .build()
            .map_err(|_| TransmissionClientBuildError::ClientBuildFailed)?;
        Ok(Self {
            http,
            permits: Arc::new(Semaphore::new(options.max_concurrency)),
            options,
            loopback_only,
        })
    }

    fn rpc_url(&self, endpoint: DownloaderEndpoint<'_>) -> Result<Url, DownloadSourceError> {
        let mut base =
            Url::parse(endpoint.base_url).map_err(|_| DownloadSourceError::InvalidResponse)?;
        let loopback = base
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|address| address.is_loopback());
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || (self.loopback_only && (base.scheme() != "http" || !loopback))
        {
            return Err(DownloadSourceError::InvalidResponse);
        }
        let path = base.path().trim_end_matches('/');
        let rpc_path = if path.is_empty() {
            "/transmission/rpc".to_owned()
        } else if path.ends_with("/rpc") {
            path.to_owned()
        } else {
            format!("{path}/rpc")
        };
        base.set_path(&rpc_path);
        Ok(base)
    }

    async fn negotiate(
        &self,
        rpc_url: &Url,
        endpoint: DownloaderEndpoint<'_>,
    ) -> Result<NegotiatedSession, DownloadSourceError> {
        let initial = legacy::session_request()?;
        match self.send(rpc_url, endpoint, None, initial).await? {
            RpcReply::Success(body) => parse_unhinted_session(None, &body),
            RpcReply::Conflict {
                session_id,
                version,
            } => {
                let hinted = version.as_deref().map(select_codec).transpose()?;
                let codec = hinted.unwrap_or(TransmissionCodec::Legacy);
                let request = session_request(codec)?;
                let (session_id, body) = self
                    .send_with_session_retry(rpc_url, endpoint, session_id, request)
                    .await?;
                let mut negotiated = parse_unhinted_session(Some(codec), &body)?;
                negotiated.session_id = Some(session_id);
                Ok(negotiated)
            }
        }
    }

    async fn send_with_session_retry(
        &self,
        rpc_url: &Url,
        endpoint: DownloaderEndpoint<'_>,
        session_id: SessionId,
        body: Vec<u8>,
    ) -> Result<(SessionId, Vec<u8>), DownloadSourceError> {
        match self
            .send(rpc_url, endpoint, Some(&session_id), body.clone())
            .await?
        {
            RpcReply::Success(response) => Ok((session_id, response)),
            RpcReply::Conflict {
                session_id: replacement,
                ..
            } => match self
                .send(rpc_url, endpoint, Some(&replacement), body)
                .await?
            {
                RpcReply::Success(response) => Ok((replacement, response)),
                RpcReply::Conflict { .. } => Err(DownloadSourceError::InvalidResponse),
            },
        }
    }

    async fn action(
        &self,
        rpc_url: &Url,
        endpoint: DownloaderEndpoint<'_>,
        session: &NegotiatedSession,
        body: Vec<u8>,
    ) -> Result<Vec<u8>, DownloadSourceError> {
        match session.session_id.as_ref() {
            Some(session_id) => self
                .send_with_session_retry(rpc_url, endpoint, session_id.clone(), body)
                .await
                .map(|(_, response)| response),
            None => match self.send(rpc_url, endpoint, None, body).await? {
                RpcReply::Success(response) => Ok(response),
                RpcReply::Conflict { .. } => Err(DownloadSourceError::InvalidResponse),
            },
        }
    }

    async fn send(
        &self,
        rpc_url: &Url,
        endpoint: DownloaderEndpoint<'_>,
        session_id: Option<&SessionId>,
        body: Vec<u8>,
    ) -> Result<RpcReply, DownloadSourceError> {
        let username = std::str::from_utf8(endpoint.credentials.username().expose())
            .map_err(|_| DownloadSourceError::Unauthorized)?;
        let password = std::str::from_utf8(endpoint.credentials.password().expose())
            .map_err(|_| DownloadSourceError::Unauthorized)?;
        if username.len() > 256 || password.len() > 4096 {
            return Err(DownloadSourceError::Unauthorized);
        }
        let mut request = self
            .http
            .post(rpc_url.clone())
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        if !username.is_empty() || !password.is_empty() {
            request = request.basic_auth(username, Some(password));
        }
        if let Some(session_id) = session_id {
            request = request.header(SESSION_HEADER, session_id.0.clone());
        }
        let response = request
            .send()
            .await
            .map_err(|error| map_transport_error(&error))?;
        if response.status() == reqwest::StatusCode::CONFLICT {
            let session_id = response
                .headers()
                .get(&SESSION_HEADER)
                .cloned()
                .filter(|value| !value.as_bytes().is_empty() && value.as_bytes().len() <= 4096)
                .map(SessionId)
                .ok_or(DownloadSourceError::InvalidResponse)?;
            let version = response
                .headers()
                .get(&RPC_VERSION_HEADER)
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty() && value.len() <= 64)
                .map(ToOwned::to_owned);
            return Ok(RpcReply::Conflict {
                session_id,
                version,
            });
        }
        classify_status(response.status())?;
        self.read_body(response).await.map(RpcReply::Success)
    }

    async fn read_body(
        &self,
        mut response: reqwest::Response,
    ) -> Result<Vec<u8>, DownloadSourceError> {
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > self.options.max_response_bytes)
        {
            return Err(DownloadSourceError::ResponseTooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| map_transport_error(&error))?
        {
            if body.len().saturating_add(chunk.len()) > self.options.max_response_bytes {
                return Err(DownloadSourceError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

#[async_trait::async_trait]
impl DownloadSource for TransmissionClient {
    async fn probe(
        &self,
        endpoint: DownloaderEndpoint<'_>,
    ) -> Result<DownloaderCapabilities, DownloadSourceError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| DownloadSourceError::Unavailable)?;
        let rpc_url = self.rpc_url(endpoint)?;
        let session = self.negotiate(&rpc_url, endpoint).await?;
        Ok(DownloaderCapabilities {
            manual_add: true,
            task_monitoring: true,
            product_version: session.info.product_version,
            api_version: session.info.api_version,
        })
    }

    async fn add(
        &self,
        endpoint: DownloaderEndpoint<'_>,
        request: AddDownloadRequest<'_>,
    ) -> Result<RemoteDownloadRef, DownloadSourceError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| DownloadSourceError::Unavailable)?;
        let source = std::str::from_utf8(request.source.expose())
            .map_err(|_| DownloadSourceError::InvalidResponse)?;
        if source.is_empty() || source.len() > 8192 {
            return Err(DownloadSourceError::InvalidResponse);
        }
        let rpc_url = self.rpc_url(endpoint)?;
        let session = self.negotiate(&rpc_url, endpoint).await?;
        let request = add_request(session.codec, source)?;
        let body = self.action(&rpc_url, endpoint, &session, request).await?;
        let remote_id = parse_add(session.codec, &body)?;
        if !valid_hash(&remote_id) {
            return Err(DownloadSourceError::InvalidResponse);
        }
        Ok(RemoteDownloadRef {
            remote_id: remote_id.to_ascii_lowercase(),
        })
    }

    async fn fetch(
        &self,
        endpoint: DownloaderEndpoint<'_>,
        query: RemoteDownloadQuery,
    ) -> Result<Vec<RemoteDownloadSnapshot>, DownloadSourceError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| DownloadSourceError::Unavailable)?;
        if query.remote_ids.is_empty()
            || query.remote_ids.len() > 100
            || query.correlation_tag.is_some()
            || query.remote_ids.iter().any(|value| !valid_hash(value))
        {
            return Err(DownloadSourceError::InvalidResponse);
        }
        let rpc_url = self.rpc_url(endpoint)?;
        let session = self.negotiate(&rpc_url, endpoint).await?;
        let request = fetch_request(session.codec, &query.remote_ids)?;
        let body = self.action(&rpc_url, endpoint, &session, request).await?;
        let torrents = parse_fetch(session.codec, &body)?;
        if torrents.len() > 100 {
            return Err(DownloadSourceError::InvalidResponse);
        }
        let requested = query
            .remote_ids
            .iter()
            .map(|value| value.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let snapshots = torrents
            .into_iter()
            .map(map_torrent)
            .collect::<Result<Vec<_>, _>>()?;
        let mut seen = BTreeSet::new();
        if snapshots.iter().any(|snapshot| {
            !seen.insert(snapshot.remote_id.clone()) || !requested.contains(&snapshot.remote_id)
        }) {
            return Err(DownloadSourceError::InvalidResponse);
        }
        Ok(snapshots)
    }
}

fn session_request(codec: TransmissionCodec) -> Result<Vec<u8>, DownloadSourceError> {
    match codec {
        TransmissionCodec::Legacy => legacy::session_request(),
        TransmissionCodec::JsonRpc2 => json_rpc::session_request(),
    }
}

fn add_request(codec: TransmissionCodec, source: &str) -> Result<Vec<u8>, DownloadSourceError> {
    match codec {
        TransmissionCodec::Legacy => legacy::add_request(source),
        TransmissionCodec::JsonRpc2 => json_rpc::add_request(source),
    }
}

fn fetch_request(
    codec: TransmissionCodec,
    remote_ids: &[String],
) -> Result<Vec<u8>, DownloadSourceError> {
    match codec {
        TransmissionCodec::Legacy => legacy::fetch_request(remote_ids),
        TransmissionCodec::JsonRpc2 => json_rpc::fetch_request(remote_ids),
    }
}

fn parse_add(codec: TransmissionCodec, body: &[u8]) -> Result<String, DownloadSourceError> {
    match codec {
        TransmissionCodec::Legacy => legacy::parse_add(body),
        TransmissionCodec::JsonRpc2 => json_rpc::parse_add(body),
    }
}

fn parse_fetch(
    codec: TransmissionCodec,
    body: &[u8],
) -> Result<Vec<super::dto::TorrentFields>, DownloadSourceError> {
    match codec {
        TransmissionCodec::Legacy => legacy::parse_fetch(body),
        TransmissionCodec::JsonRpc2 => json_rpc::parse_fetch(body),
    }
}

fn parse_unhinted_session(
    hinted: Option<TransmissionCodec>,
    body: &[u8],
) -> Result<NegotiatedSession, DownloadSourceError> {
    let (codec, info) = match hinted {
        Some(TransmissionCodec::Legacy) => {
            (TransmissionCodec::Legacy, legacy::parse_session(body)?)
        }
        Some(TransmissionCodec::JsonRpc2) => {
            (TransmissionCodec::JsonRpc2, json_rpc::parse_session(body)?)
        }
        None => match legacy::parse_session(body) {
            Ok(info) => (TransmissionCodec::Legacy, info),
            Err(_) => (TransmissionCodec::JsonRpc2, json_rpc::parse_session(body)?),
        },
    };
    let actual = select_codec(&info.api_version)?;
    if actual != codec {
        return Err(DownloadSourceError::InvalidResponse);
    }
    Ok(NegotiatedSession {
        session_id: None,
        codec,
        info,
    })
}

fn select_codec(value: &str) -> Result<TransmissionCodec, DownloadSourceError> {
    let components = value
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DownloadSourceError::UnsupportedVersion)?;
    if !(2..=4).contains(&components.len()) {
        return Err(DownloadSourceError::UnsupportedVersion);
    }
    match (components[0], components[1]) {
        (5, minor) if minor >= 3 => Ok(TransmissionCodec::Legacy),
        (6, _) => Ok(TransmissionCodec::JsonRpc2),
        _ => Err(DownloadSourceError::UnsupportedVersion),
    }
}

fn classify_status(status: reqwest::StatusCode) -> Result<(), DownloadSourceError> {
    match status.as_u16() {
        200..=299 => Ok(()),
        401 | 403 => Err(DownloadSourceError::Unauthorized),
        429 => Err(DownloadSourceError::RateLimited),
        500..=599 => Err(DownloadSourceError::Unavailable),
        _ => Err(DownloadSourceError::InvalidResponse),
    }
}

fn map_transport_error(error: &reqwest::Error) -> DownloadSourceError {
    if error.is_timeout() {
        DownloadSourceError::Timeout
    } else {
        DownloadSourceError::Unavailable
    }
}
