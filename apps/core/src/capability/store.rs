//! capability 协议事实的持久化：幂等调用记录与按 sequence 单调的事件表。
//!
//! 调用记录让同一 `request_id` 的重放返回缓存结果而不是第二次执行；事件表是 AI 通道的
//! 事实流，`sequence` 由 `SQLite AUTOINCREMENT` 保证单调，cursor 即 sequence。

use serde_json::Map;

use sqlx::Row;
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use crate::capability::model::{
    CapabilityError, CapabilityEvent, CapabilityEventPage, CapabilityEventType, CapabilityResult,
};
use crate::shared::error::{AppError, ErrorCode};

/// 幂等调用记录的持久投影。
#[derive(Clone, Debug)]
pub struct CapabilityCallRecord {
    /// 重放关联的请求 ID。
    pub request_id: Uuid,
    /// 调用携带的幂等键。
    pub idempotency_key: String,
    /// 目标能力名（线上形式）。
    pub capability: String,
    /// 已持久化的结果信封；重放时原样返回。
    pub result: CapabilityResult,
}

/// 事件拉取的上界；防止一次响应携带无界负载。
const MAX_EVENT_PAGE: i64 = 200;

/// capability 调用与事件的 `SQLite` 存储。
#[derive(Clone)]
pub struct CapabilityStore {
    pool: SqlitePool,
}

impl CapabilityStore {
    /// 创建以给定连接池支撑的存储。
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 底层连接池；能力处理器经此执行只读查询。
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 查找既有调用记录；同一 `request_id` 重放返回原结果。
    ///
    /// # Errors
    ///
    /// 数据库失败或缓存结果损坏时返回内部错误。
    pub async fn find_call(
        &self,
        request_id: Uuid,
    ) -> Result<Option<CapabilityCallRecord>, AppError> {
        let row = sqlx::query(
            "SELECT request_id,idempotency_key,capability,result_json,error_json
             FROM capability_calls WHERE request_id=?",
        )
        .bind(request_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let result = decode_result(
            row.try_get::<Option<String>, _>("result_json")
                .map_err(internal)?
                .as_deref(),
            row.try_get::<Option<String>, _>("error_json")
                .map_err(internal)?
                .as_deref(),
            request_id,
        )?;
        Ok(Some(CapabilityCallRecord {
            request_id,
            idempotency_key: row.try_get("idempotency_key").map_err(internal)?,
            capability: row.try_get("capability").map_err(internal)?,
            result,
        }))
    }

    /// 检查幂等键是否已被其他请求占用；同键不同请求是稳定冲突。
    ///
    /// # Errors
    ///
    /// 数据库失败时返回内部错误。
    pub async fn idempotency_owner(
        &self,
        idempotency_key: &str,
        capability: &str,
        current_request_id: Uuid,
    ) -> Result<Option<Uuid>, AppError> {
        let bytes = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT request_id FROM capability_calls
             WHERE idempotency_key=? AND capability=? AND request_id<>? LIMIT 1",
        )
        .bind(idempotency_key)
        .bind(capability)
        .bind(current_request_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        match bytes {
            Some(bytes) => Uuid::from_slice(&bytes)
                .map(Some)
                .map_err(|error| AppError::with_source(ErrorCode::Internal, error)),
            None => Ok(None),
        }
    }

    /// 持久化调用结果与调用方身份；重放可见即可见完整结果。
    ///
    /// # Errors
    ///
    /// 数据库失败时返回内部错误。
    pub async fn persist_call(
        &self,
        record: &CapabilityCallRecord,
        actor_kind: &str,
        actor_id: &str,
        now_us: i64,
    ) -> Result<(), AppError> {
        let status = serde_json::to_value(record.result.status)
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?
            .as_str()
            .ok_or_else(|| {
                AppError::new(ErrorCode::Internal, "capability status must be a string")
            })?
            .to_owned();
        let result_json = serde_json::to_string(&record.result)
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        sqlx::query(
            "INSERT INTO capability_calls
             (request_id,idempotency_key,capability,actor_kind,actor_id,status,result_json,error_json,created_at_us)
             VALUES (?,?,?,?,?,?,?,?,?)",
        )
        .bind(record.request_id.as_bytes().as_slice())
        .bind(&record.idempotency_key)
        .bind(&record.capability)
        .bind(actor_kind)
        .bind(actor_id)
        .bind(status)
        .bind(&result_json)
        .bind(Option::<String>::None)
        .bind(now_us)
        .execute(&self.pool)
        .await
        .map_err(internal)?;
        Ok(())
    }

    /// 追加一个事件；`sequence` 由数据库分配并回填。
    ///
    /// # Errors
    ///
    /// 事件写入失败时返回内部错误。
    pub async fn append_event(&self, event: &CapabilityEvent) -> Result<i64, AppError> {
        let progress_json = event
            .progress
            .as_ref()
            .map(|progress| {
                serde_json::to_string(progress)
                    .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
            })
            .transpose()?;
        let evidence_json = event
            .evidence_refs
            .as_ref()
            .map(|refs| {
                serde_json::to_string(refs)
                    .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
            })
            .transpose()?;
        let error_json = event
            .error
            .as_ref()
            .map(|error| {
                serde_json::to_string(error)
                    .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
            })
            .transpose()?;
        let sequence = sqlx::query_scalar::<_, i64>(
            "INSERT INTO capability_events
             (event_id,event_type,occurred_at_us,task_id,request_id,operation_id,state,progress_json,evidence_json,error_json)
             VALUES (?,?,?,?,?,?,?,?,?,?) RETURNING sequence",
        )
        .bind(event.event_id.as_bytes().as_slice())
        .bind(serde_json::to_value(event.event_type)
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?
            .as_str()
            .unwrap_or("capability.failed")
            .to_owned())
        .bind(occurred_at_us(&event.occurred_at)?)
        .bind(event.task_id.map(|id| id.as_bytes().to_vec()))
        .bind(event.request_id.map(|id| id.as_bytes().to_vec()))
        .bind(&event.operation_id)
        .bind(&event.state)
        .bind(progress_json)
        .bind(evidence_json)
        .bind(error_json)
        .fetch_one(&self.pool)
        .await
        .map_err(internal)?;
        Ok(sequence)
    }

    /// 按 cursor 拉取有序事件页；`cursor` 为上一页最大 sequence，`None` 从头开始。
    ///
    /// # Errors
    ///
    /// 数据库失败时返回内部错误。
    pub async fn list_events(
        &self,
        cursor: Option<i64>,
        limit: i64,
    ) -> Result<CapabilityEventPage, AppError> {
        let limit = limit.clamp(1, MAX_EVENT_PAGE);
        let rows = sqlx::query(
            "SELECT sequence,event_id,event_type,occurred_at_us,task_id,request_id,operation_id,
                    state,progress_json,evidence_json,error_json
             FROM capability_events WHERE sequence>? ORDER BY sequence LIMIT ?",
        )
        .bind(cursor.unwrap_or(0))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(decode_event(&row)?);
        }
        let next_cursor = if (i64::try_from(items.len()).unwrap_or(i64::MAX)) < limit {
            None
        } else {
            items
                .last()
                .map(|event| format!("capability-events:{}", event.sequence))
        };
        Ok(CapabilityEventPage { items, next_cursor })
    }
}

