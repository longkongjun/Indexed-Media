mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::policy::{
    DEFAULT_MINIMUM_AGE_SECONDS, DEFAULT_RECONCILE_INTERVAL_SECONDS,
    DEFAULT_STABLE_OBSERVATION_INTERVAL_SECONDS, PutDiscoveryPolicyCommand,
};
use mediaflow_core::discovery::service::DiscoveryService;
use mediaflow_core::platform::db::open_pool;
use mediaflow_core::platform::migrations::migrate_with_backup;

#[tokio::test]
async fn default_policy_is_backfilled_and_updates_use_optimistic_versioning() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = common::seed_inbox(db.pool()).await;
    let service = DiscoveryService::policy_only(db.pool().clone());

    let initial = service.get_policy(inbox).await.unwrap();
    assert_eq!(initial.minimum_age_seconds, DEFAULT_MINIMUM_AGE_SECONDS);
    assert_eq!(
        initial.stable_observation_interval_seconds,
        DEFAULT_STABLE_OBSERVATION_INTERVAL_SECONDS
    );
    assert_eq!(
        initial.reconcile_interval_seconds,
        DEFAULT_RECONCILE_INTERVAL_SECONDS
    );
    assert!(initial.watcher_enabled);
    assert_eq!(initial.config_version, 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT health FROM discovery_watch_states WHERE inbox_directory_id=?",
        )
        .bind(inbox.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "pending"
    );

    let changed = service
        .put_policy(
            inbox,
            PutDiscoveryPolicyCommand {
                minimum_age_seconds: 120,
                stable_observation_interval_seconds: 45,
                reconcile_interval_seconds: 1_800,
                watcher_enabled: false,
            },
            1,
        )
        .await
        .unwrap();
    assert_eq!(changed.config_version, 2);
    assert_eq!(changed.minimum_age_seconds, 120);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT health FROM discovery_watch_states WHERE inbox_directory_id=?",
        )
        .bind(inbox.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "disabled"
    );
    assert!(
        service
            .put_policy(
                inbox,
                PutDiscoveryPolicyCommand {
                    minimum_age_seconds: 60,
                    stable_observation_interval_seconds: 30,
                    reconcile_interval_seconds: 900,
                    watcher_enabled: true,
                },
                1,
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn policy_limits_are_rejected_without_mutating_the_current_version() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = common::seed_inbox(db.pool()).await;
    let service = DiscoveryService::policy_only(db.pool().clone());
    let invalid = PutDiscoveryPolicyCommand {
        minimum_age_seconds: 86_401,
        stable_observation_interval_seconds: 0,
        reconcile_interval_seconds: 59,
        watcher_enabled: true,
    };
    assert!(service.put_policy(inbox, invalid, 1).await.is_err());
    assert_eq!(service.get_policy(inbox).await.unwrap().config_version, 1);
}

#[tokio::test]
async fn task_two_database_can_apply_the_lower_numbered_forward_migration_without_downgrade() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let baseline_migrations = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() != "0009_continuous_discovery.sql" {
            std::fs::copy(
                entry.path(),
                baseline_migrations.path().join(entry.file_name()),
            )
            .unwrap();
        }
    }
    let baseline = sqlx::migrate::Migrator::new(baseline_migrations.path())
        .await
        .unwrap();
    let pool = open_pool(&fixture.database_path()).await.unwrap();
    baseline.run(&pool).await.unwrap();
    let baseline_user_version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;

    let upgraded = migrate_with_backup(fixture.config()).await.unwrap();
    for table in [
        "discovery_inbox_policies",
        "discovery_watch_states",
        "discovery_file_revisions",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            )
            .bind(table)
            .fetch_one(upgraded.pool())
            .await
            .unwrap(),
            1,
            "{table}"
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(upgraded.pool())
            .await
            .unwrap(),
        baseline_user_version
    );
}
