mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::model::{DeploymentRootView, RelativePath, RootAccess, RootId};
use mediaflow_core::organization::model::{
    OrganizationNamingPattern, OrganizationNfoPolicy, OrganizationOperation, OrganizationRuleInput,
    OrganizationTargetInput, OrganizationTargetKind,
};
use mediaflow_core::organization::target_store::{
    OrganizationTargetBoundary, OrganizationTargetStore,
};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::error::ErrorCode;
use uuid::Uuid;

use common::TestConfigDir;

fn root(id: &str, access: RootAccess, label: &str) -> DeploymentRootView {
    DeploymentRootView {
        id: RootId::parse(id).unwrap(),
        label: label.to_owned(),
        access,
    }
}

fn target_input(path: &str) -> OrganizationTargetInput {
    OrganizationTargetInput {
        kind: OrganizationTargetKind::Movie,
        display_name: "Movies".to_owned(),
        root_id: RootId::parse("media").unwrap(),
        relative_path: RelativePath::parse(path).unwrap(),
        operation: OrganizationOperation::Hardlink,
        naming_pattern: OrganizationNamingPattern::Movie,
        nfo_policy: OrganizationNfoPolicy::GenerateMissing,
        automatic: false,
        enabled: true,
        rules: vec![OrganizationRuleInput {
            media_kind: OrganizationTargetKind::Movie,
            inbox_directory_id: None,
            explicit_tag: Some("favorite-4k".to_owned()),
            enabled: true,
        }],
    }
}

fn boundary(root: DeploymentRootView, overlaps_inbox: bool) -> OrganizationTargetBoundary {
    OrganizationTargetBoundary {
        root,
        overlaps_inbox,
    }
}

#[tokio::test]
async fn target_requires_a_writable_root_and_never_persists_a_host_path() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = common::seed_account(db.pool()).await;
    let store = OrganizationTargetStore::new(db.pool().clone());
    let target_id = Uuid::now_v7();
    let private_host_path = "/Volumes/private-library";

    let error = store
        .insert(
            account_id,
            target_id,
            target_input("library/movies"),
            &boundary(
                root("media", RootAccess::ReadOnly, private_host_path),
                false,
            ),
            100,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ValidationFailed);

    let saved = store
        .insert(
            account_id,
            target_id,
            target_input("library/movies"),
            &boundary(
                root("media", RootAccess::ReadWrite, private_host_path),
                false,
            ),
            200,
        )
        .await
        .unwrap();
    assert_eq!(saved.root_id.as_str(), "media");
    assert_eq!(saved.relative_path.as_str(), "library/movies");
    assert!(
        !serde_json::to_string(&saved)
            .unwrap()
            .contains(private_host_path)
    );

    let table_sql = sqlx::query_scalar::<_, String>(
        "SELECT group_concat(sql, ' ') FROM sqlite_master
         WHERE type='table' AND name LIKE 'organization_%'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(!table_sql.contains("host_path"));
    assert!(!table_sql.contains("container_path"));
}

#[tokio::test]
async fn target_roots_cannot_overlap_inboxes_or_other_targets() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = common::seed_account(db.pool()).await;
    let store = OrganizationTargetStore::new(db.pool().clone());
    let writable = root("media", RootAccess::ReadWrite, "Media library");

    let inbox_overlap = store
        .insert(
            account_id,
            Uuid::now_v7(),
            target_input("incoming"),
            &boundary(writable.clone(), true),
            100,
        )
        .await
        .unwrap_err();
    assert_eq!(inbox_overlap.code(), ErrorCode::OrganizationTargetOverlap);

    store
        .insert(
            account_id,
            Uuid::now_v7(),
            target_input("library/movies"),
            &boundary(writable.clone(), false),
            200,
        )
        .await
        .unwrap();
    for path in ["library", "library/movies", "library/movies/2026"] {
        let error = store
            .insert(
                account_id,
                Uuid::now_v7(),
                target_input(path),
                &boundary(writable.clone(), false),
                300,
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.code(),
            ErrorCode::OrganizationTargetOverlap,
            "path={path}"
        );
    }
}

