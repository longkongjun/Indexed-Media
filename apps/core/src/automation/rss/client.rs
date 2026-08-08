use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use reqwest::header::{
    CONTENT_LENGTH, ETAG, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use url::Url;

use crate::platform::secrets::SecretBytes;

/// Bounded conditional HTTP client options.
#[derive(Clone, Copy, Debug)]
pub struct FeedClientOptions {
    /// TCP/TLS connection timeout.
    pub connect_timeout: Duration,
    /// Whole request and streamed body timeout.
    pub request_timeout: Duration,
    /// Maximum decoded response bytes.
    pub max_response_bytes: usize,
    /// Maximum concurrent feed requests.
    pub max_concurrency: usize,
}

impl Default for FeedClientOptions {
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
/// Feed client construction failure without endpoint details.
pub enum FeedClientBuildError {
    /// At least one client option is outside its fixed bounds.
    #[error("feed client options are invalid")]
    OptionsInvalid,
    /// The underlying HTTP client could not be built.
    #[error("feed HTTP client construction failed")]
    ClientBuildFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
/// Stable feed request failure without URL, headers, or body.
pub enum FeedClientError {
    /// Feed credentials or access policy rejected the request.
    #[error("feed access was rejected")]
    Unauthorized,
    /// Upstream asked the poller to delay.
    #[error("feed is rate limited")]
    RateLimited,
    /// Connect, transport, or server failure.
    #[error("feed is unavailable")]
    Unavailable,
    /// Whole-request timeout elapsed.
    #[error("feed request timed out")]
    Timeout,
    /// Decoded response exceeded the byte budget.
    #[error("feed response is too large")]
    ResponseTooLarge,
    /// URL, redirect, status, header, or response framing was invalid.
    #[error("feed response is invalid")]
    InvalidResponse,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
/// Opaque conditional request cursor; production stores it encrypted.
pub struct RssCursor {
    etag: Option<String>,
    last_modified: Option<String>,
}

impl RssCursor {
    /// Build a bounded cursor from response header values.
    ///
    /// # Errors
    ///
    /// Rejects empty, overlong, or invalid HTTP header values.
    pub fn new(
        etag: Option<String>,
        last_modified: Option<String>,
    ) -> Result<Self, FeedClientError> {
        for value in [&etag, &last_modified].into_iter().flatten() {
            if value.is_empty() || value.len() > 1024 || HeaderValue::from_str(value).is_err() {
                return Err(FeedClientError::InvalidResponse);
            }
        }
        if etag.is_none() && last_modified.is_none() {
            return Err(FeedClientError::InvalidResponse);
        }
        Ok(Self {
            etag,
            last_modified,
        })
    }

    #[must_use]
    /// Borrow the `ETag` for a conditional request.
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    #[must_use]
    /// Borrow the Last-Modified value for a conditional request.
    pub fn last_modified(&self) -> Option<&str> {
        self.last_modified.as_deref()
    }
}

impl std::fmt::Debug for RssCursor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RssCursor")
            .field("etag", &self.etag.as_ref().map(|_| "[REDACTED]"))
            .field(
                "last_modified",
                &self.last_modified.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// One bounded conditional feed response.
pub enum FeedResponse {
    /// Upstream confirmed that the cursor is current.
    NotModified {
        /// Updated or retained conditional cursor.
        cursor: Option<RssCursor>,
    },
    /// Upstream returned a complete bounded body.
    Modified {
        /// Raw XML bytes for the bounded parser.
        body: Vec<u8>,
        /// New conditional cursor when supplied.
        cursor: Option<RssCursor>,
        /// Whether the request supplied a prior cursor.
        previous_cursor_present: bool,
    },
}

impl std::fmt::Debug for FeedResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotModified { cursor } => formatter
                .debug_struct("NotModified")
                .field("cursor", cursor)
                .finish(),
            Self::Modified {
                body,
                cursor,
                previous_cursor_present,
            } => formatter
                .debug_struct("Modified")
                .field("body_bytes", &body.len())
                .field("cursor", cursor)
                .field("previous_cursor_present", previous_cursor_present)
                .finish(),
        }
    }
}

impl PartialEq for FeedResponse {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::NotModified { cursor: left }, Self::NotModified { cursor: right }) => {
                left == right
            }
            (
                Self::Modified {
                    body: left_body,
                    cursor: left_cursor,
                    previous_cursor_present: left_previous,
                },
                Self::Modified {
                    body: right_body,
                    cursor: right_cursor,
                    previous_cursor_present: right_previous,
                },
            ) => {
                left_body == right_body
                    && left_cursor == right_cursor
                    && left_previous == right_previous
            }
            _ => false,
        }
    }
}

impl Eq for FeedResponse {}

#[async_trait]
/// Transport boundary used by the feed poller.
pub trait FeedClient: Send + Sync {
    /// Fetch one feed using an optional conditional cursor.
    async fn fetch(
        &self,
        feed_url: &SecretBytes,
        cursor: Option<&RssCursor>,
    ) -> Result<FeedResponse, FeedClientError>;
}

