use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::{QueryBuilder, Row as _, Sqlite, SqlitePool};
use uuid::Uuid;

use crate::platform::outbox::{OutboxNotifier, OutboxWriter};
use crate::platform::secrets::{
    InstanceKey, IntegrationKind, SealedSecret, SecretAad, SecretBytes, SecretCipher as _,
};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, PageRequest};
use crate::tasks::events::{DOWNLOAD_TASK_CHANGED, DownloadTaskChangedPayload};

use super::model::{
    CreateDownloadTaskCommand, DownloadTaskFilter, DownloadTaskLease, DownloadTaskStatus,
    DownloadTaskView, DownloaderFailureCode, TASK_SOURCE_SCHEMA_VERSION,
};
use super::port::RemoteDownloadStatus;

const TASK_COLUMNS: &str =
    "id,connection_id,connection_display_name,display_name,status,remote_status,
     progress_basis_points,failure_code,retry_at_us,linked,projection_version,
     created_at_us,updated_at_us";
const LEASE_DURATION_US: i64 = 30_000_000;

#[derive(Deserialize, Serialize)]
struct TaskCursor {
    version: u8,
    filter_digest: String,
    updated_at_us: i64,
    id: Uuid,
}

#[derive(Deserialize, Serialize)]
struct CursorEnvelope {
    payload: String,
    checksum: String,
}

#[derive(Clone)]
/// 下载任务源、幂等投影、worker 租约和稳定分页的 `SQLite` 边界。
pub struct DownloadTaskStore {
    pool: SqlitePool,
    key: Arc<InstanceKey>,
    notifier: OutboxNotifier,
}

impl DownloadTaskStore {
    /// 打开任务 store 并加载当前实例的认证加密密钥。
    ///
    /// # Errors
    ///
    /// 实例密钥不满足文件安全边界时返回配置错误。
    pub fn open(pool: SqlitePool, config_dir: &Path) -> Result<Self, AppError> {
        Self::open_with_notifier(pool, config_dir, OutboxNotifier::new())
    }

    /// 使用共享 outbox 通知器打开任务 store。
    ///
    /// # Errors
    ///
    /// 实例密钥不满足文件安全边界时返回配置错误。
    pub fn open_with_notifier(
        pool: SqlitePool,
        config_dir: &Path,
        notifier: OutboxNotifier,
    ) -> Result<Self, AppError> {
        let key = InstanceKey::load_or_create(config_dir)
            .map(Arc::new)
            .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
        Ok(Self {
            pool,
            key,
            notifier,
        })
    }

