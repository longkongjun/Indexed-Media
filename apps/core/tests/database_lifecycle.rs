mod common;

use std::collections::BTreeSet;

use common::TestConfigDir;
use mediaflow_core::platform::backup::{restore_backup, verify_database};
use mediaflow_core::platform::db::open_pool;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::tasks::model::NewScanTask;
use mediaflow_core::tasks::store::TaskStore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[tokio::test]
async fn new_database_enables_wal_foreign_keys_busy_timeout_and_full_sync() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);

    let db = migrate_with_backup(fixture.config())
        .await
        .expect("new database migration");
    let journal_mode = sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
        .fetch_one(db.pool())
        .await
        .expect("journal mode");
    let foreign_keys = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
        .fetch_one(db.pool())
        .await
        .expect("foreign key setting");
    let busy_timeout = sqlx::query_scalar::<_, i64>("PRAGMA busy_timeout")
        .fetch_one(db.pool())
        .await
        .expect("busy timeout");
    let synchronous = sqlx::query_scalar::<_, i64>("PRAGMA synchronous")
        .fetch_one(db.pool())
        .await
        .expect("synchronous setting");

    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    assert_eq!(foreign_keys, 1);
    assert_eq!(busy_timeout, 5_000);
    assert_eq!(synchronous, 2, "SQLite FULL is numeric value 2");
    assert!(fixture.backup_files().is_empty());
}

#[tokio::test]
async fn repeated_startup_is_idempotent_and_does_not_create_a_backup_without_pending_migrations() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let first = migrate_with_backup(fixture.config())
        .await
        .expect("first migration");
    first.pool().close().await;

    let second = migrate_with_backup(fixture.config())
        .await
        .expect("repeated migration");

    let applied =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1")
            .fetch_one(second.pool())
            .await
            .expect("applied migration count");
    let expected = i64::try_from(
        std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
            .expect("migration directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|value| value == "sql"))
            .count(),
    )
    .expect("migration count fits i64");
    assert_eq!(applied, expected);
    assert!(fixture.backup_files().is_empty());
}

#[tokio::test]
async fn downloader_connections_migration_creates_a_strict_secret_safe_schema() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("new database migration");

    let table_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='downloader_connections'",
    )
    .fetch_one(db.pool())
    .await
    .expect("downloader connections table");
    assert!(table_sql.contains("STRICT"));
    assert!(table_sql.contains("secret_ciphertext"));
    assert!(!table_sql.contains("password"));
    assert!(!table_sql.contains("username"));

    let invalid_kind = sqlx::query(
        "INSERT INTO downloader_connections
         (id,kind,display_name,base_url,enabled,secret_schema_version,secret_nonce,
          secret_ciphertext,config_version,health,failure_code,checked_at_us,
          manual_add,task_monitoring,product_version,api_version,created_at_us,updated_at_us)
         VALUES (?,'other','invalid','https://downloader.invalid',1,1,?,?,1,
                 'degraded','downloader.unavailable',NULL,NULL,NULL,NULL,NULL,1,1)",
    )
    .bind(Uuid::now_v7().as_bytes().as_slice())
    .bind(vec![0_u8; 24])
    .bind(vec![0_u8; 16])
    .execute(db.pool())
    .await;
    assert!(
        invalid_kind.is_err(),
        "unknown downloader kinds must be rejected"
    );
}

#[tokio::test]
async fn download_tasks_migration_creates_a_strict_encrypted_source_schema() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("new database migration");

    let table_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='download_tasks'",
    )
    .fetch_one(db.pool())
    .await
    .expect("download tasks table");
    assert!(table_sql.contains("STRICT"));
    assert!(table_sql.contains("source_ciphertext"));
    assert!(table_sql.contains("idempotency_key_sha256"));
    assert!(table_sql.contains("blocked_config_version"));
    assert!(!table_sql.contains("magnet"));
    assert!(!table_sql.contains("source_url"));
    assert!(!table_sql.contains("FOREIGN KEY(connection_id)"));
}

#[tokio::test]
async fn automation_sources_migration_creates_strict_secret_safe_runtime_tables() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("new database migration");

    for table in [
        "automation_sources",
        "automation_source_runtime",
        "automation_webhook_nonces",
    ] {
        let table_sql = sqlx::query_scalar::<_, String>(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
        )
        .bind(table)
        .fetch_one(db.pool())
        .await
        .unwrap_or_else(|_| panic!("missing {table}"));
        assert!(table_sql.contains("STRICT"), "table={table}");
        assert!(!table_sql.contains("feed_url"), "table={table}");
        assert!(!table_sql.contains("remote_path"), "table={table}");
    }
    let source_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='automation_sources'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(source_sql.contains("secret_ciphertext"));
    assert!(source_sql.contains("kind IN ('rss','webhook','download-completion')"));

    for table in ["automation_events", "automation_webhook_rotation_receipts"] {
        let table_sql = sqlx::query_scalar::<_, String>(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
        )
        .bind(table)
        .fetch_one(db.pool())
        .await
        .unwrap_or_else(|_| panic!("missing {table}"));
        assert!(table_sql.contains("STRICT"), "table={table}");
        assert!(
            table_sql.contains("secret_ciphertext") || table_sql.contains("payload_ciphertext")
        );
        assert!(!table_sql.contains("absolute_path"), "table={table}");
        assert!(!table_sql.contains("remote_path"), "table={table}");
    }
}

