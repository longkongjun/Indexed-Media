use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::capability::DeploymentRootSet;
use mediaflow_core::discovery::model::{DeploymentRootView, RelativePath, RootAccess, RootId};
use mediaflow_core::organization::fs::{FileLocator, OrganizationFs};
use mediaflow_core::organization::model::{
    ConfirmedNfoMetadata, ConfirmedProviderId, NfoProvider, OrganizationNamingPattern,
    OrganizationNfoPolicy, OrganizationOperation, OrganizationRuleInput, OrganizationTargetInput,
    OrganizationTargetKind,
};
use mediaflow_core::organization::plan_store::{OrganizationPlanRecord, OrganizationPlanStore};
use mediaflow_core::organization::planner::{
    OrganizationLocation, OrganizationPlanner, PlanningIdentity, PlanningInput,
};
use mediaflow_core::organization::target_store::{
    OrganizationTargetBoundary, OrganizationTargetStore,
};
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::db::Db;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::tasks::processing::model::ProcessingLease;
use serde_json::json;
use uuid::Uuid;

use super::{TestConfigDir, seed_account, seed_inbox, seed_processing_lease};

pub struct OrganizationFixture {
    pub config: TestConfigDir,
    pub db: Db,
    pub account_id: Uuid,
    pub lease: ProcessingLease,
    pub plan: OrganizationPlanRecord,
    pub fs: Arc<OsCapabilityFs>,
    pub source_root: PathBuf,
    pub target_root: PathBuf,
}

impl OrganizationFixture {
    pub fn source_locator(&self) -> FileLocator {
        FileLocator::new(
            self.plan.draft.source.root_id.clone(),
            self.plan.draft.source.relative_path.clone(),
        )
    }

    pub fn destination_locator(&self) -> FileLocator {
        let destination = &self.plan.draft.destination;
        FileLocator::new(
            destination.root_id.clone(),
            destination.relative_path.clone(),
        )
    }

    pub fn source_path(&self) -> PathBuf {
        self.source_root.join("ready/Arrival.2016.mkv")
    }

    pub fn destination_path(&self) -> PathBuf {
        self.target_root
            .join(self.plan.draft.destination.relative_path.as_str())
    }

    pub fn nfo_locator(&self) -> FileLocator {
        let operation = self
            .plan
            .operations
            .iter()
            .find(|operation| {
                operation.kind
                    == mediaflow_core::organization::planner::PlannedOperationKind::EnsureMissingNfo
            })
            .expect("fixture plan must include NFO operation");
        FileLocator::new(
            operation.destination.root_id.clone(),
            operation.destination.relative_path.clone(),
        )
    }

    pub fn nfo_path(&self) -> PathBuf {
        let operation = self
            .plan
            .operations
            .iter()
            .find(|operation| {
                operation.kind
                    == mediaflow_core::organization::planner::PlannedOperationKind::EnsureMissingNfo
            })
            .expect("fixture plan must include NFO operation");
        self.target_root
            .join(operation.destination.relative_path.as_str())
    }
}

#[allow(clippy::too_many_lines)]
pub async fn organization_fixture(operation: OrganizationOperation) -> OrganizationFixture {
    organization_fixture_with_nfo(operation, OrganizationNfoPolicy::PreserveOnly).await
}

#[allow(clippy::too_many_lines)]
pub async fn organization_fixture_with_nfo(
    operation: OrganizationOperation,
    nfo_policy: OrganizationNfoPolicy,
) -> OrganizationFixture {
    let config = TestConfigDir::new(RunMode::Development);
    let roots_parent = config
        .config()
        .deployment_roots_file
        .parent()
        .unwrap()
        .to_path_buf();
    let source_root = roots_parent.join("organization-source");
    let target_root = roots_parent.join("organization-target");
    build_organization_fixture(
        config,
        operation,
        nfo_policy,
        &source_root,
        &target_root,
        operation == OrganizationOperation::Move,
    )
    .await
}

/// 在调用方提供的隔离根上构造与普通测试相同的 production FS/store fixture。
pub async fn organization_fixture_with_nfo_at(
    operation: OrganizationOperation,
    nfo_policy: OrganizationNfoPolicy,
    source_root: &Path,
    target_root: &Path,
    source_writable: bool,
) -> OrganizationFixture {
    build_organization_fixture(
        TestConfigDir::new(RunMode::Development),
        operation,
        nfo_policy,
        source_root,
        target_root,
        source_writable,
    )
    .await
}

