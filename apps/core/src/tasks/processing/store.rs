#![allow(clippy::too_many_lines)]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{SecondsFormat, TimeZone as _, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};
use uuid::Uuid;

use crate::platform::outbox::{OutboxNotifier, OutboxWriter};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::id::new_id;
use crate::shared::page::{CursorPage, PageRequest};
use crate::tasks::processing::model::{
    DecisionCheckpoint, DecisionDispatch, ProcessingAttemptReason, ProcessingCheckpoint,
    ProcessingLease, ProcessingReason, ProcessingStage, ProcessingStatus, ProcessingTaskAction,
    ProcessingTaskFilter, ProcessingTaskPageView, ProcessingTaskView, TaskCenterSummary,
    TaskCenterView,
};

/// 处理租约除非续期，否则会在 30 秒后过期。
pub const PROCESSING_LEASE_DURATION_US: i64 = 30_000_000;
/// 长时间运行的阶段处理器每 10 秒续期一次处理租约。
pub const PROCESSING_LEASE_RENEW_INTERVAL_US: i64 = 10_000_000;

#[derive(Deserialize, Serialize)]
struct ProcessingCursor {
    version: u8,
    account_id: Uuid,
    updated_at_us: i64,
    id: Uuid,
    snapshot_revision: i64,
}

#[derive(Deserialize, Serialize)]
struct TaskCenterCursor {
    version: u8,
    account_id: Uuid,
    filter_digest: String,
    updated_at_us: i64,
    id: Uuid,
    snapshot_revision: i64,
}

#[derive(Deserialize, Serialize)]
struct CursorEnvelope {
    payload: String,
    checksum: String,
}

#[derive(Clone)]
/// 绑定不可变 revision 的处理任务事务存储。
pub struct ProcessingStore {
    pool: SqlitePool,
    notifier: OutboxNotifier,
}

impl ProcessingStore {
    #[must_use]
    /// 使用独立通知器创建存储；事件会持久化但不会唤醒共享监听者。
    pub fn new(pool: SqlitePool) -> Self {
        Self::new_with_notifier(pool, OutboxNotifier::new())
    }

    #[must_use]
    /// 使用共享通知器创建存储，在任务事务提交后唤醒 outbox/SSE 交付。
    pub fn new_with_notifier(pool: SqlitePool, notifier: OutboxNotifier) -> Self {
        Self { pool, notifier }
    }

