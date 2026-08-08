use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use rusqlite::OpenFlags;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::platform::db::{
    open_standalone_read_only_pool, open_wal_aware_read_only_pool, validate_config_directory,
};
use crate::shared::error::{AppError, ErrorCode};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 一组数据库/部署根目录备份的完整性元数据。
///
/// 为保持兼容性，[`create_backup`] 会将 `sha256` 设为数据库摘要。恢复要求它等于
/// `files.database.sha256`，并验证两个文件条目及数据库 Schema 版本。无密钥摘要能检测与此清单
/// 不一致的情形，但不能认证其来源。
pub struct BackupManifest {
    /// 从源数据库记录的 `SQLite` 迁移版本。
    pub schema_version: i64,
    /// 数据库备份的小写 SHA-256 十六进制摘要。
    pub sha256: String,
    /// 组装此清单时的 RFC 3339 UTC 时间戳。
    pub created_at: String,
    /// 必需的数据库和部署根目录文件条目。
    pub files: BackupManifestFiles,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 恢复自包含 Core 配置所需的两个文件。
pub struct BackupManifestFiles {
    /// `SQLite` 在线备份产物。
    pub database: BackupFileEntry,
    /// 与该数据库配套使用的 deployment-roots JSON 持久副本。
    pub deployment_roots: BackupFileEntry,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 相对备份文件名及其小写 SHA-256 十六进制摘要。
///
/// 恢复仅接受单一文件名组件；绝对路径或包含遍历的名称无效。
pub struct BackupFileEntry {
    /// 清单旁边产物的基本文件名。
    pub filename: String,
    /// 完整文件预期的小写十六进制摘要。
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 成功调用 [`create_backup`] 生成的持久路径。
pub struct BackupArtifact {
    /// 已验证的独立 `SQLite` 数据库副本。
    pub database: PathBuf,
    /// 匹配的 deployment-roots JSON 副本。
    pub deployment_roots: PathBuf,
    /// 为两个文件原子发布的 JSON 清单。
    pub manifest: PathBuf,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
/// 成功完成 `SQLite` 完整性验证时返回的报告结构。
///
/// 以下保证适用于 [`verify_database`] 或成功恢复所返回的报告；公开字段允许调用方直接构造其他值。
pub struct VerificationReport {
    /// `PRAGMA integrity_check` 结果；验证成功时设为 `"ok"`。
    pub integrity_check: String,
    /// 外键诊断信息；验证成功时返回空列表。
    pub foreign_key_violations: Vec<String>,
    /// 成功验证期间为每个应用表收集的行数。
    pub critical_counts: BTreeMap<String, i64>,
}

/// 在前向迁移前创建并验证在线 `SQLite` 备份。
///
/// # Errors
///
/// 若 `SQLite` 备份 API、完整性验证、哈希、清单写入或持久化同步失败，则返回 [`AppError`]。
/// 失败时会移除部分备份产物。
pub async fn create_backup(
    pool: &sqlx::SqlitePool,
    source: &Path,
    deployment_roots_source: &Path,
    backups_dir: &Path,
) -> Result<BackupArtifact, AppError> {
    std::fs::create_dir_all(backups_dir).map_err(backup_error)?;
    validate_regular_file(deployment_roots_source).map_err(backup_error)?;
    let schema_version = schema_version(pool).await.map_err(backup_error)?;
    let suffix = format!(
        "{}-{}",
        Utc::now().format("%Y%m%dT%H%M%S%.6fZ"),
        uuid::Uuid::now_v7()
    );
    let basename = format!("mediaflow-v{schema_version}-{suffix}");
    let database = backups_dir.join(format!("{basename}.db"));
    let deployment_roots = backups_dir.join(format!("{basename}.deployment-roots.json"));
    let manifest_path = manifest_path(&database);

    let result = async {
        sqlite_backup(source, &database).await?;
        verify_standalone_database(&database)
            .await
            .map_err(backup_error)?;
        copy_file_durably(deployment_roots_source, &deployment_roots).map_err(backup_error)?;
        let database_digest = sha256_file(&database).map_err(backup_error)?;
        let deployment_roots_digest = sha256_file(&deployment_roots).map_err(backup_error)?;
        let manifest = BackupManifest {
            schema_version,
            sha256: database_digest.clone(),
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
            files: BackupManifestFiles {
                database: BackupFileEntry {
                    filename: filename(&database).map_err(backup_error)?,
                    sha256: database_digest,
                },
                deployment_roots: BackupFileEntry {
                    filename: filename(&deployment_roots).map_err(backup_error)?,
                    sha256: deployment_roots_digest,
                },
            },
        };
        write_manifest_atomically(&manifest_path, &manifest).map_err(backup_error)?;
        sync_directory(backups_dir).map_err(backup_error)?;
        Ok(BackupArtifact {
            database: database.clone(),
            deployment_roots: deployment_roots.clone(),
            manifest: manifest_path.clone(),
        })
    }
    .await;

    if result.is_err() {
        let _ = std::fs::remove_file(&manifest_path);
        let _ = std::fs::remove_file(&deployment_roots);
        let _ = std::fs::remove_file(&database);
    }
    result
}

/// 对现有数据库执行 `SQLite` 完整性、外键和表计数验证。
///
/// # Errors
///
/// 当文件无法以只读方式打开，或任一验证检查失败时返回 [`AppError`]。
pub async fn verify_database(database: &Path) -> Result<VerificationReport, AppError> {
    verify_database_with_pool(open_wal_aware_read_only_pool(database).await?).await
}

async fn verify_standalone_database(database: &Path) -> Result<VerificationReport, AppError> {
    verify_database_with_pool(open_standalone_read_only_pool(database).await?).await
}

async fn verify_database_with_pool(pool: sqlx::SqlitePool) -> Result<VerificationReport, AppError> {
    let integrity_rows = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
        .fetch_all(&pool)
        .await
        .map_err(database_error)?;
    if integrity_rows.len() != 1 || integrity_rows[0] != "ok" {
        pool.close().await;
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "SQLite integrity_check failed",
        ));
    }

