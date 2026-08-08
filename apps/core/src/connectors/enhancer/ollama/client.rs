use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use tokio::sync::Semaphore;
use url::Url;

use super::super::model::{EnhancementInput, allowed_ip};
use super::super::port::{
    EnhancementHints, EnhancerEndpoint, EnhancerError, EnhancerProbe, IdentificationEnhancer,
};
use super::{dto, mapper};

#[derive(Clone, Copy, Debug)]
/// Fixed HTTP and resource limits for the local Ollama adapter.
pub struct OllamaClientOptions {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub max_response_bytes: usize,
    pub max_concurrency: usize,
}

impl Default for OllamaClientOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(2),
            request_timeout: Duration::from_secs(30),
            max_response_bytes: 64 * 1024,
            max_concurrency: 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
/// Stable adapter construction failure.
pub enum OllamaClientBuildError {
    #[error("ollama client options are invalid")]
    OptionsInvalid,
    #[error("ollama client construction failed")]
    ClientBuildFailed,
}

#[derive(Clone)]
/// Redirect-free, bounded local Ollama protocol adapter.
pub struct OllamaClient {
    http: reqwest::Client,
    permits: Arc<Semaphore>,
    options: OllamaClientOptions,
}

impl std::fmt::Debug for OllamaClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OllamaClient")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl OllamaClient {
    /// Build the production adapter.
    ///
    /// # Errors
    ///
    /// Returns a stable error when fixed options or the HTTP client are invalid.
    pub fn production() -> Result<Self, OllamaClientBuildError> {
        Self::new(OllamaClientOptions::default())
    }

    /// Build an adapter with explicit limits for tests.
    ///
    /// # Errors
    ///
    /// Rejects zero/excessive limits or HTTP construction failure.
    pub fn new(options: OllamaClientOptions) -> Result<Self, OllamaClientBuildError> {
        if options.connect_timeout.is_zero()
            || options.request_timeout.is_zero()
            || options.max_response_bytes == 0
            || options.max_response_bytes > 1024 * 1024
            || !(1..=2).contains(&options.max_concurrency)
        {
            return Err(OllamaClientBuildError::OptionsInvalid);
        }
        let http = reqwest::Client::builder()
            .connect_timeout(options.connect_timeout)
            .timeout(options.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .user_agent("MediaFlow/0.1")
            .build()
            .map_err(|_| OllamaClientBuildError::ClientBuildFailed)?;
        Ok(Self {
            http,
            permits: Arc::new(Semaphore::new(options.max_concurrency)),
            options,
        })
    }

    async fn request_url(
        &self,
        endpoint: EnhancerEndpoint<'_>,
        path: &str,
    ) -> Result<Url, EnhancerError> {
        let mut url = Url::parse(endpoint.base_url).map_err(|_| EnhancerError::InvalidResponse)?;
        if url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("localhost"))
        {
            let port = url
                .port_or_known_default()
                .ok_or(EnhancerError::InvalidResponse)?;
            let addresses = tokio::net::lookup_host(("localhost", port))
                .await
                .map_err(|_| EnhancerError::Unavailable)?
                .map(|address| address.ip())
                .collect::<Vec<_>>();
            if addresses.is_empty()
                || addresses
                    .iter()
                    .copied()
                    .any(|address| !allowed_ip(address))
            {
                return Err(EnhancerError::InvalidResponse);
            }
            let selected = addresses
                .iter()
                .copied()
                .find(std::net::IpAddr::is_ipv4)
                .unwrap_or(addresses[0]);
            url.set_ip_host(selected)
                .map_err(|()| EnhancerError::InvalidResponse)?;
        }
        url.set_path(path);
        Ok(url)
    }

    async fn read_body(&self, mut response: reqwest::Response) -> Result<Vec<u8>, EnhancerError> {
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > self.options.max_response_bytes)
        {
            return Err(EnhancerError::ResponseTooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| transport(&error))? {
            if body.len().saturating_add(chunk.len()) > self.options.max_response_bytes {
                return Err(EnhancerError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        timeout_ms: u32,
    ) -> Result<Vec<u8>, EnhancerError> {
        let configured = Duration::from_millis(u64::from(timeout_ms));
        let response = request
            .timeout(configured.min(self.options.request_timeout))
            .send()
            .await
            .map_err(|error| transport(&error))?;
        match response.status() {
            StatusCode::OK => self.read_body(response).await,
            StatusCode::TOO_MANY_REQUESTS => Err(EnhancerError::Overloaded),
            status if status.is_server_error() => Err(EnhancerError::Unavailable),
            _ => Err(EnhancerError::InvalidResponse),
        }
    }
}

#[async_trait]
impl IdentificationEnhancer for OllamaClient {
    async fn probe(&self, endpoint: EnhancerEndpoint<'_>) -> Result<EnhancerProbe, EnhancerError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| EnhancerError::Unavailable)?;
        let url = self.request_url(endpoint, "/api/tags").await?;
        let body = self.send(self.http.get(url), endpoint.timeout_ms).await?;
        mapper::probe(&body, endpoint.model)
    }

    async fn enhance(
        &self,
        endpoint: EnhancerEndpoint<'_>,
        input: &EnhancementInput,
    ) -> Result<EnhancementHints, EnhancerError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| EnhancerError::Unavailable)?;
        let url = self.request_url(endpoint, "/api/chat").await?;
        let request_body = dto::request_body(endpoint.model, input)?;
        let body = self
            .send(
                self.http
                    .post(url)
                    .header(CONTENT_TYPE, "application/json")
                    .body(request_body),
                endpoint.timeout_ms,
            )
            .await?;
        mapper::hints(&body, endpoint.model)
    }
}

fn transport(error: &reqwest::Error) -> EnhancerError {
    if error.is_timeout() {
        EnhancerError::Timeout
    } else {
        EnhancerError::Unavailable
    }
}