    /// 读取内部阶段协调器绑定的账号；账号不会因此进入公开任务投影。
    ///
    /// # Errors
    ///
    /// 任务不存在时返回未找到；存储 UUID 无效或数据库失败时返回内部错误。
    pub async fn account_id(&self, task_id: Uuid) -> Result<Uuid, AppError> {
        let bytes = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT account_id FROM tasks_processing_tasks WHERE id=?",
        )
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "processing task not found"))?;
        parse_uuid(&bytes)
    }

    /// 读取从发现流程进入处理流程的持久请求有界有序页。
    ///
    /// # Errors
    ///
    /// `limit` 超出范围时返回校验错误；存储 UUID 无效或数据库失败时返回内部错误。
    pub async fn pending_revision_ids(&self, limit: u32) -> Result<Vec<Uuid>, AppError> {
        if limit == 0 || limit > 1000 {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "processing request limit is outside bounds",
            ));
        }
        sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT revision_id FROM discovery_processing_requests
             WHERE status='pending' ORDER BY created_at_us,id LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?
        .into_iter()
        .map(|bytes| parse_uuid(&bytes))
        .collect()
    }

    /// 确保稳定 revision 唯一对应一个处理任务，并消费其持久化请求。
    ///
    /// # Errors
    ///
    /// revision 不稳定或不存在时返回任务状态或未找到错误；任一原子任务、尝试、请求、快照或
    /// outbox 写入失败时返回内部错误。
    pub async fn ensure_revision(
        &self,
        revision_id: Uuid,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let existing = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT id FROM tasks_processing_tasks WHERE file_revision_id=?",
        )
        .bind(revision_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?;
        if let Some(task_id) = existing {
            sqlx::query(
                "UPDATE discovery_processing_requests
                 SET status='completed',completed_at_us=?,claimed_at_us=COALESCE(claimed_at_us,?)
                 WHERE revision_id=?",
            )
            .bind(now_us)
            .bind(now_us)
            .bind(revision_id.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            tx.commit().await.map_err(internal)?;
            return self.get_by_id(parse_uuid(&task_id)?).await;
        }
        let row = sqlx::query(
            "SELECT r.tracked_file_id,r.policy_version,r.minimum_age_seconds,
                    r.stable_observation_interval_seconds,f.inbox_directory_id,
                    f.relative_path_display,a.id AS account_id,s.status
             FROM discovery_file_revisions r
             JOIN discovery_file_revision_states s ON s.revision_id=r.id
             JOIN discovery_tracked_files f ON f.id=r.tracked_file_id
             JOIN identity_accounts a ON a.singleton_key=1
             WHERE r.id=?",
        )
        .bind(revision_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "revision not found"))?;
        if row.get::<String, _>("status") != "stable" {
            return Err(AppError::new(
                ErrorCode::TaskInvalidState,
                "revision is not stable",
            ));
        }
        let task_id = new_id();
        let attempt_id = new_id();
        let tracked_file_id = parse_uuid(&row.get::<Vec<u8>, _>("tracked_file_id"))?;
        let inbox_id = parse_uuid(&row.get::<Vec<u8>, _>("inbox_directory_id"))?;
        let account_id = parse_uuid(&row.get::<Vec<u8>, _>("account_id"))?;
        let snapshot = serde_json::json!({
            "discovery_policy_version": row.get::<i64, _>("policy_version"),
            "minimum_age_seconds": row.get::<i64, _>("minimum_age_seconds"),
            "stable_observation_interval_seconds": row
                .get::<i64, _>("stable_observation_interval_seconds"),
            "processing_schema_version": 1,
        })
        .to_string();
        sqlx::query(
            "INSERT INTO tasks_processing_tasks
             (id,account_id,discovered_file_id,file_revision_id,inbox_directory_id,
              current_attempt_id,status,stage,checkpoint,reason,recovering,attempt_count,
              cancel_requested,lease_owner,lease_expires_at_us,next_retry_at_us,
              config_snapshot_json,version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,'queued','identification','pending',NULL,0,1,0,NULL,NULL,NULL,?,1,?,?)",
        )
        .bind(task_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(tracked_file_id.as_bytes().as_slice())
        .bind(revision_id.as_bytes().as_slice())
        .bind(inbox_id.as_bytes().as_slice())
        .bind(attempt_id.as_bytes().as_slice())
        .bind(snapshot)
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "INSERT INTO tasks_processing_attempts
             (id,task_id,reason,ordinal,status,stage,created_at_us)
             VALUES (?,?,?,1,'queued','identification',?)",
        )
        .bind(attempt_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .bind(ProcessingAttemptReason::Initial.as_str())
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "UPDATE discovery_processing_requests
             SET status='completed',claimed_at_us=?,completed_at_us=? WHERE revision_id=?",
        )
        .bind(now_us)
        .bind(now_us)
        .bind(revision_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        OutboxWriter::write(
            &mut tx,
            "processing-task.state-changed",
            task_id,
            &serde_json::json!({
                "status":"queued","stage":"identification","recovering":false,"reason":null
            }),
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        self.get_by_id(task_id).await
    }

    /// 读取账户拥有的处理任务。
    ///
    /// # Errors
    ///
    /// 任务不属于所属账户时返回未找到；存储状态无效或数据库失败时返回内部错误。
    pub async fn get(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<ProcessingTaskView, AppError> {
        let row = task_query(
            "WHERE t.id=? AND t.account_id=?",
            &self.pool,
            &[task_id, account_id],
        )
        .await?;
        row.map(|row| decode_task(&row))
            .transpose()?
            .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "processing task not found"))
    }

    /// 为工作器与运行时协调读取不受账户边界限制的任务。
    ///
    /// # Errors
    ///
    /// 任务不存在时返回未找到；存储状态无效或数据库失败时返回内部错误。
    pub async fn get_by_task_id(&self, task_id: Uuid) -> Result<ProcessingTaskView, AppError> {
        self.get_by_id(task_id).await
    }

    /// 返回按 `(updated_at_us DESC,id DESC)` 排序的快照稳定任务页面。
    ///
    /// # Errors
    ///
    /// 游标无效或跨账户时返回校验错误；数据库、存储值、时间戳或游标序列化失败时返回内部错误。
    pub async fn list_tasks(
        &self,
        account_id: Uuid,
        page: &PageRequest,
    ) -> Result<CursorPage<ProcessingTaskView>, AppError> {
        let cursor = page
            .cursor
            .as_deref()
            .map(decode_processing_cursor)
            .transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.account_id != account_id)
        {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "processing cursor query does not match",
            ));
        }
        let snapshot_revision = if let Some(cursor) = &cursor {
            Some(cursor.snapshot_revision)
        } else {
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT MAX(revision) FROM tasks_processing_task_order_history
                 WHERE account_id=?",
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
                     FROM tasks_processing_task_order_history history
                     WHERE history.account_id=? AND history.revision<=?
                       AND history.revision=(
                         SELECT MAX(candidate.revision)
                         FROM tasks_processing_task_order_history candidate
                         WHERE candidate.task_id=history.task_id AND candidate.revision<=?
                       )
                 )
                 SELECT t.id,t.inbox_directory_id,t.file_revision_id,f.relative_path_display,
                        t.status,t.stage,t.checkpoint,t.decision_checkpoint,
                        t.current_task_decision_id,t.organization_plan_id,
                        t.organization_result_id,t.catalog_media_item_id,
                        t.reason,t.recovering,t.attempt_count,
                        t.next_retry_at_us,t.updated_at_us,
                        snapshot_order.updated_at_us AS snapshot_updated_at_us
                 FROM snapshot_order
                 JOIN tasks_processing_tasks t ON t.id=snapshot_order.task_id
                 JOIN discovery_tracked_files f ON f.id=t.discovered_file_id
                 WHERE snapshot_order.updated_at_us<? OR
                       (snapshot_order.updated_at_us=? AND t.id<?)
                 ORDER BY snapshot_order.updated_at_us DESC,t.id DESC LIMIT ?",
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
                     FROM tasks_processing_task_order_history history
                     WHERE history.account_id=? AND history.revision<=?
                       AND history.revision=(
                         SELECT MAX(candidate.revision)
                         FROM tasks_processing_task_order_history candidate
                         WHERE candidate.task_id=history.task_id AND candidate.revision<=?
                       )
                 )
                 SELECT t.id,t.inbox_directory_id,t.file_revision_id,f.relative_path_display,
                        t.status,t.stage,t.checkpoint,t.decision_checkpoint,
                        t.current_task_decision_id,t.organization_plan_id,
                        t.organization_result_id,t.catalog_media_item_id,
                        t.reason,t.recovering,t.attempt_count,
                        t.next_retry_at_us,t.updated_at_us,
                        snapshot_order.updated_at_us AS snapshot_updated_at_us
                 FROM snapshot_order
                 JOIN tasks_processing_tasks t ON t.id=snapshot_order.task_id
                 JOIN discovery_tracked_files f ON f.id=t.discovered_file_id
                 ORDER BY snapshot_order.updated_at_us DESC,t.id DESC LIMIT ?",
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
            if let Some(row) = selected.last() {
                Some(encode_processing_cursor(&ProcessingCursor {
                    version: 1,
                    account_id,
                    updated_at_us: row.get("snapshot_updated_at_us"),
                    id: uuid(row, "id")?,
                    snapshot_revision,
                })?)
            } else {
                None
            }
        } else {
            None
        };
        Ok(CursorPage { items, next_cursor })
    }

    /// 从同一不可变投影快照返回一页有界任务中心数据及计数。
    ///
    /// # Errors
    ///
    /// 过滤条件、限制无效，或游标绑定到其他账户或过滤集合时返回校验错误；数据库或存储投影出错时
    /// 返回内部错误。
    pub async fn list_task_center(
        &self,
        account_id: Uuid,
        filter: &ProcessingTaskFilter,
        page: &PageRequest,
    ) -> Result<ProcessingTaskPageView, AppError> {
        let filter = normalize_task_filter(filter)?;
        let digest = task_filter_digest(&filter);
        let cursor = page
            .cursor
            .as_deref()
            .map(decode_task_center_cursor)
            .transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.account_id != account_id || cursor.filter_digest != digest)
        {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "processing cursor query does not match",
            ));
        }
        let snapshot_revision = if let Some(cursor) = &cursor {
            Some(cursor.snapshot_revision)
        } else {
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT MAX(revision) FROM tasks_processing_task_order_history
                 WHERE account_id=?",
            )
            .bind(account_id.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .map_err(internal)?
        };
        let Some(snapshot_revision) = snapshot_revision else {
            return Ok(ProcessingTaskPageView {
                items: Vec::new(),
                next_cursor: None,
                summary: TaskCenterSummary {
                    pending: 0,
                    running: 0,
                    all: 0,
                    completed: 0,
                    snapshot_version: 0,
                },
            });
        };

        let summary = self
            .task_center_summary(account_id, snapshot_revision, &filter)
            .await?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "WITH snapshot_tasks AS (
                 SELECT history.* FROM tasks_processing_task_order_history history
                 WHERE history.account_id=",
        );
        query
            .push_bind(account_id.as_bytes().to_vec())
            .push(" AND history.revision<=")
            .push_bind(snapshot_revision)
            .push(
                " AND history.revision=(
                     SELECT MAX(candidate.revision)
                     FROM tasks_processing_task_order_history candidate
                     WHERE candidate.task_id=history.task_id AND candidate.revision<=",
            )
            .push_bind(snapshot_revision)
            .push(
                ")
             )
             SELECT t.id,t.inbox_directory_id,t.file_revision_id,f.relative_path_display,
                    snapshot.status,snapshot.stage,snapshot.checkpoint,
                    snapshot.decision_checkpoint,snapshot.current_task_decision_id,
                    t.organization_plan_id,t.organization_result_id,t.catalog_media_item_id,
                    snapshot.reason,snapshot.recovering,snapshot.attempt_count,
                    snapshot.next_retry_at_us,snapshot.updated_at_us,
                    snapshot.updated_at_us AS snapshot_updated_at_us
             FROM snapshot_tasks snapshot
             JOIN tasks_processing_tasks t ON t.id=snapshot.task_id
             JOIN discovery_tracked_files f ON f.id=t.discovered_file_id
             WHERE 1=1",
            );
        push_task_center_filters(&mut query, &filter, true);
        if let Some(cursor) = &cursor {
            query
                .push(" AND (snapshot.updated_at_us<")
                .push_bind(cursor.updated_at_us)
                .push(" OR (snapshot.updated_at_us=")
                .push_bind(cursor.updated_at_us)
                .push(" AND t.id<")
                .push_bind(cursor.id.as_bytes().to_vec())
                .push("))");
        }
        query
            .push(" ORDER BY snapshot.updated_at_us DESC,t.id DESC LIMIT ")
            .push_bind(i64::from(page.limit) + 1);
        let rows = query
            .build()
            .fetch_all(&self.pool)
            .await
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
                    encode_task_center_cursor(&TaskCenterCursor {
                        version: 1,
                        account_id,
                        filter_digest: digest.clone(),
                        updated_at_us: row.get("snapshot_updated_at_us"),
                        id: uuid(row, "id")?,
                        snapshot_revision,
                    })
                })
                .transpose()?
        } else {
            None
        };
        Ok(ProcessingTaskPageView {
            items,
            next_cursor,
            summary,
        })
    }

    async fn task_center_summary(
        &self,
        account_id: Uuid,
        snapshot_revision: i64,
        filter: &ProcessingTaskFilter,
    ) -> Result<TaskCenterSummary, AppError> {
        let mut query = QueryBuilder::<Sqlite>::new(
            "WITH snapshot_tasks AS (
                 SELECT history.* FROM tasks_processing_task_order_history history
                 WHERE history.account_id=",
        );
        query
            .push_bind(account_id.as_bytes().to_vec())
            .push(" AND history.revision<=")
            .push_bind(snapshot_revision)
            .push(
                " AND history.revision=(
                     SELECT MAX(candidate.revision)
                     FROM tasks_processing_task_order_history candidate
                     WHERE candidate.task_id=history.task_id AND candidate.revision<=",
            )
            .push_bind(snapshot_revision)
            .push(
                ")
             )
             SELECT
               COALESCE(SUM(CASE WHEN snapshot.status IN
                 ('waiting-confirmation','paused') THEN 1 ELSE 0 END),0) AS pending,
               COALESCE(SUM(CASE WHEN snapshot.status IN
                 ('queued','running') THEN 1 ELSE 0 END),0) AS running,
               COUNT(*) AS total,
               COALESCE(SUM(CASE WHEN snapshot.status IN
                 ('partial-success','completed','failed') THEN 1 ELSE 0 END),0) AS completed
             FROM snapshot_tasks snapshot
             JOIN tasks_processing_tasks t ON t.id=snapshot.task_id
             JOIN discovery_tracked_files f ON f.id=t.discovered_file_id
             WHERE 1=1",
            );
        push_task_center_filters(&mut query, filter, false);
        let row = query
            .build()
            .fetch_one(&self.pool)
            .await
            .map_err(internal)?;
        Ok(TaskCenterSummary {
            pending: u64::try_from(row.get::<i64, _>("pending")).map_err(internal)?,
            running: u64::try_from(row.get::<i64, _>("running")).map_err(internal)?,
            all: u64::try_from(row.get::<i64, _>("total")).map_err(internal)?,
            completed: u64::try_from(row.get::<i64, _>("completed")).map_err(internal)?,
            snapshot_version: u64::try_from(snapshot_revision).map_err(internal)?,
        })
    }

    /// 领取阶段已有已注册处理器的最早符合条件任务。
    ///
    /// # Errors
    ///
    /// 持久化状态无效或事务、outbox 失败时返回内部错误。
    pub async fn claim_next(
        &self,
        owner: &str,
        registered_stages: &[ProcessingStage],
        now_us: i64,
    ) -> Result<Option<ProcessingLease>, AppError> {
        if registered_stages.is_empty() {
            return Ok(None);
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let mut builder = QueryBuilder::<Sqlite>::new(
            "SELECT id,current_attempt_id,status,stage,recovering,attempt_count,version \
             FROM tasks_processing_tasks WHERE (status='queued' \
             OR (status='paused' AND next_retry_at_us IS NOT NULL AND next_retry_at_us<=",
        );
        builder.push_bind(now_us);
        builder.push(") OR (status='running' AND lease_expires_at_us<=");
        builder.push_bind(now_us);
        builder.push(")) AND stage IN (");
        let mut separated = builder.separated(",");
        for stage in registered_stages {
            separated.push_bind(stage.as_str());
        }
        separated.push_unseparated(") ORDER BY created_at_us,id LIMIT 1");
        let row = builder
            .build()
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?;
        let Some(row) = row else {
            tx.rollback().await.map_err(internal)?;
            return Ok(None);
        };
        let task_id = uuid(&row, "id")?;
        let previous_attempt_id = uuid(&row, "current_attempt_id")?;
        let status: String = row.get("status");
        let stage = parse_stage(row.get::<String, _>("stage").as_str())?;
        let old_version: i64 = row.get("version");
        let old_attempt_count: i64 = row.get("attempt_count");
        let recovering = status == "running" || row.get::<i64, _>("recovering") != 0;
        let (attempt_id, attempt_count) = if status == "queued" {
            let updated = sqlx::query(
                "UPDATE tasks_processing_attempts
                 SET status='running',lease_owner=?,started_at_us=?
                 WHERE id=? AND status='queued'",
            )
            .bind(owner)
            .bind(now_us)
            .bind(previous_attempt_id.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            if updated.rows_affected() != 1 {
                tx.rollback().await.map_err(internal)?;
                return Ok(None);
            }
            (previous_attempt_id, old_attempt_count)
        } else {
            if status == "running" {
                sqlx::query(
                    "UPDATE tasks_processing_attempts
                     SET status='failed',failure_code='task.lease-lost',lease_owner=NULL,
                         finished_at_us=?
                     WHERE id=? AND status='running'",
                )
                .bind(now_us)
                .bind(previous_attempt_id.as_bytes().as_slice())
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
            }
            let attempt_id = new_id();
            let reason = if status == "running" {
                ProcessingAttemptReason::LeaseRecovery
            } else {
                ProcessingAttemptReason::DependencyRecovery
            };
            sqlx::query(
                "INSERT INTO tasks_processing_attempts
                 (id,task_id,reason,ordinal,status,stage,lease_owner,started_at_us,created_at_us)
                 VALUES (?,?,?,?,'running',?,?,?,?)",
            )
            .bind(attempt_id.as_bytes().as_slice())
            .bind(task_id.as_bytes().as_slice())
            .bind(reason.as_str())
            .bind(old_attempt_count + 1)
            .bind(stage.as_str())
            .bind(owner)
            .bind(now_us)
            .bind(now_us)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            (attempt_id, old_attempt_count + 1)
        };
        let expires_at_us = now_us.saturating_add(PROCESSING_LEASE_DURATION_US);
        let updated = sqlx::query(
            "UPDATE tasks_processing_tasks
             SET status='running',current_attempt_id=?,recovering=?,attempt_count=?,
                 reason=NULL,cancel_requested=0,lease_owner=?,lease_expires_at_us=?,
                 next_retry_at_us=NULL,
                 version=version+1,updated_at_us=?
             WHERE id=? AND version=? AND (status='queued'
                OR (status='paused' AND next_retry_at_us IS NOT NULL AND next_retry_at_us<=?)
                OR (status='running' AND lease_expires_at_us<=?))",
        )
        .bind(attempt_id.as_bytes().as_slice())
        .bind(i64::from(recovering))
        .bind(attempt_count)
        .bind(owner)
        .bind(expires_at_us)
        .bind(now_us)
        .bind(task_id.as_bytes().as_slice())
        .bind(old_version)
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        if updated.rows_affected() != 1 {
            tx.rollback().await.map_err(internal)?;
            return Ok(None);
        }
        write_state_event(
            &mut tx,
            task_id,
            ProcessingStatus::Running,
            stage,
            recovering,
            None,
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        let task = self.get_by_id(task_id).await?;
        Ok(Some(ProcessingLease {
            task,
            attempt_id,
            owner: owner.to_owned(),
            version: old_version + 1,
            expires_at_us,
        }))
    }

    /// 在精确匹配所有者与版本时续期未过期的处理租约。
    ///
    /// # Errors
    ///
    /// 乐观租约不再匹配时返回 `task-lease-lost`；数据库或时间戳失败时返回内部错误。
    pub async fn renew(
        &self,
        lease: &ProcessingLease,
        now_us: i64,
    ) -> Result<ProcessingLease, AppError> {
        let expires_at_us = now_us.saturating_add(PROCESSING_LEASE_DURATION_US);
        let result = sqlx::query(
            "UPDATE tasks_processing_tasks SET lease_expires_at_us=?,version=version+1,updated_at_us=?
             WHERE id=? AND version=? AND status='running' AND lease_owner=?
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
                "processing lease renewal failed",
            ));
        }
        let mut renewed = lease.clone();
        renewed.version += 1;
        renewed.expires_at_us = expires_at_us;
        renewed.task.updated_at = timestamp(now_us)?;
        Ok(renewed)
    }

    /// 读取工作器在安全检查点观察的持久化取消标志。
    ///
    /// # Errors
    ///
    /// 任务不存在时返回未找到；数据库失败时返回内部错误。
    pub async fn cancel_requested(&self, task_id: Uuid) -> Result<bool, AppError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT cancel_requested FROM tasks_processing_tasks WHERE id=?",
        )
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .map(|value| value != 0)
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "processing task not found"))
    }

    /// 在持久化依赖检查点暂停一次运行中的尝试。
    ///
    /// # Errors
    ///
    /// 授权过期时返回 `task-lease-lost`；终态事务或 outbox 写入失败时返回内部错误。
    pub async fn finish_paused(
        &self,
        lease: &ProcessingLease,
        reason: ProcessingReason,
        next_retry_at_us: i64,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.finish_terminal_attempt(
            lease,
            FinishTransition {
                status: ProcessingStatus::Paused,
                stage: lease.task.stage,
                checkpoint: ProcessingCheckpoint::DependencyBlocked,
                reason: Some(reason),
                next_retry_at_us: Some(next_retry_at_us),
                attempt_status: "failed",
                failure_code: Some(reason.as_str()),
            },
            now_us,
        )
        .await
    }

    /// 提交识别检查点并将尚未注册的规划阶段排队。
    ///
    /// # Errors
    ///
    /// 授权过期时返回 `task-lease-lost`；检查点事务或 outbox 写入失败时返回内部错误。
    pub async fn finish_identification_complete(
        &self,
        lease: &ProcessingLease,
        reason: ProcessingReason,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        assert_live_lease_update(
            &mut tx,
            lease,
            "UPDATE tasks_processing_tasks
             SET status='queued',stage='planning',checkpoint='identification-complete',reason=?,
                 recovering=0,cancel_requested=0,lease_owner=NULL,lease_expires_at_us=NULL,
                 next_retry_at_us=NULL,version=version+1,updated_at_us=?
             WHERE id=? AND version=? AND status='running' AND lease_owner=?
               AND lease_expires_at_us>?",
            &[SqlValue::Text(reason.as_str()), SqlValue::I64(now_us)],
            now_us,
        )
        .await?;
        sqlx::query(
            "UPDATE tasks_processing_attempts SET status='queued',stage='planning',lease_owner=NULL
             WHERE id=? AND status='running'",
        )
        .bind(lease.attempt_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        write_state_event(
            &mut tx,
            lease.task.id,
            ProcessingStatus::Queued,
            ProcessingStage::Planning,
            false,
            Some(reason),
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        self.get_by_id(lease.task.id).await
    }

    /// 提交当前 plan 的稳定引用，并根据授权结论进入文件阶段或规划暂停态。
    ///
    /// # Errors
    ///
    /// 租约失效、引用无效或事务/outbox 失败时返回稳定应用错误。
    pub async fn finish_organization_plan(
        &self,
        lease: &ProcessingLease,
        plan_id: Uuid,
        authorized: bool,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        let transition = if authorized {
            OrganizationTransition {
                status: ProcessingStatus::Queued,
                stage: ProcessingStage::FileOperation,
                checkpoint: ProcessingCheckpoint::ExecutionAuthorized,
                reason: None,
                plan_id: Some(plan_id),
                result_id: None,
                catalog_media_item_id: None,
                next_retry_at_us: None,
                continue_attempt: true,
            }
        } else {
            OrganizationTransition {
                status: ProcessingStatus::Paused,
                stage: ProcessingStage::Planning,
                checkpoint: ProcessingCheckpoint::PlanningPaused,
                reason: Some(ProcessingReason::OrganizationPlanPaused),
                plan_id: Some(plan_id),
                result_id: None,
                catalog_media_item_id: None,
                next_retry_at_us: None,
                continue_attempt: false,
            }
        };
        self.finish_organization_transition(lease, transition, now_us)
            .await
    }

    /// 在无法构造安全计划时提交规划暂停态，不伪造 plan 引用。
    ///
    /// # Errors
    ///
    /// 租约失效或事务/outbox 失败时返回稳定应用错误。
    pub async fn finish_organization_planning_failed(
        &self,
        lease: &ProcessingLease,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.finish_organization_transition(
            lease,
            OrganizationTransition {
                status: ProcessingStatus::Paused,
                stage: ProcessingStage::Planning,
                checkpoint: ProcessingCheckpoint::PlanningPaused,
                reason: Some(ProcessingReason::OrganizationPlanPaused),
                plan_id: None,
                result_id: None,
                catalog_media_item_id: None,
                next_retry_at_us: None,
                continue_attempt: false,
            },
            now_us,
        )
        .await
    }

    /// 提交已核验文件结果，并进入 NFO 或完成阶段。
    ///
    /// # Errors
    ///
    /// 租约失效、引用无效或事务/outbox 失败时返回稳定应用错误。
    pub async fn finish_organization_file(
        &self,
        lease: &ProcessingLease,
        result_id: Uuid,
        nfo_pending: bool,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.finish_organization_transition(
            lease,
            OrganizationTransition {
                status: ProcessingStatus::Queued,
                stage: if nfo_pending {
                    ProcessingStage::Nfo
                } else {
                    ProcessingStage::Completion
                },
                checkpoint: if nfo_pending {
                    ProcessingCheckpoint::NfoPending
                } else {
                    ProcessingCheckpoint::LocalResultPrepared
                },
                reason: None,
                plan_id: None,
                result_id: Some(result_id),
                catalog_media_item_id: None,
                next_retry_at_us: None,
                continue_attempt: true,
            },
            now_us,
        )
        .await
    }

    /// 在文件 journal 的安全恢复边界暂停当前尝试。
    ///
    /// # Errors
    ///
    /// 租约失效或事务/outbox 失败时返回稳定应用错误。
    pub async fn finish_organization_io_pending(
        &self,
        lease: &ProcessingLease,
        next_retry_at_us: i64,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.finish_organization_transition(
            lease,
            OrganizationTransition {
                status: ProcessingStatus::Paused,
                stage: ProcessingStage::FileOperation,
                checkpoint: ProcessingCheckpoint::FileOperationExecuting,
                reason: Some(ProcessingReason::OrganizationIoTemporary),
                plan_id: None,
                result_id: None,
                catalog_media_item_id: None,
                next_retry_at_us: Some(next_retry_at_us),
                continue_attempt: false,
            },
            now_us,
        )
        .await
    }

    /// 提交需要人工处理的文件事实，不再自动重放该 operation。
    ///
    /// # Errors
    ///
    /// 租约失效或事务/outbox 失败时返回稳定应用错误。
    pub async fn finish_organization_manual_review(
        &self,
        lease: &ProcessingLease,
        result_id: Option<Uuid>,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.finish_organization_transition(
            lease,
            OrganizationTransition {
                status: ProcessingStatus::Paused,
                stage: ProcessingStage::FileOperation,
                checkpoint: ProcessingCheckpoint::FileOperationManualReview,
                reason: Some(ProcessingReason::OrganizationManualReview),
                plan_id: None,
                result_id,
                catalog_media_item_id: None,
                next_retry_at_us: None,
                continue_attempt: false,
            },
            now_us,
        )
        .await
    }

    /// 提交 NFO 阶段结果；失败保留可用媒体结果并允许只重试 NFO。
    ///
    /// # Errors
    ///
    /// 租约失效、结果引用无效或事务/outbox 失败时返回稳定应用错误。
    pub async fn finish_organization_nfo(
        &self,
        lease: &ProcessingLease,
        result_id: Uuid,
        succeeded: bool,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.finish_organization_transition(
            lease,
            OrganizationTransition {
                status: if succeeded {
                    ProcessingStatus::Queued
                } else {
                    ProcessingStatus::PartialSuccess
                },
                stage: if succeeded {
                    ProcessingStage::Completion
                } else {
                    ProcessingStage::Nfo
                },
                checkpoint: if succeeded {
                    ProcessingCheckpoint::LocalResultPrepared
                } else {
                    ProcessingCheckpoint::NfoFailed
                },
                reason: (!succeeded).then_some(ProcessingReason::OrganizationNfoFailed),
                plan_id: None,
                result_id: Some(result_id),
                catalog_media_item_id: None,
                next_retry_at_us: None,
                continue_attempt: succeeded,
            },
            now_us,
        )
        .await
    }

    /// 提交 Catalog 幂等结果并完成处理任务。
    ///
    /// # Errors
    ///
    /// 租约失效、结果引用无效或事务/outbox 失败时返回稳定应用错误。
    pub async fn finish_organization_completed(
        &self,
        lease: &ProcessingLease,
        result_id: Uuid,
        media_item_id: Uuid,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.finish_organization_transition(
            lease,
            OrganizationTransition {
                status: ProcessingStatus::Completed,
                stage: ProcessingStage::Completion,
                checkpoint: ProcessingCheckpoint::CatalogCommitted,
                reason: None,
                plan_id: None,
                result_id: Some(result_id),
                catalog_media_item_id: Some(media_item_id),
                next_retry_at_us: None,
                continue_attempt: false,
            },
            now_us,
        )
        .await
    }

    /// 在 Catalog 暂时不可用时保留本地结果并从 completion 阶段恢复。
    ///
    /// # Errors
    ///
    /// 租约失效、结果引用无效或事务/outbox 失败时返回稳定应用错误。
    pub async fn finish_organization_catalog_pending(
        &self,
        lease: &ProcessingLease,
        result_id: Uuid,
        next_retry_at_us: i64,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.finish_organization_transition(
            lease,
            OrganizationTransition {
                status: ProcessingStatus::Paused,
                stage: ProcessingStage::Completion,
                checkpoint: ProcessingCheckpoint::LocalResultPrepared,
                reason: Some(ProcessingReason::OrganizationCatalogUnavailable),
                plan_id: None,
                result_id: Some(result_id),
                catalog_media_item_id: None,
                next_retry_at_us: Some(next_retry_at_us),
                continue_attempt: false,
            },
            now_us,
        )
        .await
    }

    /// 在显式人工确认状态完成一次尝试。
    ///
    /// # Errors
    ///
    /// 授权过期时返回 `task-lease-lost`；终态事务或 outbox 写入失败时返回内部错误。
    pub async fn finish_waiting_confirmation(
        &self,
        lease: &ProcessingLease,
        reason: ProcessingReason,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.finish_terminal_attempt(
            lease,
            FinishTransition {
                status: ProcessingStatus::WaitingConfirmation,
                stage: ProcessingStage::Identification,
                checkpoint: ProcessingCheckpoint::WaitingConfirmation,
                reason: Some(reason),
                next_retry_at_us: None,
                attempt_status: "succeeded",
                failure_code: None,
            },
            now_us,
        )
        .await
    }

    /// 在取消检查点后停止一次运行中的尝试。
    ///
    /// # Errors
    ///
    /// 授权过期时返回 `task-lease-lost`；终态事务或 outbox 写入失败时返回内部错误。
    pub async fn finish_cancelled(
        &self,
        lease: &ProcessingLease,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.finish_terminal_attempt(
            lease,
            FinishTransition {
                status: ProcessingStatus::Cancelled,
                stage: lease.task.stage,
                checkpoint: ProcessingCheckpoint::Cancelled,
                reason: None,
                next_retry_at_us: None,
                attempt_status: "cancelled",
                failure_code: None,
            },
            now_us,
        )
        .await
    }

    /// 为每个租约已过期的运行任务创建新的租约恢复尝试并重新排队。
    ///
    /// # Errors
    ///
    /// 持久化状态意外竞争时返回 `task-lease-lost`；状态无效或事务、outbox 失败时返回内部错误。
    pub async fn reclaim_expired(&self, now_us: i64) -> Result<u64, AppError> {
        let mut total = 0_u64;
        loop {
            let mut tx = self
                .pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .map_err(internal)?;
            let rows = sqlx::query(
                "SELECT id,current_attempt_id,stage,attempt_count,version
                 FROM tasks_processing_tasks
                 WHERE status='running' AND lease_expires_at_us<=?
                 ORDER BY lease_expires_at_us,id LIMIT 1000",
            )
            .bind(now_us)
            .fetch_all(&mut *tx)
            .await
            .map_err(internal)?;
            if rows.is_empty() {
                tx.rollback().await.map_err(internal)?;
                break;
            }
            let batch_len = rows.len();
            for row in rows {
                let task_id = uuid(&row, "id")?;
                let previous_attempt = uuid(&row, "current_attempt_id")?;
                let stage = parse_stage(row.get::<String, _>("stage").as_str())?;
                let attempt_count: i64 = row.get("attempt_count");
                let version: i64 = row.get("version");
                sqlx::query(
                    "UPDATE tasks_processing_attempts
                     SET status='failed',failure_code='task.lease-lost',lease_owner=NULL,
                         finished_at_us=?
                     WHERE id=? AND status='running'",
                )
                .bind(now_us)
                .bind(previous_attempt.as_bytes().as_slice())
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
                let attempt_id = new_id();
                sqlx::query(
                    "INSERT INTO tasks_processing_attempts
                     (id,task_id,reason,ordinal,status,stage,created_at_us)
                     VALUES (?,?,?,?,'queued',?,?)",
                )
                .bind(attempt_id.as_bytes().as_slice())
                .bind(task_id.as_bytes().as_slice())
                .bind(ProcessingAttemptReason::LeaseRecovery.as_str())
                .bind(attempt_count + 1)
                .bind(stage.as_str())
                .bind(now_us)
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
                let updated = sqlx::query(
                    "UPDATE tasks_processing_tasks
                     SET status='queued',current_attempt_id=?,recovering=1,
                         attempt_count=attempt_count+1,cancel_requested=0,lease_owner=NULL,
                         lease_expires_at_us=NULL,version=version+1,updated_at_us=?
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
                if updated.rows_affected() != 1 {
                    return Err(AppError::new(
                        ErrorCode::TaskLeaseLost,
                        "processing recovery raced task state",
                    ));
                }
                write_state_event(
                    &mut tx,
                    task_id,
                    ProcessingStatus::Queued,
                    stage,
                    true,
                    None,
                    now_us,
                )
                .await?;
            }
            tx.commit().await.map_err(internal)?;
            self.notifier.notify_after_commit();
            total = total.saturating_add(u64::try_from(batch_len).map_err(internal)?);
            if batch_len < 1000 {
                break;
            }
        }
        Ok(total)
    }

    /// 在任务边界恰好应用一次不可变人工决定派发。
    ///
    /// # Errors
    ///
    /// 任务不属于所属账户时返回未找到；任务未等待识别确认时返回无效状态；收据、尝试或 outbox
    /// 出错时返回内部错误。
    pub async fn apply_manual_decision(
        &self,
        account_id: Uuid,
        dispatch: &DecisionDispatch,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        let task_id = dispatch.task_id();
        let decision_id = dispatch.decision_id();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let receipt = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT receipt.task_id FROM tasks_processing_manual_decision_receipts receipt
             JOIN tasks_processing_tasks task ON task.id=receipt.task_id
             WHERE receipt.decision_id=? AND task.account_id=?",
        )
        .bind(decision_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?;
        if let Some(receipt_task_id) = receipt {
            if parse_uuid(&receipt_task_id)? != task_id {
                return Err(AppError::new(
                    ErrorCode::Internal,
                    "manual decision receipt targets another task",
                ));
            }
            tx.commit().await.map_err(internal)?;
            return self.get(account_id, task_id).await;
        }
        let task = sqlx::query(
            "SELECT status,stage,attempt_count,current_attempt_id
             FROM tasks_processing_tasks WHERE id=? AND account_id=?",
        )
        .bind(task_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "processing task not found"))?;
        if task.get::<String, _>("status") != "waiting-confirmation"
            || task.get::<String, _>("stage") != "identification"
        {
            return Err(AppError::new(
                ErrorCode::TaskInvalidState,
                "processing task is not waiting for a manual decision",
            ));
        }
        let old_attempt_id = parse_uuid(&task.get::<Vec<u8>, _>("current_attempt_id"))?;
        let attempt_count: i64 = task.get("attempt_count");
        let next_attempt_count = attempt_count
            .checked_add(1)
            .ok_or_else(|| AppError::new(ErrorCode::Internal, "attempt count overflow"))?;
        sqlx::query(
            "UPDATE tasks_processing_attempts
             SET status='cancelled',lease_owner=NULL,finished_at_us=COALESCE(finished_at_us,?)
             WHERE id=? AND status IN ('queued','running')",
        )
        .bind(now_us)
        .bind(old_attempt_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        let attempt_id = new_id();
        let (stage, checkpoint, task_decision_checkpoint, receipt_checkpoint) = match dispatch {
            DecisionDispatch::Reidentify { checkpoint, .. } => (
                ProcessingStage::Identification,
                ProcessingCheckpoint::Pending,
                *checkpoint,
                *checkpoint,
            ),
            DecisionDispatch::PlanningRequested { .. } => (
                ProcessingStage::Planning,
                ProcessingCheckpoint::IdentificationComplete,
                DecisionCheckpoint::PlanningRequested,
                DecisionCheckpoint::GenericVideoSelected,
            ),
        };
        sqlx::query(
            "INSERT INTO tasks_processing_attempts
             (id,task_id,reason,ordinal,status,stage,created_at_us)
             VALUES (?,?,'manual-retry',?,'queued',?,?)",
        )
        .bind(attempt_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .bind(next_attempt_count)
        .bind(stage.as_str())
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "INSERT INTO tasks_processing_manual_decision_receipts
             (decision_id,task_id,checkpoint,applied_at_us) VALUES (?,?,?,?)",
        )
        .bind(decision_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .bind(receipt_checkpoint.as_str())
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        let updated = sqlx::query(
            "UPDATE tasks_processing_tasks
             SET current_attempt_id=?,status='queued',stage=?,checkpoint=?,
                 decision_checkpoint=?,current_task_decision_id=?,reason=NULL,recovering=0,
                 attempt_count=?,cancel_requested=0,lease_owner=NULL,lease_expires_at_us=NULL,
                 next_retry_at_us=NULL,version=version+1,updated_at_us=?
             WHERE id=? AND account_id=? AND status='waiting-confirmation'
               AND stage='identification'",
        )
        .bind(attempt_id.as_bytes().as_slice())
        .bind(stage.as_str())
        .bind(checkpoint.as_str())
        .bind(task_decision_checkpoint.as_str())
        .bind(decision_id.as_bytes().as_slice())
        .bind(next_attempt_count)
        .bind(now_us)
        .bind(task_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        if updated.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::TaskInvalidState,
                "processing task manual decision raced state",
            ));
        }
        write_state_event(
            &mut tx,
            task_id,
            ProcessingStatus::Queued,
            stage,
            false,
            None,
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        self.get(account_id, task_id).await
    }

    /// 为等待确认、暂停、失败或已取消的任务幂等排队一次人工重试。
    ///
    /// # Errors
    ///
    /// 被拒绝的请求返回校验、冲突、未找到或无效状态错误；状态转换或 outbox 事务失败时返回内部错误。
    pub async fn retry(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.control_task(
            account_id,
            task_id,
            idempotency_key,
            "processing.retry",
            now_us,
        )
        .await
    }

    /// 幂等请求取消任务；排队、暂停或等待确认时立即取消，运行中则在下一个安全检查点取消。
    ///
    /// # Errors
    ///
    /// 被拒绝的请求返回校验、冲突、未找到或无效状态错误；状态转换或 outbox 事务失败时返回内部错误。
    pub async fn request_cancel(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        self.control_task(
            account_id,
            task_id,
            idempotency_key,
            "processing.cancel",
            now_us,
        )
        .await
    }

    async fn control_task(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: &str,
        action: &str,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        validate_idempotency_key(idempotency_key)?;
        let digest: [u8; 32] = Sha256::digest(idempotency_key.as_bytes()).into();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        if let Some(result_id) =
            processing_idempotent_result(&mut tx, account_id, task_id, action, &digest).await?
        {
            tx.rollback().await.map_err(internal)?;
            return self.get(account_id, result_id).await;
        }
        let row = sqlx::query(
            "SELECT status,stage,current_attempt_id,attempt_count,version
             FROM tasks_processing_tasks WHERE id=? AND account_id=?",
        )
        .bind(task_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "processing task not found"))?;
        let status = parse_status(row.get::<String, _>("status").as_str())?;
        let stage = parse_stage(row.get::<String, _>("stage").as_str())?;
        let attempt_id = uuid(&row, "current_attempt_id")?;
        let attempt_count: i64 = row.get("attempt_count");
        let version: i64 = row.get("version");
        let (event_status, event_reason) = if action == "processing.retry" {
            if !matches!(
                status,
                ProcessingStatus::Paused
                    | ProcessingStatus::WaitingConfirmation
                    | ProcessingStatus::Cancelled
                    | ProcessingStatus::PartialSuccess
                    | ProcessingStatus::Failed
            ) {
                return Err(AppError::new(
                    ErrorCode::TaskInvalidState,
                    "processing task cannot be retried",
                ));
            }
            let next_attempt = new_id();
            sqlx::query(
                "INSERT INTO tasks_processing_attempts
                 (id,task_id,reason,ordinal,status,stage,created_at_us)
                 VALUES (?,?,?,?,'queued',?,?)",
            )
            .bind(next_attempt.as_bytes().as_slice())
            .bind(task_id.as_bytes().as_slice())
            .bind(ProcessingAttemptReason::ManualRetry.as_str())
            .bind(attempt_count + 1)
            .bind(stage.as_str())
            .bind(now_us)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            sqlx::query(
                "UPDATE tasks_processing_tasks
                 SET status='queued',current_attempt_id=?,reason=NULL,recovering=0,
                     attempt_count=attempt_count+1,cancel_requested=0,next_retry_at_us=NULL,
                     version=version+1,updated_at_us=? WHERE id=? AND version=?",
            )
            .bind(next_attempt.as_bytes().as_slice())
            .bind(now_us)
            .bind(task_id.as_bytes().as_slice())
            .bind(version)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            (ProcessingStatus::Queued, None)
        } else {
            match status {
                ProcessingStatus::Queued => {
                    sqlx::query(
                        "UPDATE tasks_processing_attempts
                         SET status='cancelled',finished_at_us=?
                         WHERE id=? AND status='queued'",
                    )
                    .bind(now_us)
                    .bind(attempt_id.as_bytes().as_slice())
                    .execute(&mut *tx)
                    .await
                    .map_err(internal)?;
                    sqlx::query(
                        "UPDATE tasks_processing_tasks
                         SET status='cancelled',checkpoint='cancelled',reason=NULL,
                             cancel_requested=0,next_retry_at_us=NULL,version=version+1,
                             updated_at_us=? WHERE id=? AND version=?",
                    )
                    .bind(now_us)
                    .bind(task_id.as_bytes().as_slice())
                    .bind(version)
                    .execute(&mut *tx)
                    .await
                    .map_err(internal)?;
                    (ProcessingStatus::Cancelled, None)
                }
                ProcessingStatus::Running => {
                    sqlx::query(
                        "UPDATE tasks_processing_tasks SET cancel_requested=1,updated_at_us=?
                         WHERE id=? AND version=? AND status='running'",
                    )
                    .bind(now_us)
                    .bind(task_id.as_bytes().as_slice())
                    .bind(version)
                    .execute(&mut *tx)
                    .await
                    .map_err(internal)?;
                    (ProcessingStatus::Running, None)
                }
                ProcessingStatus::Paused | ProcessingStatus::WaitingConfirmation => {
                    sqlx::query(
                        "UPDATE tasks_processing_tasks
                         SET status='cancelled',checkpoint='cancelled',reason=NULL,
                             cancel_requested=0,next_retry_at_us=NULL,version=version+1,
                             updated_at_us=? WHERE id=? AND version=?",
                    )
                    .bind(now_us)
                    .bind(task_id.as_bytes().as_slice())
                    .bind(version)
                    .execute(&mut *tx)
                    .await
                    .map_err(internal)?;
                    (ProcessingStatus::Cancelled, None)
                }
                ProcessingStatus::Cancelled
                | ProcessingStatus::PartialSuccess
                | ProcessingStatus::Completed
                | ProcessingStatus::Failed => {
                    return Err(AppError::new(
                        ErrorCode::TaskInvalidState,
                        "processing task cannot be cancelled",
                    ));
                }
            }
        };
        insert_processing_idempotency(
            &mut tx, account_id, task_id, action, &digest, task_id, now_us,
        )
        .await?;
        write_state_event(
            &mut tx,
            task_id,
            event_status,
            stage,
            false,
            event_reason,
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        self.get(account_id, task_id).await
    }

    async fn finish_terminal_attempt(
        &self,
        lease: &ProcessingLease,
        transition: FinishTransition,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let updated = sqlx::query(
            "UPDATE tasks_processing_tasks
             SET status=?,stage=?,checkpoint=?,reason=?,recovering=0,cancel_requested=0,
                 lease_owner=NULL,lease_expires_at_us=NULL,next_retry_at_us=?,
                 version=version+1,updated_at_us=?
             WHERE id=? AND version=? AND status='running' AND lease_owner=?
               AND lease_expires_at_us>?",
        )
        .bind(transition.status.as_str())
        .bind(transition.stage.as_str())
        .bind(transition.checkpoint.as_str())
        .bind(transition.reason.map(ProcessingReason::as_str))
        .bind(transition.next_retry_at_us)
        .bind(now_us)
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.version)
        .bind(&lease.owner)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        if updated.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::TaskLeaseLost,
                "processing terminal lease condition failed",
            ));
        }
        sqlx::query(
            "UPDATE tasks_processing_attempts
             SET status=?,failure_code=?,lease_owner=NULL,finished_at_us=?
             WHERE id=? AND status='running'",
        )
        .bind(transition.attempt_status)
        .bind(transition.failure_code)
        .bind(now_us)
        .bind(lease.attempt_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        write_state_event(
            &mut tx,
            lease.task.id,
            transition.status,
            transition.stage,
            false,
            transition.reason,
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        self.get_by_id(lease.task.id).await
    }

    async fn finish_organization_transition(
        &self,
        lease: &ProcessingLease,
        transition: OrganizationTransition,
        now_us: i64,
    ) -> Result<ProcessingTaskView, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let cancellation_requested = sqlx::query_scalar::<_, i64>(
            "SELECT cancel_requested FROM tasks_processing_tasks
             WHERE id=? AND version=? AND status='running' AND lease_owner=?
               AND lease_expires_at_us>?",
        )
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.version)
        .bind(&lease.owner)
        .bind(now_us)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::TaskLeaseLost,
                "processing organization lease condition failed",
            )
        })? != 0;
        let transition = if cancellation_requested && transition.continue_attempt {
            OrganizationTransition {
                status: ProcessingStatus::Cancelled,
                stage: lease.task.stage,
                checkpoint: ProcessingCheckpoint::Cancelled,
                reason: None,
                continue_attempt: false,
                ..transition
            }
        } else {
            transition
        };
        let updated = sqlx::query(
            "UPDATE tasks_processing_tasks
             SET status=?,stage=?,checkpoint=?,reason=?,recovering=0,cancel_requested=0,
                 lease_owner=NULL,lease_expires_at_us=NULL,next_retry_at_us=?,
                 organization_plan_id=COALESCE(?,organization_plan_id),
                 organization_result_id=COALESCE(?,organization_result_id),
                 catalog_media_item_id=COALESCE(?,catalog_media_item_id),
                 version=version+1,updated_at_us=?
             WHERE id=? AND version=? AND status='running' AND lease_owner=?
               AND lease_expires_at_us>?",
        )
        .bind(transition.status.as_str())
        .bind(transition.stage.as_str())
        .bind(transition.checkpoint.as_str())
        .bind(transition.reason.map(ProcessingReason::as_str))
        .bind(transition.next_retry_at_us)
        .bind(transition.plan_id.map(|id| id.as_bytes().to_vec()))
        .bind(transition.result_id.map(|id| id.as_bytes().to_vec()))
        .bind(
            transition
                .catalog_media_item_id
                .map(|id| id.as_bytes().to_vec()),
        )
        .bind(now_us)
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.version)
        .bind(&lease.owner)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        if updated.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::TaskLeaseLost,
                "processing organization lease condition failed",
            ));
        }
        if transition.continue_attempt {
            sqlx::query(
                "UPDATE tasks_processing_attempts
                 SET status='queued',stage=?,lease_owner=NULL
                 WHERE id=? AND status='running'",
            )
            .bind(transition.stage.as_str())
            .bind(lease.attempt_id.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        } else {
            let attempt_status = match transition.status {
                ProcessingStatus::Completed => "succeeded",
                ProcessingStatus::Cancelled => "cancelled",
                ProcessingStatus::Queued
                | ProcessingStatus::Running
                | ProcessingStatus::WaitingConfirmation
                | ProcessingStatus::Paused
                | ProcessingStatus::PartialSuccess
                | ProcessingStatus::Failed => "failed",
            };
            sqlx::query(
                "UPDATE tasks_processing_attempts
                 SET status=?,failure_code=?,lease_owner=NULL,finished_at_us=?
                 WHERE id=? AND status='running'",
            )
            .bind(attempt_status)
            .bind(transition.reason.map(ProcessingReason::as_str))
            .bind(now_us)
            .bind(lease.attempt_id.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        }
        write_state_event(
            &mut tx,
            lease.task.id,
            transition.status,
            transition.stage,
            false,
            transition.reason,
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        self.get_by_id(lease.task.id).await
    }

    async fn get_by_id(&self, task_id: Uuid) -> Result<ProcessingTaskView, AppError> {
        let row = sqlx::query(
            "SELECT t.id,t.inbox_directory_id,t.file_revision_id,f.relative_path_display,
                    t.status,t.stage,t.checkpoint,t.decision_checkpoint,
                    t.current_task_decision_id,t.organization_plan_id,
                    t.organization_result_id,t.catalog_media_item_id,
                    t.reason,t.recovering,t.attempt_count,
                    t.next_retry_at_us,t.updated_at_us
             FROM tasks_processing_tasks t
             JOIN discovery_tracked_files f ON f.id=t.discovered_file_id WHERE t.id=?",
        )
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "processing task not found"))?;
        decode_task(&row)
    }
}

