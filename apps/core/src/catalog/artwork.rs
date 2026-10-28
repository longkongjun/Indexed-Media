use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rustix::fs::{AtFlags, FileType, FlockOperation, Mode, OFlags};
use sha2::{Digest as _, Sha256};
use sqlx::{Row, SqlitePool};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::connectors::model::ProviderMediaKind;
use crate::connectors::service::ConnectorService;
use crate::connectors::tmdb::{PosterImage, TmdbClient};
use crate::shared::error::{AppError, ErrorCode};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const FRESH_FOR: Duration = Duration::from_hours(24);
const CACHE_BUDGET: CacheBudget = CacheBudget {
    entries: 512,
    bytes: 128 * 1024 * 1024,
};
// shortcut: old confirmed identities occupy the fixed quota; add LRU only if cache hit rates require it.

#[derive(Clone, Copy)]
struct CacheBudget {
    entries: u64,
    bytes: u64,
}

/// 只从正式媒体的已确认身份解析图片，缓存不修改已核验的 Catalog 资产事实。
#[derive(Clone)]
pub struct ArtworkService {
    pool: SqlitePool,
    directory: PathBuf,
    client: TmdbClient,
    connectors: ConnectorService,
    permits: Arc<Semaphore>,
}

impl ArtworkService {
    /// 为生产或明确 loopback 测试客户端绑定派生缓存。
    #[must_use]
    pub fn new(pool: SqlitePool, directory: PathBuf, client: TmdbClient) -> Self {
        Self {
            connectors: ConnectorService::new(pool.clone(), &directory, Arc::new(client.clone())),
            pool,
            directory,
            client,
            permits: Arc::new(Semaphore::new(4)),
        }
    }

    /// 为当前账户的正式媒体读取或获取海报。
    ///
    /// # Errors
    /// 资源不存在/无已确认身份时返回404；存储、提供方或图片校验失败保持局部错误。
    pub async fn poster(&self, account: Uuid, media: Uuid) -> Result<PosterImage, AppError> {
        let row = sqlx::query(
            "SELECT candidate.id,candidate.media_type,candidate.provider_id,candidate.locale
             FROM catalog_media_items item
             JOIN catalog_metadata_values metadata ON metadata.media_item_id=item.id
               AND metadata.field='title' AND metadata.source_type='tmdb'
             JOIN identification_candidates candidate
               ON hex(candidate.id)=upper(replace(metadata.source_id,'-',''))
             WHERE item.id=? AND item.account_id=?",
        )
        .bind(media.as_bytes().as_slice())
        .bind(account.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "media poster identity missing"))?;
        let kind = match row.get::<&str, _>("media_type") {
            "movie" => ProviderMediaKind::Movie,
            "tv" => ProviderMediaKind::Tv,
            _ => {
                return Err(AppError::new(
                    ErrorCode::DatabaseInvalid,
                    "invalid artwork identity",
                ));
            }
        };
        let provider_id: i64 = row.get("provider_id");
        let locale: String = row.get("locale");
        let mut digest = Sha256::new();
        digest.update(b"mediaflow.poster.w500.v1\0");
        digest.update(account.as_bytes());
        digest.update(media.as_bytes());
        digest.update(row.get::<&[u8], _>("id"));
        digest.update(provider_id.to_be_bytes());
        digest.update(locale.as_bytes());
        let key = hex::encode(digest.finalize());
        let directory = self.directory.clone();
        let cache_key = key.clone();
        let cached = tokio::task::spawn_blocking(move || read_cache(&directory, &cache_key))
            .await
            .map_err(storage)??;
        if let Some((image, true)) = cached {
            return Ok(image);
        }
        let stale = cached.map(|(image, _)| image);
        let _permit = self.permits.acquire().await.map_err(storage)?;
        let supplied = async {
            let (token, _, _) = self.connectors.load_tmdb_credential().await?;
            self.client
                .poster(&token, kind, provider_id, &locale)
                .await
                .map_err(|error| AppError::with_source(ErrorCode::ProviderUnavailable, error))
        }
        .await;
        let image = match supplied {
            Ok(Some(image)) => image,
            Ok(None) => return Err(AppError::new(ErrorCode::NotFound, "poster not available")),
            Err(error) => return stale.ok_or(error),
        };
        let directory = self.directory.clone();
        let bytes = image.bytes.clone();
        tokio::task::spawn_blocking(move || write_cache(&directory, &key, &bytes))
            .await
            .map_err(storage)??;
        Ok(image)
    }
}

