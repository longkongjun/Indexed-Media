use crate::bootstrap::config::AppConfig;
use sqlx::Row;

use crate::platform::backup::create_backup;
use crate::platform::db::{Db, open_pool, validate_config_directory};
use crate::shared::error::{AppError, ErrorCode};

// SQLx embeds this directory at compile time; every release carries its exact forward schema.
// Downloader recovery migrations are likewise compiled into the executable.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// 应用所有待处理的 Core 迁移；对现有 Schema 会先创建已验证备份。
///
/// # Errors
///
/// 若配置卷、数据库、备份或迁移无效，则返回 [`AppError`]。
pub async fn migrate_with_backup(config: &AppConfig) -> Result<Db, AppError> {
    validate_config_directory(&config.config_dir)?;
    crate::platform::secrets::InstanceKey::load_or_create(&config.config_dir)
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
    let path = config.config_dir.join("mediaflow.db");
    let pool = open_pool(&path).await?;
    let applied_versions = applied_migration_versions(&pool).await?;
    let pending = MIGRATOR
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .any(|migration| !applied_versions.contains(&migration.version));
    if pending && has_existing_schema(&pool).await? {
        create_backup(
            &pool,
            &path,
            &config.deployment_roots_file,
            &config.config_dir.join("backups"),
        )
        .await
        .map_err(|error| AppError::new(ErrorCode::BackupFailed, error.to_string()))?;
    }
    MIGRATOR
        .run(&pool)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::DatabaseInvalid, error))?;
    Ok(Db::new(pool, path))
}

async fn applied_migration_versions(
    pool: &sqlx::SqlitePool,
) -> Result<std::collections::BTreeSet<i64>, AppError> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::with_source(ErrorCode::DatabaseInvalid, error))?;
    if exists == 0 {
        return Ok(std::collections::BTreeSet::new());
    }
    sqlx::query("SELECT version FROM _sqlx_migrations WHERE success = 1")
        .fetch_all(pool)
        .await
        .map(|rows| rows.into_iter().map(|row| row.get("version")).collect())
        .map_err(|error| AppError::with_source(ErrorCode::DatabaseInvalid, error))
}

async fn has_existing_schema(pool: &sqlx::SqlitePool) -> Result<bool, AppError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master \
         WHERE type IN ('table', 'index', 'view', 'trigger') \
           AND name NOT LIKE 'sqlite_%' \
           AND name != '_sqlx_migrations'",
    )
    .fetch_one(pool)
    .await
    .map(|count| count > 0)
    .map_err(|error| AppError::with_source(ErrorCode::DatabaseInvalid, error))
}
