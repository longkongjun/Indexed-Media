use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_LENGTH, COOKIE, HeaderValue, SET_COOKIE};
use tokio::sync::Semaphore;
use url::Url;

use crate::connectors::downloader::model::DownloaderCapabilities;
use crate::connectors::downloader::port::{
    AddDownloadRequest, DownloadSource, DownloadSourceError, DownloaderEndpoint,
    RemoteDownloadQuery, RemoteDownloadRef, RemoteDownloadSnapshot,
};

use super::dto::{AddTorrentsResponse, TorrentDto};
use super::mapper::{map_torrent, valid_hash};

const MINIMUM_WEB_API_VERSION: [u32; 3] = [2, 8, 3];

#[derive(Clone, Copy, Debug)]
/// qBittorrent 客户端的有界 HTTP 与并发设置。
pub struct QbittorrentClientOptions {
    /// TCP/TLS 连接期限。
    pub connect_timeout: Duration,
    /// 每次完整请求和流式响应期限。
    pub request_timeout: Duration,
    /// 传输解码后的最大响应字节数。
    pub max_response_bytes: usize,
    /// 同时执行的完整协议操作上限。
    pub max_concurrency: usize,
}

impl Default for QbittorrentClientOptions {
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
/// 不包含用户 URL 的 qBittorrent 客户端构建失败。
pub enum QbittorrentClientBuildError {
    /// 至少一个有界客户端选项无效。
    #[error("qBittorrent client options are invalid")]
    OptionsInvalid,
    /// 无法构建底层 HTTP 客户端。
    #[error("qBittorrent HTTP client construction failed")]
    ClientBuildFailed,
}

struct SessionCookie(HeaderValue);

/// 动态连接地址、禁止重定向且有界的 qBittorrent `WebUI` API v2 适配器。
#[derive(Clone)]
pub struct QbittorrentClient {
    http: reqwest::Client,
    permits: Arc<Semaphore>,
    options: QbittorrentClientOptions,
    loopback_only: bool,
}

impl std::fmt::Debug for QbittorrentClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QbittorrentClient")
            .field("options", &self.options)
            .field("loopback_only", &self.loopback_only)
            .finish_non_exhaustive()
    }
}

impl QbittorrentClient {
    /// 构建允许已校验 HTTP(S) 家庭网络地址的生产客户端。
    ///
    /// # Errors
    ///
    /// HTTP 客户端无法构建时返回构建错误。
    pub fn production() -> Result<Self, QbittorrentClientBuildError> {
        Self::build(QbittorrentClientOptions::default(), false)
    }

    /// 构建仅允许纯 HTTP loopback 端点的测试客户端。
    ///
    /// # Errors
    ///
    /// 有界选项无效或 HTTP 客户端无法构建时返回构建错误。
    pub fn for_test_loopback(
        options: QbittorrentClientOptions,
    ) -> Result<Self, QbittorrentClientBuildError> {
        Self::build(options, true)
    }

    fn build(
        options: QbittorrentClientOptions,
        loopback_only: bool,
    ) -> Result<Self, QbittorrentClientBuildError> {
        if options.connect_timeout.is_zero()
            || options.request_timeout.is_zero()
            || options.max_response_bytes == 0
            || options.max_response_bytes > 2 * 1024 * 1024
            || options.max_concurrency == 0
            || options.max_concurrency > 4
        {
            return Err(QbittorrentClientBuildError::OptionsInvalid);
        }
        let http = reqwest::Client::builder()
            .connect_timeout(options.connect_timeout)
            .timeout(options.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .user_agent("MediaFlow/0.1")
            .build()
            .map_err(|_| QbittorrentClientBuildError::ClientBuildFailed)?;
        Ok(Self {
            http,
            permits: Arc::new(Semaphore::new(options.max_concurrency)),
            options,
            loopback_only,
        })
    }

    fn base_url(&self, endpoint: DownloaderEndpoint<'_>) -> Result<Url, DownloadSourceError> {
        let base =
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
        Ok(base)
    }

    fn api_url(base: &Url, path: &str) -> Result<Url, DownloadSourceError> {
        let value = format!("{}{path}", base.as_str().trim_end_matches('/'));
        let url = Url::parse(&value).map_err(|_| DownloadSourceError::InvalidResponse)?;
        if url.origin() != base.origin() {
            return Err(DownloadSourceError::InvalidResponse);
        }
        Ok(url)
    }

    async fn login(
        &self,
        base: &Url,
        endpoint: DownloaderEndpoint<'_>,
    ) -> Result<SessionCookie, DownloadSourceError> {
        let username = std::str::from_utf8(endpoint.credentials.username().expose())
            .map_err(|_| DownloadSourceError::Unauthorized)?;
        let password = std::str::from_utf8(endpoint.credentials.password().expose())
            .map_err(|_| DownloadSourceError::Unauthorized)?;
        if username.len() > 256 || password.len() > 4096 {
            return Err(DownloadSourceError::Unauthorized);
        }
        let response = self
            .http
            .post(Self::api_url(base, "/api/v2/auth/login")?)
            .form(&[("username", username), ("password", password)])
            .send()
            .await
            .map_err(|error| map_transport_error(&error))?;
        let status = response.status();
        classify_status(status)?;
        let cookie = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| value.split(';').next())
            .find(|value| valid_session_cookie(value))
            .filter(|value| (5..=4096).contains(&value.len()))
            .and_then(|value| HeaderValue::from_str(value).ok())
            .ok_or(DownloadSourceError::Unauthorized)?;
        let body = self.read_body(response).await?;
        let legacy_success = status == StatusCode::OK && body.as_slice() == b"Ok.";
        let current_success = status == StatusCode::NO_CONTENT && body.is_empty();
        if !legacy_success && !current_success {
            return Err(DownloadSourceError::Unauthorized);
        }
        Ok(SessionCookie(cookie))
    }

