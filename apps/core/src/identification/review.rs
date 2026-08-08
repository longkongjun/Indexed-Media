use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{SecondsFormat, TimeZone as _, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::{Row as _, SqlitePool};
use uuid::Uuid;

use crate::identification::decision::DecisionLevel;
use crate::identification::manual::model::{
    ManualDecisionKind, ReviewAction, TaskDecisionState, TaskDecisionSummary,
};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, MAX_CURSOR_BYTES, PageRequest};
use crate::tasks::processing::model::ProcessingReason;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 编入每个 `ReviewCase` 游标的可选有界过滤条件。
pub struct ReviewCaseFilter {
    /// 仅返回指定决策等级；`None` 不限制等级。
    pub level: Option<DecisionLevel>,
    /// 仅返回指定收件目录的案例；`None` 不限制目录。
    pub inbox_directory_id: Option<Uuid>,
    /// 仅返回更新时间不晚于该 Unix epoch 微秒值的案例。
    pub updated_before_us: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 不含主机路径或原始提供方/NFO 正文、可重建的活动复核投影。
pub struct ReviewCaseView {
    /// 活动复核案例的稳定 UUID。
    pub id: Uuid,
    /// 需要人工处理的文件级任务 UUID。
    pub task_id: Uuid,
    /// 案例当前绑定的不可变文件 revision UUID。
    pub file_revision_id: Uuid,
    /// 文件所属收件目录 UUID。
    pub inbox_directory_id: Uuid,
    /// 文件相对于已验证收件目录的路径。
    pub relative_path: String,
    /// 触发复核的识别结论等级。
    pub level: DecisionLevel,
    /// 面向客户端的主要脱敏原因。
    pub reason: ProcessingReason,
    /// 从安全本地证据得到的可选标题提示。
    pub title_hint: Option<String>,
    /// 接受人工决定时必须匹配的乐观并发版本。
    pub version: i64,
    /// 由当前状态推导、可安全提交的操作集合。
    pub allowed_actions: Vec<ReviewAction>,
    /// 最近一次已接受人工决定的摘要。
    pub latest_task_decision: Option<TaskDecisionSummary>,
    /// 投影最近更新的 UTC RFC3339 时间。
    pub updated_at: String,
}

#[derive(Deserialize, Serialize)]
struct ReviewCursor {
    #[serde(rename = "v", alias = "version")]
    version: u8,
    #[serde(rename = "a", alias = "account_id")]
    account_id: Uuid,
    #[serde(rename = "l", alias = "level")]
    level: Option<DecisionLevel>,
    #[serde(rename = "n", alias = "inbox_directory_id")]
    inbox_directory_id: Option<Uuid>,
    #[serde(rename = "b", alias = "updated_before_us", default)]
    updated_before_us: Option<i64>,
    #[serde(rename = "u", alias = "updated_at_us")]
    updated_at_us: i64,
    #[serde(rename = "i", alias = "id")]
    id: Uuid,
    #[serde(rename = "r", alias = "snapshot_revision")]
    snapshot_revision: i64,
}

#[derive(Deserialize, Serialize)]
struct CursorEnvelope {
    payload: String,
    checksum: String,
}

#[derive(Clone)]
/// 对活动 probable、ambiguous 与 unidentified 案例提供快照稳定分页读取。
pub struct ReviewCaseStore {
    pool: SqlitePool,
}

impl ReviewCaseStore {
    #[must_use]
    /// 创建绑定给定连接池的只读复核案例存储。
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 读取活动案例快照，并把过滤条件绑定进不透明游标防止跨查询复用。
    ///
    /// # Errors
    ///
    /// 等级不支持或游标被篡改时返回校验错误；持久化值无效或数据库失败时返回内部错误。
    #[allow(clippy::too_many_lines)]
    pub async fn list_active(
        &self,
        account_id: Uuid,
        filter: &ReviewCaseFilter,
        page: &PageRequest,
    ) -> Result<CursorPage<ReviewCaseView>, AppError> {
        if filter.level.is_some_and(|level| {
            !matches!(
                level,
                DecisionLevel::Probable | DecisionLevel::Ambiguous | DecisionLevel::Unidentified
            )
        }) {
            return Err(invalid_cursor());
        }
        let cursor = page.cursor.as_deref().map(decode_cursor).transpose()?;
        if cursor.as_ref().is_some_and(|cursor| {
            cursor.account_id != account_id
                || cursor.level != filter.level
                || cursor.inbox_directory_id != filter.inbox_directory_id
                || cursor.updated_before_us != filter.updated_before_us
        }) {
            return Err(invalid_cursor());
        }
        let snapshot_revision = if let Some(cursor) = &cursor {
            Some(cursor.snapshot_revision)
        } else {
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT MAX(revision) FROM identification_review_case_order_history
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
        let level = filter.level.map(DecisionLevel::as_str);
        let inbox = filter
            .inbox_directory_id
            .map(|value| value.as_bytes().to_vec());
        let rows = if let Some(cursor) = &cursor {
            sqlx::query(
                "WITH snapshot_order AS (
                     SELECT history.case_id,history.status,history.updated_at_us
                     FROM identification_review_case_order_history history
                     WHERE history.account_id=? AND history.revision<=?
                       AND history.revision=(
                         SELECT MAX(candidate.revision)
                         FROM identification_review_case_order_history candidate
                         WHERE candidate.case_id=history.case_id AND candidate.revision<=?
                       )
                 )
                 SELECT c.id,c.task_id,c.file_revision_id,c.inbox_directory_id,
                        f.relative_path_display,c.level,c.reason,c.title_hint,c.version,
                        t.status AS task_status,
                        manual.id AS manual_id,manual.kind AS manual_kind,
                        manual.created_at_us AS manual_created_at_us,dispatch.state AS manual_state,
                        snapshot_order.updated_at_us AS snapshot_updated_at_us
                 FROM snapshot_order
                 JOIN identification_review_cases c ON c.id=snapshot_order.case_id
                 JOIN tasks_processing_tasks t ON t.id=c.task_id
                 JOIN discovery_tracked_files f ON f.id=t.discovered_file_id
                 LEFT JOIN identification_task_decisions manual ON manual.id=(
                     SELECT candidate.id FROM identification_task_decisions candidate
                     WHERE candidate.case_id=c.id
                     ORDER BY candidate.created_at_us DESC,candidate.id DESC LIMIT 1
                 )
                 LEFT JOIN identification_decision_dispatches dispatch ON dispatch.decision_id=manual.id
                 WHERE snapshot_order.status='active'
                   AND (? IS NULL OR c.level=?)
                   AND (? IS NULL OR c.inbox_directory_id=?)
                   AND (? IS NULL OR snapshot_order.updated_at_us<?)
                   AND (snapshot_order.updated_at_us<? OR
                        (snapshot_order.updated_at_us=? AND c.id<?))
                 ORDER BY snapshot_order.updated_at_us DESC,c.id DESC LIMIT ?",
            )
            .bind(account_id.as_bytes().as_slice())
            .bind(snapshot_revision)
            .bind(snapshot_revision)
            .bind(level)
            .bind(level)
            .bind(inbox.clone())
            .bind(inbox)
            .bind(filter.updated_before_us)
            .bind(filter.updated_before_us)
            .bind(cursor.updated_at_us)
            .bind(cursor.updated_at_us)
            .bind(cursor.id.as_bytes().as_slice())
            .bind(i64::from(page.limit) + 1)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "WITH snapshot_order AS (
                     SELECT history.case_id,history.status,history.updated_at_us
                     FROM identification_review_case_order_history history
                     WHERE history.account_id=? AND history.revision<=?
                       AND history.revision=(
                         SELECT MAX(candidate.revision)
                         FROM identification_review_case_order_history candidate
                         WHERE candidate.case_id=history.case_id AND candidate.revision<=?
                       )
                 )
                 SELECT c.id,c.task_id,c.file_revision_id,c.inbox_directory_id,
                        f.relative_path_display,c.level,c.reason,c.title_hint,c.version,
                        t.status AS task_status,
                        manual.id AS manual_id,manual.kind AS manual_kind,
                        manual.created_at_us AS manual_created_at_us,dispatch.state AS manual_state,
                        snapshot_order.updated_at_us AS snapshot_updated_at_us
                 FROM snapshot_order
                 JOIN identification_review_cases c ON c.id=snapshot_order.case_id
                 JOIN tasks_processing_tasks t ON t.id=c.task_id
                 JOIN discovery_tracked_files f ON f.id=t.discovered_file_id
                 LEFT JOIN identification_task_decisions manual ON manual.id=(
                     SELECT candidate.id FROM identification_task_decisions candidate
                     WHERE candidate.case_id=c.id
                     ORDER BY candidate.created_at_us DESC,candidate.id DESC LIMIT 1
                 )
                 LEFT JOIN identification_decision_dispatches dispatch ON dispatch.decision_id=manual.id
                 WHERE snapshot_order.status='active'
                   AND (? IS NULL OR c.level=?)
                   AND (? IS NULL OR c.inbox_directory_id=?)
                   AND (? IS NULL OR snapshot_order.updated_at_us<?)
                 ORDER BY snapshot_order.updated_at_us DESC,c.id DESC LIMIT ?",
            )
            .bind(account_id.as_bytes().as_slice())
            .bind(snapshot_revision)
            .bind(snapshot_revision)
            .bind(level)
            .bind(level)
            .bind(inbox.clone())
            .bind(inbox)
            .bind(filter.updated_before_us)
            .bind(filter.updated_before_us)
            .bind(i64::from(page.limit) + 1)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(internal)?;
        let has_more = rows.len() > page.limit as usize;
        let selected = rows.iter().take(page.limit as usize).collect::<Vec<_>>();
        let items = selected
            .iter()
            .map(|row| decode_case(row))
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if has_more {
            selected
                .last()
                .map(|row| {
                    Ok(ReviewCursor {
                        version: 1,
                        account_id,
                        level: filter.level,
                        inbox_directory_id: filter.inbox_directory_id,
                        updated_before_us: filter.updated_before_us,
                        updated_at_us: row.get("snapshot_updated_at_us"),
                        id: uuid(row, "id")?,
                        snapshot_revision,
                    })
                })
                .transpose()?
                .as_ref()
                .map(encode_cursor)
                .transpose()?
        } else {
            None
        };
        Ok(CursorPage { items, next_cursor })
    }