#[derive(Clone, Copy)]
struct FinishTransition {
    status: ProcessingStatus,
    stage: ProcessingStage,
    checkpoint: ProcessingCheckpoint,
    reason: Option<ProcessingReason>,
    next_retry_at_us: Option<i64>,
    attempt_status: &'static str,
    failure_code: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct OrganizationTransition {
    status: ProcessingStatus,
    stage: ProcessingStage,
    checkpoint: ProcessingCheckpoint,
    reason: Option<ProcessingReason>,
    plan_id: Option<Uuid>,
    result_id: Option<Uuid>,
    catalog_media_item_id: Option<Uuid>,
    next_retry_at_us: Option<i64>,
    continue_attempt: bool,
}

enum SqlValue<'a> {
    Text(&'a str),
    I64(i64),
}

async fn assert_live_lease_update(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    lease: &ProcessingLease,
    sql: &str,
    prefix_values: &[SqlValue<'_>],
    now_us: i64,
) -> Result<(), AppError> {
    let mut query = sqlx::query(sql);
    for value in prefix_values {
        query = match value {
            SqlValue::Text(value) => query.bind(*value),
            SqlValue::I64(value) => query.bind(*value),
        };
    }
    let result = query
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.version)
        .bind(&lease.owner)
        .bind(now_us)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    if result.rows_affected() != 1 {
        return Err(AppError::new(
            ErrorCode::TaskLeaseLost,
            "processing terminal lease condition failed",
        ));
    }
    Ok(())
}

async fn write_state_event(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    task_id: Uuid,
    status: ProcessingStatus,
    stage: ProcessingStage,
    recovering: bool,
    reason: Option<ProcessingReason>,
    now_us: i64,
) -> Result<(), AppError> {
    OutboxWriter::write(
        tx,
        "processing-task.state-changed",
        task_id,
        &serde_json::json!({
            "status": status,
            "stage": stage,
            "recovering": recovering,
            "reason": reason,
        }),
        now_us,
    )
    .await
    .map(|_| ())
}

async fn processing_idempotent_result(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    account_id: Uuid,
    target_id: Uuid,
    action: &str,
    digest: &[u8; 32],
) -> Result<Option<Uuid>, AppError> {
    let row = sqlx::query(
        "SELECT action,target_id,result_task_id FROM tasks_processing_idempotency_bindings
         WHERE account_id=? AND key_sha256=?",
    )
    .bind(account_id.as_bytes().as_slice())
    .bind(digest.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.get::<String, _>("action") != action || uuid(&row, "target_id")? != target_id {
        return Err(AppError::new(
            ErrorCode::RequestConflict,
            "idempotency key is bound to a different processing request",
        ));
    }
    uuid(&row, "result_task_id").map(Some)
}

async fn insert_processing_idempotency(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    account_id: Uuid,
    target_id: Uuid,
    action: &str,
    digest: &[u8; 32],
    result_task_id: Uuid,
    now_us: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO tasks_processing_idempotency_bindings
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
    .map(|_| ())
    .map_err(internal)
}

fn validate_idempotency_key(value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        Err(AppError::new(
            ErrorCode::ValidationFailed,
            "invalid processing idempotency key",
        ))
    } else {
        Ok(())
    }
}

fn normalize_task_filter(filter: &ProcessingTaskFilter) -> Result<ProcessingTaskFilter, AppError> {
    let mut normalized = filter.clone();
    normalized.query = filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if normalized
        .query
        .as_ref()
        .is_some_and(|value| value.len() > 200 || value.chars().any(char::is_control))
    {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "processing task query is invalid",
        ));
    }
    Ok(normalized)
}

