#![allow(
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::discovery::observations::{FileObservation, ScanEntryError};
use crate::discovery::revisions::{ObservationSource, observe_in_connection};
use crate::platform::outbox::{OutboxNotifier, OutboxWriter};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::id::new_id;
use crate::shared::page::{CursorPage, PageRequest};
use crate::tasks::model::{
    DiscoveredFileView, NewScanTask, ScanCounts, ScanErrorView, ScanLease, ScanStatus, ScanTaskView,
};

/// 工作器租约有效期，单位为微秒（30 秒）。
pub const LEASE_DURATION_US: i64 = 30_000_000;
/// 推荐续期间隔，单位为微秒（10 秒），预留两个间隔的余量。
pub const LEASE_RENEW_INTERVAL_US: i64 = 10_000_000;

const FILE_CURSOR_MAGIC: &[u8; 4] = b"MFF3";
const FILE_CURSOR_CHECKSUM_BYTES: usize = 16;
const FILE_CURSOR_FIXED_PAYLOAD_BYTES: usize = 4 + 16 + 16 + 16 + 8;

#[derive(Serialize, Deserialize)]
struct TaskCursor {
    #[serde(rename = "v")]
    version: u8,
    #[serde(rename = "a")]
    account_id: Uuid,
    #[serde(rename = "u")]
    updated_at_us: i64,
    #[serde(rename = "i")]
    id: Uuid,
    #[serde(rename = "r")]
    snapshot_revision: i64,
}

#[derive(Serialize, Deserialize)]
struct FileCursor {
    #[serde(rename = "v")]
    version: u8,
    #[serde(rename = "a")]
    account_id: Uuid,
    #[serde(rename = "t")]
    task_id: Uuid,
    #[serde(rename = "p")]
    relative_path_bytes: Vec<u8>,
    #[serde(rename = "i")]
    id: Uuid,
    #[serde(rename = "s")]
    snapshot_seq: i64,
}

#[derive(Serialize, Deserialize)]
struct ErrorCursor {
    #[serde(rename = "v")]
    version: u8,
    #[serde(rename = "a")]
    account_id: Uuid,
    #[serde(rename = "t")]
    task_id: Uuid,
    #[serde(rename = "r")]
    attempt_id: Uuid,
    #[serde(rename = "f")]
    first_seen_at_us: i64,
    #[serde(rename = "i")]
    id: Uuid,
    #[serde(rename = "s")]
    snapshot_seq: i64,
}

#[derive(Serialize, Deserialize)]
struct CursorEnvelope {
    #[serde(rename = "p")]
    payload: String,
    #[serde(rename = "c")]
    checksum: String,
}

#[derive(Clone)]
/// 任务生命周期、观测、幂等性和发件箱事件的事务性 `SQLite` 边界。
///
/// 每个状态变更都会在同一事务内写入对应的发件箱事件，并且仅在提交后调用共享通知器。
pub struct TaskStore {
    pool: sqlx::SqlitePool,
    notifier: OutboxNotifier,
}