    /// 读取一个当前活动且归账户所有的复核案例。
    ///
    /// # Errors
    ///
    /// 案例不属于账户时返回未找到；持久化值无效时返回内部错误。
    pub async fn get_active(
        &self,
        account_id: Uuid,
        case_id: Uuid,
    ) -> Result<ReviewCaseView, AppError> {
        let row = sqlx::query(
            "SELECT c.id,c.task_id,c.file_revision_id,c.inbox_directory_id,
                    f.relative_path_display,c.level,c.reason,c.title_hint,c.version,
                    t.status AS task_status,
                    manual.id AS manual_id,manual.kind AS manual_kind,
                    manual.created_at_us AS manual_created_at_us,dispatch.state AS manual_state,
                    c.updated_at_us AS snapshot_updated_at_us
             FROM identification_review_cases c
             JOIN tasks_processing_tasks t ON t.id=c.task_id
             JOIN discovery_tracked_files f ON f.id=t.discovered_file_id
             LEFT JOIN identification_task_decisions manual ON manual.id=(
                 SELECT candidate.id FROM identification_task_decisions candidate
                 WHERE candidate.case_id=c.id
                 ORDER BY candidate.created_at_us DESC,candidate.id DESC LIMIT 1
             )
             LEFT JOIN identification_decision_dispatches dispatch ON dispatch.decision_id=manual.id
             WHERE c.id=? AND c.account_id=? AND c.status='active'",
        )
        .bind(case_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "review case not found"))?;
        decode_case(&row)
    }
}