fn task_filter_digest(filter: &ProcessingTaskFilter) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.processing.task-center.filter.v1\0");
    hasher.update(filter.view.as_str().as_bytes());
    hasher.update([0]);
    if let Some(stage) = filter.stage {
        hasher.update(stage.as_str().as_bytes());
    }
    hasher.update([0]);
    if let Some(status) = filter.status {
        hasher.update(status.as_str().as_bytes());
    }
    hasher.update([0]);
    if let Some(inbox_directory_id) = filter.inbox_directory_id {
        hasher.update(inbox_directory_id.as_bytes());
    }
    hasher.update([0]);
    if let Some(query) = &filter.query {
        hasher.update(query.as_bytes());
    }
    hex::encode(&hasher.finalize()[..16])
}

fn push_task_center_filters(
    query: &mut QueryBuilder<'_, Sqlite>,
    filter: &ProcessingTaskFilter,
    include_view: bool,
) {
    if include_view {
        match filter.view {
            TaskCenterView::Pending => {
                query.push(" AND snapshot.status IN ('waiting-confirmation','paused')");
            }
            TaskCenterView::Running => {
                query.push(" AND snapshot.status IN ('queued','running')");
            }
            TaskCenterView::All => {}
            TaskCenterView::Completed => {
                query.push(" AND snapshot.status IN ('partial-success','completed','failed')");
            }
        }
    }
    if let Some(stage) = filter.stage {
        query.push(" AND snapshot.stage=").push_bind(stage.as_str());
    }
    if let Some(status) = filter.status {
        query
            .push(" AND snapshot.status=")
            .push_bind(status.as_str());
    }
    if let Some(inbox_directory_id) = filter.inbox_directory_id {
        query
            .push(" AND t.inbox_directory_id=")
            .push_bind(inbox_directory_id.as_bytes().to_vec());
    }
    if let Some(search) = &filter.query {
        let escaped = search
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        query
            .push(" AND (f.relative_path_display LIKE ")
            .push_bind(format!("%{escaped}%"))
            .push(" ESCAPE '\\' COLLATE NOCASE");
        let compact_id = search.replace('-', "");
        if !compact_id.is_empty()
            && compact_id.len() <= 32
            && compact_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            query
                .push(" OR lower(hex(t.id)) LIKE ")
                .push_bind(format!("{}%", compact_id.to_ascii_lowercase()));
        }
        query.push(")");
    }
}