impl TaskStore {
    #[must_use]
    /// 创建带进程本地发件箱通知器的存储。
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self::new_with_notifier(pool, OutboxNotifier::new())
    }

    #[must_use]
    /// 创建提交的发件箱写入会唤醒 `notifier` 订阅者的存储。
    pub fn new_with_notifier(pool: sqlx::SqlitePool, notifier: OutboxNotifier) -> Self {
        Self { pool, notifier }
    }

    /// 原子创建排队任务、初始尝试、扫描批次、幂等绑定和事件。
    ///
    /// 重复相同账户/操作/收件箱/键会返回原始任务；原始键绝不会持久化。通知仅在提交后发生。
    ///
    /// # Errors
    ///
    /// 键无效或绑定到不同请求参数、收件箱不存在/不可用，或任意事务、行解码或发件箱操作失败时返回 [`AppError`]。
    pub async fn create(&self, request: NewScanTask) -> Result<ScanTaskView, AppError> {
        validate_idempotency_key(&request.idempotency_key)?;
        let digest: [u8; 32] = Sha256::digest(request.idempotency_key.as_bytes()).into();
        if let Some(task) = self
            .idempotent_result(
                request.account_id,
                "scan.create",
                request.inbox_directory_id,
                &digest,
            )
            .await?
        {
            return Ok(task);
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        if let Some(task) = idempotent_result_in(
            &mut tx,
            request.account_id,
            "scan.create",
            request.inbox_directory_id,
            &digest,
        )
        .await?
        {
            tx.rollback().await.map_err(internal)?;
            return Ok(task);
        }
        let inbox_version = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM discovery_inbox_directories WHERE id=? AND health='available'",
        )
        .bind(request.inbox_directory_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::InboxNotFound, "inbox unavailable"))?;
        let task_id = new_id();
        let batch_id = new_id();
        let attempt_id = new_id();
        sqlx::query(
            "INSERT INTO discovery_scan_batches
             (id,inbox_directory_id,inbox_version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?)",
        )
        .bind(batch_id.as_bytes().as_slice())
        .bind(request.inbox_directory_id.as_bytes().as_slice())
        .bind(inbox_version)
        .bind(request.now_us)
        .bind(request.now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "INSERT INTO tasks_scan_tasks
             (id,account_id,scan_batch_id,inbox_directory_id,status,stage,recovering,
              current_attempt_id,created_at_us,updated_at_us)
             VALUES (?,?,?,?,'queued','queued',0,?,?,?)",
        )
        .bind(task_id.as_bytes().as_slice())
        .bind(request.account_id.as_bytes().as_slice())
        .bind(batch_id.as_bytes().as_slice())
        .bind(request.inbox_directory_id.as_bytes().as_slice())
        .bind(attempt_id.as_bytes().as_slice())
        .bind(request.now_us)
        .bind(request.now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "INSERT INTO tasks_scan_attempts
             (id,task_id,reason,ordinal,status,created_at_us)
             VALUES (?,?,'initial',1,'queued',?)",
        )
        .bind(attempt_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .bind(request.now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        insert_idempotency(
            &mut tx,
            request.account_id,
            "scan.create",
            request.inbox_directory_id,
            &digest,
            task_id,
            request.now_us,
        )
        .await?;
        let task = ScanTaskView {
            id: task_id,
            inbox_directory_id: request.inbox_directory_id,
            status: ScanStatus::Queued,
            recovering: false,
            counts: ScanCounts::default(),
        };
        OutboxWriter::write(
            &mut tx,
            "task.state-changed",
            task_id,
            &serde_json::json!({"status":"queued","recovering":false}),
            request.now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        Ok(task)
    }

    async fn idempotent_result(
        &self,
        account_id: Uuid,
        action: &str,
        target_id: Uuid,
        digest: &[u8; 32],
    ) -> Result<Option<ScanTaskView>, AppError> {
        let mut connection = self.pool.acquire().await.map_err(internal)?;
        idempotent_result_on(&mut *connection, account_id, action, target_id, digest).await
    }

    /// 为 `owner` 认领最早排队任务，或恢复最早过期的运行中任务。
    ///
    /// 在一个立即事务中，此操作启动/替换尝试、重置计数器和批次时间戳、设置 30 秒乐观租约并发出运行状态事件。
    /// 存在竞争的候选项或空队列返回 `None`；成功提交会通知发件箱订阅者。
    ///
    /// # Errors
    ///
    /// 事务/查询/发件箱失败或持久化 ID/计数格式错误时返回 [`AppError`]。
    pub async fn claim_next(
        &self,
        owner: &str,
        now_us: i64,
    ) -> Result<Option<ScanLease>, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let row = sqlx::query(
            "SELECT id,scan_batch_id,inbox_directory_id,current_attempt_id,status,recovering,version,
                    lease_expires_at_us
             FROM tasks_scan_tasks
             WHERE status='queued' OR (status='running' AND lease_expires_at_us<=?)
             ORDER BY created_at_us,id LIMIT 1",
        )
        .bind(now_us)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?;
        let Some(row) = row else {
            tx.rollback().await.map_err(internal)?;
            return Ok(None);
        };
        let task_id = uuid(&row, "id")?;
        let batch_id = uuid(&row, "scan_batch_id")?;
        let inbox_id = uuid(&row, "inbox_directory_id")?;
        let previous_attempt = uuid(&row, "current_attempt_id")?;
        let old_version: i64 = row.get("version");
        let expired_running = row.get::<String, _>("status") == "running";
        let recovering = expired_running || row.get::<i64, _>("recovering") != 0;
        let attempt_id = if expired_running {
            let attempt_id = new_id();
            let ordinal = sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(ordinal),0)+1 FROM tasks_scan_attempts WHERE task_id=?",
            )
            .bind(task_id.as_bytes().as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
            sqlx::query(
                "UPDATE tasks_scan_attempts SET status='failed',finished_at_us=?
                 WHERE id=? AND status='running'",
            )
            .bind(now_us)
            .bind(previous_attempt.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            sqlx::query(
                "INSERT INTO tasks_scan_attempts
                 (id,task_id,reason,ordinal,status,lease_owner,started_at_us,created_at_us)
                 VALUES (?,?,'recovery',?,'running',?,?,?)",
            )
            .bind(attempt_id.as_bytes().as_slice())
            .bind(task_id.as_bytes().as_slice())
            .bind(ordinal)
            .bind(owner)
            .bind(now_us)
            .bind(now_us)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            attempt_id
        } else {
            sqlx::query(
                "UPDATE tasks_scan_attempts SET status='running',lease_owner=?,started_at_us=?
                 WHERE id=? AND status='queued'",
            )
            .bind(owner)
            .bind(now_us)
            .bind(previous_attempt.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            previous_attempt
        };
        let expires = now_us + LEASE_DURATION_US;
        let updated = sqlx::query(
            "UPDATE tasks_scan_tasks
             SET status='running',stage='enumerating',recovering=?,current_attempt_id=?,
                 visited_directories=0,observed_files=0,skipped_entries=0,errors=0,
                 lease_owner=?,lease_expires_at_us=?,version=version+1,updated_at_us=?
             WHERE id=? AND version=? AND
                  (status='queued' OR (status='running' AND lease_expires_at_us<=?))",
        )
        .bind(i64::from(recovering))
        .bind(attempt_id.as_bytes().as_slice())
        .bind(owner)
        .bind(expires)
        .bind(now_us)
        .bind(task_id.as_bytes().as_slice())
        .bind(old_version)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        if updated.rows_affected() != 1 {
            tx.rollback().await.map_err(internal)?;
            return Ok(None);
        }
        sqlx::query(
            "UPDATE discovery_scan_batches SET started_at_us=?,finished_at_us=NULL,
             visited_directories=0,observed_files=0,skipped_entries=0,errors=0,updated_at_us=?
             WHERE id=?",
        )
        .bind(now_us)
        .bind(now_us)
        .bind(batch_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        OutboxWriter::write(
            &mut tx,
            "task.state-changed",
            task_id,
            &serde_json::json!({"status":"running","recovering":recovering}),
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        Ok(Some(ScanLease {
            task: ScanTaskView {
                id: task_id,
                inbox_directory_id: inbox_id,
                status: ScanStatus::Running,
                recovering,
                counts: ScanCounts::default(),
            },
            batch_id,
            attempt_id,
            owner: owner.to_owned(),
            version: old_version + 1,
            expires_at_us: expires,
        }))
    }

    /// 在精确的当前租约/版本下持久化提供的计数器快照。
    ///
    /// 计数器更新与受间隔限制的进度事件会原子提交；返回租约的版本已递增。租约截止时间本身不会延长。
    ///
    /// # Errors
    ///
    /// 所有者/版本/状态/截止时间不再匹配时返回 [`ErrorCode::TaskLeaseLost`]。计数转换、事务或发件箱失败时返回
    /// 内部 [`AppError`]。
    pub async fn commit_batch(
        &self,
        lease: &ScanLease,
        counts: ScanCounts,
        now_us: i64,
    ) -> Result<ScanLease, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let updated = sqlx::query(
            "UPDATE tasks_scan_tasks SET
             visited_directories=?,observed_files=?,skipped_entries=?,errors=?,
             version=version+1,updated_at_us=?
             WHERE id=? AND version=? AND lease_owner=? AND status='running'
               AND lease_expires_at_us>?",
        )
        .bind(i64::try_from(counts.visited_directories).map_err(internal)?)
        .bind(i64::try_from(counts.observed_files).map_err(internal)?)
        .bind(i64::try_from(counts.skipped_entries).map_err(internal)?)
        .bind(i64::try_from(counts.errors).map_err(internal)?)
        .bind(now_us)
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.version)
        .bind(&lease.owner)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        if updated.rows_affected() != 1 {
            tx.rollback().await.map_err(internal)?;
            return Err(AppError::new(
                ErrorCode::TaskLeaseLost,
                "lease condition failed",
            ));
        }
        OutboxWriter::write_progress_if_due(&mut tx, lease.task.id, &counts, now_us).await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        let mut renewed = lease.clone();
        renewed.version += 1;
        renewed.task.counts = counts;
        Ok(renewed)
    }

    /// 原子 upsert 一个有界观测/错误批次，并检查点其进度快照。
    ///
    /// 文件身份/路径行会在扫描批次内去重；重复的范围错误会增加出现次数。成功时，返回计数从 `progress` 获取
    /// 已访问/跳过值，从持久化行总计获取观测/错误值。会在写入前检测取消；返回租约版本会推进，订阅者仅在提交后唤醒。
    ///
    /// # Errors
    ///
    /// 提供超过 500 个观测时返回 [`ErrorCode::ValidationFailed`]，正在取消时返回
    /// [`ErrorCode::TaskInvalidState`]，租约过期/陈旧时返回 [`ErrorCode::TaskLeaseLost`]。
    /// 无效值、数据库、事务或发件箱失败时返回内部 [`AppError`]。
    pub async fn record_observations(
        &self,
        lease: &ScanLease,
        files: &[FileObservation],
        errors: &[ScanEntryError],
        progress: ScanCounts,
        now_us: i64,
    ) -> Result<ScanLease, AppError> {
        if files.len() + errors.len() > 500 {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "scan database batch exceeds 500 observations",
            ));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let cancel_requested = sqlx::query_scalar::<_, i64>(
            "SELECT cancel_requested FROM tasks_scan_tasks
             WHERE id=? AND version=? AND lease_owner=? AND status='running'
               AND lease_expires_at_us>?",
        )
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.version)
        .bind(&lease.owner)
        .bind(now_us)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?;
        let Some(cancel_requested) = cancel_requested else {
            tx.rollback().await.map_err(internal)?;
            return Err(AppError::new(
                ErrorCode::TaskLeaseLost,
                "lease condition failed",
            ));
        };
        if cancel_requested != 0 {
            tx.rollback().await.map_err(internal)?;
            return Err(AppError::new(
                ErrorCode::TaskInvalidState,
                "scan cancellation reached a database batch boundary",
            ));
        }
        let mut next_file_seq = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(first_observed_seq),0)
             FROM discovery_scan_file_observations WHERE scan_batch_id=?",
        )
        .bind(lease.batch_id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        for file in files {
            if file.inbox_directory_id != lease.task.inbox_directory_id {
                tx.rollback().await.map_err(internal)?;
                return Err(AppError::new(
                    ErrorCode::ValidationFailed,
                    "observation inbox does not match task",
                ));
            }
            let discovered_id = new_id();
            let size = i64::try_from(file.size_bytes).map_err(internal)?;
            let row = sqlx::query(
                "INSERT INTO discovery_files
                 (id,inbox_directory_id,relative_path_bytes,relative_path_display,
                  identity_snapshot,size_bytes,modified_at_ns,first_seen_batch_id,
                  last_seen_batch_id,created_at_us,updated_at_us)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(inbox_directory_id,relative_path_bytes) DO UPDATE SET
                   relative_path_display=excluded.relative_path_display,
                   identity_snapshot=excluded.identity_snapshot,
                   size_bytes=excluded.size_bytes,
                   modified_at_ns=excluded.modified_at_ns,
                   last_seen_batch_id=excluded.last_seen_batch_id,
                   updated_at_us=excluded.updated_at_us
                 RETURNING id",
            )
            .bind(discovered_id.as_bytes().as_slice())
            .bind(file.inbox_directory_id.as_bytes().as_slice())
            .bind(&file.relative_path_bytes)
            .bind(&file.relative_path_display)
            .bind(&file.identity_snapshot)
            .bind(size)
            .bind(file.modified_at_ns)
            .bind(lease.batch_id.as_bytes().as_slice())
            .bind(lease.batch_id.as_bytes().as_slice())
            .bind(now_us)
            .bind(now_us)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
            let discovered_id = uuid(&row, "id")?;
            next_file_seq = next_file_seq.checked_add(1).ok_or_else(|| {
                AppError::new(ErrorCode::Internal, "file observation sequence exhausted")
            })?;
            sqlx::query(
                "INSERT INTO discovery_scan_file_observations
                 (scan_batch_id,discovered_file_id,last_observed_attempt_id,identity_snapshot,
                  size_bytes,modified_at_ns,observed_at_us,first_observed_seq)
                 VALUES (?,?,?,?,?,?,?,?)
                 ON CONFLICT(scan_batch_id,discovered_file_id) DO UPDATE SET
                   last_observed_attempt_id=excluded.last_observed_attempt_id,
                   identity_snapshot=excluded.identity_snapshot,
                   size_bytes=excluded.size_bytes,
                   modified_at_ns=excluded.modified_at_ns,
                   observed_at_us=excluded.observed_at_us",
            )
            .bind(lease.batch_id.as_bytes().as_slice())
            .bind(discovered_id.as_bytes().as_slice())
            .bind(lease.attempt_id.as_bytes().as_slice())
            .bind(&file.identity_snapshot)
            .bind(size)
            .bind(file.modified_at_ns)
            .bind(now_us)
            .bind(next_file_seq)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            observe_in_connection(&mut tx, file, ObservationSource::Scan, now_us).await?;
        }
        let mut next_error_seq = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(first_seen_seq),0) FROM tasks_scan_errors WHERE attempt_id=?",
        )
        .bind(lease.attempt_id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        for error in errors {
            let id = new_id();
            next_error_seq = next_error_seq.checked_add(1).ok_or_else(|| {
                AppError::new(ErrorCode::Internal, "scan error sequence exhausted")
            })?;
            sqlx::query(
                "INSERT INTO tasks_scan_errors
                 (id,task_id,scan_batch_id,attempt_id,code,scope,relative_path_bytes,
                  relative_path_display,first_seen_at_us,last_seen_at_us,occurrences,first_seen_seq)
                 VALUES (?,?,?,?,?,?,?,?,?,?,1,?)
                 ON CONFLICT(attempt_id,code,scope,relative_path_bytes) DO UPDATE SET
                   relative_path_display=excluded.relative_path_display,
                   last_seen_at_us=excluded.last_seen_at_us,
                   occurrences=tasks_scan_errors.occurrences+1",
            )
            .bind(id.as_bytes().as_slice())
            .bind(lease.task.id.as_bytes().as_slice())
            .bind(lease.batch_id.as_bytes().as_slice())
            .bind(lease.attempt_id.as_bytes().as_slice())
            .bind(error.code)
            .bind(error.scope)
            .bind(&error.relative_path_bytes)
            .bind(&error.relative_path_display)
            .bind(now_us)
            .bind(now_us)
            .bind(next_error_seq)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        }
        let observed_files = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM discovery_scan_file_observations
             WHERE scan_batch_id=? AND last_observed_attempt_id=?",
        )
        .bind(lease.batch_id.as_bytes().as_slice())
        .bind(lease.attempt_id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        let error_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tasks_scan_errors WHERE attempt_id=?",
        )
        .bind(lease.attempt_id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        let updated = sqlx::query(
            "UPDATE tasks_scan_tasks SET visited_directories=?,observed_files=?,
             skipped_entries=?,errors=?,version=version+1,updated_at_us=?
             WHERE id=? AND version=? AND lease_owner=? AND status='running'
               AND lease_expires_at_us>?",
        )
        .bind(i64::try_from(progress.visited_directories).map_err(internal)?)
        .bind(observed_files)
        .bind(i64::try_from(progress.skipped_entries).map_err(internal)?)
        .bind(error_count)
        .bind(now_us)
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.version)
        .bind(&lease.owner)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        if updated.rows_affected() != 1 {
            tx.rollback().await.map_err(internal)?;
            return Err(AppError::new(
                ErrorCode::TaskLeaseLost,
                "lease condition failed",
            ));
        }
        sqlx::query(
            "UPDATE discovery_scan_batches SET visited_directories=?,observed_files=?,
             skipped_entries=?,errors=?,updated_at_us=? WHERE id=?",
        )
        .bind(i64::try_from(progress.visited_directories).map_err(internal)?)
        .bind(observed_files)
        .bind(i64::try_from(progress.skipped_entries).map_err(internal)?)
        .bind(error_count)
        .bind(now_us)
        .bind(lease.batch_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        let counts = ScanCounts {
            visited_directories: progress.visited_directories,
            observed_files: u64::try_from(observed_files).map_err(internal)?,
            skipped_entries: progress.skipped_entries,
            errors: u64::try_from(error_count).map_err(internal)?,
        };
        OutboxWriter::write_progress_if_due(&mut tx, lease.task.id, &counts, now_us).await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        let mut updated = lease.clone();
        updated.version += 1;
        updated.task.counts = counts;
        Ok(updated)
    }

    /// 将匹配的有效租约延长至 `now_us` 后 30 秒，并递增其版本。
    ///
    /// # Errors
    ///
    /// 所有者/版本/状态/截止时间不再匹配时返回 [`ErrorCode::TaskLeaseLost`]；`SQLite` 无法执行更新时返回内部
    /// [`AppError`]。
    pub async fn renew(&self, lease: &ScanLease, now_us: i64) -> Result<ScanLease, AppError> {
        let expires_at_us = now_us + LEASE_DURATION_US;
        let result = sqlx::query(
            "UPDATE tasks_scan_tasks SET lease_expires_at_us=?,version=version+1,updated_at_us=?
             WHERE id=? AND version=? AND lease_owner=? AND status='running'
               AND lease_expires_at_us>?",
        )
        .bind(expires_at_us)
        .bind(now_us)
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.version)
        .bind(&lease.owner)
        .bind(now_us)
        .execute(&self.pool)
        .await
        .map_err(internal)?;
        if result.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::TaskLeaseLost,
                "lease renewal failed",
            ));
        }
        let mut renewed = lease.clone();
        renewed.version += 1;
        renewed.expires_at_us = expires_at_us;
        Ok(renewed)
    }

    /// 原子将已租约任务/尝试/批次标记为失败并发出其终态事件。
    ///
    /// # Errors
    ///
    /// 租约陈旧/过期时返回 [`ErrorCode::TaskLeaseLost`]；事务、计数转换、持久化或发件箱失败时返回内部 [`AppError`]。
    pub async fn finish_failed(
        &self,
        lease: &ScanLease,
        now_us: i64,
    ) -> Result<ScanTaskView, AppError> {
        self.finish(lease, ScanStatus::Failed, None, now_us).await
    }

    /// 记录去重的根目录范围错误，然后原子以失败状态结束租约。
    ///
    /// `code` 会作为逻辑根路径 `.` 的稳定公开错误代码持久化。
    ///
    /// # Errors
    ///
    /// 租约陈旧/过期时返回 [`ErrorCode::TaskLeaseLost`]；无法提交错误行、终态行、事务或发件箱事件时返回内部
    /// [`AppError`]。
    pub async fn finish_failed_with_root_error(
        &self,
        lease: &ScanLease,
        code: &'static str,
        now_us: i64,
    ) -> Result<ScanTaskView, AppError> {
        self.finish(lease, ScanStatus::Failed, Some(code), now_us)
            .await
    }

    /// 原子将已租约任务/尝试/批次标记为已取消，并清除其取消/租约状态。
    ///
    /// # Errors
    ///
    /// 租约陈旧/过期时返回 [`ErrorCode::TaskLeaseLost`]；事务、计数转换、持久化或发件箱失败时返回内部 [`AppError`]。
    pub async fn finish_cancelled(
        &self,
        lease: &ScanLease,
        now_us: i64,
    ) -> Result<ScanTaskView, AppError> {
        self.finish(lease, ScanStatus::Cancelled, None, now_us)
            .await
    }

    /// 原子以完成状态结束租约；当 `counts.errors > 0` 时为部分成功。
    ///
    /// 尝试/批次/任务计数器与终态发件箱事件会一同提交。
    ///
    /// # Errors
    ///
    /// 租约陈旧/过期时返回 [`ErrorCode::TaskLeaseLost`]；事务、计数转换、持久化或发件箱失败时返回内部 [`AppError`]。
    pub async fn finish_success(
        &self,
        lease: &ScanLease,
        counts: ScanCounts,
        now_us: i64,
    ) -> Result<ScanTaskView, AppError> {
        let mut current = lease.clone();
        current.task.counts = counts;
        let status = if counts.errors == 0 {
            ScanStatus::Completed
        } else {
            ScanStatus::PartialSuccess
        };
        self.finish(&current, status, None, now_us).await
    }

    async fn finish(
        &self,
        lease: &ScanLease,
        status: ScanStatus,
        root_error_code: Option<&'static str>,
        now_us: i64,
    ) -> Result<ScanTaskView, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        if let Some(code) = root_error_code {
            let id = new_id();
            let first_seen_seq = sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(first_seen_seq),0)+1
                 FROM tasks_scan_errors WHERE attempt_id=?",
            )
            .bind(lease.attempt_id.as_bytes().as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
            sqlx::query(
                "INSERT INTO tasks_scan_errors
                 (id,task_id,scan_batch_id,attempt_id,code,scope,relative_path_bytes,
                  relative_path_display,first_seen_at_us,last_seen_at_us,occurrences,first_seen_seq)
                 VALUES (?,?,?,?,?,'root',X'2E','.',?,?,1,?)
                 ON CONFLICT(attempt_id,code,scope,relative_path_bytes) DO UPDATE SET
                   last_seen_at_us=excluded.last_seen_at_us",
            )
            .bind(id.as_bytes().as_slice())
            .bind(lease.task.id.as_bytes().as_slice())
            .bind(lease.batch_id.as_bytes().as_slice())
            .bind(lease.attempt_id.as_bytes().as_slice())
            .bind(code)
            .bind(now_us)
            .bind(now_us)
            .bind(first_seen_seq)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        }
        let error_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tasks_scan_errors WHERE attempt_id=?",
        )
        .bind(lease.attempt_id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        if status == ScanStatus::Completed {
            mark_unseen_revisions_missing(
                &mut tx,
                lease.task.inbox_directory_id,
                lease.batch_id,
                now_us,
            )
            .await?;
        }
        let result = sqlx::query(
            "UPDATE tasks_scan_tasks SET status=?,stage='finished',recovering=0,
             visited_directories=?,observed_files=?,skipped_entries=?,errors=?,
             cancel_requested=0,lease_owner=NULL,lease_expires_at_us=NULL,
             version=version+1,updated_at_us=?
             WHERE id=? AND version=? AND lease_owner=? AND status='running'
               AND lease_expires_at_us>?",
        )
        .bind(status.as_str())
        .bind(i64::try_from(lease.task.counts.visited_directories).map_err(internal)?)
        .bind(i64::try_from(lease.task.counts.observed_files).map_err(internal)?)
        .bind(i64::try_from(lease.task.counts.skipped_entries).map_err(internal)?)
        .bind(error_count)
        .bind(now_us)
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.version)
        .bind(&lease.owner)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        if result.rows_affected() != 1 {
            tx.rollback().await.map_err(internal)?;
            return Err(AppError::new(
                ErrorCode::TaskLeaseLost,
                "terminal lease condition failed",
            ));
        }
        sqlx::query(
            "UPDATE tasks_scan_attempts SET status=?,finished_at_us=? WHERE id=? AND status='running'",
        )
        .bind(status.as_str())
        .bind(now_us)
        .bind(lease.attempt_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "UPDATE discovery_scan_batches SET finished_at_us=?,visited_directories=?,
             observed_files=?,skipped_entries=?,errors=?,updated_at_us=? WHERE id=?",
        )
        .bind(now_us)
        .bind(i64::try_from(lease.task.counts.visited_directories).map_err(internal)?)
        .bind(i64::try_from(lease.task.counts.observed_files).map_err(internal)?)
        .bind(i64::try_from(lease.task.counts.skipped_entries).map_err(internal)?)
        .bind(error_count)
        .bind(now_us)
        .bind(lease.batch_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        OutboxWriter::write(
            &mut tx,
            "task.state-changed",
            lease.task.id,
            &serde_json::json!({"status":status.as_str(),"recovering":false}),
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        let mut task = lease.task.clone();
        task.status = status;
        task.recovering = false;
        task.counts.errors = u64::try_from(error_count).map_err(internal)?;
        Ok(task)
    }

    /// 为拥有的失败或部分成功任务原子排入手动尝试。
    ///
    /// 新尝试、重置后的计数器、幂等绑定和排队事件会一同提交。相同重试会返回原始结果，且仅在新提交后通知。
    ///
    /// # Errors
    ///
    /// 幂等性无效/冲突、任务缺失/不属于该账户、状态不是失败/部分成功、版本存在竞争、存储数据格式错误或任意
    /// 事务/发件箱失败时返回 [`AppError`]。
    pub async fn retry(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<ScanTaskView, AppError> {
        validate_idempotency_key(idempotency_key)?;
        let digest: [u8; 32] = Sha256::digest(idempotency_key.as_bytes()).into();
        if let Some(task) = self
            .idempotent_result(account_id, "scan.retry", task_id, &digest)
            .await?
        {
            return Ok(task);
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        if let Some(task) =
            idempotent_result_in(&mut tx, account_id, "scan.retry", task_id, &digest).await?
        {
            tx.rollback().await.map_err(internal)?;
            return Ok(task);
        }
        let row = sqlx::query(
            "SELECT id,inbox_directory_id,status,version FROM tasks_scan_tasks
             WHERE id=? AND account_id=? AND reason='manual'",
        )
        .bind(task_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "task not found"))?;
        let status = decode_status(row.get::<String, _>("status").as_str())?;
        if !matches!(status, ScanStatus::Failed | ScanStatus::PartialSuccess) {
            return Err(AppError::new(
                ErrorCode::TaskInvalidState,
                "task cannot be retried",
            ));
        }
        let inbox_id = uuid(&row, "inbox_directory_id")?;
        let version: i64 = row.get("version");
        let attempt_id = new_id();
        let ordinal = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(ordinal),0)+1 FROM tasks_scan_attempts WHERE task_id=?",
        )
        .bind(task_id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "INSERT INTO tasks_scan_attempts
             (id,task_id,reason,ordinal,status,created_at_us)
             VALUES (?,?,'manual_retry',?,'queued',?)",
        )
        .bind(attempt_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .bind(ordinal)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        let result = sqlx::query(
            "UPDATE tasks_scan_tasks SET status='queued',stage='queued',recovering=0,
             current_attempt_id=?,visited_directories=0,observed_files=0,skipped_entries=0,
             errors=0,cancel_requested=0,version=version+1,updated_at_us=?
             WHERE id=? AND version=? AND status IN ('failed','partial-success')",
        )
        .bind(attempt_id.as_bytes().as_slice())
        .bind(now_us)
        .bind(task_id.as_bytes().as_slice())
        .bind(version)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        if result.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::TaskInvalidState,
                "retry raced task state",
            ));
        }
        insert_idempotency(
            &mut tx,
            account_id,
            "scan.retry",
            task_id,
            &digest,
            task_id,
            now_us,
        )
        .await?;
        OutboxWriter::write(
            &mut tx,
            "task.state-changed",
            task_id,
            &serde_json::json!({"status":"queued","recovering":false}),
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        Ok(ScanTaskView {
            id: task_id,
            inbox_directory_id: inbox_id,
            status: ScanStatus::Queued,
            recovering: false,
            counts: ScanCounts::default(),
        })
    }

    /// 幂等取消拥有的排队任务，或将运行任务标记为在批次边界停止。
    ///
    /// 排队任务/尝试取消、运行任务标记、幂等绑定和当前状态事件会原子提交；订阅者在提交后唤醒。
    ///
    /// # Errors
    ///
    /// 幂等性无效/冲突、任务缺失/不属于该账户、终态或其他不可取消状态、存储数据格式错误或事务/发件箱失败时返回
    /// [`AppError`]。
    pub async fn request_cancel(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<ScanTaskView, AppError> {
        validate_idempotency_key(idempotency_key)?;
        let digest: [u8; 32] = Sha256::digest(idempotency_key.as_bytes()).into();
        if let Some(task) = self
            .idempotent_result(account_id, "scan.cancel", task_id, &digest)
            .await?
        {
            return Ok(task);
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        if let Some(task) =
            idempotent_result_in(&mut tx, account_id, "scan.cancel", task_id, &digest).await?
        {
            tx.rollback().await.map_err(internal)?;
            return Ok(task);
        }
        let row = sqlx::query(
            "SELECT id,inbox_directory_id,status,recovering,visited_directories,
                    observed_files,skipped_entries,errors
             FROM tasks_scan_tasks WHERE id=? AND account_id=? AND reason='manual'",
        )
        .bind(task_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "task not found"))?;
        let mut task = decode_task(&row)?;
        match task.status {
            ScanStatus::Queued => {
                sqlx::query(
                    "UPDATE tasks_scan_tasks SET status='cancelled',stage='finished',
                     cancel_requested=0,version=version+1,updated_at_us=? WHERE id=? AND status='queued'",
                )
                .bind(now_us)
                .bind(task_id.as_bytes().as_slice())
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
                sqlx::query(
                    "UPDATE tasks_scan_attempts SET status='cancelled',finished_at_us=?
                     WHERE id=(SELECT current_attempt_id FROM tasks_scan_tasks WHERE id=?)",
                )
                .bind(now_us)
                .bind(task_id.as_bytes().as_slice())
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
                task.status = ScanStatus::Cancelled;
            }
            ScanStatus::Running => {
                sqlx::query(
                    "UPDATE tasks_scan_tasks SET cancel_requested=1,updated_at_us=?
                     WHERE id=? AND status='running'",
                )
                .bind(now_us)
                .bind(task_id.as_bytes().as_slice())
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
            }
            _ => {
                return Err(AppError::new(
                    ErrorCode::TaskInvalidState,
                    "task cannot be cancelled",
                ));
            }
        }
        insert_idempotency(
            &mut tx,
            account_id,
            "scan.cancel",
            task_id,
            &digest,
            task_id,
            now_us,
        )
        .await?;
        OutboxWriter::write(
            &mut tx,
            "task.state-changed",
            task_id,
            &serde_json::json!({"status":task.status.as_str(),"recovering":task.recovering}),
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        Ok(task)
    }

    /// 返回任务的持久化取消标志，与账户无关。
    ///
    /// # Errors
    ///
    /// 任务不存在时返回 [`ErrorCode::TaskNotFound`]，查询失败时返回内部 [`AppError`]。
    pub async fn cancel_requested(&self, task_id: Uuid) -> Result<bool, AppError> {
        sqlx::query_scalar::<_, i64>("SELECT cancel_requested FROM tasks_scan_tasks WHERE id=?")
            .bind(task_id.as_bytes().as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?
            .map(|value| value != 0)
            .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "task not found"))
    }

    /// 重新排入租约在 `now_us` 或更早过期的每个运行中任务。
    ///
    /// 每个任务都在各自立即事务中处理：旧尝试变为失败、恢复尝试被排队、租约所有权被清除，并发出恢复事件。
    /// 存在竞争的候选项会跳过；后续回收失败时，先前提交仍保持可见。
    ///
    /// # Errors
    ///
    /// 首个事务/查询/解码/发件箱失败时返回 [`AppError`]。
    pub async fn reclaim_expired(&self, now_us: i64) -> Result<u64, AppError> {
        let mut reclaimed = 0_u64;
        loop {
            let mut tx = self
                .pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .map_err(internal)?;
            let row = sqlx::query(
                "SELECT id,current_attempt_id,version FROM tasks_scan_tasks
                 WHERE status='running' AND lease_expires_at_us<=?
                 ORDER BY lease_expires_at_us,id LIMIT 1",
            )
            .bind(now_us)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?;
            let Some(row) = row else {
                tx.rollback().await.map_err(internal)?;
                break;
            };
            let task_id = uuid(&row, "id")?;
            let previous_attempt = uuid(&row, "current_attempt_id")?;
            let version: i64 = row.get("version");
            let attempt_id = new_id();
            let ordinal = sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(ordinal),0)+1 FROM tasks_scan_attempts WHERE task_id=?",
            )
            .bind(task_id.as_bytes().as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
            sqlx::query(
                "UPDATE tasks_scan_attempts SET status='failed',finished_at_us=?
                 WHERE id=? AND status='running'",
            )
            .bind(now_us)
            .bind(previous_attempt.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            sqlx::query(
                "INSERT INTO tasks_scan_attempts
                 (id,task_id,reason,ordinal,status,created_at_us)
                 VALUES (?,?,'recovery',?,'queued',?)",
            )
            .bind(attempt_id.as_bytes().as_slice())
            .bind(task_id.as_bytes().as_slice())
            .bind(ordinal)
            .bind(now_us)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            let result = sqlx::query(
                "UPDATE tasks_scan_tasks SET status='queued',stage='queued',recovering=1,
                 current_attempt_id=?,lease_owner=NULL,lease_expires_at_us=NULL,
                 version=version+1,updated_at_us=?
                 WHERE id=? AND version=? AND status='running' AND lease_expires_at_us<=?",
            )
            .bind(attempt_id.as_bytes().as_slice())
            .bind(now_us)
            .bind(task_id.as_bytes().as_slice())
            .bind(version)
            .bind(now_us)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            if result.rows_affected() != 1 {
                tx.rollback().await.map_err(internal)?;
                continue;
            }
            OutboxWriter::write(
                &mut tx,
                "task.state-changed",
                task_id,
                &serde_json::json!({"status":"queued","recovering":true}),
                now_us,
            )
            .await?;
            tx.commit().await.map_err(internal)?;
            self.notifier.notify_after_commit();
            reclaimed += 1;
        }
        Ok(reclaimed)
    }

    /// 仅在 `account_id` 拥有任务时加载一个任务。
    ///
    /// # Errors
    ///
    /// 任务不存在/不属于该账户时返回 [`ErrorCode::TaskNotFound`]；查询失败或持久化 UUID/状态/计数格式错误时返回
    /// 内部 [`AppError`]。
    pub async fn get(&self, account_id: Uuid, task_id: Uuid) -> Result<ScanTaskView, AppError> {
        let row = sqlx::query(
            "SELECT id,inbox_directory_id,status,recovering,visited_directories,
                    observed_files,skipped_entries,errors
             FROM tasks_scan_tasks WHERE id=? AND account_id=? AND reason='manual'",
        )
        .bind(task_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "task not found"))?;
        decode_task(&row)
    }

    /// 以快照稳定的 `(updated_at_us DESC, id DESC)` 顺序返回一个账户的任务。
    ///
    /// 首页会捕获账户最大的排序历史修订版本；后续游标会保留该修订，以便并发任务更新不会重新排序或重复分页条目。
    ///
    /// # Errors
    ///
    /// 游标格式错误、校验和无效或跨账户时返回 [`ErrorCode::ValidationFailed`]。查询、解码、时间戳/计数或游标
    /// 序列化失败时返回内部 [`AppError`]。
    pub async fn list_tasks(
        &self,
        account_id: Uuid,
        page: &PageRequest,
    ) -> Result<CursorPage<ScanTaskView>, AppError> {
        let cursor = page.cursor.as_deref().map(decode_task_cursor).transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.account_id != account_id)
        {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "task cursor query does not match",
            ));
        }
        let snapshot_revision = if let Some(cursor) = &cursor {
            Some(cursor.snapshot_revision)
        } else {
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT MAX(revision) FROM tasks_scan_task_order_history WHERE account_id=?",
            )
            .bind(account_id.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .map_err(internal)?
        };
        let Some(snapshot_revision) = snapshot_revision else {
            return Ok(CursorPage {
                items: Vec::new(),
                next_cursor: None,
            });
        };
        let rows = if let Some(cursor) = cursor {
            sqlx::query(
                "WITH snapshot_order AS (
                     SELECT history.task_id,history.updated_at_us
                     FROM tasks_scan_task_order_history history
                     WHERE history.account_id=? AND history.revision<=? AND history.revision=(
                         SELECT MAX(candidate.revision)
                         FROM tasks_scan_task_order_history candidate
                         WHERE candidate.task_id=history.task_id AND candidate.revision<=?
                     )
                 )
                 SELECT task.id,task.inbox_directory_id,task.status,task.recovering,
                        task.visited_directories,task.observed_files,task.skipped_entries,task.errors,
                        snapshot_order.updated_at_us AS snapshot_updated_at_us
                 FROM snapshot_order JOIN tasks_scan_tasks task ON task.id=snapshot_order.task_id
                 WHERE task.reason='manual' AND (snapshot_order.updated_at_us<? OR
                       (snapshot_order.updated_at_us=? AND task.id<?))
                 ORDER BY snapshot_order.updated_at_us DESC,task.id DESC LIMIT ?",
            )
            .bind(account_id.as_bytes().as_slice())
            .bind(snapshot_revision)
            .bind(snapshot_revision)
            .bind(cursor.updated_at_us)
            .bind(cursor.updated_at_us)
            .bind(cursor.id.as_bytes().as_slice())
            .bind(i64::from(page.limit) + 1)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "WITH snapshot_order AS (
                     SELECT history.task_id,history.updated_at_us
                     FROM tasks_scan_task_order_history history
                     WHERE history.account_id=? AND history.revision<=? AND history.revision=(
                         SELECT MAX(candidate.revision)
                         FROM tasks_scan_task_order_history candidate
                         WHERE candidate.task_id=history.task_id AND candidate.revision<=?
                     )
                 )
                 SELECT task.id,task.inbox_directory_id,task.status,task.recovering,
                        task.visited_directories,task.observed_files,task.skipped_entries,task.errors,
                        snapshot_order.updated_at_us AS snapshot_updated_at_us
                 FROM snapshot_order JOIN tasks_scan_tasks task ON task.id=snapshot_order.task_id
                 WHERE task.reason='manual'
                 ORDER BY snapshot_order.updated_at_us DESC,task.id DESC LIMIT ?",
            )
            .bind(account_id.as_bytes().as_slice())
            .bind(snapshot_revision)
            .bind(snapshot_revision)
            .bind(i64::from(page.limit) + 1)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(internal)?;
        let has_more = rows.len() > page.limit as usize;
        let selected = rows.iter().take(page.limit as usize).collect::<Vec<_>>();
        let items = selected
            .iter()
            .map(|row| decode_task(row))
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if has_more {
            selected
                .last()
                .map(|row| {
                    encode_cursor(&TaskCursor {
                        version: 3,
                        account_id,
                        updated_at_us: row.get("snapshot_updated_at_us"),
                        id: uuid(row, "id").expect("selected task IDs were decoded above"),
                        snapshot_revision,
                    })
                })
                .transpose()?
        } else {
            None
        };
        Ok(CursorPage { items, next_cursor })
    }

    /// 以快照稳定的原始路径/ID 顺序返回拥有任务的文件。
    ///
    /// 首页会捕获最大观测序列；后续页不能包含在该快照之后首次观察到的文件。响应仅暴露显示路径，绝不暴露原始字节。
    ///
    /// # Errors
    ///
    /// 任务不存在/不属于该账户时返回 [`ErrorCode::TaskNotFound`]，游标格式错误或跨查询时返回
    /// [`ErrorCode::ValidationFailed`]。查询、持久化值/时间戳或游标序列化失败时返回内部 [`AppError`]。
    pub async fn list_files(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        page: &PageRequest,
    ) -> Result<CursorPage<DiscoveredFileView>, AppError> {
        self.ensure_task_owned(account_id, task_id).await?;
        let cursor = page.cursor.as_deref().map(decode_file_cursor).transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.account_id != account_id || cursor.task_id != task_id)
        {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "file cursor query does not match",
            ));
        }
        let snapshot_seq = if let Some(cursor) = &cursor {
            cursor.snapshot_seq
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(o.first_observed_seq),0)
                 FROM tasks_scan_tasks t
                 JOIN discovery_scan_file_observations o ON o.scan_batch_id=t.scan_batch_id
                 WHERE t.id=? AND t.account_id=?",
            )
            .bind(task_id.as_bytes().as_slice())
            .bind(account_id.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .map_err(internal)?
        };
        let rows = if let Some(cursor) = cursor {
            sqlx::query(
                "SELECT f.id,f.relative_path_bytes,f.relative_path_display,f.size_bytes,
                        f.modified_at_ns,o.first_observed_seq
                 FROM tasks_scan_tasks t
                 JOIN discovery_scan_file_observations o ON o.scan_batch_id=t.scan_batch_id
                 JOIN discovery_files f ON f.id=o.discovered_file_id
                 WHERE t.id=? AND t.account_id=? AND o.first_observed_seq<=? AND
                       (f.relative_path_bytes>? OR (f.relative_path_bytes=? AND f.id>?))
                 ORDER BY f.relative_path_bytes ASC,f.id ASC LIMIT ?",
            )
            .bind(task_id.as_bytes().as_slice())
            .bind(account_id.as_bytes().as_slice())
            .bind(snapshot_seq)
            .bind(&cursor.relative_path_bytes)
            .bind(&cursor.relative_path_bytes)
            .bind(cursor.id.as_bytes().as_slice())
            .bind(i64::from(page.limit) + 1)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT f.id,f.relative_path_bytes,f.relative_path_display,f.size_bytes,
                        f.modified_at_ns,o.first_observed_seq
                 FROM tasks_scan_tasks t
                 JOIN discovery_scan_file_observations o ON o.scan_batch_id=t.scan_batch_id
                 JOIN discovery_files f ON f.id=o.discovered_file_id
                 WHERE t.id=? AND t.account_id=? AND o.first_observed_seq<=?
                 ORDER BY f.relative_path_bytes ASC,f.id ASC LIMIT ?",
            )
            .bind(task_id.as_bytes().as_slice())
            .bind(account_id.as_bytes().as_slice())
            .bind(snapshot_seq)
            .bind(i64::from(page.limit) + 1)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(internal)?;
        let has_more = rows.len() > page.limit as usize;
        let selected = rows.iter().take(page.limit as usize).collect::<Vec<_>>();
        let items = selected
            .iter()
            .map(|row| {
                Ok(DiscoveredFileView {
                    id: uuid(row, "id")?,
                    relative_path: row.get("relative_path_display"),
                    size_bytes: u64::try_from(row.get::<i64, _>("size_bytes")).map_err(internal)?,
                    modified_at: format_ns(row.get("modified_at_ns"))?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let next_cursor = if has_more {
            selected
                .last()
                .map(|row| {
                    encode_file_cursor(&FileCursor {
                        version: 3,
                        account_id,
                        task_id,
                        relative_path_bytes: row.get("relative_path_bytes"),
                        id: uuid(row, "id").expect("selected file IDs were decoded above"),
                        snapshot_seq,
                    })
                })
                .transpose()?
        } else {
            None
        };
        Ok(CursorPage { items, next_cursor })
    }

    /// 以快照稳定的首次发现/ID 顺序返回拥有任务当前尝试的错误。
    ///
    /// 首页会捕获当前尝试 ID 和最大首次发现序列；后续页保持在该尝试中，并排除超出快照的新添加行。
    ///
    /// # Errors
    ///
    /// 任务不存在/不属于该账户时返回 [`ErrorCode::TaskNotFound`]，游标格式错误或跨查询时返回
    /// [`ErrorCode::ValidationFailed`]。查询、持久化值/计数或游标序列化失败时返回内部 [`AppError`]。
    pub async fn list_errors(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        page: &PageRequest,
    ) -> Result<CursorPage<ScanErrorView>, AppError> {
        self.ensure_task_owned(account_id, task_id).await?;
        let cursor = page
            .cursor
            .as_deref()
            .map(decode_error_cursor)
            .transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.account_id != account_id || cursor.task_id != task_id)
        {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "error cursor query does not match",
            ));
        }
        let (attempt_id, snapshot_seq) = if let Some(cursor) = &cursor {
            (cursor.attempt_id, cursor.snapshot_seq)
        } else {
            let row = sqlx::query(
                "SELECT t.current_attempt_id,COALESCE(MAX(e.first_seen_seq),0) AS snapshot_seq
                 FROM tasks_scan_tasks t
                 LEFT JOIN tasks_scan_errors e ON e.attempt_id=t.current_attempt_id
                 WHERE t.id=? AND t.account_id=? GROUP BY t.current_attempt_id",
            )
            .bind(task_id.as_bytes().as_slice())
            .bind(account_id.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .map_err(internal)?;
            (uuid(&row, "current_attempt_id")?, row.get("snapshot_seq"))
        };
        let rows = if let Some(cursor) = cursor {
            sqlx::query(
                "SELECT e.id,e.code,e.relative_path_display,e.occurrences,e.first_seen_at_us,
                        e.first_seen_seq
                 FROM tasks_scan_tasks t JOIN tasks_scan_errors e ON e.attempt_id=?
                 WHERE t.id=? AND t.account_id=? AND e.first_seen_seq<=? AND
                       (e.first_seen_at_us>? OR (e.first_seen_at_us=? AND e.id>?))
                 ORDER BY e.first_seen_at_us ASC,e.id ASC LIMIT ?",
            )
            .bind(attempt_id.as_bytes().as_slice())
            .bind(task_id.as_bytes().as_slice())
            .bind(account_id.as_bytes().as_slice())
            .bind(snapshot_seq)
            .bind(cursor.first_seen_at_us)
            .bind(cursor.first_seen_at_us)
            .bind(cursor.id.as_bytes().as_slice())
            .bind(i64::from(page.limit) + 1)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT e.id,e.code,e.relative_path_display,e.occurrences,e.first_seen_at_us,
                        e.first_seen_seq
                 FROM tasks_scan_tasks t JOIN tasks_scan_errors e ON e.attempt_id=?
                 WHERE t.id=? AND t.account_id=? AND e.first_seen_seq<=?
                 ORDER BY e.first_seen_at_us ASC,e.id ASC LIMIT ?",
            )
            .bind(attempt_id.as_bytes().as_slice())
            .bind(task_id.as_bytes().as_slice())
            .bind(account_id.as_bytes().as_slice())
            .bind(snapshot_seq)
            .bind(i64::from(page.limit) + 1)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(internal)?;
        let has_more = rows.len() > page.limit as usize;
        let selected = rows.iter().take(page.limit as usize).collect::<Vec<_>>();
        let items = selected
            .iter()
            .map(|row| {
                Ok(ScanErrorView {
                    id: uuid(row, "id")?,
                    code: row.get("code"),
                    relative_path: row.get("relative_path_display"),
                    occurrences: u64::try_from(row.get::<i64, _>("occurrences"))
                        .map_err(internal)?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let next_cursor = if has_more {
            selected
                .last()
                .map(|row| {
                    encode_cursor(&ErrorCursor {
                        version: 2,
                        account_id,
                        task_id,
                        attempt_id,
                        first_seen_at_us: row.get("first_seen_at_us"),
                        id: uuid(row, "id").expect("selected error IDs were decoded above"),
                        snapshot_seq,
                    })
                })
                .transpose()?
        } else {
            None
        };
        Ok(CursorPage { items, next_cursor })
    }

    async fn ensure_task_owned(&self, account_id: Uuid, task_id: Uuid) -> Result<(), AppError> {
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tasks_scan_tasks
             WHERE id=? AND account_id=? AND reason='manual'",
        )
        .bind(task_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .fetch_one(&self.pool)
        .await
        .map_err(internal)?;
        if exists == 1 {
            Ok(())
        } else {
            Err(AppError::new(ErrorCode::TaskNotFound, "task not found"))
        }
    }
}

async fn mark_unseen_revisions_missing(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    inbox_id: Uuid,
    batch_id: Uuid,
    now_us: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE discovery_file_revision_states
         SET status='missing',missing_at_us=?,next_check_at_us=NULL,
             version=version+1,updated_at_us=?
         WHERE revision_id IN (
           SELECT tracked.current_revision_id FROM discovery_tracked_files tracked
           WHERE tracked.inbox_directory_id=? AND tracked.current_revision_id IS NOT NULL
             AND NOT EXISTS (
               SELECT 1 FROM discovery_scan_file_observations observed
               JOIN discovery_files file ON file.id=observed.discovered_file_id
               WHERE observed.scan_batch_id=?
                 AND file.inbox_directory_id=tracked.inbox_directory_id
                 AND file.relative_path_bytes=tracked.relative_path_bytes
             )
         ) AND status!='missing'",
    )
    .bind(now_us)
    .bind(now_us)
    .bind(inbox_id.as_bytes().as_slice())
    .bind(batch_id.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    sqlx::query(
        "UPDATE discovery_tracked_files AS tracked
         SET missing_at_us=?,version=version+1,updated_at_us=?
         WHERE inbox_directory_id=? AND NOT EXISTS (
           SELECT 1 FROM discovery_scan_file_observations observed
           JOIN discovery_files file ON file.id=observed.discovered_file_id
           WHERE observed.scan_batch_id=?
             AND file.inbox_directory_id=tracked.inbox_directory_id
             AND file.relative_path_bytes=tracked.relative_path_bytes
         )",
    )
    .bind(now_us)
    .bind(now_us)
    .bind(inbox_id.as_bytes().as_slice())
    .bind(batch_id.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(internal)
}

fn decode_task_cursor(value: &str) -> Result<TaskCursor, AppError> {
    decode_cursor(value, |cursor: &TaskCursor| cursor.version == 3, "task")
}

fn encode_cursor(cursor: &impl Serialize) -> Result<String, AppError> {
    let payload = serde_json::to_vec(cursor).map_err(internal)?;
    let envelope = CursorEnvelope {
        payload: URL_SAFE_NO_PAD.encode(&payload),
        checksum: cursor_checksum(&payload),
    };
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).map_err(internal)?);
    if encoded.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(AppError::new(
            ErrorCode::Internal,
            "encoded cursor exceeds the bounded cursor size",
        ));
    }
    Ok(encoded)
}

fn decode_file_cursor(value: &str) -> Result<FileCursor, AppError> {
    decode_compact_file_cursor(value)
        .or_else(|_| decode_cursor(value, |cursor: &FileCursor| cursor.version == 2, "file"))
}

fn encode_file_cursor(cursor: &FileCursor) -> Result<String, AppError> {
    let mut payload =
        Vec::with_capacity(FILE_CURSOR_FIXED_PAYLOAD_BYTES + cursor.relative_path_bytes.len());
    payload.extend_from_slice(FILE_CURSOR_MAGIC);
    payload.extend_from_slice(cursor.account_id.as_bytes());
    payload.extend_from_slice(cursor.task_id.as_bytes());
    payload.extend_from_slice(cursor.id.as_bytes());
    payload.extend_from_slice(&cursor.snapshot_seq.to_be_bytes());
    payload.extend_from_slice(&cursor.relative_path_bytes);

    let mut bytes = Vec::with_capacity(payload.len() + FILE_CURSOR_CHECKSUM_BYTES);
    bytes.extend_from_slice(&payload);
    bytes.extend_from_slice(&file_cursor_checksum(&payload));
    let encoded = URL_SAFE_NO_PAD.encode(bytes);
    if encoded.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(AppError::new(
            ErrorCode::Internal,
            "encoded file cursor exceeds the bounded cursor size",
        ));
    }
    Ok(encoded)
}

fn decode_compact_file_cursor(value: &str) -> Result<FileCursor, AppError> {
    if value.is_empty() || value.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(invalid_file_cursor());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| invalid_file_cursor())?;
    if bytes.len() < FILE_CURSOR_FIXED_PAYLOAD_BYTES + FILE_CURSOR_CHECKSUM_BYTES {
        return Err(invalid_file_cursor());
    }
    let payload_end = bytes.len() - FILE_CURSOR_CHECKSUM_BYTES;
    let (payload, checksum) = bytes.split_at(payload_end);
    if payload.get(..FILE_CURSOR_MAGIC.len()) != Some(FILE_CURSOR_MAGIC)
        || checksum != file_cursor_checksum(payload)
    {
        return Err(invalid_file_cursor());
    }

    let account_id = Uuid::from_slice(&payload[4..20]).map_err(|_| invalid_file_cursor())?;
    let task_id = Uuid::from_slice(&payload[20..36]).map_err(|_| invalid_file_cursor())?;
    let id = Uuid::from_slice(&payload[36..52]).map_err(|_| invalid_file_cursor())?;
    let snapshot_seq = i64::from_be_bytes(
        payload[52..60]
            .try_into()
            .map_err(|_| invalid_file_cursor())?,
    );
    if snapshot_seq < 0 {
        return Err(invalid_file_cursor());
    }
    Ok(FileCursor {
        version: 3,
        account_id,
        task_id,
        relative_path_bytes: payload[60..].to_vec(),
        id,
        snapshot_seq,
    })
}

