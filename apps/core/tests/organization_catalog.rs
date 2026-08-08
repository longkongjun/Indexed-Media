mod common;

use async_trait::async_trait;
use common::organization::organization_fixture;
use mediaflow_core::catalog::CatalogLocalResultPort;
use mediaflow_core::catalog::model::VerifiedLocalResult;
use mediaflow_core::catalog::store::CatalogStore;
use mediaflow_core::discovery::model::{RootAccess, RootId};
use mediaflow_core::identification::organization_port::{
    ConfirmedOrganizationIdentity, OrganizationIdentityPort,
};
use mediaflow_core::organization::coordinator::{
    OrganizationCompletionHandler, OrganizationFileOperationHandler, OrganizationPlanningHandler,
    SqliteOrganizationPlanningPort,
};
use mediaflow_core::organization::executor::OrganizationExecutor;
use mediaflow_core::organization::fs::OrganizationFs;
use mediaflow_core::organization::journal_store::JournalStore;
use mediaflow_core::organization::model::{
    ConfirmedNfoMetadata, ConfirmedProviderId, NfoProvider, OrganizationOperation,
};
use mediaflow_core::organization::plan_service::{
    OrganizationPlanService, OrganizationPlanningPort,
};
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use mediaflow_core::shared::error::{AppError, ErrorCode};
use mediaflow_core::tasks::processing::model::{ProcessingStage, ProcessingStatus};
use mediaflow_core::tasks::processing::store::ProcessingStore;
use mediaflow_core::tasks::processing::worker::{ProcessingStageHandler, ProcessingWorker};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

#[derive(Clone)]
struct FixedIdentity {
    candidate_id: Uuid,
}

struct CatalogResponseLoss {
    inner: Arc<CatalogStore>,
    lose_first_response: AtomicBool,
}

#[async_trait]
impl CatalogLocalResultPort for CatalogResponseLoss {
    async fn apply_local_result(
        &self,
        account_id: Uuid,
        result: &VerifiedLocalResult,
        now_us: i64,
    ) -> Result<Uuid, AppError> {
        let media_id = self
            .inner
            .apply_verified_local_result(account_id, result, now_us)
            .await?;
        if self.lose_first_response.swap(false, Ordering::SeqCst) {
            Err(AppError::new(
                ErrorCode::Internal,
                "simulated catalog response loss",
            ))
        } else {
            Ok(media_id)
        }
    }
}

#[async_trait]
impl OrganizationIdentityPort for FixedIdentity {
    async fn confirmed_identity(
        &self,
        _account_id: Uuid,
        _task_id: Uuid,
    ) -> Result<ConfirmedOrganizationIdentity, AppError> {
        Ok(ConfirmedOrganizationIdentity::Movie {
            title: "Arrival".to_owned(),
            year: Some(2016),
            version_label: None,
            candidate_id: self.candidate_id,
            nfo_metadata: ConfirmedNfoMetadata {
                original_title: None,
                year: Some(2016),
                plot: None,
                provider_id: Some(ConfirmedProviderId {
                    provider: NfoProvider::Tmdb,
                    value: "329865".to_owned(),
                }),
            },
        })
    }
}

