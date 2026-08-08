use std::sync::Arc;
use std::time::Duration;

use chrono::{SecondsFormat, TimeZone as _, Utc};
use serde::Serialize;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::events::{
    AUTOMATION_EVENT_CHANGED, AutomationEventChangedPayload, CATALOG_MEDIA_CHANGED,
    CatalogMediaChangedPayload, DOWNLOAD_TASK_CHANGED, DownloadTaskChangedPayload,
    IDENTIFICATION_ENHANCER_CHANGED, INBOX_DISCOVERY_HEALTH_CHANGED, INTEGRATION_HEALTH_CHANGED,
    IdentificationEnhancerChangedPayload, InboxDiscoveryHealthChangedPayload,
    IntegrationHealthChangedPayload, ORGANIZATION_RESULT_CHANGED, ORGANIZATION_TARGET_CHANGED,
    OrganizationResultChangedPayload, OrganizationTargetChangedPayload,
    PROCESSING_TASK_IDENTIFICATION_DECIDED, PROCESSING_TASK_STATE_CHANGED,
    ProcessingTaskIdentificationDecidedPayload, ProcessingTaskStateChangedPayload, SCHEMA_VERSION,
    StreamGapPayload, TASK_DECISION_ACCEPTED, TASK_PROGRESS, TASK_STATE_CHANGED,
    TaskDecisionAcceptedPayload, TaskEventEnvelope, TaskStateChangedPayload,
};
use crate::tasks::model::ScanCounts;

/// 单个任务持久化进度事件之间的最小经过微秒数。
pub const PROGRESS_INTERVAL_US: i64 = 250_000;
/// JavaScript 可精确表示、因而允许出现在事件线上 的最大整数。
pub const MAX_PUBLIC_EVENT_ID: i64 = 9_007_199_254_740_991;
/// 保留策略可以删除事件前所需的最小事件年龄。
pub const RETENTION_AGE_US: i64 = 24 * 60 * 60 * 1_000_000;
/// 任一已删除事件之后必须保留的较新事件数。
pub const RETAINED_SUCCESSORS: i64 = 100_000;
const READ_PAGE_MAX: u32 = 200;
const CLEANUP_BATCH: i64 = 1_000;

#[derive(Clone, Debug)]
/// 提交后用于唤醒 outbox/SSE 读取器的进程本地代际信号。
///
/// 克隆实例共享一个会回绕的代际计数器。它不携带事件数据，因此接收方必须在每次观测到变化后查询持久 outbox。
pub struct OutboxNotifier {
    generation: watch::Sender<u64>,
}

impl Default for OutboxNotifier {
    fn default() -> Self {
        Self::new()
    }
}

impl OutboxNotifier {
    #[must_use]
    /// 创建代际为零的通知器。
    pub fn new() -> Self {
        let (generation, _) = watch::channel(0);
        Self { generation }
    }

    #[must_use]
    /// 订阅代际变化；当前代际立即可用。
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }

    /// 递增代际并唤醒接收方。
    ///
    /// 此方法不验证事务状态；调用方只能在对应 outbox 事务提交后调用它，从而避免接收方因回滚事件被唤醒。
    pub fn notify_after_commit(&self) {
        self.generation
            .send_modify(|value| *value = value.wrapping_add(1));
    }
}

/// 在调用方拥有的事务中追加 schema-v1 任务事件的无状态辅助器。
pub struct OutboxWriter;