    async fn get(
        &self,
        url: Url,
        cookie: &SessionCookie,
        query: &[(&str, &str)],
    ) -> Result<Vec<u8>, DownloadSourceError> {
        let response = self
            .http
            .get(url)
            .header(COOKIE, cookie.0.clone())
            .query(query)
            .send()
            .await
            .map_err(|error| map_transport_error(&error))?;
        classify_status(response.status())?;
        self.read_body(response).await
    }

    async fn post_form(
        &self,
        url: Url,
        cookie: &SessionCookie,
        form: &[(&str, &str)],
    ) -> Result<Vec<u8>, DownloadSourceError> {
        let response = self
            .http
            .post(url)
            .header(COOKIE, cookie.0.clone())
            .form(form)
            .send()
            .await
            .map_err(|error| map_transport_error(&error))?;
        classify_status(response.status())?;
        self.read_body(response).await
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

    async fn fetch_with_session(
        &self,
        base: &Url,
        cookie: &SessionCookie,
        query: RemoteDownloadQuery,
    ) -> Result<Vec<RemoteDownloadSnapshot>, DownloadSourceError> {
        let by_ids = !query.remote_ids.is_empty() && query.correlation_tag.is_none();
        let by_tag = query.remote_ids.is_empty() && query.correlation_tag.is_some();
        if (!by_ids && !by_tag)
            || query.remote_ids.len() > 100
            || query.remote_ids.iter().any(|value| !valid_hash(value))
            || query
                .correlation_tag
                .as_deref()
                .is_some_and(|tag| !valid_tag(tag))
        {
            return Err(DownloadSourceError::InvalidResponse);
        }
        let joined = query.remote_ids.join("|");
        let query_pair = if by_ids {
            ("hashes", joined.as_str())
        } else {
            (
                "tag",
                query
                    .correlation_tag
                    .as_deref()
                    .ok_or(DownloadSourceError::InvalidResponse)?,
            )
        };
        let body = self
            .get(
                Self::api_url(base, "/api/v2/torrents/info")?,
                cookie,
                &[query_pair],
            )
            .await?;
        let torrents: Vec<TorrentDto> =
            serde_json::from_slice(&body).map_err(|_| DownloadSourceError::InvalidResponse)?;
        if torrents.len() > 100 {
            return Err(DownloadSourceError::InvalidResponse);
        }
        let requested = query
            .remote_ids
            .iter()
            .map(|value| value.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let mut seen = BTreeSet::new();
        let snapshots = torrents
            .into_iter()
            .map(map_torrent)
            .collect::<Result<Vec<_>, _>>()?;
        if snapshots.iter().any(|snapshot| {
            !seen.insert(snapshot.remote_id.clone())
                || (by_ids && !requested.contains(&snapshot.remote_id))
        }) {
            return Err(DownloadSourceError::InvalidResponse);
        }
        Ok(snapshots)
    }
}

fn valid_session_cookie(value: &str) -> bool {
    let Some((name, token)) = value.split_once('=') else {
        return false;
    };
    let valid_name = name == "SID"
        || name
            .strip_prefix("QBT_SID_")
            .and_then(|port| port.parse::<u16>().ok())
            .is_some_and(|port| port > 0);
    valid_name
        && !token.is_empty()
        && token.len() <= 4096
        && token
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b';')
}

#[async_trait::async_trait]
impl DownloadSource for QbittorrentClient {
    async fn probe(
        &self,
        endpoint: DownloaderEndpoint<'_>,
    ) -> Result<DownloaderCapabilities, DownloadSourceError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| DownloadSourceError::Unavailable)?;
        let base = self.base_url(endpoint)?;
        let cookie = self.login(&base, endpoint).await?;
        let product = self
            .get(Self::api_url(&base, "/api/v2/app/version")?, &cookie, &[])
            .await?;
        let api = self
            .get(
                Self::api_url(&base, "/api/v2/app/webapiVersion")?,
                &cookie,
                &[],
            )
            .await?;
        let product_version = parse_product_version(&product)?;
        let api_version = parse_api_version(&api)?;
        if api_version.1 < MINIMUM_WEB_API_VERSION {
            return Err(DownloadSourceError::UnsupportedVersion);
        }
        Ok(DownloaderCapabilities {
            manual_add: true,
            task_monitoring: true,
            product_version,
            api_version: api_version.0,
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
        let base = self.base_url(endpoint)?;
        let cookie = self.login(&base, endpoint).await?;
        let source = std::str::from_utf8(request.source.expose())
            .map_err(|_| DownloadSourceError::InvalidResponse)?;
        if source.is_empty() || source.len() > 8192 || !valid_tag(request.correlation_tag) {
            return Err(DownloadSourceError::InvalidResponse);
        }
        let created = self
            .post_form(
                Self::api_url(&base, "/api/v2/torrents/createTags")?,
                &cookie,
                &[("tags", request.correlation_tag)],
            )
            .await?;
        if created.as_slice() != b"Ok." && !created.is_empty() {
            return Err(DownloadSourceError::InvalidResponse);
        }
        let add_result = self
            .post_form(
                Self::api_url(&base, "/api/v2/torrents/add")?,
                &cookie,
                &[("urls", source), ("tags", request.correlation_tag)],
            )
            .await;
        match add_result {
            Ok(body) if valid_add_response(&body) => {}
            Ok(_) => return Err(DownloadSourceError::InvalidResponse),
            Err(DownloadSourceError::Timeout | DownloadSourceError::Unavailable) => {}
            Err(error) => return Err(error),
        }
        let snapshots = self
            .fetch_with_session(
                &base,
                &cookie,
                RemoteDownloadQuery {
                    remote_ids: Vec::new(),
                    correlation_tag: Some(request.correlation_tag.to_owned()),
                },
            )
            .await?;
        match snapshots.as_slice() {
            [snapshot] => Ok(RemoteDownloadRef {
                remote_id: snapshot.remote_id.clone(),
            }),
            [] => Err(DownloadSourceError::RemoteMissing),
            _ => Err(DownloadSourceError::CorrelationAmbiguous),
        }
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
        let base = self.base_url(endpoint)?;
        let cookie = self.login(&base, endpoint).await?;
        self.fetch_with_session(&base, &cookie, query).await
    }
}

fn parse_product_version(body: &[u8]) -> Result<String, DownloadSourceError> {
    let value = std::str::from_utf8(body)
        .map_err(|_| DownloadSourceError::InvalidResponse)?
        .trim()
        .trim_start_matches('v');
    if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
        return Err(DownloadSourceError::InvalidResponse);
    }
    Ok(value.to_owned())
}

fn parse_api_version(body: &[u8]) -> Result<(String, [u32; 3]), DownloadSourceError> {
    let value = std::str::from_utf8(body)
        .map_err(|_| DownloadSourceError::InvalidResponse)?
        .trim();
    if value.is_empty() || value.len() > 64 {
        return Err(DownloadSourceError::InvalidResponse);
    }
    let components = value
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DownloadSourceError::InvalidResponse)?;
    if !(2..=4).contains(&components.len()) {
        return Err(DownloadSourceError::InvalidResponse);
    }
    let version = [
        components[0],
        components[1],
        components.get(2).copied().unwrap_or_default(),
    ];
    Ok((value.to_owned(), version))
}

fn valid_tag(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_add_response(body: &[u8]) -> bool {
    if body == b"Ok." || body.is_empty() {
        return true;
    }
    let Ok(response) = serde_json::from_slice::<AddTorrentsResponse>(body) else {
        return false;
    };
    if response.failure_count != 0 {
        return false;
    }
    match (response.success_count, response.pending_count) {
        (1, 0) => {
            response.added_torrent_ids.len() == 1 && valid_hash(&response.added_torrent_ids[0])
        }
        (0, 1) => response.added_torrent_ids.is_empty(),
        _ => false,
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
