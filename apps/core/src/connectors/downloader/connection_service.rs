use std::path::Path;
use std::sync::Arc;

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::connectors::downloader::qbittorrent::QbittorrentClient;
use crate::connectors::downloader::transmission::TransmissionClient;
use crate::connectors::model::IntegrationHealth;
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, PageRequest};

use super::connection_store::DownloaderConnectionStore;
use super::model::{
    DownloaderConnection, DownloaderConnectionInput, DownloaderConnectionProbe,
    DownloaderConnectionTestResult, DownloaderConnectionView, DownloaderFailureCode,
    DownloaderKind, LoadedDownloaderCredentials,
};
use super::port::{DownloadSource, DownloadSourceError, DownloaderEndpoint};

#[derive(Clone)]
/// 只允许 qBittorrent 与 Transmission 两种编译期内置适配器的注册表。
pub struct DownloaderRegistry {
    qbittorrent: Arc<dyn DownloadSource>,
    transmission: Arc<dyn DownloadSource>,
}

impl DownloaderRegistry {
    #[must_use]
    /// 使用两个显式适配器构建注册表；主要用于测试替身注入。
    pub fn new(
        qbittorrent: Arc<dyn DownloadSource>,
        transmission: Arc<dyn DownloadSource>,
    ) -> Self {
        Self {
            qbittorrent,
            transmission,
        }
    }

    /// 构建两个有界、禁止重定向的生产适配器。
    ///
    /// # Errors
    ///
    /// 任一底层 HTTP 客户端无法构建时返回启动配置错误。
    pub fn production() -> Result<Self, AppError> {
        let qbittorrent = QbittorrentClient::production()
            .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
        let transmission = TransmissionClient::production()
            .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
        Ok(Self::new(Arc::new(qbittorrent), Arc::new(transmission)))
    }

    #[must_use]
    /// 返回指定内置类型的不可变适配器。
    pub fn source(&self, kind: DownloaderKind) -> Arc<dyn DownloadSource> {
        match kind {
            DownloaderKind::Qbittorrent => self.qbittorrent.clone(),
            DownloaderKind::Transmission => self.transmission.clone(),
        }
    }
}

#[derive(Clone)]
/// 协调候选探测、认证加密持久化、能力健康投影和版本化删除。
pub struct DownloaderConnectionService {
    store: DownloaderConnectionStore,
    registry: DownloaderRegistry,
}

impl DownloaderConnectionService {
    /// 打开下载器连接服务并固定当前实例密钥。
    ///
    /// # Errors
    ///
    /// 实例密钥不安全或不可用时返回配置错误。
    pub fn open(
        pool: SqlitePool,
        config_dir: &Path,
        registry: DownloaderRegistry,
    ) -> Result<Self, AppError> {
        Ok(Self {
            store: DownloaderConnectionStore::open(pool, config_dir)?,
            registry,
        })
    }

    /// 返回一页脱敏、RFC3339 时间的连接投影。
    ///
    /// # Errors
    ///
    /// 游标、数据库或持久行无效时返回稳定应用错误。
    pub async fn list(
        &self,
        page: &PageRequest,
    ) -> Result<CursorPage<DownloaderConnectionView>, AppError> {
        let page = self.store.list_page(page).await?;
        Ok(CursorPage {
            items: page
                .items
                .into_iter()
                .map(project)
                .collect::<Result<Vec<_>, _>>()?,
            next_cursor: page.next_cursor,
        })
    }

    /// 读取一项脱敏连接。
    ///
    /// # Errors
    ///
    /// 连接不存在或持久行无效时返回稳定应用错误。
    pub async fn get(&self, id: Uuid) -> Result<DownloaderConnectionView, AppError> {
        let record = self
            .store
            .get(id)
            .await?
            .ok_or_else(|| not_found("downloader connection not found"))?;
        project(record)
    }

    /// 探测候选输入但不保存连接、凭据或能力。
    ///
    /// # Errors
    ///
    /// 候选结构无效时返回校验错误；协议失败作为公开脱敏测试结果返回。
    pub async fn test(
        &self,
        input: DownloaderConnectionInput,
    ) -> Result<DownloaderConnectionTestResult, AppError> {
        let input = input.validate()?;
        let credentials = LoadedDownloaderCredentials::decode(input.credentials.expose())?;
        let source = self.registry.source(input.kind);
        let result = source
            .probe(DownloaderEndpoint {
                base_url: &input.base_url,
                credentials: &credentials,
            })
            .await;
        let checked_at = format_time(chrono::Utc::now().timestamp_micros())?;
        Ok(match result {
            Ok(capabilities) => DownloaderConnectionTestResult {
                reachable: true,
                health: IntegrationHealth::Healthy,
                capabilities: Some(capabilities),
                failure_code: None,
                checked_at,
            },
            Err(error) => {
                let projection = failed_probe(error);
                DownloaderConnectionTestResult {
                    reachable: false,
                    health: projection.health,
                    capabilities: None,
                    failure_code: projection.failure_code,
                    checked_at,
                }
            }
        })
    }