impl OutboxWriter {
    /// 将一个受支持的任务事件序列化并插入 `transaction`。
    ///
    /// 返回 ID 是 `SQLite` 行 ID。此方法既不提交也不通知订阅者；两者仍由调用方负责，以保持领域状态和事件可见性的原子性。
    ///
    /// # Errors
    ///
    /// 事件类型不在显式 schema-v1 白名单、负载 JSON 序列化失败或 `SQLite` 插入失败时，返回 [`AppError`]。
    pub async fn write<T: Serialize>(
        transaction: &mut Transaction<'_, Sqlite>,
        event_type: &str,
        aggregate_id: Uuid,
        payload: &T,
        now_us: i64,
    ) -> Result<i64, AppError> {
        if !matches!(
            event_type,
            TASK_PROGRESS
                | TASK_STATE_CHANGED
                | INBOX_DISCOVERY_HEALTH_CHANGED
                | PROCESSING_TASK_STATE_CHANGED
                | PROCESSING_TASK_IDENTIFICATION_DECIDED
                | TASK_DECISION_ACCEPTED
                | CATALOG_MEDIA_CHANGED
                | ORGANIZATION_TARGET_CHANGED
                | ORGANIZATION_RESULT_CHANGED
                | INTEGRATION_HEALTH_CHANGED
                | DOWNLOAD_TASK_CHANGED
                | AUTOMATION_EVENT_CHANGED
                | IDENTIFICATION_ENHANCER_CHANGED
        ) {
            return Err(AppError::new(
                ErrorCode::Internal,
                "unsupported outbox event type",
            ));
        }
        let payload = serde_json::to_string(payload)
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        let result = sqlx::query(
            "INSERT INTO platform_outbox_events
             (event_type,schema_version,aggregate_id,payload_json,committed_at_us)
             VALUES (?,'1',?,?,?)",
        )
        .bind(event_type)
        .bind(aggregate_id.as_bytes().as_slice())
        .bind(payload)
        .bind(now_us)
        .execute(&mut **transaction)
        .await
        .map_err(internal)?;
        Ok(result.last_insert_rowid())
    }

    /// 在同一事务中认领任务的进度间隔并追加进度事件。
    ///
    /// 任务不存在或在不足 250 ms 前发出进度时返回 `None`。若调用方未提交，认领和事件插入会一同回滚。
    ///
    /// # Errors
    ///
    /// 任一计数超过 [`MAX_PUBLIC_EVENT_ID`]，或间隔更新、负载序列化或 outbox 插入失败时，返回 [`AppError`]。
    pub async fn write_progress_if_due(
        transaction: &mut Transaction<'_, Sqlite>,
        task_id: Uuid,
        counts: &ScanCounts,
        now_us: i64,
    ) -> Result<Option<i64>, AppError> {
        if [
            counts.visited_directories,
            counts.observed_files,
            counts.skipped_entries,
            counts.errors,
        ]
        .into_iter()
        .any(|value| value > MAX_PUBLIC_EVENT_ID as u64)
        {
            return Err(AppError::new(
                ErrorCode::Internal,
                "progress count exceeds the public event range",
            ));
        }
        let due_before = now_us.saturating_sub(PROGRESS_INTERVAL_US);
        let claimed = sqlx::query(
            "UPDATE tasks_scan_tasks SET last_progress_event_at_us=?
             WHERE id=? AND (last_progress_event_at_us IS NULL OR last_progress_event_at_us<=?)",
        )
        .bind(now_us)
        .bind(task_id.as_bytes().as_slice())
        .bind(due_before)
        .execute(&mut **transaction)
        .await
        .map_err(internal)?;
        if claimed.rows_affected() == 0 {
            return Ok(None);
        }
        Self::write(transaction, TASK_PROGRESS, task_id, counts, now_us)
            .await
            .map(Some)
    }
}

#[derive(Clone)]
/// 用于有序、可重放任务事件的验证型 `SQLite` 读取器。
pub struct OutboxReader {
    pool: SqlitePool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 包含保留事件或合成保留缺口事件的重放结果。
///
/// [`OutboxReader::replay`] 返回值中的 `events` 与 `gap` 互斥。
pub struct ReplayBatch {
    /// 严格位于请求游标之后的有序事件。
    pub events: Vec<TaskEventEnvelope>,
    /// 请求游标早于保留历史时的合成缺口。
    pub gap: Option<TaskEventEnvelope>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// outbox 投递尝试中记录的稳定原因。
pub enum DeliveryFailureCode {
    /// SSE 对等方在投递完成前关闭。
    ClientDisconnected,
    /// 已持久化事件违反了 Schema 或负载不变量。
    DecodeFailed,
    /// 查询/重放持久 outbox 失败。
    ReaderFailed,
}

impl DeliveryFailureCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ClientDisconnected => "client.disconnected",
            Self::DecodeFailed => "event.decode_failed",
            Self::ReaderFailed => "event.reader_failed",
        }
    }
}

