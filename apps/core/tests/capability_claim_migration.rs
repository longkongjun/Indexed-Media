//! Upgrade the actual previous schema, preserving audit rows and a pre-upgrade backup.
mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::migrations::migrate_with_backup;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::borrow::Cow;
use uuid::Uuid;

#[tokio::test]
async fn previous_schema_upgrades_claims_and_keeps_verified_original_backup() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let all = sqlx::migrate!("./migrations");
    let previous = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 38)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    let pool = SqlitePoolOptions::new()
        .connect_with(
            SqliteConnectOptions::new()
                .filename(fixture.database_path())
                .create_if_missing(true),
        )
        .await
        .unwrap();
    previous.run(&pool).await.unwrap();
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO capability_calls (request_id,idempotency_key,capability,actor_kind,actor_id,status,result_json,created_at_us) VALUES (?,'legacy-key','media.task-status','ai','dsh','succeeded','{\"legacy\":true}',1)")
        .bind(id.as_bytes().as_slice()).execute(&pool).await.unwrap();
    pool.close().await;
    assert!(fixture.backup_files().is_empty());
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(version, 40);
    let legacy: String =
        sqlx::query_scalar("SELECT result_json FROM capability_calls WHERE request_id=?")
            .bind(id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(legacy, "{\"legacy\":true}");
    let claims: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capability_request_claims")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(claims, 0);
    assert_eq!(fixture.backup_files().len(), 1);
    let backup = SqlitePoolOptions::new()
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&fixture.backup_files()[0])
                .read_only(true),
        )
        .await
        .unwrap();
    let original: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&backup)
        .await
        .unwrap();
    assert_eq!(original, 38);
    backup.close().await;
    db.pool().close().await;
    let again = migrate_with_backup(fixture.config()).await.unwrap();
    assert_eq!(
        fixture.backup_files().len(),
        1,
        "no extra backup when nothing is pending"
    );
    again.pool().close().await;
}