#[tokio::test]
async fn completion_and_reconcile_migration_is_strict_idempotent_and_path_free() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("new database migration");

    for table in [
        "download_completion_signals",
        "discovery_reconcile_requests",
        "automation_event_command_receipts",
    ] {
        let sql = sqlx::query_scalar::<_, String>(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
        )
        .bind(table)
        .fetch_one(db.pool())
        .await
        .unwrap_or_else(|_| panic!("missing {table}"));
        assert!(sql.contains("STRICT"), "table={table}");
        let lowercase = sql.to_ascii_lowercase();
        for forbidden in [
            "absolute_path",
            "remote_path",
            "save_path",
            "download_source",
        ] {
            assert!(
                !lowercase.contains(forbidden),
                "table={table} field={forbidden}"
            );
        }
    }
    let indexes = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type='index'
         AND name IN ('automation_sources_one_enabled_completion_mapping',
                      'download_completion_signals_claim_idx',
                      'discovery_reconcile_requests_task_idx') ORDER BY name",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(indexes.len(), 3);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        30
    );
}

#[tokio::test]
async fn local_enhancer_migration_seeds_one_strict_disabled_prompt_free_configuration() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("new database migration");
    let sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='identification_enhancer'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(sql.contains("STRICT"));
    let columns = sqlx::query_scalar::<_, String>(
        "SELECT name FROM pragma_table_info('identification_enhancer') ORDER BY cid",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    for forbidden in [
        "prompt",
        "response",
        "basename",
        "parent",
        "provider_id",
        "path",
    ] {
        assert!(columns.iter().all(|column| !column.contains(forbidden)));
    }
    let row = sqlx::query_as::<_, (i64, String, String, i64, String, String)>(
        "SELECT enabled,base_url,model,config_version,health,fallback_code
         FROM identification_enhancer WHERE singleton_key=1",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            0,
            "http://127.0.0.1:11434".to_owned(),
            "qwen3:4b".to_owned(),
            1,
            "degraded".to_owned(),
            "automation.source-disabled".to_owned(),
        )
    );
    let evidence_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='identification_evidence'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(evidence_sql.contains("'enhancer'"));
    assert!(evidence_sql.contains("'enhancer.hint.title'"));
    assert!(evidence_sql.contains("'enhancer.fallback.timeout'"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        30
    );
}

#[tokio::test]
async fn organization_targets_migration_creates_strict_bounded_aggregate_tables() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("new database migration");

    for table in [
        "organization_targets",
        "organization_profiles",
        "organization_rules",
    ] {
        let table_sql = sqlx::query_scalar::<_, String>(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
        )
        .bind(table)
        .fetch_one(db.pool())
        .await
        .unwrap_or_else(|_| panic!("missing {table}"));
        assert!(table_sql.contains("STRICT"), "table={table}");
    }

    let target_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='organization_targets'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(target_sql.contains("UNIQUE(account_id,root_id,relative_path_bytes)"));
    assert!(!target_sql.contains("host_path"));
    assert!(!target_sql.contains("container_path"));

    let profile_foreign_keys = sqlx::query_scalar::<_, String>(
        "SELECT \"table\" FROM pragma_foreign_key_list('organization_profiles')",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    let rule_foreign_keys = sqlx::query_scalar::<_, String>(
        "SELECT \"table\" FROM pragma_foreign_key_list('organization_rules')",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(profile_foreign_keys, vec!["organization_targets"]);
    assert_eq!(rule_foreign_keys, vec!["organization_targets"]);
}

#[tokio::test]
async fn organization_plans_migration_creates_strict_immutable_snapshot_and_receipt_tables() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("new database migration");
    for table in [
        "organization_config_snapshots",
        "organization_config_snapshot_rules",
        "organization_config_provenance",
        "organization_plans",
        "organization_plan_operations",
        "organization_plan_recalculation_receipts",
        "organization_plan_authorization_receipts",
    ] {
        let table_sql = sqlx::query_scalar::<_, String>(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
        )
        .bind(table)
        .fetch_one(db.pool())
        .await
        .unwrap_or_else(|_| panic!("missing {table}"));
        assert!(table_sql.contains("STRICT"), "table={table}");
    }
    let triggers = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'organization_%_immutable'",
    )
    .fetch_all(db.pool())
    .await
    .unwrap()
    .into_iter()
    .collect::<BTreeSet<_>>();
    for trigger in [
        "organization_config_snapshots_immutable",
        "organization_config_snapshot_rules_immutable",
        "organization_config_provenance_immutable",
        "organization_plans_immutable",
        "organization_plan_operations_immutable",
    ] {
        assert!(triggers.contains(trigger));
    }
}

