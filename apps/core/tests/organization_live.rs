#![cfg(unix)]

mod common;

use async_trait::async_trait;
use common::organization::organization_fixture_with_nfo_at;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::catalog::store::CatalogStore;
use mediaflow_core::discovery::capability::DeploymentRootSet;
use mediaflow_core::discovery::model::{RelativePath, RootId};
use mediaflow_core::identification::organization_port::{
    ConfirmedOrganizationIdentity, OrganizationIdentityPort,
};
use mediaflow_core::organization::coordinator::{
    OrganizationCompletionHandler, OrganizationFileOperationHandler, OrganizationNfoHandler,
};
use mediaflow_core::organization::executor::{OrganizationExecutor, RollbackCommand};
use mediaflow_core::organization::fs::{
    FileLocator, FileOperationError, FileOperationSpec, OrganizationFs,
    ProcessingStopToken as FileStopToken,
};
use mediaflow_core::organization::journal_store::{
    JournalStatus, JournalStore, LocalNfoStatus, LocalResultStatus,
};
use mediaflow_core::organization::model::{
    ConfirmedNfoMetadata, ConfirmedProviderId, NfoProvider, OrganizationNfoPolicy,
    OrganizationOperation,
};
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use mediaflow_core::shared::error::AppError;
use mediaflow_core::tasks::processing::model::{ProcessingStage, ProcessingStatus};
use mediaflow_core::tasks::processing::store::ProcessingStore;
use mediaflow_core::tasks::processing::worker::{
    ProcessingStageHandler, ProcessingStopToken as WorkerStopToken,
};
use serde_json::json;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

const SENTINEL: &[u8] = b"mediaflow-organization-live-owned\n";

#[derive(Clone)]
struct LiveIdentity {
    candidate_id: Uuid,
}

#[async_trait]
impl OrganizationIdentityPort for LiveIdentity {
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

fn live_root(name: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| {
        panic!("{name} must be set by apps/core/scripts/test-m3-organization-live.sh")
    }));
    std::fs::canonicalize(path).expect("live test root must exist and be canonicalizable")
}

fn assert_sentinel(root: &Path) {
    assert_eq!(
        std::fs::read(root.join(".mediaflow-live-owned")).unwrap(),
        SENTINEL,
        "script ownership marker changed"
    );
    assert_eq!(
        std::fs::read(root.join(".outside-operation-sentinel")).unwrap(),
        SENTINEL,
        "an operation changed the root-level canary"
    );
}

fn locator(root: &str, path: &str) -> FileLocator {
    FileLocator::new(
        RootId::parse(root).unwrap(),
        RelativePath::parse(path).unwrap(),
    )
}

fn assert_no_operation_temps(root: &Path) {
    fn visit(path: &Path) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                visit(&entry.path());
            } else {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                assert!(
                    !(name.starts_with(".mediaflow-") && name.ends_with(".tmp")),
                    "temporary operation file was not cleaned: {}",
                    entry.path().display()
                );
            }
        }
    }
    visit(root);
}

