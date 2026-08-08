use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::shared::error::{AppError, ErrorCode};

#[derive(Clone, Debug)]
/// 已打开的 Core 数据库连接池，以及用于备份和诊断的路径。
///
/// 克隆实例共享相同的 `SQLx` 连接池，并保留相同的数据库路径。
pub struct Db {
    pool: SqlitePool,
    path: PathBuf,
}

impl Db {
    #[must_use]
    /// 将已打开的连接池与其后备数据库路径关联。
    pub fn new(pool: SqlitePool, path: PathBuf) -> Self {
        Self { pool, path }
    }

    #[must_use]
    /// 借用共享的 `SQLite` 连接池。
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    #[must_use]
    /// 借用创建此句柄时使用的数据库文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// 使用 `MediaFlow` 的持久性和并发设置打开可写 `SQLite` 连接池。
///
/// # Errors
///
/// 若路径无法表示为 `SQLite` 选项或无法打开，则返回 [`AppError`]。
pub async fn open_pool(path: &Path) -> Result<SqlitePool, AppError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .synchronous(SqliteSynchronous::Full);
    SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::DatabaseInvalid, error))
}

pub(crate) async fn open_wal_aware_read_only_pool(path: &Path) -> Result<SqlitePool, AppError> {
    if !path.is_file() {
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "database file does not exist",
        ));
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::DatabaseInvalid, error))
}

pub(crate) async fn open_standalone_read_only_pool(path: &Path) -> Result<SqlitePool, AppError> {
    if !path.is_file() {
        return Err(AppError::new(
            ErrorCode::DatabaseInvalid,
            "database file does not exist",
        ));
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .immutable(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::DatabaseInvalid, error))
}

/// 验证配置目录可写且位于受支持的本地文件系统上。
///
/// # Errors
///
/// 若元数据读取、文件系统检测、持久探针创建或清理失败，则返回 [`AppError`]。
pub fn validate_config_directory(path: &Path) -> Result<(), AppError> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
    if !metadata.is_dir() {
        return Err(AppError::new(
            ErrorCode::ConfigInvalid,
            "config path is not a directory",
        ));
    }
    if is_known_incompatible_filesystem(path)? {
        return Err(AppError::new(
            ErrorCode::ConfigInvalid,
            "config directory uses an unsupported network filesystem",
        ));
    }

    let probe = path.join(format!(".mediaflow-write-probe-{}", uuid::Uuid::now_v7()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let file = options
        .open(&probe)
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
    file.sync_all()
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
    drop(file);
    std::fs::remove_file(&probe)
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn is_known_incompatible_filesystem(path: &Path) -> Result<bool, AppError> {
    let filesystem_type = rustix::fs::statfs(path)
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?
        .f_type as u64;
    Ok(is_incompatible_filesystem_magic(filesystem_type))
}

#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps)]
fn is_known_incompatible_filesystem(_path: &Path) -> Result<bool, AppError> {
    Ok(false)
}

#[must_use]
/// 返回 Linux 文件系统魔数是否标识 NFS、CIFS 或 SMB。
///
/// 这些网络文件系统不能作为可写的 `SQLite` 配置卷。
pub const fn is_incompatible_filesystem_magic(magic: u64) -> bool {
    const NFS_SUPER_MAGIC: u64 = 0x6969;
    const CIFS_SUPER_MAGIC: u64 = 0xFF53_4D42;
    const SMB_SUPER_MAGIC: u64 = 0x517B;
    matches!(magic, NFS_SUPER_MAGIC | CIFS_SUPER_MAGIC | SMB_SUPER_MAGIC)
}

#[cfg(test)]
mod tests {
    use super::is_incompatible_filesystem_magic;

    #[test]
    fn linux_network_filesystem_magics_are_rejected_and_local_is_allowed() {
        assert!(is_incompatible_filesystem_magic(0x6969));
        assert!(is_incompatible_filesystem_magic(0xFF53_4D42));
        assert!(is_incompatible_filesystem_magic(0x517B));
        assert!(!is_incompatible_filesystem_magic(0xEF53));
    }
}