#[tokio::test]
async fn organization_journals_migration_creates_strict_recovery_result_and_receipt_tables() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("new database migration");
    for table in [
        "organization_plan_nfo_inputs",
        "organization_file_operation_journals",
        "organization_local_results",
        "organization_rollback_receipts",
    ] {
        let table_sql = sqlx::query_scalar::<_, String>(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
        )
        .bind(table)
        .fetch_one(db.pool())
        .await
        .unwrap_or_else(|_| panic!("missing {table}"));
        assert!(table_sql.contains("STRICT"), "table={table}");
        assert!(!table_sql.contains("host_path"), "table={table}");
        assert!(!table_sql.contains("container_path"), "table={table}");
    }
    let recovery_index = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type='index'
         AND name='organization_journals_recovery_idx'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(recovery_index.contains("prepared"));
    assert!(recovery_index.contains("executing"));
    assert!(recovery_index.contains("applied"));
    assert!(recovery_index.contains("manual-review"));
    let journal_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type='table'
         AND name='organization_file_operation_journals'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(journal_sql.contains("nfo_preexisting"));
    assert!(journal_sql.contains("nfo_outcome"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'
             AND name='organization_plan_nfo_inputs_immutable'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn v23_processing_task_is_backed_up_and_upgraded_to_organization_checkpoints() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let legacy_pool = open_pool(&fixture.database_path())
        .await
        .expect("legacy database pool");
    let migration_dir = tempfile::tempdir().expect("legacy migration directory");
    for version in 1..=23 {
        let prefix = format!("{version:04}_");
        let source = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
            .expect("migration directory")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .expect("versioned migration");
        std::fs::copy(source.path(), migration_dir.path().join(source.file_name()))
            .expect("copy legacy migration");
    }
    sqlx::migrate::Migrator::new(migration_dir.path())
        .await
        .expect("legacy migrator")
        .run(&legacy_pool)
        .await
        .expect("apply real migrations 0001 through 0023");

    let account_id = common::seed_account(&legacy_pool).await;
    let inbox_id = common::seed_inbox(&legacy_pool).await;
    let revision_id =
        common::seed_stable_revision(&legacy_pool, inbox_id, b"movies/v23.mkv", vec![23]).await;
    let tracked_file_id = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT tracked_file_id FROM discovery_file_revisions WHERE id=?",
    )
    .bind(revision_id.as_bytes().as_slice())
    .fetch_one(&legacy_pool)
    .await
    .unwrap();
    let task_id = Uuid::now_v7();
    let attempt_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tasks_processing_tasks
         (id,account_id,discovered_file_id,file_revision_id,inbox_directory_id,
          current_attempt_id,status,stage,checkpoint,reason,recovering,attempt_count,
          cancel_requested,lease_owner,lease_expires_at_us,next_retry_at_us,
          config_snapshot_json,version,created_at_us,updated_at_us)
         VALUES (?,?,?,?,?,?,'queued','identification','pending',NULL,0,1,0,
                 NULL,NULL,NULL,'{}',1,1,1)",
    )
    .bind(task_id.as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
    .bind(tracked_file_id)
    .bind(revision_id.as_bytes().as_slice())
    .bind(inbox_id.as_bytes().as_slice())
    .bind(attempt_id.as_bytes().as_slice())
    .execute(&legacy_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks_processing_attempts
         (id,task_id,reason,ordinal,status,stage,lease_owner,created_at_us)
         VALUES (?,?,'initial',1,'queued','identification',NULL,1)",
    )
    .bind(attempt_id.as_bytes().as_slice())
    .bind(task_id.as_bytes().as_slice())
    .execute(&legacy_pool)
    .await
    .unwrap();
    legacy_pool.close().await;

    let db = migrate_with_backup(fixture.config())
        .await
        .expect("v23 task migrates to organization-capable schema");
    assert_eq!(fixture.backup_files().len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks_processing_tasks WHERE id=?")
            .bind(task_id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    sqlx::query(
        "UPDATE tasks_processing_tasks
         SET status='partial-success',stage='nfo',checkpoint='nfo-failed',
             reason='organization.nfo-failed',version=version+1,updated_at_us=2
         WHERE id=?",
    )
    .bind(task_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .expect("organization state satisfies upgraded checks");
    let stored = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT status,stage,checkpoint,reason FROM tasks_processing_tasks WHERE id=?",
    )
    .bind(task_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        stored,
        (
            "partial-success".to_owned(),
            "nfo".to_owned(),
            "nfo-failed".to_owned(),
            Some("organization.nfo-failed".to_owned()),
        )
    );
    let columns = sqlx::query_scalar::<_, String>(
        "SELECT name FROM pragma_table_info('tasks_processing_tasks')",
    )
    .fetch_all(db.pool())
    .await
    .unwrap()
    .into_iter()
    .collect::<BTreeSet<_>>();
    for column in [
        "organization_plan_id",
        "organization_result_id",
        "catalog_media_item_id",
    ] {
        assert!(columns.contains(column));
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "ok"
    );
}

#[tokio::test]
async fn pre_0022_target_database_is_backed_up_then_upgraded_without_losing_the_target() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let legacy_pool = open_pool(&fixture.database_path())
        .await
        .expect("legacy database pool");
    let migration_dir = tempfile::tempdir().expect("legacy migration directory");
    for version in 1..=21 {
        let prefix = format!("{version:04}_");
        let source = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
            .expect("migration directory")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .expect("versioned migration");
        std::fs::copy(source.path(), migration_dir.path().join(source.file_name()))
            .expect("copy legacy migration");
    }
    sqlx::migrate::Migrator::new(migration_dir.path())
        .await
        .expect("legacy migrator")
        .run(&legacy_pool)
        .await
        .expect("apply real migrations 0001 through 0021");
    let account_id = common::seed_account(&legacy_pool).await;
    let target_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO organization_targets
         (id,account_id,kind,display_name,root_id,relative_path_bytes,
          relative_path_display,config_version,created_at_us,updated_at_us)
         VALUES (?,?,'movie','Movies','media',x'4d6f76696573','Movies',1,1,1)",
    )
    .bind(target_id.as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
    .execute(&legacy_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO organization_profiles
         (target_id,operation,naming_pattern,nfo_policy,automatic,enabled,
          config_version,created_at_us,updated_at_us)
         VALUES (?,'copy','movie','preserve-only',0,1,1,1,1)",
    )
    .bind(target_id.as_bytes().as_slice())
    .execute(&legacy_pool)
    .await
    .unwrap();
    legacy_pool.close().await;

    let db = migrate_with_backup(fixture.config())
        .await
        .expect("forward migration");
    assert_eq!(fixture.backup_files().len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT display_name FROM organization_targets WHERE id=?")
            .bind(target_id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "Movies"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='organization_plans'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='table' AND name='organization_file_operation_journals'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn pre_0021_database_is_backed_up_then_upgraded_without_losing_existing_data() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let legacy_pool = open_pool(&fixture.database_path())
        .await
        .expect("legacy database pool");
    let migration_dir = tempfile::tempdir().expect("legacy migration directory");
    for version in 1..=20 {
        let prefix = format!("{version:04}_");
        let source = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
            .expect("migration directory")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .expect("versioned migration");
        std::fs::copy(source.path(), migration_dir.path().join(source.file_name()))
            .expect("copy legacy migration");
    }
    sqlx::migrate::Migrator::new(migration_dir.path())
        .await
        .expect("legacy migrator")
        .run(&legacy_pool)
        .await
        .expect("apply real migrations 0001 through 0020");
    let account_id = common::seed_account(&legacy_pool).await;
    legacy_pool.close().await;

    let db = migrate_with_backup(fixture.config())
        .await
        .expect("forward migration");
    assert_eq!(fixture.backup_files().len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, Vec<u8>>("SELECT id FROM identity_accounts WHERE id=?")
            .bind(account_id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .expect("preserved account"),
        account_id.as_bytes()
    );
    let organization_tables = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'organization_%'",
    )
    .fetch_all(db.pool())
    .await
    .unwrap()
    .into_iter()
    .collect::<BTreeSet<_>>();
    for table in [
        "organization_targets",
        "organization_profiles",
        "organization_rules",
    ] {
        assert!(organization_tables.contains(table));
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn v12_review_cases_migrate_with_stable_ids_initial_versions_and_empty_manual_checkpoints() {
    use mediaflow_core::identification::decision::{
        DecisionLevel, DecisionReason, IdentificationDecisionDraft,
    };
    use mediaflow_core::identification::store::{IdentificationCommit, IdentificationStore};
    use mediaflow_core::tasks::processing::model::{
        ProcessingCheckpoint, ProcessingLease, ProcessingStage, ProcessingStatus,
        ProcessingTaskView,
    };

    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let legacy_pool = open_pool(&fixture.database_path())
        .await
        .expect("legacy database pool");
    let migration_dir = tempfile::tempdir().expect("legacy migration directory");
    for version in 1..=12 {
        let prefix = format!("{version:04}_");
        let source = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
            .expect("migration directory")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .expect("versioned migration");
        std::fs::copy(source.path(), migration_dir.path().join(source.file_name()))
            .expect("copy legacy migration");
    }
    sqlx::migrate::Migrator::new(migration_dir.path())
        .await
        .expect("legacy migrator")
        .run(&legacy_pool)
        .await
        .expect("apply real migrations 0001 through 0012");
    let account = common::seed_account(&legacy_pool).await;
    let inbox = common::seed_inbox(&legacy_pool).await;
    let revision =
        common::seed_stable_revision(&legacy_pool, inbox, b"movies/legacy-review.mkv", vec![1])
            .await;
    let tracked_file_id = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT tracked_file_id FROM discovery_file_revisions WHERE id=?",
    )
    .bind(revision.as_bytes().as_slice())
    .fetch_one(&legacy_pool)
    .await
    .unwrap();
    let task_id = Uuid::now_v7();
    let processing_attempt_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tasks_processing_tasks
         (id,account_id,discovered_file_id,file_revision_id,inbox_directory_id,
          current_attempt_id,status,stage,checkpoint,reason,recovering,attempt_count,
          cancel_requested,lease_owner,lease_expires_at_us,next_retry_at_us,
          config_snapshot_json,version,created_at_us,updated_at_us)
         VALUES (?,?,?,?,?,?,'running','identification','pending',NULL,0,1,0,
                 'legacy-review',200000000,NULL,'{}',1,91000000,92000000)",
    )
    .bind(task_id.as_bytes().as_slice())
    .bind(account.as_bytes().as_slice())
    .bind(tracked_file_id)
    .bind(revision.as_bytes().as_slice())
    .bind(inbox.as_bytes().as_slice())
    .bind(processing_attempt_id.as_bytes().as_slice())
    .execute(&legacy_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks_processing_attempts
         (id,task_id,reason,ordinal,status,stage,lease_owner,started_at_us,created_at_us)
         VALUES (?,?,'initial',1,'running','identification','legacy-review',92000000,91000000)",
    )
    .bind(processing_attempt_id.as_bytes().as_slice())
    .bind(task_id.as_bytes().as_slice())
    .execute(&legacy_pool)
    .await
    .unwrap();
    let lease = ProcessingLease {
        task: ProcessingTaskView {
            id: task_id,
            inbox_directory_id: inbox,
            file_revision_id: revision,
            relative_path: "movies/legacy-review.mkv".to_owned(),
            status: ProcessingStatus::Running,
            stage: ProcessingStage::Identification,
            checkpoint: ProcessingCheckpoint::Pending,
            decision_checkpoint: None,
            current_task_decision_id: None,
            organization_plan_id: None,
            organization_result_id: None,
            catalog_media_item_id: None,
            reason: None,
            recovering: false,
            attempt_count: 1,
            next_retry_at: None,
            allowed_actions: vec![
                mediaflow_core::tasks::processing::model::ProcessingTaskAction::Cancel,
            ],
            updated_at: "1970-01-01T00:01:32Z".to_owned(),
        },
        attempt_id: processing_attempt_id,
        owner: "legacy-review".to_owned(),
        version: 1,
        expires_at_us: 200_000_000,
    };
    let identification = IdentificationStore::new(legacy_pool.clone());
    let attempt = identification
        .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 93_000_000)
        .await
        .unwrap();
    let committed = identification
        .commit(IdentificationCommit {
            lease: &lease,
            attempt_id: attempt.id,
            evidence: &[],
            candidates: &[],
            decision: &IdentificationDecisionDraft {
                level: DecisionLevel::Unidentified,
                selected_candidate: None,
                reasons: vec![DecisionReason::NoCandidate],
                retry_at_us: None,
                graph: None,
                rule_version: 1,
            },
            title_hint: Some("legacy review"),
            now_us: 94_000_000,
        })
        .await
        .unwrap();
    let case_id = committed.review_case_id.unwrap();
    legacy_pool.close().await;

    let db = migrate_with_backup(fixture.config())
        .await
        .expect("v12 review data migrates to current schema");
    let (stored_id, version) = sqlx::query_as::<_, (Vec<u8>, i64)>(
        "SELECT id,version FROM identification_review_cases WHERE id=? AND account_id=?",
    )
    .bind(case_id.as_bytes().as_slice())
    .bind(account.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(Uuid::from_slice(&stored_id).unwrap(), case_id);
    assert_eq!(version, 1);
    let checkpoint = sqlx::query_as::<_, (Option<Vec<u8>>, Option<String>)>(
        "SELECT current_task_decision_id,decision_checkpoint FROM tasks_processing_tasks WHERE id=?",
    )
    .bind(lease.task.id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(checkpoint, (None, None));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn v5_scan_data_with_legacy_idempotency_conflicts_migrates_without_losing_history() {
    let fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let legacy_pool = open_pool(&fixture.database_path())
        .await
        .expect("legacy database pool");
    let migration_dir = tempfile::tempdir().expect("legacy migration directory");
    for version in 1..=5 {
        let prefix = format!("{version:04}_");
        let source = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
            .expect("migration directory")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .expect("versioned migration");
        std::fs::copy(source.path(), migration_dir.path().join(source.file_name()))
            .expect("copy legacy migration");
    }
    sqlx::migrate::Migrator::new(migration_dir.path())
        .await
        .expect("legacy migrator")
        .run(&legacy_pool)
        .await
        .expect("apply real migrations 0001 through 0005");

    let account_id = Uuid::now_v7();
    let inbox_one = Uuid::now_v7();
    let inbox_two = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(&legacy_pool)
    .await
    .unwrap();
    for (inbox_id, path) in [
        (inbox_one, b"one".as_slice()),
        (inbox_two, b"two".as_slice()),
    ] {
        sqlx::query(
            "INSERT INTO discovery_inbox_directories
             (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
              health,last_checked_at_us,version,created_at_us,updated_at_us)
             VALUES (?,'incoming',?,?,X'01',X'02','available',1,1,1,1)",
        )
        .bind(inbox_id.as_bytes().as_slice())
        .bind(path)
        .bind(String::from_utf8_lossy(path).as_ref())
        .execute(&legacy_pool)
        .await
        .unwrap();
    }
    let task_one = insert_legacy_scan(&legacy_pool, account_id, inbox_one, 100).await;
    let task_two = insert_legacy_scan(&legacy_pool, account_id, inbox_two, 200).await;
    let (task_one_id, batch_one, attempt_one) = task_one;
    let (task_two_id, _, _) = task_two;
    let digest: [u8; 32] = Sha256::digest(b"legacy-shared-key").into();
    for (action, target, result, created_at) in [
        ("scan.create", inbox_one, task_one_id, 100),
        ("scan.create", inbox_two, task_two_id, 200),
        ("scan.cancel", task_two_id, task_two_id, 300),
    ] {
        sqlx::query(
            "INSERT INTO tasks_idempotency_keys
             (account_id,action,target_id,key_sha256,result_task_id,created_at_us)
             VALUES (?,?,?,?,?,?)",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(action)
        .bind(target.as_bytes().as_slice())
        .bind(digest.as_slice())
        .bind(result.as_bytes().as_slice())
        .bind(created_at)
        .execute(&legacy_pool)
        .await
        .unwrap();
    }
    for (index, path) in [b"b.mkv".as_slice(), b"a.mkv".as_slice()]
        .into_iter()
        .enumerate()
    {
        let file_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO discovery_files
             (id,inbox_directory_id,relative_path_bytes,relative_path_display,identity_snapshot,
              size_bytes,modified_at_ns,first_seen_batch_id,last_seen_batch_id,created_at_us,updated_at_us)
             VALUES (?,?,?,?,X'01',1,1,?,?,?,?)",
        )
        .bind(file_id.as_bytes().as_slice())
        .bind(inbox_one.as_bytes().as_slice())
        .bind(path)
        .bind(String::from_utf8_lossy(path).as_ref())
        .bind(batch_one.as_bytes().as_slice())
        .bind(batch_one.as_bytes().as_slice())
        .bind(400 + i64::try_from(index).unwrap())
        .bind(400 + i64::try_from(index).unwrap())
        .execute(&legacy_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO discovery_scan_file_observations
             (scan_batch_id,discovered_file_id,last_observed_attempt_id,identity_snapshot,
              size_bytes,modified_at_ns,observed_at_us) VALUES (?,?,?,X'01',1,1,?)",
        )
        .bind(batch_one.as_bytes().as_slice())
        .bind(file_id.as_bytes().as_slice())
        .bind(attempt_one.as_bytes().as_slice())
        .bind(400 + i64::try_from(index).unwrap())
        .execute(&legacy_pool)
        .await
        .unwrap();
    }
    for (index, code) in ["entry.unavailable", "entry.changed"]
        .into_iter()
        .enumerate()
    {
        let error_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO tasks_scan_errors
             (id,task_id,scan_batch_id,attempt_id,code,scope,relative_path_bytes,
              relative_path_display,first_seen_at_us,last_seen_at_us)
             VALUES (?,?,?,?,?,'entry',?,?,?,?)",
        )
        .bind(error_id.as_bytes().as_slice())
        .bind(task_one_id.as_bytes().as_slice())
        .bind(batch_one.as_bytes().as_slice())
        .bind(attempt_one.as_bytes().as_slice())
        .bind(code)
        .bind(code.as_bytes())
        .bind(code)
        .bind(500 + i64::try_from(index).unwrap())
        .bind(500 + i64::try_from(index).unwrap())
        .execute(&legacy_pool)
        .await
        .unwrap();
    }
    legacy_pool.close().await;

    let db = migrate_with_backup(fixture.config())
        .await
        .expect("v5 scan data migrates to current schema");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks_idempotency_keys")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        3,
        "all legacy idempotency results remain durable"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks_idempotency_bindings")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    let observation_sequences = sqlx::query_scalar::<_, i64>(
        "SELECT first_observed_seq FROM discovery_scan_file_observations ORDER BY first_observed_seq",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(observation_sequences, vec![1, 2]);
    let error_sequences = sqlx::query_scalar::<_, i64>(
        "SELECT first_seen_seq FROM tasks_scan_errors ORDER BY first_seen_seq",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(error_sequences, vec![1, 2]);

    let store = TaskStore::new(db.pool().clone());
    let historical_winner = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_one,
            idempotency_key: "legacy-shared-key".to_owned(),
            now_us: 1_000,
        })
        .await
        .unwrap();
    assert_eq!(historical_winner.id, task_one_id);
    let historical_conflict = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_two,
            idempotency_key: "legacy-shared-key".to_owned(),
            now_us: 1_000,
        })
        .await
        .unwrap_err();
    assert_eq!(historical_conflict.code(), ErrorCode::RequestConflict);
    let historical_action_conflict = store
        .request_cancel(account_id, task_two_id, "legacy-shared-key", 1_000)
        .await
        .unwrap_err();
    assert_eq!(
        historical_action_conflict.code(),
        ErrorCode::RequestConflict
    );
    db.pool().close().await;

    let reopened = migrate_with_backup(fixture.config())
        .await
        .expect("current schema starts repeatedly");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks_idempotency_keys")
            .fetch_one(reopened.pool())
            .await
            .unwrap(),
        3
    );
    assert_eq!(fixture.backup_files().len(), 1);
}

