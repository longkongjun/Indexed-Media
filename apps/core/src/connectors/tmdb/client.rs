use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_LENGTH, HeaderValue, RETRY_AFTER};
use serde::de::DeserializeOwned;
use tokio::sync::Semaphore;
use url::Url;

use crate::connectors::model::{
    EpisodeIdentity, FieldLanguageSource, MetadataCandidate, ProviderError, ProviderMediaKind,
    TmdbExternalIdRequest, TmdbSearchRequest,
};
use crate::connectors::service::TmdbConnectionTester;
use crate::connectors::tmdb::dto::{EpisodeDto, FindResponseDto, SearchPageDto, WorkDto};
use crate::connectors::tmdb::mapper::{map_work, merge_localized, original_locale, valid_locale};
use crate::platform::secrets::SecretBytes;

const PRODUCTION_ORIGIN: &str = "https://api.themoviedb.org/3/";
const MAX_RESULTS: usize = 20;
const MAX_SEARCH_PAGES: u16 = 3;

/// TMDB 客户端的有界 HTTP 与并发设置。
#[derive(Clone, Copy, Debug)]
pub struct TmdbClientOptions {
    /// TCP/TLS 连接期限。
    pub connect_timeout: Duration,
    /// 整体请求及流式响应体期限。
    pub request_timeout: Duration,
    /// 传输解码后的最大响应体字节数。
    pub max_response_bytes: usize,
    /// 同时进行的提供方请求上限。
    pub max_concurrency: usize,
}

impl Default for TmdbClientOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(15),
            max_response_bytes: 2 * 1024 * 1024,
            max_concurrency: 4,
        }
    }
}

/// 不暴露所提供 URL 的客户端构建失败。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TmdbClientBuildError {
    /// 测试来源不是纯 loopback HTTP `/3/` 基础 URL。
    #[error("test TMDB origin is forbidden")]
    TestOriginForbidden,
    /// 至少一个有界客户端选项无效。
    #[error("TMDB client options are invalid")]
    OptionsInvalid,
    /// 无法构建底层 HTTP 客户端。
    #[error("TMDB HTTP client construction failed")]
    ClientBuildFailed,
}

/// 固定来源、禁止重定向且有界的 TMDB API 客户端。
#[derive(Clone)]
pub struct TmdbClient {
    http: reqwest::Client,
    origin: Url,
    origin_display: Arc<str>,
    permits: Arc<Semaphore>,
    options: TmdbClientOptions,
}