    let foreign_key_rows = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .map_err(database_error)?;
    let foreign_key_violations = foreign_key_rows
        .iter()
        .map(|row| {
            let table: String = row.get(0);
            let row_id: Option<i64> = row.get(1);
            let parent: String = row.get(2);
            let foreign_key: i64 = row.get(3);
            format!("{table}:{row_id:?}:{parent}:{foreign_key}")
        })
        .collect::<Vec<_>>();
    if !foreign_key_violations.is_empty() {
        pool.close().await;
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "SQLite foreign_key_check failed",
        ));
    }

    let table_names = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations' \
         ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .map_err(database_error)?;
    let mut critical_counts = BTreeMap::new();
    for table in table_names {
        let quoted = table.replace('"', "\"\"");
        let count = sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM \"{quoted}\""))
            .fetch_one(&pool)
            .await
            .map_err(database_error)?;
        critical_counts.insert(table, count);
    }
    pool.close().await;

    Ok(VerificationReport {
        integrity_check: "ok".to_owned(),
        foreign_key_violations,
        critical_counts,
    })
}

/// 将已验证备份恢复到新的空配置目录中。
///
/// # Errors
///
/// 若目标非空、清单/哈希无效、复制失败或复制后的数据库验证失败，则返回 [`AppError`]。
pub async fn restore_backup(
    backup: &Path,
    config_dir: &Path,
) -> Result<VerificationReport, AppError> {
    restore_backup_with_failure(backup, config_dir, RestoreFailurePoint::None).await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestoreFailurePoint {
    None,
    AfterDatabaseCopy,
    BeforeStagedValidation,
    AfterDatabasePublish,
}

async fn restore_backup_with_failure(
    backup: &Path,
    config_dir: &Path,
    failure_point: RestoreFailurePoint,
) -> Result<VerificationReport, AppError> {
    ensure_empty_restore_target(config_dir)?;
    validate_config_directory(config_dir)?;

    let (manifest, deployment_roots_backup) = verify_manifest(backup)?;
    let pool = open_standalone_read_only_pool(backup).await?;
    let actual_schema_version = schema_version(&pool).await?;
    pool.close().await;
    if manifest.schema_version != actual_schema_version {
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "backup schema version does not match its manifest",
        ));
    }
    let source_verification = verify_standalone_database(backup).await?;

    let restore_id = uuid::Uuid::now_v7();
    let staged_database = config_dir.join(format!(".mediaflow-restore-{restore_id}.db.tmp"));
    let staged_deployment_roots = config_dir.join(format!(
        ".mediaflow-restore-{restore_id}.deployment-roots.json.tmp"
    ));
    let target_database = config_dir.join("mediaflow.db");
    let target_deployment_roots = config_dir.join("deployment-roots.json");
    let mut guard = RestoreGuard::new([staged_database.clone(), staged_deployment_roots.clone()]);

    copy_file_durably(backup, &staged_database)?;
    inject_restore_failure(
        failure_point,
        RestoreFailurePoint::AfterDatabaseCopy,
        "injected restore copy failure",
    )?;
    copy_file_durably(&deployment_roots_backup, &staged_deployment_roots)?;
    inject_restore_failure(
        failure_point,
        RestoreFailurePoint::BeforeStagedValidation,
        "injected restore validation failure",
    )?;

    verify_hash(&staged_database, &manifest.files.database.sha256)?;
    verify_hash(
        &staged_deployment_roots,
        &manifest.files.deployment_roots.sha256,
    )?;
    let staged_pool = open_standalone_read_only_pool(&staged_database).await?;
    let staged_schema_version = schema_version(&staged_pool).await?;
    staged_pool.close().await;
    if staged_schema_version != manifest.schema_version {
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "staged database schema version does not match its manifest",
        ));
    }
    let staged_verification = verify_standalone_database(&staged_database).await?;
    if staged_verification != source_verification {
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "staged database verification differs from backup",
        ));
    }

    std::fs::hard_link(&staged_database, &target_database).map_err(database_error)?;
    guard.record_published(target_database);
    std::fs::remove_file(&staged_database).map_err(database_error)?;
    inject_restore_failure(
        failure_point,
        RestoreFailurePoint::AfterDatabasePublish,
        "injected restore publish failure",
    )?;
    std::fs::hard_link(&staged_deployment_roots, &target_deployment_roots)
        .map_err(database_error)?;
    guard.record_published(target_deployment_roots);
    std::fs::remove_file(&staged_deployment_roots).map_err(database_error)?;
    sync_directory(config_dir).map_err(database_error)?;
    guard.commit();
    Ok(staged_verification)
}