fn cache_directory(path: &Path) -> Result<rustix::fd::OwnedFd, AppError> {
    let root = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(storage)?;
    match rustix::fs::mkdirat(&root, "artwork-cache", Mode::RUSR | Mode::WUSR | Mode::XUSR) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(error) => return Err(storage(error)),
    }
    let directory = rustix::fs::openat(
        &root,
        "artwork-cache",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(storage)?;
    let metadata = rustix::fs::fstat(&directory).map_err(storage)?;
    if metadata.st_mode & 0o077 != 0 {
        return Err(storage("artwork cache is not private"));
    }
    Ok(directory)
}

fn read_cache(path: &Path, key: &str) -> Result<Option<(PosterImage, bool)>, AppError> {
    let directory = cache_directory(path)?;
    let fd = match rustix::fs::openat(
        &directory,
        format!("{key}.image"),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(storage(error)),
    };
    let file = File::from(fd);
    let metadata = file.metadata().map_err(storage)?;
    if !metadata.is_file()
        || metadata.len() > MAX_BYTES
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err(storage("invalid artwork cache file"));
    }
    let fresh = metadata
        .modified()
        .ok()
        .and_then(|time| SystemTime::now().duration_since(time).ok())
        .is_some_and(|age| age < FRESH_FOR);
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(storage)?;
    let image = PosterImage::from_bytes(bytes).map_err(storage)?;
    Ok(Some((image, fresh)))
}

fn write_cache(path: &Path, key: &str, bytes: &[u8]) -> Result<(), AppError> {
    write_cache_with_budget(path, key, bytes, CACHE_BUDGET).map(|_| ())
}

fn cache_has_room(
    directory: &rustix::fd::OwnedFd,
    target: &str,
    incoming_bytes: u64,
    budget: CacheBudget,
) -> Result<bool, AppError> {
    let mut entries = 0_u64;
    let mut total_bytes = 0_u64;
    let mut replaced_bytes = None;
    for entry in rustix::fs::Dir::read_from(directory).map_err(storage)? {
        let entry = entry.map_err(storage)?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let metadata =
            rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(storage)?;
        // Unknown special entries cannot safely contribute to a regular-file cache budget.
        if !FileType::from_raw_mode(metadata.st_mode).is_file() || metadata.st_nlink != 1 {
            return Ok(false);
        }
        let size = u64::try_from(metadata.st_size).map_err(storage)?;
        entries = entries.saturating_add(1);
        total_bytes = total_bytes.saturating_add(size);
        if entries > budget.entries {
            return Ok(false);
        }
        if name.to_bytes() == target.as_bytes() {
            if metadata.st_mode & 0o077 != 0 || size > MAX_BYTES {
                return Err(storage("invalid artwork cache replacement"));
            }
            replaced_bytes = Some(size);
        }
    }
    let next_entries = entries.saturating_add(u64::from(replaced_bytes.is_none()));
    let next_bytes = total_bytes
        .saturating_sub(replaced_bytes.unwrap_or_default())
        .saturating_add(incoming_bytes);
    Ok(next_entries <= budget.entries && next_bytes <= budget.bytes)
}

fn write_cache_with_budget(
    path: &Path,
    key: &str,
    bytes: &[u8],
    budget: CacheBudget,
) -> Result<bool, AppError> {
    let directory = cache_directory(path)?;
    // Hold the directory lock through metadata accounting and rename, including other service instances.
    rustix::fs::flock(&directory, FlockOperation::LockExclusive).map_err(storage)?;
    let target = format!("{key}.image");
    let incoming_bytes = u64::try_from(bytes.len()).map_err(storage)?;
    if incoming_bytes > MAX_BYTES || !cache_has_room(&directory, &target, incoming_bytes, budget)? {
        return Ok(false);
    }
    let temporary = format!(".{key}-{}.tmp", Uuid::now_v7());
    let fd = rustix::fs::openat(
        &directory,
        &temporary,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(storage)?;
    let result = (|| {
        let mut file = File::from(fd);
        file.write_all(bytes).map_err(storage)?;
        file.sync_all().map_err(storage)?;
        rustix::fs::renameat(&directory, &temporary, &directory, &target).map_err(storage)?;
        rustix::fs::fsync(directory.as_fd()).map_err(storage)?;
        Ok(true)
    })();
    if result.is_err() {
        let _ = rustix::fs::unlinkat(&directory, &temporary, rustix::fs::AtFlags::empty());
    }
    result
}

fn storage(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}
fn database(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::DatabaseInvalid, error)
}