fn decode_case(row: &sqlx::sqlite::SqliteRow) -> Result<ReviewCaseView, AppError> {
    let latest_task_decision = decode_latest_decision(row)?;
    let allowed_actions = if row.get::<String, _>("task_status") != "waiting-confirmation"
        || latest_task_decision
            .as_ref()
            .is_some_and(|decision| decision.state == TaskDecisionState::Accepted)
    {
        Vec::new()
    } else {
        vec![
            ReviewAction::SelectProviderCandidate,
            ReviewAction::RematchWithHints,
            ReviewAction::SelectGenericVideo,
        ]
    };
    Ok(ReviewCaseView {
        id: uuid(row, "id")?,
        task_id: uuid(row, "task_id")?,
        file_revision_id: uuid(row, "file_revision_id")?,
        inbox_directory_id: uuid(row, "inbox_directory_id")?,
        relative_path: row.get("relative_path_display"),
        level: parse_level(&row.get::<String, _>("level"))?,
        reason: parse_reason(&row.get::<String, _>("reason"))?,
        title_hint: row.get("title_hint"),
        version: row.get("version"),
        allowed_actions,
        latest_task_decision,
        updated_at: timestamp(row.get("snapshot_updated_at_us"))?,
    })
}

fn decode_latest_decision(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<Option<TaskDecisionSummary>, AppError> {
    let id: Option<Vec<u8>> = row.get("manual_id");
    let Some(id) = id else {
        return Ok(None);
    };
    Ok(Some(TaskDecisionSummary {
        id: Uuid::from_slice(&id).map_err(internal)?,
        kind: ManualDecisionKind::parse(&row.get::<String, _>("manual_kind"))
            .ok_or_else(|| stored("manual decision kind"))?,
        state: TaskDecisionState::parse(&row.get::<String, _>("manual_state"))
            .ok_or_else(|| stored("manual decision state"))?,
        created_at: timestamp(row.get("manual_created_at_us"))?,
    }))
}

fn parse_level(value: &str) -> Result<DecisionLevel, AppError> {
    match value {
        "probable" => Ok(DecisionLevel::Probable),
        "ambiguous" => Ok(DecisionLevel::Ambiguous),
        "unidentified" => Ok(DecisionLevel::Unidentified),
        _ => Err(stored("review level")),
    }
}

fn parse_reason(value: &str) -> Result<ProcessingReason, AppError> {
    match value {
        "identification.ambiguous" => Ok(ProcessingReason::IdentificationAmbiguous),
        "identification.multiple-strong-candidates" => {
            Ok(ProcessingReason::IdentificationMultipleStrongCandidates)
        }
        "identification.no-candidate" => Ok(ProcessingReason::IdentificationNoCandidate),
        "identification.probable-title" => Ok(ProcessingReason::IdentificationProbableTitle),
        _ => Err(stored("review reason")),
    }
}

fn encode_cursor(cursor: &ReviewCursor) -> Result<String, AppError> {
    let payload = serde_json::to_vec(cursor).map_err(internal)?;
    let envelope = CursorEnvelope {
        payload: URL_SAFE_NO_PAD.encode(&payload),
        checksum: checksum(&payload),
    };
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).map_err(internal)?);
    if encoded.len() > MAX_CURSOR_BYTES {
        return Err(stored("review cursor size"));
    }
    Ok(encoded)
}