async fn insert_legacy_scan(
    pool: &sqlx::SqlitePool,
    account_id: Uuid,
    inbox_id: Uuid,
    now_us: i64,
) -> (Uuid, Uuid, Uuid) {
    let task_id = Uuid::now_v7();
    let batch_id = Uuid::now_v7();
    let attempt_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO discovery_scan_batches
         (id,inbox_directory_id,inbox_version,created_at_us,updated_at_us) VALUES (?,?,1,?,?)",
    )
    .bind(batch_id.as_bytes().as_slice())
    .bind(inbox_id.as_bytes().as_slice())
    .bind(now_us)
    .bind(now_us)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks_scan_tasks
         (id,account_id,scan_batch_id,inbox_directory_id,status,stage,recovering,
          current_attempt_id,created_at_us,updated_at_us)
         VALUES (?,?,?,?,'queued','queued',0,?,?,?)",
    )
    .bind(task_id.as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
    .bind(batch_id.as_bytes().as_slice())
    .bind(inbox_id.as_bytes().as_slice())
    .bind(attempt_id.as_bytes().as_slice())
    .bind(now_us)
    .bind(now_us)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks_scan_attempts (id,task_id,reason,ordinal,status,created_at_us)
         VALUES (?,?,'initial',1,'queued',?)",
    )
    .bind(attempt_id.as_bytes().as_slice())
    .bind(task_id.as_bytes().as_slice())
    .bind(now_us)
    .execute(pool)
    .await
    .unwrap();
    (task_id, batch_id, attempt_id)
}