#[allow(clippy::too_many_lines)]
async fn build_organization_fixture(
    config: TestConfigDir,
    operation: OrganizationOperation,
    nfo_policy: OrganizationNfoPolicy,
    source_root: &Path,
    target_root: &Path,
    source_writable: bool,
) -> OrganizationFixture {
    std::fs::create_dir_all(source_root.join("ready")).unwrap();
    std::fs::create_dir_all(target_root).unwrap();
    std::fs::write(source_root.join("ready/Arrival.2016.mkv"), b"arrival-media").unwrap();
    let source_root = std::fs::canonicalize(source_root).unwrap();
    let target_root = std::fs::canonicalize(target_root).unwrap();
    let source_access = if source_writable {
        "read-write"
    } else {
        "read-only"
    };
    std::fs::write(
        &config.config().deployment_roots_file,
        serde_json::to_vec(&json!({"roots":[
            {"id":"incoming","label":"Incoming","container_path":source_root,"access":source_access},
            {"id":"library","label":"Library","container_path":target_root,"access":"read-write"}
        ]}))
        .unwrap(),
    )
    .unwrap();
    let roots =
        DeploymentRootSet::load(&config.config().deployment_roots_file, RunMode::Development)
            .unwrap();
    let fs = Arc::new(OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap());
    let db = migrate_with_backup(config.config()).await.unwrap();
    let account_id = seed_account(db.pool()).await;
    let inbox_id = seed_inbox(db.pool()).await;
    let lease = seed_processing_lease(
        db.pool(),
        inbox_id,
        b"ready/Arrival.2016.mkv",
        vec![0; 16],
        "organization-executor",
    )
    .await;
    let source_locator = FileLocator::new(
        RootId::parse("incoming").unwrap(),
        RelativePath::parse("ready/Arrival.2016.mkv").unwrap(),
    );
    let observed = fs.observe(&source_locator).unwrap().unwrap();
    sqlx::query(
        "UPDATE discovery_file_revisions
         SET identity_snapshot=?,size_bytes=?,modified_at_ns=? WHERE id=?",
    )
    .bind(observed.identity().snapshot_bytes())
    .bind(i64::try_from(observed.size_bytes()).unwrap())
    .bind(i64::try_from(observed.modified_at_ns()).unwrap())
    .bind(lease.task.file_revision_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let target = OrganizationTargetStore::new(db.pool().clone())
        .insert(
            account_id,
            Uuid::now_v7(),
            OrganizationTargetInput {
                kind: OrganizationTargetKind::Movie,
                display_name: "Movies".to_owned(),
                root_id: RootId::parse("library").unwrap(),
                relative_path: RelativePath::parse("Movies").unwrap(),
                operation,
                naming_pattern: OrganizationNamingPattern::Movie,
                nfo_policy,
                automatic: true,
                enabled: true,
                rules: vec![OrganizationRuleInput {
                    media_kind: OrganizationTargetKind::Movie,
                    inbox_directory_id: Some(inbox_id),
                    explicit_tag: None,
                    enabled: true,
                }],
            },
            &OrganizationTargetBoundary {
                root: DeploymentRootView {
                    id: RootId::parse("library").unwrap(),
                    label: "Library".to_owned(),
                    access: RootAccess::ReadWrite,
                },
                overlaps_inbox: false,
            },
            100,
        )
        .await
        .unwrap();
    let draft = OrganizationPlanner::plan(&PlanningInput {
        task_id: lease.task.id,
        file_revision_id: lease.task.file_revision_id,
        selected_identity_id: Some(Uuid::now_v7()),
        source: OrganizationLocation {
            root_id: RootId::parse("incoming").unwrap(),
            relative_path: RelativePath::parse("ready/Arrival.2016.mkv").unwrap(),
        },
        source_inbox_id: inbox_id,
        source_writable,
        source_unchanged: true,
        destination_exists: false,
        same_filesystem: true,
        explicit_tags: BTreeSet::new(),
        one_time_authorized: false,
        identity: PlanningIdentity::Movie {
            title: "Arrival".to_owned(),
            year: Some(2016),
            version_label: None,
        },
        nfo_metadata: ConfirmedNfoMetadata {
            original_title: None,
            year: Some(2016),
            plot: None,
            provider_id: Some(ConfirmedProviderId {
                provider: NfoProvider::Tmdb,
                value: "329865".to_owned(),
            }),
        },
        target,
    })
    .unwrap();
    let plan = OrganizationPlanStore::new(db.pool().clone())
        .persist_next(account_id, draft, 200)
        .await
        .unwrap();
    OrganizationFixture {
        config,
        db,
        account_id,
        lease,
        plan,
        fs,
        source_root,
        target_root,
    }
}