    /// 保存一项加密连接并提交一次成功或失败的独立健康探测。
    ///
    /// # Errors
    ///
    /// 输入、密钥、数据库或探测投影提交失败时返回稳定应用错误。
    pub async fn create(
        &self,
        input: DownloaderConnectionInput,
    ) -> Result<DownloaderConnectionView, AppError> {
        let id = Uuid::now_v7();
        let record = self
            .store
            .insert(id, input, chrono::Utc::now().timestamp_micros())
            .await?;
        self.probe_saved(&record).await?;
        self.get(id).await
    }

    /// 以当前版本完整替换并重新探测连接。
    ///
    /// # Errors
    ///
    /// 连接不存在、输入/密钥/数据库或乐观并发失败时返回稳定应用错误。
    pub async fn replace(
        &self,
        id: Uuid,
        expected_version: i64,
        input: DownloaderConnectionInput,
    ) -> Result<DownloaderConnectionView, AppError> {
        if self.store.get(id).await?.is_none() {
            return Err(not_found("downloader connection not found"));
        }
        let record = self
            .store
            .replace(
                id,
                expected_version,
                input,
                chrono::Utc::now().timestamp_micros(),
            )
            .await?;
        self.probe_saved(&record).await?;
        self.get(id).await
    }

    /// 以当前版本删除本地连接，不发送远端删除命令。
    ///
    /// # Errors
    ///
    /// 连接不存在、数据库或乐观并发失败时返回稳定应用错误。
    pub async fn delete(&self, id: Uuid, expected_version: i64) -> Result<(), AppError> {
        if self.store.get(id).await?.is_none() {
            return Err(not_found("downloader connection not found"));
        }
        self.store.delete(id, expected_version).await
    }

    async fn probe_saved(&self, record: &DownloaderConnection) -> Result<(), AppError> {
        let credentials = self.store.load_secret(record.id).await?;
        let source = self.registry.source(record.kind);
        let result = source
            .probe(DownloaderEndpoint {
                base_url: &record.base_url,
                credentials: &credentials,
            })
            .await;
        let projection = match result {
            Ok(capabilities) => DownloaderConnectionProbe {
                health: IntegrationHealth::Healthy,
                failure_code: None,
                capabilities: Some(capabilities),
            },
            Err(error) => failed_probe(error),
        };
        self.store
            .commit_probe(
                record.id,
                record.config_version,
                projection,
                chrono::Utc::now().timestamp_micros(),
            )
            .await
    }
}

fn failed_probe(error: DownloadSourceError) -> DownloaderConnectionProbe {
    let (health, failure_code) = match error {
        DownloadSourceError::Unauthorized => (
            IntegrationHealth::Unauthorized,
            DownloaderFailureCode::IntegrationUnauthorized,
        ),
        DownloadSourceError::RateLimited => (
            IntegrationHealth::RateLimited,
            DownloaderFailureCode::IntegrationRateLimited,
        ),
        DownloadSourceError::UnsupportedVersion => (
            IntegrationHealth::Degraded,
            DownloaderFailureCode::UnsupportedVersion,
        ),
        DownloadSourceError::Timeout => (
            IntegrationHealth::Unavailable,
            DownloaderFailureCode::ProviderTimeout,
        ),
        DownloadSourceError::ResponseTooLarge => (
            IntegrationHealth::Unavailable,
            DownloaderFailureCode::ResponseTooLarge,
        ),
        DownloadSourceError::InvalidResponse
        | DownloadSourceError::CorrelationAmbiguous
        | DownloadSourceError::RemoteMissing => (
            IntegrationHealth::Unavailable,
            DownloaderFailureCode::InvalidResponse,
        ),
        DownloadSourceError::Unavailable => (
            IntegrationHealth::Unavailable,
            DownloaderFailureCode::IntegrationUnavailable,
        ),
    };
    DownloaderConnectionProbe {
        health,
        failure_code: Some(failure_code),
        capabilities: None,
    }
}

fn project(record: DownloaderConnection) -> Result<DownloaderConnectionView, AppError> {
    Ok(DownloaderConnectionView {
        id: record.id,
        kind: record.kind,
        display_name: record.display_name,
        base_url: record.base_url,
        enabled: record.enabled,
        config_version: record.config_version,
        capabilities: record.capabilities,
        health: record.health,
        checked_at: record.checked_at_us.map(format_time).transpose()?,
        failure_code: record.failure_code,
        updated_at: format_time(record.updated_at_us)?,
    })
}

fn format_time(value: i64) -> Result<String, AppError> {
    chrono::DateTime::from_timestamp_micros(value)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "invalid downloader timestamp"))
}

fn not_found(message: &'static str) -> AppError {
    AppError::new(ErrorCode::NotFound, message)
}
