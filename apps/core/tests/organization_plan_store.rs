mod common;

use std::collections::BTreeSet;

use common::TestConfigDir;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::model::{DeploymentRootView, RelativePath, RootAccess, RootId};
use mediaflow_core::discovery::organization_projection::OrganizationInboxProjection;
use mediaflow_core::organization::model::{
    ConfirmedNfoMetadata, ConfirmedProviderId, NfoProvider, OrganizationNamingPattern,
    OrganizationNfoPolicy, OrganizationOperation, OrganizationRuleInput, OrganizationTargetInput,
    OrganizationTargetKind,
};
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::organization::planner::{
    OrganizationLocation, OrganizationPlanner, PlanAuthorization, PlanningIdentity, PlanningInput,
};
use mediaflow_core::organization::target_store::{
    OrganizationTargetBoundary, OrganizationTargetStore,
};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::error::ErrorCode;
use uuid::Uuid;

async fn setup() -> (
    TestConfigDir,
    mediaflow_core::platform::db::Db,
    Uuid,
    Uuid,
    Uuid,
    mediaflow_core::organization::planner::PlanDraft,
) {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = common::seed_account(db.pool()).await;
    let inbox_id = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox_id,
        b"ready/Arrival.2016.mkv",
        vec![7],
        "organization-plan",
    )
    .await;
    let target_id = Uuid::now_v7();
    let target_store = OrganizationTargetStore::new(db.pool().clone());
    let target = target_store
        .insert(
            account_id,
            target_id,
            OrganizationTargetInput {
                kind: OrganizationTargetKind::Movie,
                display_name: "Movies".to_owned(),
                root_id: RootId::parse("media").unwrap(),
                relative_path: RelativePath::parse("Movies").unwrap(),
                operation: OrganizationOperation::Copy,
                naming_pattern: OrganizationNamingPattern::Movie,
                nfo_policy: OrganizationNfoPolicy::GenerateMissing,
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
                    id: RootId::parse("media").unwrap(),
                    label: "Media".to_owned(),
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
        source_writable: true,
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
            original_title: Some("Arrival".to_owned()),
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
    (fixture, db, account_id, lease.task.id, target_id, draft)
}

#[tokio::test]
async fn snapshot_provenance_plan_and_operations_commit_before_becoming_visible() {
    let (_fixture, db, account_id, task_id, _target_id, draft) = setup().await;
    let source = OrganizationInboxProjection::new(db.pool().clone())
        .source_for_task(account_id, task_id)
        .await
        .unwrap();
    assert_eq!(source.root_id.as_str(), "incoming");
    assert_eq!(source.relative_path.as_str(), "ready/Arrival.2016.mkv");
    let store = OrganizationPlanStore::new(db.pool().clone());
    let expected_nfo_input = draft.nfo_input.clone();
    let saved = store.persist_next(account_id, draft, 200).await.unwrap();

    assert_eq!(saved.task_id, task_id);
    assert_eq!(saved.version, 1);
    assert_eq!(saved.authorization, PlanAuthorization::Automatic);
    assert_eq!(saved.operations.len(), 2);
    assert_eq!(saved.draft.nfo_input, expected_nfo_input);
    assert!(
        saved
            .operations
            .iter()
            .all(|operation| !operation.id.is_nil())
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM organization_plan_nfo_inputs WHERE plan_id=?",
        )
        .bind(saved.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
    assert!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM organization_config_provenance WHERE snapshot_id=?",
        )
        .bind(saved.snapshot_id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap()
            >= 6
    );

    let mut invalid = saved.draft.clone();
    invalid.task_id = Uuid::now_v7();
    let error = store
        .persist_next(account_id, invalid, 300)
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM organization_plans")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn plan_versions_are_immutable_and_recalculation_receipts_are_idempotent() {
    let (_fixture, db, account_id, task_id, _target_id, draft) = setup().await;
    let store = OrganizationPlanStore::new(db.pool().clone());
    let first = store
        .recalculate(account_id, "recalculate-once", draft.clone(), 200)
        .await
        .unwrap();
    let replay = store
        .recalculate(account_id, "recalculate-once", draft.clone(), 300)
        .await
        .unwrap();
    assert_eq!(replay.id, first.id);
    assert_eq!(replay.version, 1);

    let mut changed = draft;
    changed.target.config_version = 2;
    let conflict = store
        .recalculate(account_id, "recalculate-once", changed.clone(), 400)
        .await
        .unwrap_err();
    assert_eq!(conflict.code(), ErrorCode::RequestConflict);
    let second = store
        .recalculate(account_id, "recalculate-twice", changed, 500)
        .await
        .unwrap();
    assert_eq!(second.version, 2);

    let stale = store
        .assert_current(account_id, task_id, 1)
        .await
        .unwrap_err();
    assert_eq!(stale.code(), ErrorCode::ConfigVersionConflict);
    store.assert_current(account_id, task_id, 2).await.unwrap();

    let immutable = sqlx::query("UPDATE organization_plans SET naming='mutated' WHERE id=?")
        .bind(first.id.as_bytes().as_slice())
        .execute(db.pool())
        .await;
    assert!(immutable.is_err());
}

#[tokio::test]
async fn one_time_authorization_is_plan_scoped_idempotent_and_cannot_authorize_stale_versions() {
    let (_fixture, db, account_id, task_id, _target_id, mut draft) = setup().await;
    draft.authorization = PlanAuthorization::Paused;
    let store = OrganizationPlanStore::new(db.pool().clone());
    let plan = store.persist_next(account_id, draft, 200).await.unwrap();
    let authorized = store
        .authorize_once(account_id, task_id, 1, "authorize-once", 300)
        .await
        .unwrap();
    assert_eq!(authorized.authorization, PlanAuthorization::OneTime);
    let replay = store
        .authorize_once(account_id, task_id, 1, "authorize-once", 400)
        .await
        .unwrap();
    assert_eq!(replay.id, plan.id);

    let mut changed = plan.draft.clone();
    changed.target.config_version += 1;
    let current = store.persist_next(account_id, changed, 500).await.unwrap();
    assert_eq!(current.version, 2);
    let stale = store
        .authorize_once(account_id, task_id, 1, "stale-authorization", 600)
        .await
        .unwrap_err();
    assert_eq!(stale.code(), ErrorCode::ConfigVersionConflict);
}

#[tokio::test]
async fn persisted_plans_protect_their_target_from_deletion() {
    let (_fixture, db, account_id, _task_id, target_id, draft) = setup().await;
    OrganizationPlanStore::new(db.pool().clone())
        .persist_next(account_id, draft, 200)
        .await
        .unwrap();
    let error = OrganizationTargetStore::new(db.pool().clone())
        .delete(account_id, target_id, 1)
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ResourceConflict);
}
