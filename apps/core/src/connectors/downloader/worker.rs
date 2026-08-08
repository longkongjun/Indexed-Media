use std::collections::HashMap;
use std::sync::Arc;

use crate::platform::task_runtime::TaskClock;
use crate::shared::error::{AppError, ErrorCode};

use super::connection_service::DownloaderRegistry;
use super::connection_store::DownloaderConnectionStore;
use super::model::{
    DownloadTaskLease, DownloaderConnection, DownloaderConnectionProbe, DownloaderFailureCode,
};
use super::port::{
    AddDownloadRequest, DownloadSourceError, DownloaderEndpoint, RemoteDownloadQuery,
    RemoteDownloadSnapshot,
};
use super::task_store::DownloadTaskStore;

const MONITOR_BATCH: u16 = 100;

/// 对一个连接执行先查询后提交、批量监控和有限重试的下载任务 worker。
pub struct DownloadTaskWorker {
    id: String,
    tasks: DownloadTaskStore,
    connections: DownloaderConnectionStore,
    registry: DownloaderRegistry,
    clock: Arc<dyn TaskClock>,
}

impl DownloadTaskWorker {
    #[must_use]
    /// 使用共享持久化边界、内置适配器注册表与注入时钟创建 worker。
    pub fn new(
        id: String,
        tasks: DownloadTaskStore,
        connections: DownloaderConnectionStore,
        registry: DownloaderRegistry,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        Self {
            id,
            tasks,
            connections,
            registry,
            clock,
        }
    }

    /// 处理最早连接的一批到期任务；队列为空时返回零。
    ///
    /// 未关联任务总是先按 correlation tag 查询，只有零匹配才调用远端新增；随后所有已关联任务用
    /// 单次有界查询同步。外部失败只改变本批任务和本连接的脱敏投影。
    ///
    /// # Errors
    ///
    /// 租约、密钥、数据库、关联唯一性或持久 outbox 提交失败时返回稳定应用错误。
    #[allow(clippy::too_many_lines)]
    pub async fn run_once(&self) -> Result<usize, AppError> {
        let now_us = self.clock.now_us();
        let mut leases = self
            .tasks
            .claim_batch(&self.id, now_us, MONITOR_BATCH)
            .await?;
        if leases.is_empty() {
            return Ok(0);
        }
        let count = leases.len();
        let connection_id = leases[0].connection_id;
        if leases
            .iter()
            .any(|lease| lease.connection_id != connection_id)
        {
            return Err(AppError::new(
                ErrorCode::Internal,
                "download task batch crossed connection boundary",
            ));
        }
        let Some(connection) = self.connections.get(connection_id).await? else {
            for lease in &leases {
                self.tasks
                    .fail(
                        lease.task_id,
                        Some(&self.id),
                        DownloaderFailureCode::IntegrationNotConfigured,
                        now_us,
                    )
                    .await?;
            }
            return Ok(count);
        };
        if !connection.enabled {
            for lease in &leases {
                self.tasks
                    .fail(
                        lease.task_id,
                        Some(&self.id),
                        DownloaderFailureCode::IntegrationNotConfigured,
                        now_us,
                    )
                    .await?;
            }
            return Ok(count);
        }
        let credentials = self.connections.load_secret(connection.id).await?;
        let source = self.registry.source(connection.kind);
        let endpoint = DownloaderEndpoint {
            base_url: &connection.base_url,
            credentials: &credentials,
        };
        let mut linked = Vec::with_capacity(leases.len());
        for (index, lease) in leases.iter_mut().enumerate() {
            if let Some(remote_id) = &lease.remote_id {
                linked.push((index, remote_id.clone()));
                continue;
            }
            let recovered = source
                .fetch(
                    endpoint,
                    RemoteDownloadQuery {
                        remote_ids: Vec::new(),
                        correlation_tag: Some(lease.correlation_tag.clone()),
                    },
                )
                .await;
            let remote_id = match recovered {
                Ok(matches) if matches.is_empty() => source
                    .add(
                        endpoint,
                        AddDownloadRequest {
                            source: &lease.source,
                            correlation_tag: &lease.correlation_tag,
                        },
                    )
                    .await
                    .map(|reference| reference.remote_id),
                Ok(matches) if matches.len() == 1 => Ok(matches[0].remote_id.clone()),
                Ok(_) => Err(DownloadSourceError::CorrelationAmbiguous),
                Err(error) => Err(error),
            };
            match remote_id {
                Ok(remote_id) => {
                    self.tasks
                        .commit_remote_ref(lease.task_id, &self.id, &remote_id, now_us)
                        .await?;
                    lease.remote_id = Some(remote_id.clone());
                    linked.push((index, remote_id));
                }
                Err(error) => {
                    self.handle_error(&connection, lease, error, now_us).await?;
                }
            }
        }
        if linked.is_empty() {
            return Ok(count);
        }
        let snapshots = source
            .fetch(
                endpoint,
                RemoteDownloadQuery {
                    remote_ids: linked.iter().map(|(_, id)| id.clone()).collect(),
                    correlation_tag: None,
                },
            )
            .await;
        let snapshots = match snapshots {
            Ok(snapshots) => match index_snapshots(snapshots) {
                Ok(snapshots) => snapshots,
                Err(error) => {
                    for (index, _) in &linked {
                        self.handle_error(&connection, &leases[*index], error, now_us)
                            .await?;
                    }
                    return Ok(count);
                }
            },
            Err(error) => {
                for (index, _) in &linked {
                    self.handle_error(&connection, &leases[*index], error, now_us)
                        .await?;
                }
                return Ok(count);
            }
        };
        for (index, remote_id) in linked {
            let lease = &leases[index];
            if let Some(snapshot) = snapshots.get(&remote_id) {
                self.tasks
                    .commit_snapshot(
                        lease.task_id,
                        &self.id,
                        snapshot.status,
                        snapshot.progress_basis_points,
                        now_us,
                    )
                    .await?;
            } else {
                self.tasks
                    .fail(
                        lease.task_id,
                        Some(&self.id),
                        DownloaderFailureCode::RemoteMissing,
                        now_us,
                    )
                    .await?;
            }
        }
        Ok(count)
    }