#[tokio::test]
async fn verified_movie_result_is_committed_once_to_catalog_and_completes_the_task() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let tasks = ProcessingStore::new(fixture.db.pool().clone());
    std::fs::create_dir_all(fixture.target_root.join("Movies")).unwrap();
    let identity: Arc<dyn OrganizationIdentityPort> = Arc::new(FixedIdentity {
        candidate_id: fixture.plan.draft.selected_identity_id.unwrap(),
    });
    let organization_fs: Arc<dyn OrganizationFs> = fixture.fs.clone();
    let planning_port: Arc<dyn OrganizationPlanningPort> =
        Arc::new(SqliteOrganizationPlanningPort::new(
            fixture.db.pool().clone(),
            identity.clone(),
            organization_fs,
            BTreeMap::from([
                (RootId::parse("incoming").unwrap(), RootAccess::ReadOnly),
                (RootId::parse("library").unwrap(), RootAccess::ReadWrite),
            ]),
        ));
    OrganizationPlanningHandler::new_dynamic(
        OrganizationPlanService::new(planning_port, fixture.db.pool().clone()),
        tasks.clone(),
        Arc::new(ManualTaskClock::new(93_000_000)),
    )
    .run(
        &fixture.lease,
        mediaflow_core::tasks::processing::worker::ProcessingStopToken::default(),
    )
    .await
    .unwrap();
    let queued = tasks
        .get(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(queued.stage, ProcessingStage::FileOperation);

    let clock = Arc::new(ManualTaskClock::new(100_000_000));
    let executor = OrganizationExecutor::unscoped(
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fixture.fs.clone(),
        clock.clone(),
    );
    let catalog = Arc::new(CatalogStore::new(fixture.db.pool().clone()));
    let handlers: Vec<Arc<dyn ProcessingStageHandler>> = vec![
        Arc::new(OrganizationFileOperationHandler::new(
            executor,
            tasks.clone(),
            clock.clone(),
        )),
        Arc::new(OrganizationCompletionHandler::new_dynamic(
            fixture.db.pool().clone(),
            JournalStore::new(fixture.db.pool().clone()),
            identity,
            catalog.clone(),
            tasks.clone(),
            clock.clone(),
        )),
    ];
    let worker = ProcessingWorker::new(
        "organization-catalog".to_owned(),
        tasks.clone(),
        handlers,
        clock,
    )
    .unwrap();

    assert_eq!(
        worker.run_once().await.unwrap(),
        Some(fixture.lease.task.id)
    );
    assert_eq!(
        worker.run_once().await.unwrap(),
        Some(fixture.lease.task.id)
    );
    assert_eq!(worker.run_once().await.unwrap(), None);

    let completed = tasks
        .get(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(completed.status, ProcessingStatus::Completed);
    assert_eq!(completed.stage, ProcessingStage::Completion);
    assert_eq!(completed.organization_plan_id, Some(fixture.plan.id));
    assert!(completed.organization_result_id.is_some());
    let media_id = completed.catalog_media_item_id.unwrap();
    let detail = catalog.detail(fixture.account_id, media_id).await.unwrap();
    assert_eq!(detail.item.title, "Arrival");
    assert_eq!(detail.item.year, Some(2016));
    assert_eq!(detail.versions.len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM catalog_applied_local_results WHERE task_id=?",
        )
        .bind(fixture.lease.task.id.as_bytes().as_slice())
        .fetch_one(fixture.db.pool())
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn catalog_response_loss_recovers_idempotently_without_a_second_media_tree() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let tasks = ProcessingStore::new(fixture.db.pool().clone());
    tasks
        .finish_organization_plan(&fixture.lease, fixture.plan.id, true, 93_000_000)
        .await
        .unwrap();
    let clock = Arc::new(ManualTaskClock::new(100_000_000));
    let executor = OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fixture.fs.clone(),
        clock.clone(),
    );
    let identity: Arc<dyn OrganizationIdentityPort> = Arc::new(FixedIdentity {
        candidate_id: fixture.plan.draft.selected_identity_id.unwrap(),
    });
    let catalog = Arc::new(CatalogStore::new(fixture.db.pool().clone()));
    let lossy_catalog: Arc<dyn CatalogLocalResultPort> = Arc::new(CatalogResponseLoss {
        inner: catalog.clone(),
        lose_first_response: AtomicBool::new(true),
    });
    let handlers: Vec<Arc<dyn ProcessingStageHandler>> = vec![
        Arc::new(OrganizationFileOperationHandler::new(
            executor,
            tasks.clone(),
            clock.clone(),
        )),
        Arc::new(OrganizationCompletionHandler::new(
            fixture.account_id,
            fixture.db.pool().clone(),
            identity,
            lossy_catalog,
            tasks.clone(),
            clock.clone(),
        )),
    ];
    let worker = ProcessingWorker::new(
        "organization-catalog-loss".to_owned(),
        tasks.clone(),
        handlers,
        clock.clone(),
    )
    .unwrap();

    worker.run_once().await.unwrap();
    worker.run_once().await.unwrap();
    let paused = tasks
        .get(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(paused.status, ProcessingStatus::Paused);
    assert_eq!(paused.stage, ProcessingStage::Completion);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM catalog_media_items")
            .fetch_one(fixture.db.pool())
            .await
            .unwrap(),
        1
    );

    drop(worker);
    clock.set(131_000_000);
    let recovered_executor = OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fixture.fs.clone(),
        clock.clone(),
    );
    let recovered_handlers: Vec<Arc<dyn ProcessingStageHandler>> = vec![
        Arc::new(OrganizationFileOperationHandler::new(
            recovered_executor,
            tasks.clone(),
            clock.clone(),
        )),
        Arc::new(OrganizationCompletionHandler::new(
            fixture.account_id,
            fixture.db.pool().clone(),
            Arc::new(FixedIdentity {
                candidate_id: fixture.plan.draft.selected_identity_id.unwrap(),
            }),
            Arc::new(CatalogStore::new(fixture.db.pool().clone())),
            tasks.clone(),
            clock.clone(),
        )),
    ];
    let recovered_worker = ProcessingWorker::new(
        "organization-catalog-recovered".to_owned(),
        ProcessingStore::new(fixture.db.pool().clone()),
        recovered_handlers,
        clock,
    )
    .unwrap();
    recovered_worker.run_once().await.unwrap();
    let completed = tasks
        .get(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(completed.status, ProcessingStatus::Completed);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM catalog_media_items")
            .fetch_one(fixture.db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM catalog_applied_local_results")
            .fetch_one(fixture.db.pool())
            .await
            .unwrap(),
        1
    );
    for table in [
        "tasks_processing_tasks",
        "organization_plans",
        "organization_file_operation_journals",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(fixture.db.pool())
                .await
                .unwrap(),
            1,
            "unexpected duplicate row in {table} after runtime reconstruction"
        );
    }
}