impl OutboxReader {
    #[must_use]
    /// 将重放和投递记账绑定到 `pool`。
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 读取至多 `limit` 个 ID 严格大于 `after_id` 的已验证事件。
    ///
    /// # Errors
    ///
    /// `after_id` 为负或 `limit` 不在 `1..=200` 时返回 [`ErrorCode::ValidationFailed`]。
    /// 查询失败或已存储事件格式错误时返回内部 [`AppError`]；解码失败还会尽力更新投递尝试。
    pub async fn after(
        &self,
        after_id: i64,
        limit: u32,
    ) -> Result<Vec<TaskEventEnvelope>, AppError> {
        if after_id < 0 || limit == 0 || limit > READ_PAGE_MAX {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "outbox page request is outside bounds",
            ));
        }
        let rows = sqlx::query(
            "SELECT id,event_type,schema_version,aggregate_id,payload_json,committed_at_us
             FROM platform_outbox_events WHERE id>? ORDER BY id ASC LIMIT ?",
        )
        .bind(after_id)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        self.decode_rows(rows).await
    }

    /// 返回最早保留事件 ID；outbox 为空时返回 `None`。
    ///
    /// # Errors
    ///
    /// `SQLite` 无法执行聚合查询时返回 [`AppError`]。
    pub async fn minimum_available_id(&self) -> Result<Option<i64>, AppError> {
        sqlx::query_scalar("SELECT MIN(id) FROM platform_outbox_events")
            .fetch_one(&self.pool)
            .await
            .map_err(internal)
    }

    /// 在 `cursor` 之后生成一致的重放分页，并检测已被保留策略移除的历史。
    ///
    /// 最小 ID 检查和行选择在一个读取事务中进行。早于 `minimum - 1` 的游标只产生带 `now_us` 时间戳的
    /// 合成缺口；`None` 从最早保留事件开始。
    ///
    /// # Errors
    ///
    /// 游标小于一或 limit 不在 `1..=200` 时返回 [`ErrorCode::ValidationFailed`]。事务/查询/提交失败、
    /// 缺口时间无效或已持久化事件格式错误时返回内部 [`AppError`]。
    pub async fn replay(
        &self,
        cursor: Option<i64>,
        limit: u32,
        now_us: i64,
    ) -> Result<ReplayBatch, AppError> {
        if cursor.is_some_and(|value| value < 1) || limit == 0 || limit > READ_PAGE_MAX {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "event cursor or page is invalid",
            ));
        }
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let minimum =
            sqlx::query_scalar::<_, Option<i64>>("SELECT MIN(id) FROM platform_outbox_events")
                .fetch_one(&mut *tx)
                .await
                .map_err(internal)?;
        let Some(minimum) = minimum else {
            tx.commit().await.map_err(internal)?;
            return Ok(ReplayBatch {
                events: Vec::new(),
                gap: None,
            });
        };
        if let Some(cursor) = cursor
            && cursor < minimum.saturating_sub(1)
        {
            tx.commit().await.map_err(internal)?;
            return Ok(ReplayBatch {
                events: Vec::new(),
                gap: Some(gap_event(minimum, now_us)?),
            });
        }
        let after = cursor.unwrap_or_else(|| minimum.saturating_sub(1));
        let rows = sqlx::query(
            "SELECT id,event_type,schema_version,aggregate_id,payload_json,committed_at_us
             FROM platform_outbox_events WHERE id>? ORDER BY id ASC LIMIT ?",
        )
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(ReplayBatch {
            events: self.decode_rows(rows).await?,
            gap: None,
        })
    }

    /// 递增 `id` 的投递尝试次数，并存储或清除其最新失败代码。
    ///
    /// 缺失事件是成功的空操作。
    ///
    /// # Errors
    ///
    /// `SQLite` 无法执行更新时返回 [`AppError`]。
    pub async fn record_delivery_attempt(
        &self,
        id: i64,
        failure: Option<DeliveryFailureCode>,
    ) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE platform_outbox_events
             SET delivery_attempts=delivery_attempts+1,last_delivery_error=? WHERE id=?",
        )
        .bind(failure.map(DeliveryFailureCode::as_str))
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(internal)?;
        Ok(())
    }

    async fn decode_rows(
        &self,
        rows: Vec<sqlx::sqlite::SqliteRow>,
    ) -> Result<Vec<TaskEventEnvelope>, AppError> {
        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let id: i64 = row.get("id");
            match decode_row(&row) {
                Ok(event) => events.push(event),
                Err(error) => {
                    let _ = self
                        .record_delivery_attempt(id, Some(DeliveryFailureCode::DecodeFailed))
                        .await;
                    return Err(error);
                }
            }
        }
        Ok(events)
    }
}