#[tokio::test]
async fn profile_operation_rules_and_versions_replace_atomically() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = common::seed_account(db.pool()).await;
    let store = OrganizationTargetStore::new(db.pool().clone());
    let writable = root("media", RootAccess::ReadWrite, "Media library");
    let target_id = Uuid::now_v7();
    let saved = store
        .insert(
            account_id,
            target_id,
            target_input("library/movies"),
            &boundary(writable.clone(), false),
            100,
        )
        .await
        .unwrap();
    assert_eq!(saved.config_version, 1);
    assert_eq!(saved.operation, OrganizationOperation::Hardlink);
    assert_eq!(saved.rules.len(), 1);

    let mut replacement = target_input("library/films");
    replacement.display_name = "Films".to_owned();
    replacement.operation = OrganizationOperation::Copy;
    replacement.nfo_policy = OrganizationNfoPolicy::PreserveOnly;
    replacement.automatic = true;
    replacement.rules = vec![
        OrganizationRuleInput {
            media_kind: OrganizationTargetKind::Movie,
            inbox_directory_id: Some(Uuid::now_v7()),
            explicit_tag: None,
            enabled: true,
        },
        OrganizationRuleInput {
            media_kind: OrganizationTargetKind::Movie,
            inbox_directory_id: None,
            explicit_tag: Some("lectures".to_owned()),
            enabled: false,
        },
    ];

    let stale = store
        .replace(
            account_id,
            target_id,
            0,
            replacement.clone(),
            &boundary(writable.clone(), false),
            200,
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code(), ErrorCode::ConfigVersionConflict);

    let mut too_many_rules = replacement.clone();
    too_many_rules.rules = (0..101)
        .map(|index| OrganizationRuleInput {
            media_kind: OrganizationTargetKind::Movie,
            inbox_directory_id: None,
            explicit_tag: Some(format!("tag-{index}")),
            enabled: true,
        })
        .collect();
    let invalid = store
        .replace(
            account_id,
            target_id,
            1,
            too_many_rules,
            &boundary(writable.clone(), false),
            250,
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.code(), ErrorCode::ValidationFailed);
    assert_eq!(
        store.get(account_id, target_id).await.unwrap().unwrap(),
        saved
    );

    let replaced = store
        .replace(
            account_id,
            target_id,
            1,
            replacement,
            &boundary(writable, false),
            300,
        )
        .await
        .unwrap();
    assert_eq!(replaced.config_version, 2);
    assert_eq!(replaced.display_name, "Films");
    assert_eq!(replaced.operation, OrganizationOperation::Copy);
    assert_eq!(replaced.nfo_policy, OrganizationNfoPolicy::PreserveOnly);
    assert!(replaced.automatic);
    assert_eq!(replaced.rules.len(), 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM organization_rules WHERE target_id=?",)
            .bind(target_id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn versioned_deletion_removes_the_complete_target_aggregate() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = common::seed_account(db.pool()).await;
    let store = OrganizationTargetStore::new(db.pool().clone());
    let target_id = Uuid::now_v7();
    store
        .insert(
            account_id,
            target_id,
            target_input("library/movies"),
            &boundary(root("media", RootAccess::ReadWrite, "Media library"), false),
            100,
        )
        .await
        .unwrap();

    let stale = store.delete(account_id, target_id, 2).await.unwrap_err();
    assert_eq!(stale.code(), ErrorCode::ConfigVersionConflict);
    assert_eq!(store.list(account_id).await.unwrap().len(), 1);

    store.delete(account_id, target_id, 1).await.unwrap();
    assert!(store.get(account_id, target_id).await.unwrap().is_none());
    assert!(store.list(account_id).await.unwrap().is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM organization_profiles WHERE target_id=?",
        )
        .bind(target_id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM organization_rules WHERE target_id=?",)
            .bind(target_id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}