struct RestoreGuard {
    temporary: Vec<PathBuf>,
    published: Vec<PathBuf>,
    committed: bool,
}

impl RestoreGuard {
    fn new(temporary: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            temporary: temporary.into_iter().collect(),
            published: Vec::new(),
            committed: false,
        }
    }

    fn record_published(&mut self, path: PathBuf) {
        self.published.push(path);
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for path in self.published.iter().rev().chain(self.temporary.iter()) {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn inject_restore_failure(
    actual: RestoreFailurePoint,
    expected: RestoreFailurePoint,
    message: &'static str,
) -> Result<(), AppError> {
    if actual == expected {
        return Err(AppError::new(ErrorCode::DatabaseInvalid, message));
    }
    Ok(())
}

fn ensure_empty_restore_target(config_dir: &Path) -> Result<(), AppError> {
    if config_dir.exists() {
        if !config_dir.is_dir() {
            return Err(AppError::new(
                ErrorCode::RestoreTargetNotEmpty,
                "restore target is not a directory",
            ));
        }
        let mut entries = std::fs::read_dir(config_dir).map_err(database_error)?;
        if entries
            .next()
            .transpose()
            .map_err(database_error)?
            .is_some()
        {
            return Err(AppError::new(
                ErrorCode::RestoreTargetNotEmpty,
                "restore target contains files",
            ));
        }
    } else {
        std::fs::create_dir(config_dir).map_err(database_error)?;
    }
    Ok(())
}

fn verify_manifest(backup: &Path) -> Result<(BackupManifest, PathBuf), AppError> {
    let manifest_bytes = std::fs::read(manifest_path(backup)).map_err(database_error)?;
    let manifest: BackupManifest =
        serde_json::from_slice(&manifest_bytes).map_err(database_error)?;
    chrono::DateTime::parse_from_rfc3339(&manifest.created_at).map_err(database_error)?;
    let backup_filename = filename(backup)?;
    validate_manifest_filename(&manifest.files.database.filename)?;
    validate_paired_manifest_filenames(
        &backup_filename,
        &manifest.files.deployment_roots.filename,
    )?;
    if manifest.files.database.filename != backup_filename
        || !constant_time_equal(
            manifest.sha256.as_bytes(),
            manifest.files.database.sha256.as_bytes(),
        )
    {
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "backup database entry does not match its manifest",
        ));
    }
    verify_hash(backup, &manifest.files.database.sha256)?;
    let parent = backup
        .parent()
        .ok_or_else(|| AppError::new(ErrorCode::DatabaseInvalid, "invalid backup path"))?;
    let deployment_roots = parent.join(&manifest.files.deployment_roots.filename);
    validate_regular_file(&deployment_roots)?;
    verify_hash(&deployment_roots, &manifest.files.deployment_roots.sha256)?;
    Ok((manifest, deployment_roots))
}