#[allow(clippy::too_many_lines)]
fn decode_row(row: &sqlx::sqlite::SqliteRow) -> Result<TaskEventEnvelope, AppError> {
    let id: i64 = row.get("id");
    let event_type: String = row.get("event_type");
    let schema_version: String = row.get("schema_version");
    if !(1..=MAX_PUBLIC_EVENT_ID).contains(&id) || schema_version != SCHEMA_VERSION {
        return Err(AppError::new(
            ErrorCode::Internal,
            "invalid outbox envelope",
        ));
    }
    let aggregate: Vec<u8> = row.get("aggregate_id");
    let task_id = Uuid::from_slice(&aggregate)
        .map_err(|_| AppError::new(ErrorCode::Internal, "invalid outbox aggregate"))?;
    let committed_at_us: i64 = row.get("committed_at_us");
    let occurred_at = rfc3339(committed_at_us)?;
    let payload: String = row.get("payload_json");
    match event_type.as_str() {
        TASK_PROGRESS => {
            let payload: ScanCounts = serde_json::from_str(&payload)
                .map_err(|_| AppError::new(ErrorCode::Internal, "invalid progress payload"))?;
            if [
                payload.visited_directories,
                payload.observed_files,
                payload.skipped_entries,
                payload.errors,
            ]
            .into_iter()
            .any(|value| value > MAX_PUBLIC_EVENT_ID as u64)
            {
                return Err(AppError::new(
                    ErrorCode::Internal,
                    "progress payload exceeds the public range",
                ));
            }
            Ok(TaskEventEnvelope::Progress {
                id,
                schema_version,
                occurred_at,
                task_id,
                payload,
            })
        }
        TASK_STATE_CHANGED => Ok(TaskEventEnvelope::StateChanged {
            id,
            schema_version,
            occurred_at,
            task_id,
            payload: serde_json::from_str::<TaskStateChangedPayload>(&payload)
                .map_err(|_| AppError::new(ErrorCode::Internal, "invalid state payload"))?,
        }),
        INBOX_DISCOVERY_HEALTH_CHANGED => Ok(TaskEventEnvelope::InboxDiscoveryHealthChanged {
            id,
            schema_version,
            occurred_at,
            task_id: None,
            payload: serde_json::from_str::<InboxDiscoveryHealthChangedPayload>(&payload).map_err(
                |_| AppError::new(ErrorCode::Internal, "invalid discovery health payload"),
            )?,
        }),
        PROCESSING_TASK_STATE_CHANGED => Ok(TaskEventEnvelope::ProcessingTaskStateChanged {
            id,
            schema_version,
            occurred_at,
            task_id,
            payload: serde_json::from_str::<ProcessingTaskStateChangedPayload>(&payload).map_err(
                |_| AppError::new(ErrorCode::Internal, "invalid processing state payload"),
            )?,
        }),
        PROCESSING_TASK_IDENTIFICATION_DECIDED => {
            Ok(TaskEventEnvelope::ProcessingTaskIdentificationDecided {
                id,
                schema_version,
                occurred_at,
                task_id,
                payload: serde_json::from_str::<ProcessingTaskIdentificationDecidedPayload>(
                    &payload,
                )
                .map_err(|_| {
                    AppError::new(
                        ErrorCode::Internal,
                        "invalid identification decision payload",
                    )
                })?,
            })
        }
        TASK_DECISION_ACCEPTED => {
            decode_task_decision_event(id, schema_version, occurred_at, task_id, &payload)
        }
        CATALOG_MEDIA_CHANGED => Ok(TaskEventEnvelope::CatalogMediaChanged {
            id,
            schema_version,
            occurred_at,
            task_id: None,
            payload: serde_json::from_str::<CatalogMediaChangedPayload>(&payload)
                .map_err(|_| AppError::new(ErrorCode::Internal, "invalid catalog payload"))?,
        }),
        ORGANIZATION_TARGET_CHANGED => Ok(TaskEventEnvelope::OrganizationTargetChanged {
            id,
            schema_version,
            occurred_at,
            task_id: None,
            payload: serde_json::from_str::<OrganizationTargetChangedPayload>(&payload).map_err(
                |_| AppError::new(ErrorCode::Internal, "invalid organization target payload"),
            )?,
        }),
        ORGANIZATION_RESULT_CHANGED => {
            let payload = serde_json::from_str::<OrganizationResultChangedPayload>(&payload)
                .map_err(|_| {
                    AppError::new(ErrorCode::Internal, "invalid organization result payload")
                })?;
            Ok(TaskEventEnvelope::OrganizationResultChanged {
                id,
                schema_version,
                occurred_at,
                task_id: payload.processing_task_id,
                payload,
            })
        }
        INTEGRATION_HEALTH_CHANGED => Ok(TaskEventEnvelope::IntegrationHealthChanged {
            id,
            schema_version,
            occurred_at,
            task_id: None,
            payload: serde_json::from_str::<IntegrationHealthChangedPayload>(&payload).map_err(
                |_| AppError::new(ErrorCode::Internal, "invalid integration health payload"),
            )?,
        }),
        DOWNLOAD_TASK_CHANGED => {
            decode_download_task_event(id, schema_version, occurred_at, task_id, &payload)
        }
        AUTOMATION_EVENT_CHANGED => {
            let payload =
                serde_json::from_str::<AutomationEventChangedPayload>(&payload).map_err(|_| {
                    AppError::new(ErrorCode::Internal, "invalid automation event payload")
                })?;
            if payload.automation_event_id != task_id
                || !(1..=MAX_PUBLIC_EVENT_ID).contains(&payload.projection_version)
            {
                return Err(AppError::new(
                    ErrorCode::Internal,
                    "invalid automation event projection",
                ));
            }
            Ok(TaskEventEnvelope::AutomationEventChanged {
                id,
                schema_version,
                occurred_at,
                task_id: None,
                payload,
            })
        }
        IDENTIFICATION_ENHANCER_CHANGED => {
            let payload = serde_json::from_str::<IdentificationEnhancerChangedPayload>(&payload)
                .map_err(|_| {
                    AppError::new(ErrorCode::Internal, "invalid enhancer event payload")
                })?;
            if !(1..=MAX_PUBLIC_EVENT_ID).contains(&payload.projection_version) {
                return Err(AppError::new(
                    ErrorCode::Internal,
                    "invalid enhancer projection version",
                ));
            }
            Ok(TaskEventEnvelope::IdentificationEnhancerChanged {
                id,
                schema_version,
                occurred_at,
                task_id: None,
                payload,
            })
        }
        _ => Err(AppError::new(
            ErrorCode::Internal,
            "unknown outbox event type",
        )),
    }
}