fn file_cursor_checksum(payload: &[u8]) -> [u8; FILE_CURSOR_CHECKSUM_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.file-cursor.v3\0");
    hasher.update(payload);
    let digest = hasher.finalize();
    let mut checksum = [0_u8; FILE_CURSOR_CHECKSUM_BYTES];
    checksum.copy_from_slice(&digest[..FILE_CURSOR_CHECKSUM_BYTES]);
    checksum
}

fn invalid_file_cursor() -> AppError {
    AppError::new(ErrorCode::ValidationFailed, "invalid file cursor")
}

fn decode_error_cursor(value: &str) -> Result<ErrorCursor, AppError> {
    decode_cursor(value, |cursor: &ErrorCursor| cursor.version == 2, "error")
}

fn decode_cursor<T: for<'de> Deserialize<'de>>(
    value: &str,
    valid: impl FnOnce(&T) -> bool,
    kind: &str,
) -> Result<T, AppError> {
    if value.is_empty() || value.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            format!("invalid {kind} cursor"),
        ));
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorEnvelope>(&bytes).ok())
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::ValidationFailed,
                format!("invalid {kind} cursor"),
            )
        })?;
    let payload = URL_SAFE_NO_PAD.decode(envelope.payload).map_err(|_| {
        AppError::new(
            ErrorCode::ValidationFailed,
            format!("invalid {kind} cursor"),
        )
    })?;
    if envelope.checksum != cursor_checksum(&payload) {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            format!("invalid {kind} cursor"),
        ));
    }
    let cursor = serde_json::from_slice::<T>(&payload)
        .ok()
        .filter(valid)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::ValidationFailed,
                format!("invalid {kind} cursor"),
            )
        })?;
    Ok(cursor)
}