impl std::fmt::Debug for TmdbClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TmdbClient")
            .field("origin", &self.origin_display)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl TmdbClient {
    /// 使用固定官方 API 来源构建生产客户端。
    ///
    /// # Errors
    ///
    /// TLS 客户端无法构建时返回 [`TmdbClientBuildError`]。
    pub fn production() -> Result<Self, TmdbClientBuildError> {
        let origin =
            Url::parse(PRODUCTION_ORIGIN).map_err(|_| TmdbClientBuildError::ClientBuildFailed)?;
        Self::build(origin, TmdbClientOptions::default())
    }

    /// 仅为纯 loopback `/3/` 来源构建伪服务器客户端。
    ///
    /// # Errors
    ///
    /// 拒绝非 loopback 主机、TLS、凭据、query/fragment、错误基础路径或无效限制。
    pub fn for_test_loopback(
        origin: Url,
        options: TmdbClientOptions,
    ) -> Result<Self, TmdbClientBuildError> {
        let loopback = origin
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|address| address.is_loopback());
        if origin.scheme() != "http"
            || !loopback
            || origin.path() != "/3/"
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(TmdbClientBuildError::TestOriginForbidden);
        }
        Self::build(origin, options)
    }

    fn build(origin: Url, options: TmdbClientOptions) -> Result<Self, TmdbClientBuildError> {
        if options.connect_timeout.is_zero()
            || options.request_timeout.is_zero()
            || options.max_response_bytes == 0
            || options.max_response_bytes > 2 * 1024 * 1024
            || options.max_concurrency == 0
            || options.max_concurrency > 4
        {
            return Err(TmdbClientBuildError::OptionsInvalid);
        }
        let http = reqwest::Client::builder()
            .connect_timeout(options.connect_timeout)
            .timeout(options.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .user_agent("MediaFlow/0.1")
            .build()
            .map_err(|_| TmdbClientBuildError::ClientBuildFailed)?;
        let origin_display: Arc<str> = Arc::from(origin.as_str());
        Ok(Self {
            http,
            origin,
            origin_display,
            permits: Arc::new(Semaphore::new(options.max_concurrency)),
            options,
        })
    }

    #[must_use]
    /// 返回供诊断或测试使用的固定无凭据来源。
    pub fn origin(&self) -> &str {
        &self.origin_display
    }

    /// 最多搜索三页，并保留至多 20 个类型化候选。
    ///
    /// # Errors
    ///
    /// 输入无效、HTTP 失败、响应越界或 DTO 无效时返回稳定提供方错误。
    pub async fn search(
        &self,
        token: &SecretBytes,
        request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        validate_search(request)?;
        let path = match request.media_kind {
            ProviderMediaKind::Movie => "search/movie",
            ProviderMediaKind::Tv => "search/tv",
        };
        let mut candidates = Vec::new();
        let mut page = 1_u16;
        loop {
            let mut query = vec![
                ("query", request.title.clone()),
                ("language", request.locale.clone()),
                ("page", page.to_string()),
            ];
            if let Some(region) = &request.region {
                query.push(("region", region.clone()));
            }
            if let Some(year) = request.year {
                query.push((
                    match request.media_kind {
                        ProviderMediaKind::Movie => "primary_release_year",
                        ProviderMediaKind::Tv => "first_air_date_year",
                    },
                    year.to_string(),
                ));
            }
            let response: SearchPageDto = self.get_json(token, path, &query).await?;
            if response.page != Some(page) || response.total_pages.is_none() {
                return Err(ProviderError::InvalidResponse);
            }
            for work in &response.results {
                candidates.push(map_work(
                    work,
                    request.media_kind,
                    &request.locale,
                    FieldLanguageSource::Preferred,
                    chrono::Utc::now().timestamp_micros(),
                )?);
                if candidates.len() == MAX_RESULTS {
                    return Ok(candidates);
                }
            }
            let total_pages = response
                .total_pages
                .unwrap_or_default()
                .min(MAX_SEARCH_PAGES);
            if page >= total_pages {
                return Ok(candidates);
            }
            page += 1;
        }
    }

    /// 在文本搜索前通过 TMDB `/find` 解析显式 IMDb/TheTVDB ID。
    ///
    /// # Errors
    ///
    /// ID/locale 无效、HTTP 失败、越界或 DTO 无效时返回稳定错误。
    pub async fn find_external(
        &self,
        token: &SecretBytes,
        request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        if !valid_external_id(&request.external_id) || !valid_locale(&request.locale) {
            return Err(ProviderError::InvalidResponse);
        }
        let path = format!("find/{}", request.external_id);
        let query = [
            ("external_source", request.source.as_tmdb_value().to_owned()),
            ("language", request.locale.clone()),
        ];
        let response: FindResponseDto = self.get_json(token, &path, &query).await?;
        let now_us = chrono::Utc::now().timestamp_micros();
        let mut candidates = Vec::new();
        for (kind, results) in [
            (ProviderMediaKind::Movie, response.movie_results),
            (ProviderMediaKind::Tv, response.tv_results),
        ] {
            for work in &results {
                candidates.push(map_work(
                    work,
                    kind,
                    &request.locale,
                    FieldLanguageSource::Preferred,
                    now_us,
                )?);
                if candidates.len() == MAX_RESULTS {
                    return Ok(candidates);
                }
            }
        }
        Ok(candidates)
    }

    /// 按首选、原始语言、`en-US` 顺序获取详情并合并有界字段。
    ///
    /// # Errors
    ///
    /// ID/locale 无效、HTTP 失败或 DTO 冲突时返回稳定提供方错误。
    pub async fn details_with_language_fallback(
        &self,
        token: &SecretBytes,
        media_kind: ProviderMediaKind,
        provider_id: i64,
        preferred_locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        if provider_id <= 0 || !valid_locale(preferred_locale) {
            return Err(ProviderError::InvalidResponse);
        }
        let preferred = self
            .details_dto(token, media_kind, provider_id, preferred_locale)
            .await?;
        let original = preferred.original_language().and_then(original_locale);
        let now_us = chrono::Utc::now().timestamp_micros();
        let mut candidate = map_work(
            &preferred,
            media_kind,
            preferred_locale,
            FieldLanguageSource::Preferred,
            now_us,
        )?;
        if let Some(original) = original
            .as_deref()
            .filter(|value| *value != preferred_locale)
        {
            let dto = self
                .details_dto(token, media_kind, provider_id, original)
                .await?;
            merge_localized(
                &mut candidate,
                map_work(
                    &dto,
                    media_kind,
                    original,
                    FieldLanguageSource::Original,
                    now_us,
                )?,
            )?;
        }
        if preferred_locale != "en-US" && original.as_deref() != Some("en-US") {
            let dto = self
                .details_dto(token, media_kind, provider_id, "en-US")
                .await?;
            merge_localized(
                &mut candidate,
                map_work(
                    &dto,
                    media_kind,
                    "en-US",
                    FieldLanguageSource::English,
                    now_us,
                )?,
            )?;
        }
        Ok(candidate)
    }

    /// 通过 TMDB 详情端点核验一组有界显式季/集身份。
    ///
    /// # Errors
    ///
    /// 拒绝无效或重复引用、缺失集数、DTO 身份不匹配及常规有界 HTTP/提供方失败；最多接受
    /// 100 个集数引用。
    pub async fn verify_episodes(
        &self,
        token: &SecretBytes,
        series_id: i64,
        episodes: &[(u16, u16)],
        locale: &str,
    ) -> Result<Vec<EpisodeIdentity>, ProviderError> {
        let mut unique = std::collections::BTreeSet::new();
        if series_id <= 0
            || episodes.is_empty()
            || episodes.len() > 100
            || !valid_locale(locale)
            || episodes.iter().any(|reference| {
                reference.1 == 0
                    || reference.0 > 999
                    || reference.1 > 999
                    || !unique.insert(*reference)
            })
        {
            return Err(ProviderError::InvalidResponse);
        }
        let futures = episodes.iter().map(|&(season, episode)| async move {
            let path = format!("tv/{series_id}/season/{season}/episode/{episode}");
            let dto: EpisodeDto = self
                .get_json(token, &path, &[("language", locale.to_owned())])
                .await?;
            if dto.id <= 0
                || dto.season_number != season
                || dto.episode_number != episode
                || dto.name.trim().is_empty()
                || dto.name.chars().count() > 512
            {
                return Err(ProviderError::InvalidResponse);
            }
            Ok(EpisodeIdentity { season, episode })
        });
        futures_util::future::join_all(futures)
            .await
            .into_iter()
            .collect()
    }

    async fn details_dto(
        &self,
        token: &SecretBytes,
        media_kind: ProviderMediaKind,
        provider_id: i64,
        locale: &str,
    ) -> Result<WorkDto, ProviderError> {
        let family = match media_kind {
            ProviderMediaKind::Movie => "movie",
            ProviderMediaKind::Tv => "tv",
        };
        self.get_json(
            token,
            &format!("{family}/{provider_id}"),
            &[("language", locale.to_owned())],
        )
        .await
    }

    #[doc(hidden)]
    /// 访问有界静态相对端点，供连接和错误分类测试使用。
    ///
    /// # Errors
    ///
    /// `path` 为空、过长、包含 `..` 或不受支持字符时返回 [`ProviderError::InvalidResponse`]；
    /// 也会返回凭据、网络、超时、限流、响应大小及响应状态对应的常规 [`ProviderError`]。
    pub async fn probe(&self, token: &SecretBytes, path: &str) -> Result<(), ProviderError> {
        if path.is_empty()
            || path.len() > 128
            || path.contains("..")
            || !path
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_'))
        {
            return Err(ProviderError::InvalidResponse);
        }
        self.get_bytes(token, path, &[]).await.map(|_| ())
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        token: &SecretBytes,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ProviderError> {
        let bytes = self.get_bytes(token, path, query).await?;
        serde_json::from_slice(&bytes).map_err(|_| ProviderError::InvalidResponse)
    }

    async fn get_bytes(
        &self,
        token: &SecretBytes,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<u8>, ProviderError> {
        let _permit =
            self.permits
                .acquire()
                .await
                .map_err(|_| ProviderError::TemporarilyUnavailable {
                    retry_at_us: retry_after_seconds(60),
                })?;
        let url = self
            .origin
            .join(path)
            .map_err(|_| ProviderError::InvalidResponse)?;
        if url.origin() != self.origin.origin() {
            return Err(ProviderError::InvalidResponse);
        }
        let token = std::str::from_utf8(token.expose())
            .ok()
            .filter(|value| (16..=4096).contains(&value.len()))
            .ok_or(ProviderError::CredentialsInvalid)?;
        let authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| ProviderError::CredentialsInvalid)?;
        let query = query
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect::<Vec<_>>();
        let response = self
            .http
            .get(url)
            .header(AUTHORIZATION, authorization)
            .query(&query)
            .send()
            .await
            .map_err(|error| map_transport_error(&error))?;
        match response.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err(ProviderError::CredentialsInvalid),
            429 => {
                let seconds = response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(60)
                    .clamp(1, 86_400);
                return Err(ProviderError::RateLimited {
                    retry_at_us: retry_after_seconds(seconds),
                });
            }
            500..=599 => {
                return Err(ProviderError::TemporarilyUnavailable {
                    retry_at_us: retry_after_seconds(60),
                });
            }
            _ => return Err(ProviderError::InvalidResponse),
        }
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > self.options.max_response_bytes)
        {
            return Err(ProviderError::ResponseTooLarge);
        }
        let mut response = response;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| map_transport_error(&error))?
        {
            if body.len().saturating_add(chunk.len()) > self.options.max_response_bytes {
                return Err(ProviderError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

fn validate_search(request: &TmdbSearchRequest) -> Result<(), ProviderError> {
    if request.title.trim().is_empty()
        || request.title.chars().count() > 512
        || request.title.chars().any(char::is_control)
        || !valid_locale(&request.locale)
        || request.region.as_deref().is_some_and(|region| {
            region.len() != 2 || !region.bytes().all(|byte| byte.is_ascii_uppercase())
        })
    {
        Err(ProviderError::InvalidResponse)
    } else {
        Ok(())
    }
}

fn valid_external_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn map_transport_error(error: &reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::TemporarilyUnavailable {
            retry_at_us: retry_after_seconds(60),
        }
    }
}

fn retry_after_seconds(seconds: i64) -> i64 {
    chrono::Utc::now()
        .timestamp_micros()
        .saturating_add(seconds.saturating_mul(1_000_000))
}

#[async_trait::async_trait]
impl TmdbConnectionTester for TmdbClient {
    async fn test(
        &self,
        token: &SecretBytes,
        _locale: &str,
        _region: Option<&str>,
    ) -> Result<(), ProviderError> {
        self.probe(token, "configuration").await
    }
}