#[tokio::test]
#[ignore = "requires explicit isolated live roots and acknowledgement sentinel"]
#[allow(clippy::too_many_lines)]
async fn isolated_roots_cover_recovery_catalog_operations_and_safe_rollback() {
    let source_root = live_root("MEDIAFLOW_ORGANIZATION_LIVE_SOURCE_TEST_ROOT");
    let target_root = live_root("MEDIAFLOW_ORGANIZATION_LIVE_TARGET_TEST_ROOT");
    assert_sentinel(&source_root);
    assert_sentinel(&target_root);
    assert_eq!(
        std::fs::metadata(&source_root).unwrap().dev(),
        std::fs::metadata(&target_root).unwrap().dev(),
        "base roots must share a device for the hardlink acceptance case"
    );

    let fixture = organization_fixture_with_nfo_at(
        OrganizationOperation::Copy,
        OrganizationNfoPolicy::GenerateMissing,
        &source_root,
        &target_root,
        true,
    )
    .await;
    std::fs::create_dir_all(fixture.nfo_path().parent().unwrap()).unwrap();
    let original_nfo = b"<movie><title>operator-owned</title></movie>\n";
    std::fs::write(fixture.nfo_path(), original_nfo).unwrap();

    let tasks = ProcessingStore::new(fixture.db.pool().clone());
    tasks
        .finish_organization_plan(&fixture.lease, fixture.plan.id, true, 10_000)
        .await
        .unwrap();
    let file_lease = tasks
        .claim_next(
            "organization-live-file",
            &[ProcessingStage::FileOperation],
            11_000,
        )
        .await
        .unwrap()
        .unwrap();
    let journals = JournalStore::new(fixture.db.pool().clone());
    let expected = fixture
        .fs
        .observe(&fixture.source_locator())
        .unwrap()
        .unwrap();
    let prepared = journals
        .prepare_file(fixture.account_id, &fixture.plan, expected.clone(), 12_000)
        .await
        .unwrap();
    let executing = journals
        .mark_executing(prepared.id, prepared.projection_version, 13_000)
        .await
        .unwrap();
    fixture
        .fs
        .copy_no_clobber(
            &FileOperationSpec::new(
                executing.operation_id,
                expected,
                fixture.destination_locator(),
            ),
            &FileStopToken::default(),
        )
        .unwrap();
    assert_eq!(
        journals
            .get(fixture.account_id, executing.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        JournalStatus::Executing,
        "the simulated process must stop after filesystem apply and before journal apply"
    );

    let declarations = DeploymentRootSet::load(
        &fixture.config.config().deployment_roots_file,
        RunMode::Development,
    )
    .unwrap();
    let restarted_fs =
        Arc::new(OsCapabilityFs::open(declarations.declarations(), RunMode::Development).unwrap());
    let clock = Arc::new(ManualTaskClock::new(20_000));
    let restarted_executor = OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        journals.clone(),
        restarted_fs.clone(),
        clock.clone(),
    );
    OrganizationFileOperationHandler::new(restarted_executor.clone(), tasks.clone(), clock.clone())
        .run(&file_lease, WorkerStopToken::default())
        .await
        .unwrap();
    let after_file = tasks
        .get(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(after_file.stage, ProcessingStage::Nfo);
    let nfo_lease = tasks
        .claim_next("organization-live-nfo", &[ProcessingStage::Nfo], 21_000)
        .await
        .unwrap()
        .unwrap();
    OrganizationNfoHandler::new(restarted_executor.clone(), tasks.clone(), clock.clone())
        .run(&nfo_lease, WorkerStopToken::default())
        .await
        .unwrap();
    assert_eq!(std::fs::read(fixture.nfo_path()).unwrap(), original_nfo);
    let local_result = journals
        .result_for_task(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(local_result.nfo_status, LocalNfoStatus::Preserved);

    let completion_lease = tasks
        .claim_next(
            "organization-live-completion",
            &[ProcessingStage::Completion],
            22_000,
        )
        .await
        .unwrap()
        .unwrap();
    let catalog = Arc::new(CatalogStore::new(fixture.db.pool().clone()));
    OrganizationCompletionHandler::new(
        fixture.account_id,
        fixture.db.pool().clone(),
        Arc::new(LiveIdentity {
            candidate_id: fixture.plan.draft.selected_identity_id.unwrap(),
        }),
        catalog.clone(),
        tasks.clone(),
        clock.clone(),
    )
    .run(&completion_lease, WorkerStopToken::default())
    .await
    .unwrap();
    let completed = tasks
        .get(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(completed.status, ProcessingStatus::Completed);
    let media = catalog
        .detail(fixture.account_id, completed.catalog_media_item_id.unwrap())
        .await
        .unwrap();
    assert_eq!(media.item.title, "Arrival");
    assert_eq!(media.versions.len(), 1);
    let rollback_result = journals
        .result_for_task(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap()
        .unwrap();

    let compensated = restarted_executor
        .rollback(RollbackCommand {
            task_id: fixture.lease.task.id,
            result_version: rollback_result.version,
            idempotency_key: "organization-live-safe-rollback".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(compensated.status, LocalResultStatus::Compensated);
    assert!(!fixture.destination_path().exists());
    assert_eq!(std::fs::read(fixture.nfo_path()).unwrap(), original_nfo);
    assert!(fixture.source_path().exists());

    std::fs::write(
        source_root.join("ready/Example.Show.S01E02.mkv"),
        b"episode-media",
    )
    .unwrap();
    std::fs::create_dir_all(target_root.join("Series/Example Show/Season 01")).unwrap();
    let episode_source = locator("incoming", "ready/Example.Show.S01E02.mkv");
    let episode_target = locator(
        "library",
        "Series/Example Show/Season 01/Example Show - S01E02.mkv",
    );
    let episode = FileOperationSpec::new(
        Uuid::now_v7(),
        restarted_fs.observe(&episode_source).unwrap().unwrap(),
        episode_target.clone(),
    );
    restarted_fs.move_no_clobber(&episode).unwrap();
    assert!(!source_root.join("ready/Example.Show.S01E02.mkv").exists());
    assert_eq!(
        std::fs::read(target_root.join(episode_target.relative_path().as_str())).unwrap(),
        b"episode-media"
    );

    std::fs::write(source_root.join("ready/Clips.001.mkv"), b"generic-media").unwrap();
    std::fs::create_dir_all(target_root.join("Generic/Clips")).unwrap();
    let generic_source = locator("incoming", "ready/Clips.001.mkv");
    let generic_target = locator("library", "Generic/Clips/001 - Clips.mkv");
    let generic = FileOperationSpec::new(
        Uuid::now_v7(),
        restarted_fs.observe(&generic_source).unwrap().unwrap(),
        generic_target.clone(),
    );
    restarted_fs.hardlink_no_clobber(&generic).unwrap();
    assert_eq!(
        std::fs::metadata(source_root.join(generic_source.relative_path().as_str()))
            .unwrap()
            .ino(),
        std::fs::metadata(target_root.join(generic_target.relative_path().as_str()))
            .unwrap()
            .ino()
    );

    if let Some(cross_root) = std::env::var_os("MEDIAFLOW_ORGANIZATION_LIVE_CROSS_DEVICE_TEST_ROOT")
    {
        let cross_root = std::fs::canonicalize(cross_root).unwrap();
        assert_sentinel(&cross_root);
        assert_ne!(
            std::fs::metadata(&source_root).unwrap().dev(),
            std::fs::metadata(&cross_root).unwrap().dev()
        );
        let cross_config = fixture
            .config
            .config()
            .config_dir
            .join("cross-device-roots.json");
        std::fs::write(
            &cross_config,
            serde_json::to_vec(&json!({"roots":[
                {"id":"incoming","label":"Incoming","container_path":source_root,"access":"read-write"},
                {"id":"cross","label":"Cross device","container_path":cross_root,"access":"read-write"}
            ]}))
            .unwrap(),
        )
        .unwrap();
        let roots = DeploymentRootSet::load(&cross_config, RunMode::Development).unwrap();
        let cross_fs = OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap();
        std::fs::write(source_root.join("ready/cross-device.mkv"), b"cross-device").unwrap();
        let cross_source = locator("incoming", "ready/cross-device.mkv");
        let cross_target = locator("cross", "cross-device.mkv");
        let operation = FileOperationSpec::new(
            Uuid::now_v7(),
            cross_fs.observe(&cross_source).unwrap().unwrap(),
            cross_target,
        );
        assert_eq!(
            cross_fs.move_no_clobber(&operation).unwrap_err(),
            FileOperationError::CompositeMoveRequired
        );
        assert!(source_root.join("ready/cross-device.mkv").exists());
        assert!(!cross_root.join("cross-device.mkv").exists());
        assert_sentinel(&cross_root);
    }

    assert_sentinel(&source_root);
    assert_sentinel(&target_root);
    assert_no_operation_temps(&source_root);
    assert_no_operation_temps(&target_root);
    eprintln!(
        "organization live facts: movie copy/restart/catalog/rollback, episode move, generic hardlink, existing NFO preserved"
    );
}