    /// 重新排队所有连接配置版本已增加的阻塞任务。
    ///
    /// # Errors
    ///
    /// 数据库、outbox 或持久投影无效时返回稳定应用错误。
    pub async fn prepare(&self) -> Result<u64, AppError> {
        self.tasks
            .requeue_after_config_change(self.clock.now_us())
            .await
    }

    async fn handle_error(
        &self,
        connection: &DownloaderConnection,
        lease: &DownloadTaskLease,
        error: DownloadSourceError,
        now_us: i64,
    ) -> Result<(), AppError> {
        let (failure, terminal_until_config, retryable) = classify(error);
        if matches!(
            failure,
            DownloaderFailureCode::IntegrationUnauthorized
                | DownloaderFailureCode::UnsupportedVersion
                | DownloaderFailureCode::IntegrationRateLimited
                | DownloaderFailureCode::IntegrationUnavailable
        ) {
            let health = match failure {
                DownloaderFailureCode::IntegrationUnauthorized => {
                    crate::connectors::model::IntegrationHealth::Unauthorized
                }
                DownloaderFailureCode::IntegrationRateLimited => {
                    crate::connectors::model::IntegrationHealth::RateLimited
                }
                DownloaderFailureCode::IntegrationUnavailable => {
                    crate::connectors::model::IntegrationHealth::Unavailable
                }
                _ => crate::connectors::model::IntegrationHealth::Degraded,
            };
            self.connections
                .commit_probe(
                    connection.id,
                    connection.config_version,
                    DownloaderConnectionProbe {
                        health,
                        failure_code: Some(failure),
                        capabilities: None,
                    },
                    now_us,
                )
                .await?;
        }
        if terminal_until_config {
            self.tasks
                .fail_until_config_change(
                    lease.task_id,
                    &self.id,
                    failure,
                    connection.config_version,
                    now_us,
                )
                .await?;
        } else if retryable {
            self.tasks
                .schedule_retry(
                    lease.task_id,
                    &self.id,
                    failure,
                    retry_time(now_us, lease.attempt_count)?,
                    now_us,
                )
                .await?;
        } else {
            self.tasks
                .fail(lease.task_id, Some(&self.id), failure, now_us)
                .await?;
        }
        Ok(())
    }
}

fn index_snapshots(
    snapshots: Vec<RemoteDownloadSnapshot>,
) -> Result<HashMap<String, RemoteDownloadSnapshot>, DownloadSourceError> {
    let mut indexed = HashMap::with_capacity(snapshots.len());
    for snapshot in snapshots {
        if indexed
            .insert(snapshot.remote_id.clone(), snapshot)
            .is_some()
        {
            return Err(DownloadSourceError::InvalidResponse);
        }
    }
    Ok(indexed)
}

const fn classify(error: DownloadSourceError) -> (DownloaderFailureCode, bool, bool) {
    match error {
        DownloadSourceError::Unauthorized => {
            (DownloaderFailureCode::IntegrationUnauthorized, true, false)
        }
        DownloadSourceError::UnsupportedVersion => {
            (DownloaderFailureCode::UnsupportedVersion, true, false)
        }
        DownloadSourceError::RateLimited => {
            (DownloaderFailureCode::IntegrationRateLimited, false, true)
        }
        DownloadSourceError::Unavailable => {
            (DownloaderFailureCode::IntegrationUnavailable, false, true)
        }
        DownloadSourceError::Timeout => (DownloaderFailureCode::ProviderTimeout, false, true),
        DownloadSourceError::ResponseTooLarge => {
            (DownloaderFailureCode::ResponseTooLarge, false, false)
        }
        DownloadSourceError::InvalidResponse => {
            (DownloaderFailureCode::InvalidResponse, false, false)
        }
        DownloadSourceError::CorrelationAmbiguous => {
            (DownloaderFailureCode::CorrelationAmbiguous, false, false)
        }
        DownloadSourceError::RemoteMissing => (DownloaderFailureCode::RemoteMissing, false, false),
    }
}

fn retry_time(now_us: i64, attempt_count: u32) -> Result<i64, AppError> {
    let shift = attempt_count.saturating_sub(1).min(8);
    let seconds = 1_i64 << shift;
    now_us
        .checked_add(seconds * 1_000_000)
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "download retry time overflow"))
}
