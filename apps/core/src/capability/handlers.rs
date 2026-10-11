//! A1 只读能力实现：任务状态、候选查询与计划预览。
//!
//! 全部为无副作用查询；跨模块读取走只读投影（公共 store API 或显式 SELECT），不修改
//! 任何领域状态。副作用能力自 A2 起在本模块登记。

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use sqlx::Row;
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use crate::capability::model::{
    CapabilityEvidenceKind, CapabilityEvidenceRef, CapabilityNextAction, CapabilityResultStatus,
};
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::processing::store::ProcessingStore;

/// 一个能力执行的产物；由网关组装为结果信封。
#[derive(Debug)]
pub struct CapabilityOutcome {
    /// 结构化结果状态。
    pub status: CapabilityResultStatus,
    /// 关联任务。
    pub task_id: Option<Uuid>,
    /// 能力结果对象。
    pub result: Option<Map<String, Value>>,
    /// 证据引用。
    pub evidence: Vec<CapabilityEvidenceRef>,
    /// 允许的下一步动作。
    pub next_actions: Vec<CapabilityNextAction>,
}

/// `media.task-status`：查询处理任务的阶段、状态与脱敏原因。
///
/// # Errors
///
/// 任务不存在或数据库失败时返回稳定应用错误。
pub async fn task_status(
    pool: &SqlitePool,
    input: &Map<String, Value>,
) -> Result<CapabilityOutcome, AppError> {
    let task_id = input_uuid(input, "task_id")?;
    let store = ProcessingStore::new(pool.clone());
    let task = store.get_by_task_id(task_id).await?;
    let mut result = Map::new();
    result.insert("stage".into(), Value::String(stage_str(&task)));
    result.insert("status".into(), Value::String(status_str(&task)));
    result.insert(
        "reason".into(),
        task.reason.as_ref().map_or(Value::Null, |reason| {
            Value::String(reason.as_str().to_owned())
        }),
    );
    result.insert("recovering".into(), Value::Bool(task.recovering));
    result.insert(
        "plan_present".into(),
        Value::Bool(task.organization_plan_id.is_some()),
    );
    let version = task_version(pool, task_id).await?;
    result.insert(
        "revision".into(),
        Value::String(format!("task.version:{version}")),
    );
    Ok(CapabilityOutcome {
        status: CapabilityResultStatus::Succeeded,
        task_id: Some(task_id),
        result: Some(result),
        evidence: vec![CapabilityEvidenceRef {
            id: task_id.to_string(),
            kind: CapabilityEvidenceKind::Task,
        }],
        next_actions: Vec::new(),
    })
}

/// `media.match-candidates`：按待确认记录查询候选证据与当前版本。
///
/// # Errors
///
/// 输入缺少标识、记录不存在或数据库失败时返回稳定应用错误。
pub async fn match_candidates(
    pool: &SqlitePool,
    input: &Map<String, Value>,
) -> Result<CapabilityOutcome, AppError> {
    let case_id = match (
        input_uuid(input, "case_id").ok(),
        input_uuid(input, "task_id").ok(),
    ) {
        (Some(case_id), _) => case_id,
        (None, Some(task_id)) => active_case_for_task(pool, task_id).await?,
        (None, None) => {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "match-candidates requires case_id or task_id",
            ));
        }
    };
    let row = sqlx::query(
        "SELECT c.id,c.task_id,c.level,c.reason,c.title_hint,c.version,
                (SELECT count(*) FROM identification_candidates cand
                 WHERE cand.attempt_id=c.attempt_id) AS candidate_count
         FROM identification_review_cases c WHERE c.id=? AND c.status='active'",
    )
    .bind(case_id.as_bytes().as_slice())
    .fetch_optional(pool)
    .await
    .map_err(internal)?
    .ok_or_else(|| AppError::new(ErrorCode::NotFound, "active review case not found"))?;
    let task_id_bytes: Vec<u8> = row.try_get("task_id").map_err(internal)?;
    let task_id = Uuid::from_slice(&task_id_bytes).map_err(bad_uuid)?;
    let mut result = Map::new();
    result.insert(
        "level".into(),
        Value::String(row.try_get::<String, _>("level").map_err(internal)?),
    );
    result.insert(
        "reason".into(),
        Value::String(row.try_get::<String, _>("reason").map_err(internal)?),
    );
    result.insert(
        "title_hint".into(),
        row.try_get::<Option<String>, _>("title_hint")
            .map_err(internal)?
            .map_or(Value::Null, Value::String),
    );
    result.insert(
        "candidate_count".into(),
        Value::from(row.try_get::<i64, _>("candidate_count").map_err(internal)?),
    );
    result.insert(
        "revision".into(),
        Value::String(format!(
            "case.version:{}",
            row.try_get::<i64, _>("version").map_err(internal)?
        )),
    );
    Ok(CapabilityOutcome {
        status: CapabilityResultStatus::Succeeded,
        task_id: Some(task_id),
        result: Some(result),
        evidence: vec![CapabilityEvidenceRef {
            id: case_id.to_string(),
            kind: CapabilityEvidenceKind::ReviewCase,
        }],
        next_actions: vec![CapabilityNextAction::AskUser],
    })
}

