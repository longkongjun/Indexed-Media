use std::sync::Arc;

use crate::automation::runtime::AutomationRuntime;
use crate::bootstrap::config::AppConfig;
use crate::catalog::store::CatalogStore;
use crate::connectors::downloader::runtime::DownloadTaskRuntime;
use crate::connectors::enhancer::ollama::OllamaClient;
use crate::connectors::enhancer::service::EnhancerService;
use crate::connectors::service::ConnectorService;
use crate::connectors::tmdb::{TmdbCachePolicy, TmdbClient, TmdbProvider};
use crate::discovery::capability::{CapabilityFs, DeploymentRootSet};
use crate::discovery::coordinator::DiscoveryRuntime;
use crate::identification::organization_port::{
    OrganizationIdentityPort, SqliteOrganizationIdentityPort,
};
use crate::identification::service::IdentificationStageHandler;
use crate::organization::coordinator::{
    OrganizationCompletionHandler, OrganizationFileOperationHandler, OrganizationNfoHandler,
    OrganizationPlanningHandler, SqliteOrganizationPlanningPort,
};
use crate::organization::executor::OrganizationExecutor;
use crate::organization::fs::OrganizationFs;
use crate::organization::journal_store::JournalStore;
use crate::organization::plan_service::{OrganizationPlanService, OrganizationPlanningPort};
use crate::organization::plan_store::OrganizationPlanStore;
use crate::platform::capability_fs::OsCapabilityFs;
use crate::platform::db::Db;
use crate::platform::http::build_router;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::task_runtime::{ProcessingTaskRuntime, SystemTaskClock};
use crate::shared::error::AppError;
use crate::shared::error::ErrorCode;
use crate::tasks::processing::store::ProcessingStore;
use crate::tasks::processing::worker::ProcessingStageHandler;

/// 根据已校验的配置与可选数据库组装完整的 Core HTTP 路由。
///
/// 当 `db` 为 `None` 时，仅暴露存活、就绪与受保护的静态资源兜底路由；
/// 依赖数据库的身份、发现、任务与事件路由会被省略。
pub fn assemble_router(config: AppConfig, db: Option<Db>) -> axum::Router {
    build_router(config, db)
}

/// 组装生产 watcher、启动/周期对账协调器与共享 outbox 通知器。
///
/// # Errors
///
/// 部署根目录配置无法重新验证时返回 [`AppError`]。底层 watcher 不可用会由运行时降级，
/// 不阻止周期对账。
pub fn assemble_discovery_runtime(
    config: &AppConfig,
    db: &Db,
    notifier: OutboxNotifier,
) -> Result<DiscoveryRuntime, AppError> {
    DiscoveryRuntime::from_app(config, db, notifier)
}

/// 在数据库、实例密钥与根能力校验后组装有界生产识别 worker。
///
/// # Errors
///
/// 根能力或固定来源 TLS 客户端无法打开，或 worker 注册失败时返回配置错误。
pub fn assemble_processing_runtime(
    config: &AppConfig,
    db: &Db,
    notifier: OutboxNotifier,
) -> Result<ProcessingTaskRuntime, AppError> {
    let roots = DeploymentRootSet::load(&config.deployment_roots_file, config.mode)?;
    let os_fs = Arc::new(OsCapabilityFs::open(roots.declarations(), config.mode)?);
    let identification_fs: Arc<dyn CapabilityFs> = os_fs.clone();
    let organization_fs: Arc<dyn OrganizationFs> = os_fs;
    let client = TmdbClient::production()
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
    let connectors = ConnectorService::new_with_notifier(
        db.pool().clone(),
        &config.config_dir,
        Arc::new(client.clone()),
        notifier.clone(),
    );
    connectors.ensure_key_ready()?;
    let provider = Arc::new(TmdbProvider::new(
        db.pool().clone(),
        client,
        TmdbCachePolicy::default(),
    ));
    let clock: Arc<dyn crate::platform::task_runtime::TaskClock> = Arc::new(SystemTaskClock);
    let enhancer = OllamaClient::production()
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
    let identification_handler: Arc<dyn ProcessingStageHandler> = Arc::new(
        IdentificationStageHandler::new(
            db.pool().clone(),
            notifier.clone(),
            connectors,
            provider,
            Some(identification_fs),
            clock.clone(),
        )
        .with_enhancer(EnhancerService::new(
            db.pool().clone(),
            Arc::new(enhancer),
            notifier.clone(),
        )),
    );
    let identity_port: Arc<dyn OrganizationIdentityPort> =
        Arc::new(SqliteOrganizationIdentityPort::new(db.pool().clone()));
    let root_access = roots
        .views()
        .iter()
        .map(|root| (root.id.clone(), root.access))
        .collect();
    let planning_port: Arc<dyn OrganizationPlanningPort> =
        Arc::new(SqliteOrganizationPlanningPort::new(
            db.pool().clone(),
            identity_port.clone(),
            organization_fs.clone(),
            root_access,
        ));
    let tasks = ProcessingStore::new_with_notifier(db.pool().clone(), notifier.clone());
    let executor = OrganizationExecutor::unscoped(
        OrganizationPlanStore::new(db.pool().clone()),
        JournalStore::new_with_notifier(db.pool().clone(), notifier.clone()),
        organization_fs,
        clock.clone(),
    );
    let planning_handler: Arc<dyn ProcessingStageHandler> =
        Arc::new(OrganizationPlanningHandler::new_dynamic(
            OrganizationPlanService::new(planning_port, db.pool().clone()),
            tasks.clone(),
            clock.clone(),
        ));
    let file_handler: Arc<dyn ProcessingStageHandler> = Arc::new(
        OrganizationFileOperationHandler::new(executor.clone(), tasks.clone(), clock.clone()),
    );
    let nfo_handler: Arc<dyn ProcessingStageHandler> = Arc::new(OrganizationNfoHandler::new(
        executor,
        tasks.clone(),
        clock.clone(),
    ));
    let catalog = Arc::new(CatalogStore::new_with_notifier(
        db.pool().clone(),
        notifier.clone(),
    ));
    let completion_handler: Arc<dyn ProcessingStageHandler> =
        Arc::new(OrganizationCompletionHandler::new_dynamic(
            db.pool().clone(),
            JournalStore::new_with_notifier(db.pool().clone(), notifier.clone()),
            identity_port,
            catalog,
            tasks,
            clock.clone(),
        ));
    ProcessingTaskRuntime::new(
        db.pool().clone(),
        notifier,
        &[
            identification_handler,
            planning_handler,
            file_handler,
            nfo_handler,
            completion_handler,
        ],
        clock,
    )
}

/// 组装两个内置下载器适配器、持久租约与共享 outbox 的生产下载任务运行时。
///
/// # Errors
///
/// 任一底层 HTTP 客户端无法安全构建时返回启动配置错误。
pub fn assemble_download_task_runtime(
    db: &Db,
    notifier: OutboxNotifier,
) -> Result<DownloadTaskRuntime, AppError> {
    DownloadTaskRuntime::production(db.pool().clone(), notifier)
}

/// Assemble durable automation lease recovery and the fixed-action event worker.
///
/// # Errors
///
/// Returns a configuration error when encrypted stores cannot open.
pub fn assemble_automation_runtime(
    config: &AppConfig,
    db: &Db,
    notifier: OutboxNotifier,
) -> Result<AutomationRuntime, AppError> {
    AutomationRuntime::production(db.pool().clone(), &config.config_dir, notifier)
}