fn encode_task_center_cursor(cursor: &TaskCenterCursor) -> Result<String, AppError> {
    let payload = serde_json::to_vec(cursor).map_err(internal)?;
    let envelope = CursorEnvelope {
        payload: URL_SAFE_NO_PAD.encode(&payload),
        checksum: task_center_cursor_checksum(&payload),
    };
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).map_err(internal)?);
    if encoded.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(AppError::new(
            ErrorCode::Internal,
            "processing task-center cursor exceeds the bounded size",
        ));
    }
    Ok(encoded)
}

fn decode_task_center_cursor(value: &str) -> Result<TaskCenterCursor, AppError> {
    if value.is_empty() || value.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(invalid_processing_cursor());
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorEnvelope>(&bytes).ok())
        .ok_or_else(invalid_processing_cursor)?;
    let payload = URL_SAFE_NO_PAD
        .decode(envelope.payload)
        .map_err(|_| invalid_processing_cursor())?;
    if envelope.checksum != task_center_cursor_checksum(&payload) {
        return Err(invalid_processing_cursor());
    }
    serde_json::from_slice::<TaskCenterCursor>(&payload)
        .ok()
        .filter(|cursor| cursor.version == 1)
        .ok_or_else(invalid_processing_cursor)
}

fn task_center_cursor_checksum(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.processing.task-center.cursor.v1\0");
    hasher.update(payload);
    hex::encode(&hasher.finalize()[..16])
}