fn cursor_checksum(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.cursor.v2\0");
    hasher.update(payload);
    hex::encode(&hasher.finalize()[..16])
}

fn format_ns(value: i64) -> Result<String, AppError> {
    let seconds = value.div_euclid(1_000_000_000);
    let nanos = u32::try_from(value.rem_euclid(1_000_000_000)).map_err(internal)?;
    chrono::DateTime::from_timestamp(seconds, nanos)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true))
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "stored file timestamp invalid"))
}

async fn insert_idempotency(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: Uuid,
    action: &str,
    target_id: Uuid,
    digest: &[u8; 32],
    result_task_id: Uuid,
    now_us: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO tasks_idempotency_bindings
         (account_id,key_sha256,action,target_id,result_task_id,created_at_us)
         VALUES (?,?,?,?,?,?)",
    )
    .bind(account_id.as_bytes().as_slice())
    .bind(digest.as_slice())
    .bind(action)
    .bind(target_id.as_bytes().as_slice())
    .bind(result_task_id.as_bytes().as_slice())
    .bind(now_us)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    sqlx::query(
        "INSERT INTO tasks_idempotency_keys
         (account_id,action,target_id,key_sha256,result_task_id,created_at_us)
         VALUES (?,?,?,?,?,?)",
    )
    .bind(account_id.as_bytes().as_slice())
    .bind(action)
    .bind(target_id.as_bytes().as_slice())
    .bind(digest.as_slice())
    .bind(result_task_id.as_bytes().as_slice())
    .bind(now_us)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(())
}