/// `media.create-plan`（plan-only 预览）：返回任务当前已持久化计划的脱敏摘要。
///
/// A1 不生成或修改计划；尚未规划的任务返回 `needs_input` 而不是伪造预览。
///
/// # Errors
///
/// 任务不存在或数据库失败时返回稳定应用错误。
pub async fn plan_preview(
    pool: &SqlitePool,
    input: &Map<String, Value>,
) -> Result<CapabilityOutcome, AppError> {
    let task_id = input_uuid(input, "task_id")?;
    let store = ProcessingStore::new(pool.clone());
    let task = store.get_by_task_id(task_id).await?;
    let Some(plan_id) = task.organization_plan_id else {
        return Ok(CapabilityOutcome {
            status: CapabilityResultStatus::NeedsInput,
            task_id: Some(task_id),
            result: Some(single_field("detail", "task has no persisted plan yet")),
            evidence: Vec::new(),
            next_actions: vec![CapabilityNextAction::AskUser],
        });
    };
    let row = sqlx::query(
        "SELECT version,
                (SELECT count(*) FROM organization_plan_operations ops WHERE ops.plan_id=organization_plans.id) AS operation_count
         FROM organization_plans WHERE id=?",
    )
    .bind(plan_id.as_bytes().as_slice())
    .fetch_optional(pool)
    .await
    .map_err(internal)?
    .ok_or_else(|| AppError::new(ErrorCode::NotFound, "persisted plan not found"))?;
    let version: i64 = row.try_get("version").map_err(internal)?;
    let operation_count: i64 = row.try_get("operation_count").map_err(internal)?;
    let mut result = Map::new();
    result.insert("plan_id".into(), Value::String(plan_id.to_string()));
    result.insert("plan_version".into(), Value::from(version));
    result.insert("operation_count".into(), Value::from(operation_count));
    result.insert("preview_only".into(), Value::Bool(true));
    result.insert(
        "revision".into(),
        Value::String(format!("plan.version:{version}")),
    );
    Ok(CapabilityOutcome {
        status: CapabilityResultStatus::Succeeded,
        task_id: Some(task_id),
        result: Some(result),
        evidence: vec![CapabilityEvidenceRef {
            id: plan_id.to_string(),
            kind: CapabilityEvidenceKind::Plan,
        }],
        next_actions: Vec::new(),
    })
}

/// 校验任务的 `expected_revision`（`task.version:N`）；不匹配是稳定 conflict。
///
/// # Errors
///
/// 格式错误、任务不存在或版本不匹配时返回稳定应用错误。
pub async fn assert_task_revision(
    pool: &SqlitePool,
    task_id: Uuid,
    expected_revision: &str,
) -> Result<(), AppError> {
    let Some(expected) = expected_revision
        .strip_prefix("task.version:")
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "expected_revision must use task.version:<n>",
        ));
    };
    let actual = task_version(pool, task_id).await?;
    if actual != expected {
        return Err(AppError::new(
            ErrorCode::ConfigVersionConflict,
            "task revision has moved on",
        ));
    }
    Ok(())
}

async fn task_version(pool: &SqlitePool, task_id: Uuid) -> Result<i64, AppError> {
    sqlx::query_scalar::<_, i64>("SELECT version FROM tasks_processing_tasks WHERE id=?")
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "processing task not found"))
}

async fn active_case_for_task(pool: &SqlitePool, task_id: Uuid) -> Result<Uuid, AppError> {
    let bytes = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT id FROM identification_review_cases WHERE task_id=? AND status='active' LIMIT 1",
    )
    .bind(task_id.as_bytes().as_slice())
    .fetch_optional(pool)
    .await
    .map_err(internal)?;
    match bytes {
        Some(bytes) => Uuid::from_slice(&bytes).map_err(bad_uuid),
        None => Err(AppError::new(
            ErrorCode::NotFound,
            "task has no active review case",
        )),
    }
}

fn input_uuid(input: &Map<String, Value>, field: &str) -> Result<Uuid, AppError> {
    input
        .get(field)
        .and_then(|value| value.as_str())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| AppError::new(ErrorCode::ValidationFailed, "task_id must be a uuid"))
}

fn stage_str(task: &crate::tasks::processing::model::ProcessingTaskView) -> String {
    serde_json::to_value(task.stage)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn status_str(task: &crate::tasks::processing::model::ProcessingTaskView) -> String {
    serde_json::to_value(task.status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn single_field(key: &str, value: &str) -> Map<String, Value> {
    let mut map = BTreeMap::new();
    map.insert(key.to_owned(), Value::String(value.to_owned()));
    map.into_iter().collect()
}

fn internal(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn bad_uuid(error: uuid::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