fn encode_processing_cursor(cursor: &ProcessingCursor) -> Result<String, AppError> {
    let payload = serde_json::to_vec(cursor).map_err(internal)?;
    let envelope = CursorEnvelope {
        payload: URL_SAFE_NO_PAD.encode(&payload),
        checksum: processing_cursor_checksum(&payload),
    };
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).map_err(internal)?);
    if encoded.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(AppError::new(
            ErrorCode::Internal,
            "processing cursor exceeds the bounded size",
        ));
    }
    Ok(encoded)
}

fn decode_processing_cursor(value: &str) -> Result<ProcessingCursor, AppError> {
    if value.is_empty() || value.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(invalid_processing_cursor());
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorEnvelope>(&bytes).ok())
        .ok_or_else(invalid_processing_cursor)?;
    let payload = URL_SAFE_NO_PAD
        .decode(envelope.payload)
        .map_err(|_| invalid_processing_cursor())?;
    if envelope.checksum != processing_cursor_checksum(&payload) {
        return Err(invalid_processing_cursor());
    }
    serde_json::from_slice::<ProcessingCursor>(&payload)
        .ok()
        .filter(|cursor| cursor.version == 1)
        .ok_or_else(invalid_processing_cursor)
}

fn processing_cursor_checksum(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.processing.cursor.v1\0");
    hasher.update(payload);
    hex::encode(&hasher.finalize()[..16])
}