fn decode_download_task_event(
    id: i64,
    schema_version: String,
    occurred_at: String,
    task_id: Uuid,
    payload: &str,
) -> Result<TaskEventEnvelope, AppError> {
    let payload = serde_json::from_str::<DownloadTaskChangedPayload>(payload)
        .map_err(|_| AppError::new(ErrorCode::Internal, "invalid download task payload"))?;
    if !(1..=MAX_PUBLIC_EVENT_ID).contains(&payload.projection_version) {
        return Err(AppError::new(
            ErrorCode::Internal,
            "invalid download task projection version",
        ));
    }
    Ok(TaskEventEnvelope::DownloadTaskChanged {
        id,
        schema_version,
        occurred_at,
        task_id,
        payload,
    })
}

fn decode_task_decision_event(
    id: i64,
    schema_version: String,
    occurred_at: String,
    task_id: Uuid,
    payload: &str,
) -> Result<TaskEventEnvelope, AppError> {
    let payload = serde_json::from_str::<TaskDecisionAcceptedPayload>(payload)
        .map_err(|_| AppError::new(ErrorCode::Internal, "invalid task decision payload"))?;
    if payload.case_version < 1 {
        return Err(AppError::new(
            ErrorCode::Internal,
            "invalid task decision case version",
        ));
    }
    Ok(TaskEventEnvelope::TaskDecisionAccepted {
        id,
        schema_version,
        occurred_at,
        task_id,
        payload,
    })
}

fn gap_event(minimum: i64, now_us: i64) -> Result<TaskEventEnvelope, AppError> {
    if !(2..=MAX_PUBLIC_EVENT_ID).contains(&minimum) {
        return Err(AppError::new(
            ErrorCode::Internal,
            "gap boundary exceeds the public event range",
        ));
    }
    Ok(TaskEventEnvelope::StreamGap {
        id: minimum.saturating_sub(1),
        schema_version: SCHEMA_VERSION.to_owned(),
        occurred_at: rfc3339(now_us)?,
        task_id: None,
        payload: StreamGapPayload {
            minimum_available_id: minimum,
        },
    })
}

fn rfc3339(timestamp_us: i64) -> Result<String, AppError> {
    Utc.timestamp_micros(timestamp_us)
        .single()
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Micros, true))
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "event timestamp is outside range"))
}

