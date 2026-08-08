use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::connectors::model::{
    CacheStatus, EpisodeIdentity, MetadataCandidate, ProviderError, ProviderMediaKind,
    TmdbExternalIdRequest, TmdbSearchRequest,
};
use crate::connectors::service::MetadataProvider;
use crate::connectors::store::{ConnectorStore, TmdbCacheRecord};
use crate::connectors::tmdb::client::TmdbClient;
use crate::connectors::tmdb::singleflight::SingleFlight;
use crate::platform::secrets::SecretBytes;

/// 持久化 TMDB 结果的新鲜期与 stale-if-error 窗口。
#[derive(Clone, Copy, Debug)]
pub struct TmdbCachePolicy {
    /// 文本搜索与外部 ID 结果的新鲜期。
    pub search_fresh: Duration,
    /// 详情结果的新鲜期。
    pub detail_fresh: Duration,
    /// 空结果的新鲜期。
    pub negative_fresh: Duration,
    /// 暂时性错误时允许额外使用过期值的时长。
    pub stale_grace: Duration,
}

impl Default for TmdbCachePolicy {
    fn default() -> Self {
        Self {
            search_fresh: Duration::from_hours(1),
            detail_fresh: Duration::from_hours(24),
            negative_fresh: Duration::from_mins(15),
            stale_grace: Duration::from_hours(168),
        }
    }
}

/// 使用持久化 SHA-256 键与按键 single-flight 抑制的缓存元数据提供方。
#[derive(Clone)]
pub struct TmdbProvider {
    store: ConnectorStore,
    client: TmdbClient,
    policy: TmdbCachePolicy,
    flights: SingleFlight,
}

impl TmdbProvider {
    #[must_use]
    /// 将提供方绑定到 Core 数据库与固定来源客户端。
    pub fn new(pool: SqlitePool, client: TmdbClient, policy: TmdbCachePolicy) -> Self {
        Self {
            store: ConnectorStore::new(pool),
            client,
            policy,
            flights: SingleFlight::default(),
        }
    }

    /// 非空候选按构造时传入的 `policy.search_fresh` 缓存，空结果按 `policy.negative_fresh`
    /// 负缓存，并在暂时性错误时回退到过期值；默认新鲜期分别为一小时和 15 分钟。
    ///
    /// # Errors
    ///
    /// 返回不含上游正文或凭据的稳定缓存/提供方错误。
    pub async fn search(
        &self,
        token: &SecretBytes,
        request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        let key = cache_key(b"search", request)?;
        let now_us = chrono::Utc::now().timestamp_micros();
        if let Some(cached) = self.load(key, now_us).await?
            && cached.fresh_until_us > now_us
        {
            return decode_cache(&cached, CacheStatus::Fresh, now_us);
        }
        let _flight = self.flights.enter(key).await;
        let now_us = chrono::Utc::now().timestamp_micros();
        let cached = self.load(key, now_us).await?;
        if let Some(cached) = cached
            .as_ref()
            .filter(|value| value.fresh_until_us > now_us)
        {
            return decode_cache(cached, CacheStatus::Fresh, now_us);
        }
        match self.client.search(token, request).await {
            Ok(candidates) => {
                let saved_at_us = chrono::Utc::now().timestamp_micros();
                self.save(
                    key,
                    &candidates,
                    if candidates.is_empty() {
                        self.policy.negative_fresh
                    } else {
                        self.policy.search_fresh
                    },
                    saved_at_us,
                )
                .await?;
                Ok(candidates)
            }
            Err(error) if temporary(error) => cached.map_or(Err(error), |value| {
                decode_cache(&value, CacheStatus::Stale, now_us)
            }),
            Err(error) => Err(error),
        }
    }

    /// 使用与文本搜索相同的分支缓存策略解析外部 ID：非空候选使用 `policy.search_fresh`，
    /// 空结果使用 `policy.negative_fresh`，暂时性错误可回退到过期值。
    ///
    /// # Errors
    ///
    /// 返回稳定缓存或提供方错误。
    pub async fn find_external(
        &self,
        token: &SecretBytes,
        request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        let key = cache_key(b"find", request)?;
        let now_us = chrono::Utc::now().timestamp_micros();
        if let Some(cached) = self.load(key, now_us).await?
            && cached.fresh_until_us > now_us
        {
            return decode_cache(&cached, CacheStatus::Fresh, now_us);
        }
        let _flight = self.flights.enter(key).await;
        let now_us = chrono::Utc::now().timestamp_micros();
        let cached = self.load(key, now_us).await?;
        if let Some(cached) = cached
            .as_ref()
            .filter(|value| value.fresh_until_us > now_us)
        {
            return decode_cache(cached, CacheStatus::Fresh, now_us);
        }
        match self.client.find_external(token, request).await {
            Ok(candidates) => {
                let saved_at_us = chrono::Utc::now().timestamp_micros();
                self.save(
                    key,
                    &candidates,
                    if candidates.is_empty() {
                        self.policy.negative_fresh
                    } else {
                        self.policy.search_fresh
                    },
                    saved_at_us,
                )
                .await?;
                Ok(candidates)
            }
            Err(error) if temporary(error) => cached.map_or(Err(error), |value| {
                decode_cache(&value, CacheStatus::Stale, now_us)
            }),
            Err(error) => Err(error),
        }
    }

