use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::connectors::downloader::model::CreateDownloadTaskCommand;
use crate::connectors::downloader::task_service::DownloadTaskService;
use crate::connectors::model::SecretString;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::task_runtime::TaskClock;
use crate::shared::error::{AppError, ErrorCode};

use super::event_store::{AutomationEventPayload, AutomationEventStore};
use super::model::AutomationFailureCode;

/// Redacted downstream action failure used by fixed automation ports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutomationActionError {
    code: AutomationFailureCode,
    recoverable: bool,
}

impl AutomationActionError {
    #[must_use]
    /// Build a temporary integration-unavailable failure.
    pub const fn unavailable() -> Self {
        Self {
            code: AutomationFailureCode::IntegrationUnavailable,
            recoverable: true,
        }
    }

    #[must_use]
    /// Build an explicit redacted downstream failure.
    pub const fn new(code: AutomationFailureCode, recoverable: bool) -> Self {
        Self { code, recoverable }
    }
}

#[async_trait]
/// Narrow port for creating a managed download task.
pub trait DownloadTaskCreationPort: Send + Sync {
    /// Idempotently create or recover one downstream download task.
    async fn create_download(
        &self,
        connection_id: Uuid,
        source: &[u8],
        display_name: &str,
        idempotency_key: &str,
    ) -> Result<Uuid, AutomationActionError>;
}

#[async_trait]
/// Narrow port for requesting one registered inbox reconciliation.
pub trait InboxReconcilePort: Send + Sync {
    /// Idempotently create or recover one downstream reconcile request.
    async fn reconcile_inbox(
        &self,
        inbox_id: Uuid,
        idempotency_key: &str,
    ) -> Result<InboxReconcileResult, AutomationActionError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Stable downstream reconcile relation and frozen result count when already terminal.
pub struct InboxReconcileResult {
    pub request_id: Uuid,
    pub result_count: i64,
}

#[async_trait]
impl DownloadTaskCreationPort for DownloadTaskService {
    async fn create_download(
        &self,
        connection_id: Uuid,
        source: &[u8],
        display_name: &str,
        idempotency_key: &str,
    ) -> Result<Uuid, AutomationActionError> {
        let source = String::from_utf8(source.to_vec()).map_err(|_| {
            AutomationActionError::new(AutomationFailureCode::PayloadInvalid, false)
        })?;
        self.create(
            CreateDownloadTaskCommand {
                connection_id,
                source: SecretString::new(source),
                display_name: display_name.to_owned(),
            },
            idempotency_key,
        )
        .await
        .map(|task| task.id)
        .map_err(|error| map_app_error(&error))
    }
}

#[derive(Clone, Copy, Debug, Default)]
/// Explicit unavailable reconcile port for isolated workers and tests.
pub struct UnavailableInboxReconcilePort;

#[async_trait]
impl InboxReconcilePort for UnavailableInboxReconcilePort {
    async fn reconcile_inbox(
        &self,
        _inbox_id: Uuid,
        _idempotency_key: &str,
    ) -> Result<InboxReconcileResult, AutomationActionError> {
        Err(AutomationActionError::unavailable())
    }
}

#[derive(Clone)]
/// Executes one leased event exclusively through two fixed downstream ports.
pub struct AutomationWorker {
    store: AutomationEventStore,
    downloads: Arc<dyn DownloadTaskCreationPort>,
    reconciles: Arc<dyn InboxReconcilePort>,
    clock: Arc<dyn TaskClock>,
    lease_duration_us: i64,
}

impl std::fmt::Debug for AutomationWorker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AutomationWorker")
            .field("lease_duration_us", &self.lease_duration_us)
            .finish_non_exhaustive()
    }
}

impl AutomationWorker {
    /// Open a worker around explicit downstream ports and clock.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the event store key cannot be opened.
    pub fn open(
        pool: SqlitePool,
        config_dir: &Path,
        downloads: Arc<dyn DownloadTaskCreationPort>,
        reconciles: Arc<dyn InboxReconcilePort>,
        clock: Arc<dyn TaskClock>,
    ) -> Result<Self, AppError> {
        Self::open_with_notifier(
            pool,
            config_dir,
            downloads,
            reconciles,
            clock,
            OutboxNotifier::new(),
        )
    }

    /// Open a worker with the shared post-commit outbox notifier.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the event store key cannot be opened.
    pub fn open_with_notifier(
        pool: SqlitePool,
        config_dir: &Path,
        downloads: Arc<dyn DownloadTaskCreationPort>,
        reconciles: Arc<dyn InboxReconcilePort>,
        clock: Arc<dyn TaskClock>,
        notifier: OutboxNotifier,
    ) -> Result<Self, AppError> {
        Ok(Self {
            store: AutomationEventStore::open_with_notifier(pool, config_dir, notifier)?,
            downloads,
            reconciles,
            clock,
            lease_duration_us: 30_000_000,
        })
    }

    #[must_use]
    /// Borrow the durable store for orchestration and tests.
    pub const fn store(&self) -> &AutomationEventStore {
        &self.store
    }

    /// Reclaim expired leases before starting the worker loop.
    ///
    /// # Errors
    ///
    /// Returns a persistence error.
    pub async fn prepare(&self) -> Result<u64, AppError> {
        self.store.reclaim_expired(self.clock.now_us()).await
    }

    /// Claim and execute at most one due event.
    ///
    /// # Errors
    ///
    /// Returns store, lease, or checkpoint persistence errors.
    pub async fn run_once(&self) -> Result<Option<Uuid>, AppError> {
        let now = self.clock.now_us();
        let Some(lease) = self.store.claim_one(now, self.lease_duration_us).await? else {
            return Ok(None);
        };
        let key = format!("automation:{}", lease.id);
        let result = match &lease.payload {
            AutomationEventPayload::CreateDownload {
                connection_id,
                source,
                display_name,
            } => self
                .downloads
                .create_download(*connection_id, source.expose(), display_name, &key)
                .await
                .map(|id| ("download-task", id, 0_i64)),
            AutomationEventPayload::ReconcileInbox { inbox_id } => self
                .reconciles
                .reconcile_inbox(*inbox_id, &key)
                .await
                .map(|result| ("reconcile-request", result.request_id, result.result_count)),
        };
        match result {
            Ok((kind, downstream_id, result_count)) => {
                self.store
                    .complete(
                        &lease,
                        kind,
                        downstream_id,
                        result_count,
                        self.clock.now_us(),
                    )
                    .await?;
            }
            Err(error) => {
                self.store
                    .fail(&lease, error.code, error.recoverable, self.clock.now_us())
                    .await?;
            }
        }
        Ok(Some(lease.id))
    }
}

fn map_app_error(error: &AppError) -> AutomationActionError {
    match error.code() {
        ErrorCode::RequestConflict | ErrorCode::ResourceConflict => {
            AutomationActionError::new(AutomationFailureCode::DownstreamConflict, false)
        }
        ErrorCode::ValidationFailed => {
            AutomationActionError::new(AutomationFailureCode::PayloadInvalid, false)
        }
        ErrorCode::IntegrationUnauthorized => {
            AutomationActionError::new(AutomationFailureCode::IntegrationUnauthorized, true)
        }
        ErrorCode::IntegrationRateLimited => {
            AutomationActionError::new(AutomationFailureCode::IntegrationRateLimited, true)
        }
        _ => AutomationActionError::unavailable(),
    }
}