/// Redirect-free, bounded feed HTTP adapter.
#[derive(Clone)]
pub struct FeedHttpClient {
    http: reqwest::Client,
    permits: Arc<Semaphore>,
    options: FeedClientOptions,
    loopback_only: bool,
}

impl std::fmt::Debug for FeedHttpClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FeedHttpClient")
            .field("options", &self.options)
            .field("loopback_only", &self.loopback_only)
            .finish_non_exhaustive()
    }
}

impl FeedHttpClient {
    /// Build a production client that accepts validated HTTPS feed URLs.
    ///
    /// # Errors
    ///
    /// Returns a stable construction error for invalid options or HTTP setup.
    pub fn production() -> Result<Self, FeedClientBuildError> {
        Self::build(FeedClientOptions::default(), false)
    }

    /// Build a test client restricted to plain HTTP loopback URLs.
    ///
    /// # Errors
    ///
    /// Returns a stable construction error for invalid options or HTTP setup.
    pub fn for_test_loopback(options: FeedClientOptions) -> Result<Self, FeedClientBuildError> {
        Self::build(options, true)
    }

    fn build(
        options: FeedClientOptions,
        loopback_only: bool,
    ) -> Result<Self, FeedClientBuildError> {
        if options.connect_timeout.is_zero()
            || options.request_timeout.is_zero()
            || options.max_response_bytes == 0
            || options.max_response_bytes > 2 * 1024 * 1024
            || options.max_concurrency == 0
            || options.max_concurrency > 4
        {
            return Err(FeedClientBuildError::OptionsInvalid);
        }
        let http = reqwest::Client::builder()
            .connect_timeout(options.connect_timeout)
            .timeout(options.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .user_agent("MediaFlow/0.1")
            .build()
            .map_err(|_| FeedClientBuildError::ClientBuildFailed)?;
        Ok(Self {
            http,
            permits: Arc::new(Semaphore::new(options.max_concurrency)),
            options,
            loopback_only,
        })
    }

    fn url(&self, secret: &SecretBytes) -> Result<Url, FeedClientError> {
        let text =
            std::str::from_utf8(secret.expose()).map_err(|_| FeedClientError::InvalidResponse)?;
        let url = Url::parse(text).map_err(|_| FeedClientError::InvalidResponse)?;
        let loopback = url
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|address| address.is_loopback());
        let valid_scheme = if self.loopback_only {
            url.scheme() == "http" && loopback
        } else {
            url.scheme() == "https"
        };
        if !valid_scheme
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.cannot_be_a_base()
        {
            return Err(FeedClientError::InvalidResponse);
        }
        Ok(url)
    }

    async fn read_body(&self, mut response: reqwest::Response) -> Result<Vec<u8>, FeedClientError> {
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > self.options.max_response_bytes)
        {
            return Err(FeedClientError::ResponseTooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| map_transport_error(&error))?
        {
            if body.len().saturating_add(chunk.len()) > self.options.max_response_bytes {
                return Err(FeedClientError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

#[async_trait]
impl FeedClient for FeedHttpClient {
    async fn fetch(
        &self,
        feed_url: &SecretBytes,
        cursor: Option<&RssCursor>,
    ) -> Result<FeedResponse, FeedClientError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| FeedClientError::Unavailable)?;
        let mut request = self.http.get(self.url(feed_url)?);
        if let Some(value) = cursor.and_then(RssCursor::etag) {
            request = request.header(IF_NONE_MATCH, value);
        }
        if let Some(value) = cursor.and_then(RssCursor::last_modified) {
            request = request.header(IF_MODIFIED_SINCE, value);
        }
        let response = request
            .send()
            .await
            .map_err(|error| map_transport_error(&error))?;
        let status = response.status();
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(FeedClientError::Unauthorized);
            }
            StatusCode::TOO_MANY_REQUESTS => return Err(FeedClientError::RateLimited),
            status if status.is_server_error() => return Err(FeedClientError::Unavailable),
            StatusCode::NOT_MODIFIED => {
                let next = response_cursor(&response)?.or_else(|| cursor.cloned());
                return Ok(FeedResponse::NotModified { cursor: next });
            }
            StatusCode::OK => {}
            _ => return Err(FeedClientError::InvalidResponse),
        }
        let next = response_cursor(&response)?;
        let body = self.read_body(response).await?;
        Ok(FeedResponse::Modified {
            body,
            cursor: next,
            previous_cursor_present: cursor.is_some(),
        })
    }
}

fn response_cursor(response: &reqwest::Response) -> Result<Option<RssCursor>, FeedClientError> {
    let etag = optional_header(response, ETAG)?;
    let last_modified = optional_header(response, LAST_MODIFIED)?;
    if etag.is_none() && last_modified.is_none() {
        Ok(None)
    } else {
        RssCursor::new(etag, last_modified).map(Some)
    }
}

fn optional_header(
    response: &reqwest::Response,
    name: reqwest::header::HeaderName,
) -> Result<Option<String>, FeedClientError> {
    response
        .headers()
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| FeedClientError::InvalidResponse)
        })
        .transpose()
}

fn map_transport_error(error: &reqwest::Error) -> FeedClientError {
    if error.is_timeout() {
        FeedClientError::Timeout
    } else {
        FeedClientError::Unavailable
    }
}