async fn idempotent_result_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: Uuid,
    action: &str,
    target_id: Uuid,
    digest: &[u8; 32],
) -> Result<Option<ScanTaskView>, AppError> {
    idempotent_result_on(&mut **tx, account_id, action, target_id, digest).await
}

async fn idempotent_result_on<'e, E>(
    executor: E,
    account_id: Uuid,
    action: &str,
    target_id: Uuid,
    digest: &[u8; 32],
) -> Result<Option<ScanTaskView>, AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row = sqlx::query(
        "SELECT k.action,k.target_id,t.id,t.inbox_directory_id,t.status,t.recovering,
                t.visited_directories,t.observed_files,t.skipped_entries,t.errors
         FROM tasks_idempotency_bindings k JOIN tasks_scan_tasks t ON t.id=k.result_task_id
         WHERE k.account_id=? AND k.key_sha256=?",
    )
    .bind(account_id.as_bytes().as_slice())
    .bind(digest.as_slice())
    .fetch_optional(executor)
    .await
    .map_err(internal)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored_action: String = row.get("action");
    let stored_target = uuid(&row, "target_id")?;
    if stored_action != action || stored_target != target_id {
        return Err(AppError::new(
            ErrorCode::RequestConflict,
            "idempotency key is bound to a different request",
        ));
    }
    decode_task(&row).map(Some)
}