fn validate_manifest_filename(value: &str) -> Result<(), AppError> {
    let path = Path::new(value);
    let mut components = path.components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "backup manifest contains an invalid filename",
        ));
    }
    Ok(())
}

fn validate_paired_manifest_filenames(
    database_filename: &str,
    deployment_roots_filename: &str,
) -> Result<(), AppError> {
    validate_manifest_filename(database_filename)?;
    validate_manifest_filename(deployment_roots_filename)?;
    let basename = database_filename
        .strip_suffix(".db")
        .filter(|value| !value.is_empty());
    let Some(basename) = basename else {
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "backup database has an invalid filename",
        ));
    };
    let expected = format!("{basename}.deployment-roots.json");
    if deployment_roots_filename != expected || deployment_roots_filename == database_filename {
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "backup configuration filename does not match its database",
        ));
    }
    Ok(())
}

fn verify_hash(path: &Path, expected: &str) -> Result<(), AppError> {
    let actual = sha256_file(path).map_err(database_error)?;
    if !constant_time_equal(expected.as_bytes(), actual.as_bytes()) {
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "backup digest does not match its manifest",
        ));
    }
    Ok(())
}

fn manifest_path(backup: &Path) -> PathBuf {
    backup.with_extension("manifest.json")
}

fn validate_regular_file(path: &Path) -> Result<(), AppError> {
    let metadata = std::fs::symlink_metadata(path).map_err(database_error)?;
    if !metadata.file_type().is_file() {
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "backup source is not a regular file",
        ));
    }
    std::fs::File::open(path).map_err(database_error)?;
    Ok(())
}

fn filename(path: &Path) -> Result<String, AppError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::new(ErrorCode::DatabaseInvalid, "invalid backup filename"))
}

fn copy_file_durably(source: &Path, target: &Path) -> Result<(), AppError> {
    let mut source = std::fs::File::open(source).map_err(database_error)?;
    let mut destination = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(database_error)?;
    std::io::copy(&mut source, &mut destination).map_err(database_error)?;
    destination.sync_all().map_err(database_error)
}

async fn schema_version(pool: &sqlx::SqlitePool) -> Result<i64, AppError> {
    let user_version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
        .fetch_one(pool)
        .await
        .map_err(database_error)?;
    let migration_table = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(pool)
    .await
    .map_err(database_error)?;
    if migration_table == 0 {
        return Ok(user_version);
    }
    let migration_version = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(version) FROM _sqlx_migrations WHERE success = 1",
    )
    .fetch_one(pool)
    .await
    .map_err(database_error)?
    .unwrap_or(0);
    Ok(user_version.max(migration_version))
}