    /// 按构造时传入的 `policy.detail_fresh` 使用详情缓存加载合并结果；默认策略的新鲜期为
    /// 24 小时。
    ///
    /// # Errors
    ///
    /// 返回稳定缓存或提供方错误。
    pub async fn details(
        &self,
        token: &SecretBytes,
        media_kind: ProviderMediaKind,
        provider_id: i64,
        locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        let key = cache_key(b"details", &(media_kind, provider_id, locale))?;
        let now_us = chrono::Utc::now().timestamp_micros();
        if let Some(cached) = self.load(key, now_us).await?
            && cached.fresh_until_us > now_us
        {
            return one_cached(&cached, CacheStatus::Fresh, now_us);
        }
        let _flight = self.flights.enter(key).await;
        let now_us = chrono::Utc::now().timestamp_micros();
        let cached = self.load(key, now_us).await?;
        if let Some(cached) = cached
            .as_ref()
            .filter(|value| value.fresh_until_us > now_us)
        {
            return one_cached(cached, CacheStatus::Fresh, now_us);
        }
        match self
            .client
            .details_with_language_fallback(token, media_kind, provider_id, locale)
            .await
        {
            Ok(candidate) => {
                let saved_at_us = chrono::Utc::now().timestamp_micros();
                self.save(
                    key,
                    std::slice::from_ref(&candidate),
                    self.policy.detail_fresh,
                    saved_at_us,
                )
                .await?;
                Ok(candidate)
            }
            Err(error) if temporary(error) => cached.map_or(Err(error), |value| {
                one_cached(&value, CacheStatus::Stale, now_us)
            }),
            Err(error) => Err(error),
        }
    }

    async fn load(
        &self,
        key: [u8; 32],
        now_us: i64,
    ) -> Result<Option<TmdbCacheRecord>, ProviderError> {
        self.store.load_tmdb_cache(&key, now_us).await.map_err(|_| {
            ProviderError::TemporarilyUnavailable {
                retry_at_us: now_us.saturating_add(60_000_000),
            }
        })
    }

    async fn save(
        &self,
        key: [u8; 32],
        candidates: &[MetadataCandidate],
        fresh: Duration,
        now_us: i64,
    ) -> Result<(), ProviderError> {
        let response = if candidates.is_empty() {
            None
        } else {
            Some(serde_json::to_vec(candidates).map_err(|_| ProviderError::InvalidResponse)?)
        };
        if response
            .as_ref()
            .is_some_and(|bytes| bytes.len() > 2 * 1024 * 1024)
        {
            return Err(ProviderError::InvalidResponse);
        }
        let fresh_until_us = add_duration(now_us, fresh);
        let stale_until_us = add_duration(fresh_until_us, self.policy.stale_grace);
        self.store
            .save_tmdb_cache(
                &key,
                response.as_deref(),
                fresh_until_us,
                stale_until_us,
                now_us,
            )
            .await
            .map_err(|_| ProviderError::TemporarilyUnavailable {
                retry_at_us: now_us.saturating_add(60_000_000),
            })
    }
}

fn cache_key<T: serde::Serialize>(namespace: &[u8], value: &T) -> Result<[u8; 32], ProviderError> {
    let encoded = serde_json::to_vec(value).map_err(|_| ProviderError::InvalidResponse)?;
    let mut digest = Sha256::new();
    digest.update(b"mediaflow.tmdb-cache.v1\0");
    digest.update(namespace);
    digest.update([0]);
    digest.update(encoded);
    Ok(digest.finalize().into())
}

fn decode_cache(
    cached: &TmdbCacheRecord,
    status: CacheStatus,
    now_us: i64,
) -> Result<Vec<MetadataCandidate>, ProviderError> {
    decode_cache_bytes(cached.response_json.as_deref(), status, now_us)
}

fn decode_cache_bytes(
    bytes: Option<&[u8]>,
    status: CacheStatus,
    now_us: i64,
) -> Result<Vec<MetadataCandidate>, ProviderError> {
    let Some(bytes) = bytes else {
        return Ok(Vec::new());
    };
    let mut candidates: Vec<MetadataCandidate> =
        serde_json::from_slice(bytes).map_err(|_| ProviderError::InvalidResponse)?;
    if candidates.len() > 20 {
        return Err(ProviderError::InvalidResponse);
    }
    for candidate in &mut candidates {
        candidate.cache_status = status;
        candidate.retrieved_at_us = now_us;
    }
    Ok(candidates)
}

fn one_cached(
    cached: &TmdbCacheRecord,
    status: CacheStatus,
    now_us: i64,
) -> Result<MetadataCandidate, ProviderError> {
    let mut candidates = decode_cache(cached, status, now_us)?;
    if candidates.len() != 1 {
        return Err(ProviderError::InvalidResponse);
    }
    Ok(candidates.remove(0))
}

fn temporary(error: ProviderError) -> bool {
    matches!(
        error,
        ProviderError::RateLimited { .. }
            | ProviderError::TemporarilyUnavailable { .. }
            | ProviderError::Timeout
    )
}

fn add_duration(timestamp_us: i64, duration: Duration) -> i64 {
    timestamp_us.saturating_add(i64::try_from(duration.as_micros()).unwrap_or(i64::MAX))
}

#[async_trait::async_trait]
impl MetadataProvider for TmdbProvider {
    async fn find_external(
        &self,
        token: &SecretBytes,
        request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Self::find_external(self, token, request).await
    }

    async fn search(
        &self,
        token: &SecretBytes,
        request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Self::search(self, token, request).await
    }

    async fn details(
        &self,
        token: &SecretBytes,
        media_kind: ProviderMediaKind,
        provider_id: i64,
        locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        Self::details(self, token, media_kind, provider_id, locale).await
    }

    async fn verify_episodes(
        &self,
        token: &SecretBytes,
        series_id: i64,
        episodes: &[(u16, u16)],
        locale: &str,
    ) -> Result<Vec<EpisodeIdentity>, ProviderError> {
        self.client
            .verify_episodes(token, series_id, episodes, locale)
            .await
    }
}