fn decode_task(row: &sqlx::sqlite::SqliteRow) -> Result<ScanTaskView, AppError> {
    Ok(ScanTaskView {
        id: uuid(row, "id")?,
        inbox_directory_id: uuid(row, "inbox_directory_id")?,
        status: decode_status(row.get::<String, _>("status").as_str())?,
        recovering: row.get::<i64, _>("recovering") != 0,
        counts: ScanCounts {
            visited_directories: u64::try_from(row.get::<i64, _>("visited_directories"))
                .map_err(internal)?,
            observed_files: u64::try_from(row.get::<i64, _>("observed_files")).map_err(internal)?,
            skipped_entries: u64::try_from(row.get::<i64, _>("skipped_entries"))
                .map_err(internal)?,
            errors: u64::try_from(row.get::<i64, _>("errors")).map_err(internal)?,
        },
    })
}

fn decode_status(value: &str) -> Result<ScanStatus, AppError> {
    match value {
        "queued" => Ok(ScanStatus::Queued),
        "running" => Ok(ScanStatus::Running),
        "partial-success" => Ok(ScanStatus::PartialSuccess),
        "completed" => Ok(ScanStatus::Completed),
        "failed" => Ok(ScanStatus::Failed),
        "cancelled" => Ok(ScanStatus::Cancelled),
        _ => Err(AppError::new(
            ErrorCode::Internal,
            "stored task status invalid",
        )),
    }
}