async fn sqlite_backup(source: &Path, destination: &Path) -> Result<(), AppError> {
    let source = source.to_owned();
    let destination = destination.to_owned();
    tokio::task::spawn_blocking(move || {
        let source = rusqlite::Connection::open_with_flags(
            &source,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(backup_error)?;
        let mut destination_connection = rusqlite::Connection::open_with_flags(
            &destination,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(backup_error)?;
        let backup = rusqlite::backup::Backup::new(&source, &mut destination_connection)
            .map_err(backup_error)?;
        backup
            .run_to_completion(128, Duration::from_millis(10), None)
            .map_err(backup_error)?;
        drop(backup);
        destination_connection
            .close()
            .map_err(|(_, error)| backup_error(error))?;
        std::fs::File::open(&destination)
            .and_then(|file| file.sync_all())
            .map_err(backup_error)
    })
    .await
    .map_err(backup_error)?
}

fn write_manifest_atomically(path: &Path, manifest: &BackupManifest) -> Result<(), AppError> {
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::now_v7()));
    let bytes = serde_json::to_vec_pretty(manifest).map_err(backup_error)?;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(backup_error)?;
        file.write_all(&bytes).map_err(backup_error)?;
        file.write_all(b"\n").map_err(backup_error)?;
        file.sync_all().map_err(backup_error)?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(backup_error)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn sha256_file(path: &Path) -> Result<String, AppError> {
    let mut file = std::fs::File::open(path).map_err(database_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer).map_err(database_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

fn backup_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::BackupFailed, error.to_string())
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::DatabaseInvalid, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::db::open_pool;

    async fn backup_fixture() -> (tempfile::TempDir, BackupArtifact) {
        let root = tempfile::tempdir().expect("temporary backup fixture");
        let database = root.path().join("source.db");
        let pool = open_pool(&database).await.expect("source pool");
        sqlx::query("PRAGMA user_version = 7")
            .execute(&pool)
            .await
            .expect("schema version");
        sqlx::query("CREATE TABLE sentinel (value TEXT NOT NULL)")
            .execute(&pool)
            .await
            .expect("sentinel table");
        sqlx::query("INSERT INTO sentinel (value) VALUES ('preserve me')")
            .execute(&pool)
            .await
            .expect("sentinel row");
        let deployment_roots = root.path().join("source-deployment-roots.json");
        std::fs::write(&deployment_roots, b"[]\n").expect("deployment roots");
        let artifact = create_backup(
            &pool,
            &database,
            &deployment_roots,
            &root.path().join("backups"),
        )
        .await
        .expect("backup fixture");
        pool.close().await;
        (root, artifact)
    }

    fn assert_restore_target_empty(target: &Path) {
        assert!(target.is_dir(), "restore target should remain a directory");
        let entries = std::fs::read_dir(target)
            .expect("restore target entries")
            .collect::<Result<Vec<_>, _>>()
            .expect("read restore target");
        assert!(
            entries.is_empty(),
            "failed restore must remove every artifact"
        );
    }

    #[test]
    fn deployment_roots_manifest_filename_must_match_database_basename() {
        assert!(
            validate_paired_manifest_filenames(
                "mediaflow-v7-snapshot.db",
                "mediaflow-v7-snapshot.deployment-roots.json",
            )
            .is_ok()
        );
        for invalid in [
            "mediaflow-v7-snapshot.db",
            "unrelated.deployment-roots.json",
            "../mediaflow-v7-snapshot.deployment-roots.json",
            "/tmp/mediaflow-v7-snapshot.deployment-roots.json",
        ] {
            assert!(
                validate_paired_manifest_filenames("mediaflow-v7-snapshot.db", invalid).is_err(),
                "accepted invalid paired filename: {invalid}"
            );
        }
    }

    #[tokio::test]
    async fn restore_copy_failure_removes_staged_files() {
        let (root, artifact) = backup_fixture().await;
        let target = root.path().join("restore-copy-failure");

        restore_backup_with_failure(
            &artifact.database,
            &target,
            RestoreFailurePoint::AfterDatabaseCopy,
        )
        .await
        .expect_err("injected copy failure");

        assert_restore_target_empty(&target);
    }

    #[tokio::test]
    async fn restore_validation_failure_removes_staged_files() {
        let (root, artifact) = backup_fixture().await;
        let target = root.path().join("restore-validation-failure");

        restore_backup_with_failure(
            &artifact.database,
            &target,
            RestoreFailurePoint::BeforeStagedValidation,
        )
        .await
        .expect_err("injected validation failure");

        assert_restore_target_empty(&target);
    }

    #[tokio::test]
    async fn restore_second_publish_failure_removes_half_published_group() {
        let (root, artifact) = backup_fixture().await;
        let target = root.path().join("restore-publish-failure");

        restore_backup_with_failure(
            &artifact.database,
            &target,
            RestoreFailurePoint::AfterDatabasePublish,
        )
        .await
        .expect_err("injected second publish failure");

        assert_restore_target_empty(&target);
    }
}