fn decode_cursor(value: &str) -> Result<ReviewCursor, AppError> {
    if value.is_empty() || value.len() > MAX_CURSOR_BYTES {
        return Err(invalid_cursor());
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorEnvelope>(&bytes).ok())
        .ok_or_else(invalid_cursor)?;
    let payload = URL_SAFE_NO_PAD
        .decode(envelope.payload)
        .map_err(|_| invalid_cursor())?;
    if envelope.checksum != checksum(&payload) {
        return Err(invalid_cursor());
    }
    serde_json::from_slice::<ReviewCursor>(&payload)
        .ok()
        .filter(|cursor| cursor.version == 1)
        .ok_or_else(invalid_cursor)
}

fn checksum(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.identification.review.cursor.v1\0");
    hasher.update(payload);
    hex::encode(&hasher.finalize()[..16])
}

fn timestamp(value: i64) -> Result<String, AppError> {
    Utc.timestamp_micros(value)
        .single()
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Micros, true))
        .ok_or_else(|| stored("review timestamp"))
}

fn uuid(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Uuid, AppError> {
    Uuid::from_slice(&row.get::<Vec<u8>, _>(column)).map_err(internal)
}

fn invalid_cursor() -> AppError {
    AppError::new(ErrorCode::ValidationFailed, "invalid review case cursor")
}

fn stored(kind: &str) -> AppError {
    AppError::new(ErrorCode::Internal, format!("stored {kind} is invalid"))
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