#[tokio::test]
async fn config_path_that_is_not_a_directory_is_rejected_before_database_creation() {
    let mut fixture = TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let file_path = fixture.config().deployment_roots_file.clone();
    fixture.config_mut().config_dir = file_path;

    let error = migrate_with_backup(fixture.config()).await.unwrap_err();

    assert_eq!(error.code(), ErrorCode::ConfigInvalid);
}

#[tokio::test]
async fn sqlite_paths_with_uri_reserved_characters_are_opened_as_native_paths() {
    let root = tempfile::tempdir().expect("reserved path root");
    let config_dir = root.path().join("config#question?percent%");
    std::fs::create_dir(&config_dir).expect("reserved config directory");
    let database = config_dir.join("mediaflow#main?.db");

    let pool = open_pool(&database).await.expect("path-native SQLite pool");
    sqlx::query("CREATE TABLE reserved_path_sentinel (value INTEGER NOT NULL)")
        .execute(&pool)
        .await
        .expect("reserved-path schema");
    pool.close().await;

    assert!(database.is_file());
}

#[tokio::test]
async fn public_database_verification_includes_committed_uncheckpointed_wal_rows() {
    let root = tempfile::tempdir().expect("WAL verification root");
    let database = root.path().join("wal-source.db");
    let pool = open_pool(&database).await.expect("writable WAL pool");
    let mut writer = pool.acquire().await.expect("held writer connection");
    sqlx::query("PRAGMA wal_autocheckpoint = 0")
        .execute(&mut *writer)
        .await
        .expect("disable WAL autocheckpoint");
    sqlx::query("CREATE TABLE wal_only (value TEXT NOT NULL)")
        .execute(&mut *writer)
        .await
        .expect("WAL-only schema");
    sqlx::query("INSERT INTO wal_only (value) VALUES ('committed')")
        .execute(&mut *writer)
        .await
        .expect("committed WAL row");

    let report = verify_database(&database)
        .await
        .expect("WAL-aware public verification");

    assert_eq!(report.critical_counts.get("wal_only"), Some(&1));
    drop(writer);
    pool.close().await;
}