fn uuid(row: &sqlx::sqlite::SqliteRow, name: &str) -> Result<Uuid, AppError> {
    Uuid::from_slice(row.get::<Vec<u8>, _>(name).as_slice()).map_err(internal)
}

fn validate_idempotency_key(value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        Err(AppError::new(
            ErrorCode::ValidationFailed,
            "invalid idempotency key",
        ))
    } else {
        Ok(())
    }
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

#[cfg(test)]
mod tests {
    use super::{FileCursor, decode_file_cursor, encode_cursor};
    use uuid::Uuid;

    #[test]
    fn file_cursor_decoder_keeps_existing_v2_cursors_compatible() {
        let legacy = FileCursor {
            version: 2,
            account_id: Uuid::now_v7(),
            task_id: Uuid::now_v7(),
            relative_path_bytes: b"movie.mkv".to_vec(),
            id: Uuid::now_v7(),
            snapshot_seq: 42,
        };
        let encoded = encode_cursor(&legacy).expect("旧版短游标应可编码");

        let decoded = decode_file_cursor(&encoded).expect("旧版文件游标应继续可读");

        assert_eq!(decoded.version, legacy.version);
        assert_eq!(decoded.account_id, legacy.account_id);
        assert_eq!(decoded.task_id, legacy.task_id);
        assert_eq!(decoded.relative_path_bytes, legacy.relative_path_bytes);
        assert_eq!(decoded.id, legacy.id);
        assert_eq!(decoded.snapshot_seq, legacy.snapshot_seq);
    }
}