#[derive(Clone)]
/// 持久事件 outbox 的仅前缀保留策略。
pub struct OutboxRetention {
    pool: SqlitePool,
}

impl OutboxRetention {
    #[must_use]
    /// 将保留清理绑定到 `pool`。
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 删除至多 1,000 个既已满 24 小时、又有 100,000 个后继事件的最早事件。
    ///
    /// 清理仅针对前缀：一旦出现尚不够旧的事件，即使时间戳不规则，也不会删除后续事件。
    ///
    /// # Errors
    ///
    /// `SQLite` 无法执行有界删除时返回 [`AppError`]。
    pub async fn cleanup(&self, now_us: i64) -> Result<u64, AppError> {
        let threshold = now_us.saturating_sub(RETENTION_AGE_US);
        let result = sqlx::query(
            "WITH retention_boundary AS (
                 SELECT id FROM platform_outbox_events
                 ORDER BY id DESC LIMIT 1 OFFSET 99999
             ), prefix AS (
                 SELECT id,committed_at_us FROM platform_outbox_events
                 WHERE id < COALESCE((SELECT id FROM retention_boundary), 0)
                 ORDER BY id ASC LIMIT 1000
             ), classified AS (
                 SELECT id,
                        MAX(CASE WHEN committed_at_us>=? THEN 1 ELSE 0 END)
                        OVER (ORDER BY id ASC ROWS UNBOUNDED PRECEDING) AS blocked
                 FROM prefix
             ), deletable AS (
                 SELECT id FROM classified WHERE blocked=0
             )
             DELETE FROM platform_outbox_events WHERE id IN (SELECT id FROM deletable)",
        )
        .bind(threshold)
        .execute(&self.pool)
        .await
        .map_err(internal)?;
        Ok(result.rows_affected())
    }

    /// 重复执行有界清理，并在完整批次之间 yield，直到删除行数少于 1,000。
    ///
    /// 返回所有成功批次的饱和删除总数。
    ///
    /// # Errors
    ///
    /// 第一个失败批次发生时返回 [`AppError`]；先前已提交批次仍保持删除状态。
    pub async fn cleanup_cycle(&self, now_us: i64) -> Result<u64, AppError> {
        let mut total = 0_u64;
        loop {
            let deleted = self.cleanup(now_us).await?;
            total = total.saturating_add(deleted);
            if deleted < CLEANUP_BATCH as u64 {
                return Ok(total);
            }
            tokio::task::yield_now().await;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 按小时运行的保留运行时报告的最新失败。
pub struct OutboxMaintenanceFailure {
    /// 稳定应用错误分类。
    pub code: ErrorCode,
    /// 清理开始并失败时的 UTC 微秒。
    pub occurred_at_us: i64,
}

#[derive(Clone)]
/// [`OutboxRetention::cleanup_cycle`] 的串行化按小时后台运行器。
pub struct OutboxMaintenanceRuntime {
    retention: OutboxRetention,
    interval: Duration,
    failures: watch::Sender<Option<OutboxMaintenanceFailure>>,
    run_lock: Arc<Mutex<()>>,
}

impl OutboxMaintenanceRuntime {
    #[must_use]
    /// 创建间隔一小时、失败 watch 状态为空的运行时。
    pub fn new(pool: SqlitePool) -> Self {
        let (failures, _) = watch::channel(None);
        Self {
            retention: OutboxRetention::new(pool),
            interval: Duration::from_hours(1),
            failures,
            run_lock: Arc::new(Mutex::new(())),
        }
    }

    #[must_use]
    /// 订阅最新维护失败；初始值为 `None`。
    pub fn subscribe_failures(&self) -> watch::Receiver<Option<OutboxMaintenanceFailure>> {
        self.failures.subscribe()
    }

    #[must_use]
    /// 启动不终止的保留循环并返回其 Tokio join 句柄。
    ///
    /// 每次失败都会更新订阅者并输出稳定的 `outbox.maintenance_failed` 标记；循环会在休眠后继续。
    /// 丢弃句柄不会取消任务。
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let guard = self.run_lock.lock().await;
                let now = Utc::now().timestamp_micros();
                if let Err(error) = self.retention.cleanup_cycle(now).await {
                    self.failures.send_replace(Some(OutboxMaintenanceFailure {
                        code: error.code(),
                        occurred_at_us: now,
                    }));
                    eprintln!("outbox.maintenance_failed");
                }
                drop(guard);
                tokio::time::sleep(self.interval).await;
            }
        })
    }
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