#[tokio::test]
async fn existing_schema_is_backed_up_and_verified_before_forward_migration() {
    let fixture = TestConfigDir::with_schema_version(1).await;

    let db = migrate_with_backup(fixture.config())
        .await
        .expect("forward migration");

    let backups = fixture.backup_files();
    assert_eq!(backups.len(), 1, "one consistent SQLite backup");
    assert_eq!(fixture.manifest_files().len(), 1, "one backup manifest");
    let manifest = fixture.read_manifest();
    assert_eq!(manifest.schema_version, 1);
    let digest = hex::encode(Sha256::digest(
        std::fs::read(&backups[0]).expect("backup bytes"),
    ));
    assert_eq!(manifest.sha256, digest);
    assert!(!manifest.created_at.is_empty());

    let backup_report = verify_database(&backups[0]).await.expect("verified backup");
    assert_eq!(backup_report.integrity_check, "ok");
    let legacy_value = sqlx::query_scalar::<_, String>("SELECT value FROM legacy_sentinel")
        .fetch_one(&open_pool(&backups[0]).await.expect("backup pool"))
        .await
        .expect("legacy backup row");
    assert_eq!(legacy_value, "before-migration");

    let tables = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
    )
    .fetch_all(db.pool())
    .await
    .expect("migrated tables")
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert!(tables.contains("platform_metadata"));
    assert!(tables.contains("platform_audit_events"));
    assert!(tables.contains("identity_accounts"));
    assert!(tables.contains("identity_sessions"));
    assert!(tables.contains("identity_login_throttles"));
    assert!(tables.contains("discovery_inbox_directories"));
    assert!(tables.contains("discovery_scan_batches"));
    assert!(tables.contains("discovery_files"));
    assert!(tables.contains("discovery_scan_file_observations"));
    assert!(tables.contains("tasks_scan_tasks"));
    assert!(tables.contains("tasks_scan_attempts"));
    assert!(tables.contains("tasks_scan_errors"));
    assert!(tables.contains("tasks_idempotency_keys"));
    assert!(tables.contains("platform_outbox_events"));
}