#[cfg(test)]
mod cache_budget_tests {
    use super::{CacheBudget, write_cache_with_budget};

    fn key(index: u8) -> String {
        format!("{index:064x}")
    }

    fn write(path: &std::path::Path, index: u8, bytes: &[u8], budget: CacheBudget) -> bool {
        write_cache_with_budget(path, &key(index), bytes, budget).unwrap()
    }

    #[test]
    fn cache_budget_refuses_new_entries_and_byte_growth() {
        let entry_limit = tempfile::tempdir().unwrap();
        let budget = CacheBudget {
            entries: 2,
            bytes: 16,
        };
        assert!(write(entry_limit.path(), 1, b"1234", budget));
        assert!(write(entry_limit.path(), 2, b"5678", budget));
        assert!(!write(entry_limit.path(), 3, b"9", budget));
        assert_eq!(
            std::fs::read_dir(entry_limit.path().join("artwork-cache"))
                .unwrap()
                .count(),
            2
        );
        let byte_limit = tempfile::tempdir().unwrap();
        let budget = CacheBudget {
            entries: 4,
            bytes: 8,
        };
        assert!(write(byte_limit.path(), 1, b"1234", budget));
        assert!(write(byte_limit.path(), 2, b"5678", budget));
        assert!(!write(byte_limit.path(), 3, b"9", budget));
    }

    #[test]
    fn cache_budget_replacement_counts_net_bytes_and_preserves_rejected_target() {
        let fixture = tempfile::tempdir().unwrap();
        let budget = CacheBudget {
            entries: 2,
            bytes: 8,
        };
        assert!(write(fixture.path(), 1, b"1234", budget));
        assert!(write(fixture.path(), 2, b"5678", budget));
        assert!(write(fixture.path(), 1, b"abcd", budget));
        assert!(!write(fixture.path(), 1, b"abcde", budget));
        let file = fixture
            .path()
            .join("artwork-cache")
            .join(format!("{}.image", key(1)));
        assert_eq!(std::fs::read(&file).unwrap(), b"abcd");
        assert!(write(fixture.path(), 1, b"ab", budget));
        assert!(write(fixture.path(), 2, b"123456", budget));
        assert_eq!(
            std::fs::read_dir(fixture.path().join("artwork-cache"))
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn cache_budget_serializes_independent_writers_before_reserving_space() {
        let fixture = tempfile::tempdir().unwrap();
        let budget = CacheBudget {
            entries: 2,
            bytes: 8,
        };
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
        let writers = (0..12)
            .map(|index| {
                let barrier = barrier.clone();
                let path = fixture.path().to_owned();
                std::thread::spawn(move || {
                    barrier.wait();
                    write(&path, index, b"1234", budget)
                })
            })
            .collect::<Vec<_>>();
        let stored = writers
            .into_iter()
            .map(|writer| usize::from(writer.join().unwrap()))
            .sum::<usize>();
        assert_eq!(stored, 2);
        assert_eq!(
            std::fs::read_dir(fixture.path().join("artwork-cache"))
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn cache_budget_preserves_foreign_files_and_never_follows_links() {
        let fixture = tempfile::tempdir().unwrap();
        let budget = CacheBudget {
            entries: 2,
            bytes: 8,
        };
        assert!(write(fixture.path(), 1, b"1234", budget));
        let directory = fixture.path().join("artwork-cache");
        let foreign = directory.join("keep.txt");
        std::fs::write(&foreign, b"keep").unwrap();
        assert!(!write(fixture.path(), 2, b"x", budget));
        assert_eq!(std::fs::read(&foreign).unwrap(), b"keep");
        std::fs::remove_file(&foreign).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("private");
        std::fs::write(&sentinel, b"private").unwrap();
        let link = directory.join("foreign-link");
        std::os::unix::fs::symlink(&sentinel, &link).unwrap();
        assert!(!write(
            fixture.path(),
            2,
            b"x",
            CacheBudget {
                entries: 10,
                bytes: 100
            }
        ));
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"private");
    }
}