fn decode_result(
    result_json: Option<&str>,
    error_json: Option<&str>,
    request_id: Uuid,
) -> Result<CapabilityResult, AppError> {
    if let Some(json) = result_json {
        return serde_json::from_str(json)
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error));
    }
    let error: CapabilityError = serde_json::from_str(error_json.unwrap_or("{}"))
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
    Ok(CapabilityResult {
        protocol_version: crate::capability::model::PROTOCOL_VERSION.to_owned(),
        request_id,
        task_id: error.task_id,
        status: crate::capability::model::CapabilityResultStatus::Failed,
        result: None,
        evidence: Vec::new(),
        next_actions: Vec::new(),
        error: Some(error),
    })
}

fn decode_event(row: &sqlx::sqlite::SqliteRow) -> Result<CapabilityEvent, AppError> {
    let sequence: i64 = row.try_get("sequence").map_err(internal)?;
    let event_id_bytes: Vec<u8> = row.try_get("event_id").map_err(internal)?;
    let event_id = Uuid::from_slice(&event_id_bytes)
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
    let event_type_raw: String = row.try_get("event_type").map_err(internal)?;
    let event_type: CapabilityEventType =
        serde_json::from_value(serde_json::Value::String(event_type_raw))
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
    let task_id = option_uuid(row, "task_id")?;
    let request_id = option_uuid(row, "request_id")?;
    let progress_json: Option<String> = row.try_get("progress_json").map_err(internal)?;
    let evidence_json: Option<String> = row.try_get("evidence_json").map_err(internal)?;
    let error_json: Option<String> = row.try_get("error_json").map_err(internal)?;
    Ok(CapabilityEvent {
        event_id,
        sequence,
        event_type,
        occurred_at: occurred_at_rfc3339(row.try_get("occurred_at_us").map_err(internal)?)?,
        task_id,
        request_id,
        operation_id: row.try_get("operation_id").map_err(internal)?,
        state: row.try_get("state").map_err(internal)?,
        progress: progress_json
            .map(|json| {
                serde_json::from_str::<Map<String, serde_json::Value>>(json.as_str())
                    .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
            })
            .transpose()?,
        evidence_refs: evidence_json
            .map(|json| {
                serde_json::from_str::<Vec<String>>(json.as_str())
                    .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
            })
            .transpose()?,
        error: error_json
            .map(|json| {
                serde_json::from_str::<CapabilityError>(json.as_str())
                    .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
            })
            .transpose()?,
    })
}

fn option_uuid(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Option<Uuid>, AppError> {
    let bytes: Option<Vec<u8>> = row.try_get(column).map_err(internal)?;
    bytes
        .map(|bytes| {
            Uuid::from_slice(&bytes)
                .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
        })
        .transpose()
}

fn occurred_at_rfc3339(us: i64) -> Result<String, AppError> {
    chrono::TimeZone::timestamp_micros(&chrono::Utc, us)
        .single()
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "stored event timestamp is invalid"))
}

fn occurred_at_us(occurred_at: &str) -> Result<i64, AppError> {
    chrono::DateTime::parse_from_rfc3339(occurred_at)
        .map(|parsed| parsed.timestamp_micros())
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))
}

fn internal(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