#[tokio::test]
async fn backup_pairs_database_with_only_the_allowlisted_deployment_roots_config() {
    let fixture = TestConfigDir::with_schema_version(1).await;
    let secret = fixture.config().config_dir.join("bootstrap-secret.txt");
    std::fs::write(&secret, b"MUST_NOT_ENTER_BACKUP").expect("secret fixture");

    migrate_with_backup(fixture.config())
        .await
        .expect("backup-producing migration");

    let configs = fixture.backup_config_files();
    assert_eq!(configs.len(), 1, "one allowlisted config backup");
    assert_eq!(
        std::fs::read(&configs[0]).expect("config backup"),
        std::fs::read(&fixture.config().deployment_roots_file).expect("source config")
    );
    let manifest_path = fixture.manifest_files().remove(0);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    let config_filename = configs[0].file_name().unwrap().to_str().unwrap();
    assert_eq!(
        manifest["files"]["deployment_roots"]["filename"],
        config_filename
    );
    assert_eq!(
        manifest["files"]["deployment_roots"]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    let manifest_text = serde_json::to_string(&manifest).unwrap();
    assert!(!manifest_text.contains(&fixture.config().deployment_roots_file.display().to_string()));
    assert!(!manifest_text.contains("bootstrap-secret"));
    assert!(
        !std::fs::read_dir(fixture.backups_dir())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains("secret"))
    );
}