fn invalid_processing_cursor() -> AppError {
    AppError::new(
        ErrorCode::ValidationFailed,
        "invalid processing task cursor",
    )
}

async fn task_query(
    suffix: &str,
    pool: &SqlitePool,
    ids: &[Uuid],
) -> Result<Option<sqlx::sqlite::SqliteRow>, AppError> {
    let sql = format!(
        "SELECT t.id,t.inbox_directory_id,t.file_revision_id,f.relative_path_display,
                t.status,t.stage,t.checkpoint,t.decision_checkpoint,
                t.current_task_decision_id,t.organization_plan_id,
                t.organization_result_id,t.catalog_media_item_id,
                t.reason,t.recovering,t.attempt_count,
                t.next_retry_at_us,t.updated_at_us
         FROM tasks_processing_tasks t
         JOIN discovery_tracked_files f ON f.id=t.discovered_file_id {suffix}"
    );
    let mut query = sqlx::query(&sql);
    for id in ids {
        query = query.bind(id.as_bytes().as_slice());
    }
    query.fetch_optional(pool).await.map_err(internal)
}

fn decode_task(row: &sqlx::sqlite::SqliteRow) -> Result<ProcessingTaskView, AppError> {
    let attempt_count = u64::try_from(row.get::<i64, _>("attempt_count")).map_err(internal)?;
    let status = parse_status(row.get::<String, _>("status").as_str())?;
    Ok(ProcessingTaskView {
        id: uuid(row, "id")?,
        inbox_directory_id: uuid(row, "inbox_directory_id")?,
        file_revision_id: uuid(row, "file_revision_id")?,
        relative_path: row.get("relative_path_display"),
        status,
        stage: parse_stage(row.get::<String, _>("stage").as_str())?,
        checkpoint: parse_checkpoint(row.get::<String, _>("checkpoint").as_str())?,
        decision_checkpoint: row
            .get::<Option<String>, _>("decision_checkpoint")
            .as_deref()
            .map(parse_decision_checkpoint)
            .transpose()?,
        current_task_decision_id: row
            .get::<Option<Vec<u8>>, _>("current_task_decision_id")
            .as_deref()
            .map(parse_uuid)
            .transpose()?,
        organization_plan_id: row
            .get::<Option<Vec<u8>>, _>("organization_plan_id")
            .as_deref()
            .map(parse_uuid)
            .transpose()?,
        organization_result_id: row
            .get::<Option<Vec<u8>>, _>("organization_result_id")
            .as_deref()
            .map(parse_uuid)
            .transpose()?,
        catalog_media_item_id: row
            .get::<Option<Vec<u8>>, _>("catalog_media_item_id")
            .as_deref()
            .map(parse_uuid)
            .transpose()?,
        reason: row
            .get::<Option<String>, _>("reason")
            .as_deref()
            .map(parse_reason)
            .transpose()?,
        recovering: row.get::<i64, _>("recovering") != 0,
        attempt_count,
        next_retry_at: row
            .get::<Option<i64>, _>("next_retry_at_us")
            .map(timestamp)
            .transpose()?,
        allowed_actions: allowed_actions(status),
        updated_at: timestamp(row.get("updated_at_us"))?,
    })
}