    /// 验收、加密并幂等创建一项排队任务。
    ///
    /// 相同幂等键与完全相同的规范请求返回原任务；键被不同请求占用时返回冲突。
    ///
    /// # Errors
    ///
    /// 输入、连接、密钥、幂等关系或数据库无效时返回稳定应用错误。
    pub async fn accept(
        &self,
        command: CreateDownloadTaskCommand,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<DownloadTaskView, AppError> {
        validate_idempotency_key(idempotency_key)?;
        let command = command.validate()?;
        let source_digest = digest(&[b"mediaflow.download-source.v1\0", command.source.expose()]);
        let request_digest = digest(&[
            b"mediaflow.download-request.v1\0",
            command.connection_id.as_bytes(),
            &source_digest,
            command.display_name.as_bytes(),
        ]);
        let key_digest = digest(&[
            b"mediaflow.download-idempotency.v1\0",
            idempotency_key.as_bytes(),
        ]);
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        if let Some((id, persisted_request)) = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
            "SELECT id,request_sha256 FROM download_tasks WHERE idempotency_key_sha256=?",
        )
        .bind(key_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        {
            if persisted_request != request_digest {
                return Err(request_conflict());
            }
            let id = Uuid::from_slice(&id).map_err(invalid_database)?;
            tx.commit().await.map_err(database_error)?;
            return self
                .get(id)
                .await?
                .ok_or_else(|| invalid_database("idempotent download task is missing"));
        }
        let connection_display_name = sqlx::query_scalar::<_, String>(
            "SELECT display_name FROM downloader_connections WHERE id=?",
        )
        .bind(command.connection_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "downloader connection not found"))?;
        let id = Uuid::now_v7();
        let sealed = self
            .key
            .seal(
                &task_source_aad(id, TASK_SOURCE_SCHEMA_VERSION),
                command.source.expose(),
            )
            .map_err(secret_error)?;
        sqlx::query(
            "INSERT INTO download_tasks
             (id,connection_id,connection_display_name,display_name,source_sha256,
              source_schema_version,source_nonce,source_ciphertext,idempotency_key_sha256,
              request_sha256,status,remote_id,remote_status,progress_basis_points,
              failure_code,retry_at_us,lease_owner,lease_expires_at_us,attempt_count,
              linked,projection_version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,?,?,?,?,'queued',NULL,NULL,0,NULL,NULL,NULL,NULL,0,0,1,?,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(command.connection_id.as_bytes().as_slice())
        .bind(connection_display_name)
        .bind(command.display_name)
        .bind(source_digest.as_slice())
        .bind(i64::from(sealed.schema_version()))
        .bind(sealed.nonce().as_slice())
        .bind(sealed.ciphertext())
        .bind(key_digest.as_slice())
        .bind(request_digest.as_slice())
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if is_unique(&error) {
                request_conflict()
            } else {
                database_error(error)
            }
        })?;
        write_task_event(&mut tx, id, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        self.get(id)
            .await?
            .ok_or_else(|| invalid_database("accepted download task is missing"))
    }

    /// 按 ID 返回一项脱敏任务投影。
    ///
    /// # Errors
    ///
    /// 数据库或持久行无效时返回内部错误。
    pub async fn get(&self, id: Uuid) -> Result<Option<DownloadTaskView>, AppError> {
        let query = format!("SELECT {TASK_COLUMNS} FROM download_tasks WHERE id=?");
        sqlx::query(&query)
            .bind(id.as_bytes().as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(database_error)?
            .map(|row| decode_task(&row))
            .transpose()
    }

    /// 解密指定任务的下载源，并保持返回值 Debug 脱敏与析构清零。
    ///
    /// # Errors
    ///
    /// 任务不存在、密文记录无效或认证失败时返回稳定应用错误。
    pub async fn load_source(&self, id: Uuid) -> Result<SecretBytes, AppError> {
        let row = sqlx::query_as::<_, (i64, Vec<u8>, Vec<u8>)>(
            "SELECT source_schema_version,source_nonce,source_ciphertext
             FROM download_tasks WHERE id=?",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "download task not found"))?;
        let schema_version = u16::try_from(row.0)
            .map_err(|_| invalid_database("invalid task source schema version"))?;
        let nonce: [u8; 24] = row
            .1
            .try_into()
            .map_err(|_| invalid_database("invalid task source nonce"))?;
        let sealed = SealedSecret::from_parts(schema_version, nonce, row.2)
            .map_err(|error| invalid_database(error.to_string()))?;
        self.key
            .open(&task_source_aad(id, schema_version), &sealed)
            .map_err(secret_error)
    }

    /// 按过滤条件和 `(updated_at_us,id)` 倒序返回脱敏任务页。
    ///
    /// # Errors
    ///
    /// 过滤、分页、游标、数据库或持久行无效时返回稳定应用错误。
    pub async fn list(
        &self,
        filter: &DownloadTaskFilter,
        page: &PageRequest,
    ) -> Result<CursorPage<DownloadTaskView>, AppError> {
        validate_filter(filter)?;
        if page.limit == 0 || page.limit > crate::shared::page::MAX_PAGE_LIMIT {
            return Err(validation("invalid download task page"));
        }
        let filter_digest = filter_digest(filter);
        let cursor = page
            .cursor
            .as_deref()
            .map(decode_cursor)
            .transpose()?
            .map(|cursor| {
                if cursor.filter_digest == filter_digest {
                    Ok(cursor)
                } else {
                    Err(validation("download task cursor does not match filter"))
                }
            })
            .transpose()?;
        let mut query = QueryBuilder::<Sqlite>::new("SELECT ");
        query
            .push(TASK_COLUMNS)
            .push(" FROM download_tasks WHERE 1=1");
        if let Some(connection_id) = filter.connection_id {
            query
                .push(" AND connection_id=")
                .push_bind(connection_id.as_bytes().to_vec());
        }
        if let Some(status) = filter.status {
            query.push(" AND status=").push_bind(status.as_str());
        }
        if let Some(value) = filter.query.as_deref() {
            query
                .push(" AND instr(lower(display_name),")
                .push_bind(value.trim().to_lowercase())
                .push(")>0");
        }
        if let Some(cursor) = &cursor {
            query
                .push(" AND (updated_at_us<")
                .push_bind(cursor.updated_at_us)
                .push(" OR (updated_at_us=")
                .push_bind(cursor.updated_at_us)
                .push(" AND id<")
                .push_bind(cursor.id.as_bytes().to_vec())
                .push("))");
        }
        query
            .push(" ORDER BY updated_at_us DESC,id DESC LIMIT ")
            .push_bind(i64::from(page.limit) + 1);
        let rows = query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(database_error)?;
        let has_more = rows.len() > page.limit as usize;
        let mut decoded = rows
            .iter()
            .take(page.limit as usize)
            .map(decode_task_with_order)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if has_more {
            decoded
                .last()
                .map(|(_, updated_at_us, id)| {
                    encode_cursor(&TaskCursor {
                        version: 1,
                        filter_digest,
                        updated_at_us: *updated_at_us,
                        id: *id,
                    })
                })
                .transpose()?
        } else {
            None
        };
        let items = decoded.drain(..).map(|(item, _, _)| item).collect();
        Ok(CursorPage { items, next_cursor })
    }

    /// 以最早创建顺序声明一项可运行任务，并返回解密后的私有 worker 快照。
    ///
    /// # Errors
    ///
    /// 租约时间溢出、数据库、密文或持久投影无效时返回稳定应用错误。
    pub async fn claim(
        &self,
        owner: &str,
        now_us: i64,
    ) -> Result<Option<DownloadTaskLease>, AppError> {
        Ok(self.claim_batch(owner, now_us, 1).await?.into_iter().next())
    }

    /// 按最早任务的连接声明至多 100 项同连接任务。
    ///
    /// 该边界使 runtime 可以用一个有界远端查询同步同连接任务，同时不会跨连接传播故障。
    ///
    /// # Errors
    ///
    /// owner、批量大小、租约时间、数据库、密文或持久投影无效时返回稳定应用错误。
    pub async fn claim_batch(
        &self,
        owner: &str,
        now_us: i64,
        limit: u16,
    ) -> Result<Vec<DownloadTaskLease>, AppError> {
        validate_owner(owner)?;
        if limit == 0 || limit > 100 {
            return Err(validation("download task claim batch is outside bounds"));
        }
        let expires_at = now_us
            .checked_add(LEASE_DURATION_US)
            .ok_or_else(|| validation("download task lease time overflow"))?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let connection_id = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT connection_id FROM download_tasks
             WHERE status IN ('queued','submitting','monitoring','retry-wait')
               AND (status!='retry-wait' OR retry_at_us<=?)
               AND (lease_owner IS NULL OR lease_expires_at_us<=?)
             ORDER BY created_at_us ASC,id ASC LIMIT 1",
        )
        .bind(now_us)
        .bind(now_us)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        let Some(connection_id) = connection_id else {
            tx.commit().await.map_err(database_error)?;
            return Ok(Vec::new());
        };
        let rows = sqlx::query_as::<_, (Vec<u8>, Option<String>, String)>(
            "SELECT id,remote_id,status FROM download_tasks
             WHERE connection_id=?
               AND status IN ('queued','submitting','monitoring','retry-wait')
               AND (status!='retry-wait' OR retry_at_us<=?)
               AND (lease_owner IS NULL OR lease_expires_at_us<=?)
             ORDER BY created_at_us ASC,id ASC LIMIT ?",
        )
        .bind(connection_id.as_slice())
        .bind(now_us)
        .bind(now_us)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        let connection_id = Uuid::from_slice(&connection_id).map_err(invalid_database)?;
        let mut claimed = Vec::with_capacity(rows.len());
        for (id, remote_id, previous_status) in rows {
            let id = Uuid::from_slice(&id).map_err(invalid_database)?;
            let next_status = if remote_id.is_some() {
                "monitoring"
            } else {
                "submitting"
            };
            let public_changed = previous_status != next_status;
            sqlx::query(
                "UPDATE download_tasks
                 SET status=?,
                     lease_owner=?,lease_expires_at_us=?,attempt_count=attempt_count+1,
                     projection_version=projection_version+?,
                     updated_at_us=CASE WHEN ? THEN ? ELSE updated_at_us END WHERE id=?",
            )
            .bind(next_status)
            .bind(owner)
            .bind(expires_at)
            .bind(i64::from(public_changed))
            .bind(public_changed)
            .bind(now_us)
            .bind(id.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
            let attempt_count =
                sqlx::query_scalar::<_, i64>("SELECT attempt_count FROM download_tasks WHERE id=?")
                    .bind(id.as_bytes().as_slice())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(database_error)?;
            if public_changed {
                write_task_event(&mut tx, id, now_us).await?;
            }
            claimed.push((id, remote_id, attempt_count));
        }
        tx.commit().await.map_err(database_error)?;
        if !claimed.is_empty() {
            self.notifier.notify_after_commit();
        }
        let mut leases = Vec::with_capacity(claimed.len());
        for (id, remote_id, attempt_count) in claimed {
            leases.push(DownloadTaskLease {
                task_id: id,
                connection_id,
                source: self.load_source(id).await?,
                remote_id,
                correlation_tag: format!("mediaflow-{id}"),
                attempt_count: u32::try_from(attempt_count)
                    .map_err(|_| invalid_database("invalid download task attempt count"))?,
            });
        }
        Ok(leases)
    }

    /// 在持有租约时提交首次远端 hash 关联。
    ///
    /// # Errors
    ///
    /// hash、唯一性、租约或数据库无效时返回稳定应用错误。
    pub async fn commit_remote_ref(
        &self,
        id: Uuid,
        owner: &str,
        remote_id: &str,
        now_us: i64,
    ) -> Result<DownloadTaskView, AppError> {
        validate_owner(owner)?;
        validate_remote_id(remote_id)?;
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let result = sqlx::query(
            "UPDATE download_tasks SET remote_id=?,remote_status='unknown',linked=1,
             status='monitoring',failure_code=NULL,retry_at_us=NULL,blocked_config_version=NULL,
             projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND lease_owner=? AND status IN ('submitting','monitoring')",
        )
        .bind(remote_id)
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(owner)
        .execute(&mut *tx)
        .await;
        match result {
            Ok(result) if result.rows_affected() == 1 => {
                write_task_event(&mut tx, id, now_us).await?;
                tx.commit().await.map_err(database_error)?;
                self.notifier.notify_after_commit();
                self.require(id).await
            }
            Ok(_) => Err(lease_lost()),
            Err(error) if is_unique(&error) => Err(request_conflict()),
            Err(error) => Err(database_error(error)),
        }
    }

    /// 在持有租约时提交一项有界远端状态快照。
    ///
    /// # Errors
    ///
    /// 进度、租约、状态或数据库无效时返回稳定应用错误。
    pub async fn commit_snapshot(
        &self,
        id: Uuid,
        owner: &str,
        status: RemoteDownloadStatus,
        progress_basis_points: u16,
        now_us: i64,
    ) -> Result<DownloadTaskView, AppError> {
        validate_owner(owner)?;
        if progress_basis_points > 10_000 {
            return Err(validation("download progress is outside bounds"));
        }
        let (task_status, failure_code, clear_lease) = match status {
            RemoteDownloadStatus::Completed => ("completed", None, true),
            RemoteDownloadStatus::Failed => (
                "failed",
                Some(DownloaderFailureCode::InvalidResponse.as_str()),
                true,
            ),
            _ => ("monitoring", None, false),
        };
        let remote_status = remote_status_value(status);
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let result = sqlx::query(
            "UPDATE download_tasks SET status=?,remote_status=?,progress_basis_points=?,
             failure_code=?,retry_at_us=NULL,blocked_config_version=NULL,
             lease_owner=CASE WHEN ? THEN NULL ELSE lease_owner END,
             lease_expires_at_us=CASE WHEN ? THEN NULL ELSE lease_expires_at_us END,
             projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND lease_owner=? AND linked=1
               AND status IN ('monitoring','submitting')
               AND (status!=? OR remote_status!=? OR progress_basis_points!=?
                    OR failure_code IS NOT ?)",
        )
        .bind(task_status)
        .bind(remote_status)
        .bind(i64::from(progress_basis_points))
        .bind(failure_code)
        .bind(clear_lease)
        .bind(clear_lease)
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(owner)
        .bind(task_status)
        .bind(remote_status)
        .bind(i64::from(progress_basis_points))
        .bind(failure_code)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if result.rows_affected() == 0 {
            let matching_lease = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM download_tasks
                 WHERE id=? AND lease_owner=? AND linked=1
                   AND status IN ('monitoring','submitting')",
            )
            .bind(id.as_bytes().as_slice())
            .bind(owner)
            .fetch_one(&mut *tx)
            .await
            .map_err(database_error)?;
            tx.commit().await.map_err(database_error)?;
            if matching_lease == 1 {
                return self.require(id).await;
            }
            return Err(lease_lost());
        }
        if status == RemoteDownloadStatus::Completed {
            write_completion_signal(&mut tx, id, now_us).await?;
        }
        write_task_event(&mut tx, id, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        self.require(id).await
    }

    /// 在持有租约时安排一次有界的可恢复重试，并保留现有进度与远端关联。
    ///
    /// # Errors
    ///
    /// 重试时间、租约、状态或数据库无效时返回稳定应用错误。
    pub async fn schedule_retry(
        &self,
        id: Uuid,
        owner: &str,
        failure: DownloaderFailureCode,
        retry_at_us: i64,
        now_us: i64,
    ) -> Result<DownloadTaskView, AppError> {
        validate_owner(owner)?;
        let latest = now_us
            .checked_add(86_400_000_000)
            .ok_or_else(|| validation("download retry time overflow"))?;
        if retry_at_us <= now_us || retry_at_us > latest {
            return Err(validation("download retry time is outside bounds"));
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let result = sqlx::query(
            "UPDATE download_tasks SET status='retry-wait',failure_code=?,retry_at_us=?,
             blocked_config_version=NULL,lease_owner=NULL,lease_expires_at_us=NULL,
             projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND lease_owner=? AND status NOT IN ('completed','failed')",
        )
        .bind(failure.as_str())
        .bind(retry_at_us)
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(owner)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if result.rows_affected() != 1 {
            return Err(lease_lost());
        }
        write_task_event(&mut tx, id, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        self.require(id).await
    }

    /// 将非终态任务标记为不可恢复失败，并清除租约。
    ///
    /// `owner=None` 是内部管理恢复路径；常规 worker 必须提供自己的租约 owner。
    ///
    /// # Errors
    ///
    /// 任务状态、租约或数据库无效时返回稳定应用错误。
    pub async fn fail(
        &self,
        id: Uuid,
        owner: Option<&str>,
        failure: DownloaderFailureCode,
        now_us: i64,
    ) -> Result<DownloadTaskView, AppError> {
        if let Some(owner) = owner {
            validate_owner(owner)?;
        }
        let mut query =
            QueryBuilder::<Sqlite>::new("UPDATE download_tasks SET status='failed',failure_code=");
        query
            .push_bind(failure.as_str())
            .push(
            ",retry_at_us=NULL,blocked_config_version=NULL,lease_owner=NULL,lease_expires_at_us=NULL,
                    projection_version=projection_version+1,updated_at_us=",
            )
            .push_bind(now_us)
            .push(" WHERE id=")
            .push_bind(id.as_bytes().to_vec())
            .push(" AND status NOT IN ('completed','failed')");
        if let Some(owner) = owner {
            query.push(" AND lease_owner=").push_bind(owner);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let result = query
            .build()
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        if result.rows_affected() != 1 {
            return Err(if owner.is_some() {
                lease_lost()
            } else {
                AppError::new(
                    ErrorCode::TaskInvalidState,
                    "download task is already terminal",
                )
            });
        }
        write_task_event(&mut tx, id, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        self.require(id).await
    }

    /// 因认证或协议版本阻塞任务，直到该连接配置版本实际增加。
    ///
    /// # Errors
    ///
    /// failure、配置版本、租约、状态或数据库无效时返回稳定应用错误。
    pub async fn fail_until_config_change(
        &self,
        id: Uuid,
        owner: &str,
        failure: DownloaderFailureCode,
        config_version: i64,
        now_us: i64,
    ) -> Result<DownloadTaskView, AppError> {
        validate_owner(owner)?;
        if config_version < 1
            || !matches!(
                failure,
                DownloaderFailureCode::IntegrationUnauthorized
                    | DownloaderFailureCode::UnsupportedVersion
            )
        {
            return Err(validation("invalid configuration-blocked download failure"));
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let result = sqlx::query(
            "UPDATE download_tasks SET status='failed',failure_code=?,retry_at_us=NULL,
             blocked_config_version=?,lease_owner=NULL,lease_expires_at_us=NULL,
             projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND lease_owner=? AND status NOT IN ('completed','failed')",
        )
        .bind(failure.as_str())
        .bind(config_version)
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(owner)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if result.rows_affected() != 1 {
            return Err(lease_lost());
        }
        write_task_event(&mut tx, id, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        self.require(id).await
    }

    /// 重新排队因旧连接配置而阻塞、且连接配置版本已经增加的任务。
    ///
    /// # Errors
    ///
    /// 数据库、outbox 或持久投影无效时返回稳定应用错误。
    pub async fn requeue_after_config_change(&self, now_us: i64) -> Result<u64, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let ids = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT task.id FROM download_tasks task
             JOIN downloader_connections connection ON connection.id=task.connection_id
             WHERE task.status='failed' AND task.blocked_config_version IS NOT NULL
               AND connection.enabled=1
               AND connection.config_version>task.blocked_config_version
             ORDER BY task.created_at_us ASC,task.id ASC LIMIT 100",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        for id in &ids {
            sqlx::query(
                "UPDATE download_tasks SET status='queued',failure_code=NULL,retry_at_us=NULL,
                 blocked_config_version=NULL,projection_version=projection_version+1,
                 updated_at_us=? WHERE id=?",
            )
            .bind(now_us)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
            write_task_event(
                &mut tx,
                Uuid::from_slice(id).map_err(invalid_database)?,
                now_us,
            )
            .await?;
        }
        tx.commit().await.map_err(database_error)?;
        if !ids.is_empty() {
            self.notifier.notify_after_commit();
        }
        u64::try_from(ids.len()).map_err(|_| invalid_database("requeue count overflow"))
    }

    async fn require(&self, id: Uuid) -> Result<DownloadTaskView, AppError> {
        self.get(id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "download task not found"))
    }
}

async fn write_task_event(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    id: Uuid,
    now_us: i64,
) -> Result<(), AppError> {
    let row = sqlx::query_as::<_, (i64, String, Option<String>, i64, Option<String>)>(
        "SELECT projection_version,status,remote_status,progress_basis_points,failure_code
         FROM download_tasks WHERE id=?",
    )
    .bind(id.as_bytes().as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    let status = DownloadTaskStatus::parse(&row.1)
        .ok_or_else(|| invalid_database("unknown download task event status"))?;
    let remote_status = row.2.as_deref().map(parse_remote_status).transpose()?;
    let progress_basis_points = u16::try_from(row.3)
        .ok()
        .filter(|value| *value <= 10_000)
        .ok_or_else(|| invalid_database("invalid download task event progress"))?;
    let failure_code = row
        .4
        .as_deref()
        .map(|value| {
            DownloaderFailureCode::parse(value)
                .ok_or_else(|| invalid_database("unknown download task event failure"))
        })
        .transpose()?;
    OutboxWriter::write(
        tx,
        DOWNLOAD_TASK_CHANGED,
        id,
        &DownloadTaskChangedPayload {
            projection_version: row.0,
            status,
            remote_status,
            progress_basis_points,
            failure_code,
        },
        now_us,
    )
    .await?;
    Ok(())
}

async fn write_completion_signal(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    id: Uuid,
    now_us: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO download_completion_signals
         (download_task_id,downloader_connection_id,completion_projection_version,status,outcome,
          automation_event_id,attempt_count,lease_token,lease_expires_at_us,created_at_us,updated_at_us)
         SELECT id,connection_id,projection_version,'pending',NULL,NULL,0,NULL,NULL,?,?
         FROM download_tasks WHERE id=? AND status='completed'
         ON CONFLICT(download_task_id) DO NOTHING",
    )
    .bind(now_us)
    .bind(now_us)
    .bind(id.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(database_error)
}

fn decode_task(row: &sqlx::sqlite::SqliteRow) -> Result<DownloadTaskView, AppError> {
    let id = Uuid::from_slice(
        row.try_get::<Vec<u8>, _>("id")
            .map_err(database_error)?
            .as_slice(),
    )
    .map_err(invalid_database)?;
    let connection_id = Uuid::from_slice(
        row.try_get::<Vec<u8>, _>("connection_id")
            .map_err(database_error)?
            .as_slice(),
    )
    .map_err(invalid_database)?;
    let status = row.try_get::<String, _>("status").map_err(database_error)?;
    let status = DownloadTaskStatus::parse(&status)
        .ok_or_else(|| invalid_database("unknown download task status"))?;
    let progress = row
        .try_get::<i64, _>("progress_basis_points")
        .map_err(database_error)?;
    let progress_basis_points = u16::try_from(progress)
        .ok()
        .filter(|value| *value <= 10_000)
        .ok_or_else(|| invalid_database("invalid download progress"))?;
    let failure_code = row
        .try_get::<Option<String>, _>("failure_code")
        .map_err(database_error)?
        .map(|value| {
            DownloaderFailureCode::parse(&value)
                .ok_or_else(|| invalid_database("unknown download failure code"))
        })
        .transpose()?;
    let retry_at_us = row
        .try_get::<Option<i64>, _>("retry_at_us")
        .map_err(database_error)?;
    let created_at_us = row
        .try_get::<i64, _>("created_at_us")
        .map_err(database_error)?;
    let updated_at_us = row
        .try_get::<i64, _>("updated_at_us")
        .map_err(database_error)?;
    Ok(DownloadTaskView {
        id,
        connection_id,
        connection_display_name: row
            .try_get("connection_display_name")
            .map_err(database_error)?,
        display_name: row.try_get("display_name").map_err(database_error)?,
        status,
        remote_status: row.try_get("remote_status").map_err(database_error)?,
        progress_basis_points,
        failure_code,
        retry_at: retry_at_us.map(format_time).transpose()?,
        linked: decode_bool(row.try_get("linked").map_err(database_error)?)?,
        projection_version: row.try_get("projection_version").map_err(database_error)?,
        created_at: format_time(created_at_us)?,
        updated_at: format_time(updated_at_us)?,
    })
}

fn decode_task_with_order(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<(DownloadTaskView, i64, Uuid), AppError> {
    let item = decode_task(row)?;
    let updated_at_us = row.try_get("updated_at_us").map_err(database_error)?;
    Ok((item.clone(), updated_at_us, item.id))
}

fn task_source_aad(id: Uuid, schema_version: u16) -> SecretAad {
    SecretAad::new(id, IntegrationKind::DownloadTaskSource, 1, schema_version)
}

fn validate_idempotency_key(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 255
        || value.chars().any(char::is_control)
        || value.trim() != value
    {
        return Err(validation("invalid download idempotency key"));
    }
    Ok(())
}

fn validate_owner(value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(validation("invalid download task lease owner"));
    }
    Ok(())
}

fn validate_remote_id(value: &str) -> Result<(), AppError> {
    if !matches!(value.len(), 40 | 64)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(validation("invalid remote download id"));
    }
    Ok(())
}

fn validate_filter(filter: &DownloadTaskFilter) -> Result<(), AppError> {
    if filter.query.as_deref().is_some_and(|value| {
        value.trim().is_empty()
            || value.chars().count() > 120
            || value.chars().any(char::is_control)
    }) {
        return Err(validation("invalid download task query"));
    }
    Ok(())
}

fn filter_digest(filter: &DownloadTaskFilter) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.download-task.filter.v1\0");
    if let Some(connection_id) = filter.connection_id {
        hasher.update(connection_id.as_bytes());
    }
    hasher.update([0]);
    if let Some(status) = filter.status {
        hasher.update(status.as_str().as_bytes());
    }
    hasher.update([0]);
    if let Some(query) = filter.query.as_deref() {
        hasher.update(query.trim().to_lowercase().as_bytes());
    }
    hex::encode(&hasher.finalize()[..16])
}

fn encode_cursor(cursor: &TaskCursor) -> Result<String, AppError> {
    let payload = serde_json::to_vec(cursor).map_err(invalid_database)?;
    let envelope = CursorEnvelope {
        payload: URL_SAFE_NO_PAD.encode(&payload),
        checksum: cursor_checksum(&payload),
    };
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).map_err(invalid_database)?);
    if encoded.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(invalid_database("download task cursor exceeds bounds"));
    }
    Ok(encoded)
}

fn decode_cursor(value: &str) -> Result<TaskCursor, AppError> {
    if value.is_empty() || value.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(validation("invalid download task cursor"));
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorEnvelope>(&bytes).ok())
        .ok_or_else(|| validation("invalid download task cursor"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(envelope.payload)
        .map_err(|_| validation("invalid download task cursor"))?;
    if envelope.checksum != cursor_checksum(&payload) {
        return Err(validation("invalid download task cursor"));
    }
    serde_json::from_slice::<TaskCursor>(&payload)
        .ok()
        .filter(|cursor| cursor.version == 1)
        .ok_or_else(|| validation("invalid download task cursor"))
}

fn cursor_checksum(payload: &[u8]) -> String {
    hex::encode(&digest(&[b"mediaflow.download-task.cursor.v1\0", payload])[..16])
}

fn digest(parts: &[&[u8]]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().to_vec()
}

const fn remote_status_value(status: RemoteDownloadStatus) -> &'static str {
    match status {
        RemoteDownloadStatus::Queued => "queued",
        RemoteDownloadStatus::Downloading => "downloading",
        RemoteDownloadStatus::Paused => "paused",
        RemoteDownloadStatus::Completed => "completed",
        RemoteDownloadStatus::Failed => "failed",
        RemoteDownloadStatus::Unknown => "unknown",
    }
}

fn parse_remote_status(value: &str) -> Result<RemoteDownloadStatus, AppError> {
    Ok(match value {
        "queued" => RemoteDownloadStatus::Queued,
        "downloading" => RemoteDownloadStatus::Downloading,
        "paused" => RemoteDownloadStatus::Paused,
        "completed" => RemoteDownloadStatus::Completed,
        "failed" => RemoteDownloadStatus::Failed,
        "unknown" => RemoteDownloadStatus::Unknown,
        _ => return Err(invalid_database("unknown remote download status")),
    })
}

fn format_time(value: i64) -> Result<String, AppError> {
    chrono::DateTime::from_timestamp_micros(value)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .ok_or_else(|| invalid_database("invalid download task timestamp"))
}

fn decode_bool(value: i64) -> Result<bool, AppError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid_database("invalid SQLite boolean")),
    }
}

fn is_unique(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.is_unique_violation())
}

fn request_conflict() -> AppError {
    AppError::new(
        ErrorCode::RequestConflict,
        "download idempotency or remote reference conflict",
    )
}

fn lease_lost() -> AppError {
    AppError::new(ErrorCode::TaskLeaseLost, "download task lease was lost")
}

fn validation(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn database_error(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn secret_error(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn invalid_database(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}