#[tokio::test]
async fn backup_failure_blocks_forward_migration() {
    let fixture = TestConfigDir::with_schema_version(1).await;
    fixture.block_backup_directory_with_file();

    let error = migrate_with_backup(fixture.config()).await.unwrap_err();

    assert_eq!(error.code(), ErrorCode::BackupFailed);
    assert_eq!(fixture.schema_version().await, 1);
    let pool = open_pool(&fixture.database_path())
        .await
        .expect("legacy pool after failed migration");
    let platform_table_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'platform_metadata'",
    )
    .fetch_one(&pool)
    .await
    .expect("platform table count");
    assert_eq!(platform_table_count, 0);
}

#[tokio::test]
async fn restore_refuses_a_non_empty_target_directory() {
    let fixture = TestConfigDir::with_schema_version(1).await;
    migrate_with_backup(fixture.config())
        .await
        .expect("backup-producing migration");
    let backup = fixture.backup_files().remove(0);
    let target = fixture.non_empty_target();

    let error = restore_backup(&backup, &target).await.unwrap_err();

    assert_eq!(error.code(), ErrorCode::RestoreTargetNotEmpty);
    assert_eq!(
        std::fs::read(target.join("keep.txt")).expect("sentinel remains"),
        b"do not overwrite"
    );
    assert!(!target.join("mediaflow.db").exists());
}

#[tokio::test]
async fn restore_to_empty_target_verifies_integrity_foreign_keys_and_counts() {
    let fixture = TestConfigDir::with_schema_version(1).await;
    migrate_with_backup(fixture.config())
        .await
        .expect("backup-producing migration");
    let backup = fixture.backup_files().remove(0);
    let target = fixture.empty_target();

    let report = restore_backup(&backup, &target)
        .await
        .expect("clean restore");

    assert_eq!(report.integrity_check, "ok");
    assert!(report.foreign_key_violations.is_empty());
    assert_eq!(report.critical_counts.get("legacy_sentinel"), Some(&1));
    assert!(target.join("mediaflow.db").is_file());
    assert_eq!(
        std::fs::read(target.join("deployment-roots.json")).expect("restored roots config"),
        std::fs::read(&fixture.config().deployment_roots_file).expect("source roots config")
    );
}

#[tokio::test]
async fn restore_rejects_a_manifest_schema_version_that_does_not_match_the_backup() {
    let fixture = TestConfigDir::with_schema_version(1).await;
    migrate_with_backup(fixture.config())
        .await
        .expect("backup-producing migration");
    let backup = fixture.backup_files().remove(0);
    let manifest_path = fixture.manifest_files().remove(0);
    let mut manifest = fixture.read_manifest();
    manifest.schema_version = 999;
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("tampered manifest"),
    )
    .expect("write tampered manifest");
    let target = fixture.empty_target();

    let error = restore_backup(&backup, &target).await.unwrap_err();

    assert_eq!(error.code(), ErrorCode::DatabaseInvalid);
    assert!(!target.join("mediaflow.db").exists());
}

#[tokio::test]
async fn restore_rejects_a_tampered_deployment_roots_backup_without_leaving_artifacts() {
    let fixture = TestConfigDir::with_schema_version(1).await;
    migrate_with_backup(fixture.config())
        .await
        .expect("backup-producing migration");
    let backup = fixture.backup_files().remove(0);
    let config_backup = fixture.backup_config_files().remove(0);
    std::fs::write(&config_backup, b"[\"tampered\"]\n").expect("tampered config backup");
    let target = fixture.empty_target();

    let error = restore_backup(&backup, &target).await.unwrap_err();

    assert_eq!(error.code(), ErrorCode::DatabaseInvalid);
    assert!(
        std::fs::read_dir(&target)
            .expect("restore target")
            .next()
            .is_none(),
        "failed restore must leave neither final nor temporary artifacts"
    );
}

#[tokio::test]
async fn restore_rejects_manifest_redirect_to_a_hash_matching_sibling_file() {
    let fixture = TestConfigDir::with_schema_version(1).await;
    migrate_with_backup(fixture.config())
        .await
        .expect("backup-producing migration");
    let backup = fixture.backup_files().remove(0);
    let manifest_path = fixture.manifest_files().remove(0);
    let sibling = fixture.backups_dir().join("unrelated-secret.json");
    std::fs::write(&sibling, b"{\"secret\":true}\n").expect("sibling fixture");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["files"]["deployment_roots"]["filename"] =
        sibling.file_name().unwrap().to_str().unwrap().into();
    manifest["files"]["deployment_roots"]["sha256"] =
        hex::encode(Sha256::digest(std::fs::read(&sibling).unwrap())).into();
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .expect("redirected manifest");
    let target = fixture.empty_target();

    let error = restore_backup(&backup, &target).await.unwrap_err();

    assert_eq!(error.code(), ErrorCode::DatabaseInvalid);
    assert!(std::fs::read_dir(&target).unwrap().next().is_none());
}