fn parse_status(value: &str) -> Result<ProcessingStatus, AppError> {
    match value {
        "queued" => Ok(ProcessingStatus::Queued),
        "running" => Ok(ProcessingStatus::Running),
        "waiting-confirmation" => Ok(ProcessingStatus::WaitingConfirmation),
        "paused" => Ok(ProcessingStatus::Paused),
        "cancelled" => Ok(ProcessingStatus::Cancelled),
        "partial-success" => Ok(ProcessingStatus::PartialSuccess),
        "completed" => Ok(ProcessingStatus::Completed),
        "failed" => Ok(ProcessingStatus::Failed),
        _ => invalid("processing status"),
    }
}

fn allowed_actions(status: ProcessingStatus) -> Vec<ProcessingTaskAction> {
    match status {
        ProcessingStatus::WaitingConfirmation => vec![
            ProcessingTaskAction::Review,
            ProcessingTaskAction::Retry,
            ProcessingTaskAction::Cancel,
        ],
        ProcessingStatus::Paused => {
            vec![ProcessingTaskAction::Retry, ProcessingTaskAction::Cancel]
        }
        ProcessingStatus::Cancelled
        | ProcessingStatus::PartialSuccess
        | ProcessingStatus::Failed => {
            vec![ProcessingTaskAction::Retry]
        }
        ProcessingStatus::Queued | ProcessingStatus::Running => {
            vec![ProcessingTaskAction::Cancel]
        }
        ProcessingStatus::Completed => Vec::new(),
    }
}

fn parse_stage(value: &str) -> Result<ProcessingStage, AppError> {
    match value {
        "identification" => Ok(ProcessingStage::Identification),
        "planning" => Ok(ProcessingStage::Planning),
        "file-operation" => Ok(ProcessingStage::FileOperation),
        "nfo" => Ok(ProcessingStage::Nfo),
        "completion" => Ok(ProcessingStage::Completion),
        _ => invalid("processing stage"),
    }
}

fn parse_checkpoint(value: &str) -> Result<ProcessingCheckpoint, AppError> {
    match value {
        "pending" => Ok(ProcessingCheckpoint::Pending),
        "identification-complete" => Ok(ProcessingCheckpoint::IdentificationComplete),
        "planning-requested" => Ok(ProcessingCheckpoint::PlanningRequested),
        "plan-prepared" => Ok(ProcessingCheckpoint::PlanPrepared),
        "execution-authorized" => Ok(ProcessingCheckpoint::ExecutionAuthorized),
        "planning-paused" => Ok(ProcessingCheckpoint::PlanningPaused),
        "file-operation-prepared" => Ok(ProcessingCheckpoint::FileOperationPrepared),
        "file-operation-executing" => Ok(ProcessingCheckpoint::FileOperationExecuting),
        "file-operation-verified" => Ok(ProcessingCheckpoint::FileOperationVerified),
        "file-operation-manual-review" => Ok(ProcessingCheckpoint::FileOperationManualReview),
        "nfo-pending" => Ok(ProcessingCheckpoint::NfoPending),
        "nfo-verified" => Ok(ProcessingCheckpoint::NfoVerified),
        "nfo-failed" => Ok(ProcessingCheckpoint::NfoFailed),
        "local-result-prepared" => Ok(ProcessingCheckpoint::LocalResultPrepared),
        "catalog-committed" => Ok(ProcessingCheckpoint::CatalogCommitted),
        "waiting-confirmation" => Ok(ProcessingCheckpoint::WaitingConfirmation),
        "dependency-blocked" => Ok(ProcessingCheckpoint::DependencyBlocked),
        "skipped-auxiliary" => Ok(ProcessingCheckpoint::SkippedAuxiliary),
        "cancelled" => Ok(ProcessingCheckpoint::Cancelled),
        _ => invalid("processing checkpoint"),
    }
}

fn parse_decision_checkpoint(value: &str) -> Result<DecisionCheckpoint, AppError> {
    match value {
        "manual-decision-pending" => Ok(DecisionCheckpoint::ManualDecisionPending),
        "rematch-pending" => Ok(DecisionCheckpoint::RematchPending),
        "generic-video-selected" => Ok(DecisionCheckpoint::GenericVideoSelected),
        "planning-requested" => Ok(DecisionCheckpoint::PlanningRequested),
        _ => invalid("manual decision checkpoint"),
    }
}

fn parse_reason(value: &str) -> Result<ProcessingReason, AppError> {
    match value {
        "identification.ambiguous" => Ok(ProcessingReason::IdentificationAmbiguous),
        "identification.confirmed-external-id" => {
            Ok(ProcessingReason::IdentificationConfirmedExternalId)
        }
        "identification.confirmed-title-year" => {
            Ok(ProcessingReason::IdentificationConfirmedTitleYear)
        }
        "identification.multiple-strong-candidates" => {
            Ok(ProcessingReason::IdentificationMultipleStrongCandidates)
        }
        "identification.no-candidate" => Ok(ProcessingReason::IdentificationNoCandidate),
        "identification.probable-title" => Ok(ProcessingReason::IdentificationProbableTitle),
        "identification.provider-unavailable" => {
            Ok(ProcessingReason::IdentificationProviderUnavailable)
        }
        "identification.provider-unauthorized" => {
            Ok(ProcessingReason::IdentificationProviderUnauthorized)
        }
        "identification.revision-changed" => Ok(ProcessingReason::IdentificationRevisionChanged),
        "auxiliary.sample" => Ok(ProcessingReason::AuxiliarySample),
        "auxiliary.trailer" => Ok(ProcessingReason::AuxiliaryTrailer),
        "auxiliary.extra" => Ok(ProcessingReason::AuxiliaryExtra),
        "watcher.unavailable" => Ok(ProcessingReason::WatcherUnavailable),
        "reconcile.failed" => Ok(ProcessingReason::ReconcileFailed),
        "integration.unconfigured" => Ok(ProcessingReason::IntegrationUnconfigured),
        "integration.healthy" => Ok(ProcessingReason::IntegrationHealthy),
        "integration.unauthorized" => Ok(ProcessingReason::IntegrationUnauthorized),
        "integration.rate-limited" => Ok(ProcessingReason::IntegrationRateLimited),
        "integration.unavailable" => Ok(ProcessingReason::IntegrationUnavailable),
        "organization.plan-paused" => Ok(ProcessingReason::OrganizationPlanPaused),
        "organization.io-temporary" => Ok(ProcessingReason::OrganizationIoTemporary),
        "organization.manual-review" => Ok(ProcessingReason::OrganizationManualReview),
        "organization.nfo-failed" => Ok(ProcessingReason::OrganizationNfoFailed),
        "organization.catalog-unavailable" => Ok(ProcessingReason::OrganizationCatalogUnavailable),
        _ => invalid("processing reason"),
    }
}

fn timestamp(value: i64) -> Result<String, AppError> {
    Utc.timestamp_micros(value)
        .single()
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Micros, true))
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "stored timestamp is invalid"))
}

fn uuid(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Uuid, AppError> {
    parse_uuid(&row.get::<Vec<u8>, _>(column))
}

fn parse_uuid(bytes: &[u8]) -> Result<Uuid, AppError> {
    Uuid::from_slice(bytes).map_err(internal)
}

fn invalid<T>(kind: &str) -> Result<T, AppError> {
    Err(AppError::new(
        ErrorCode::Internal,
        format!("stored {kind} is invalid"),
    ))
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
