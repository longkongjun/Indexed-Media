use std::collections::BTreeSet;

use chrono::{SecondsFormat, TimeZone as _, Utc};
use serde_json::to_string;
use sha2::{Digest as _, Sha256};
use sqlx::{Row as _, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

use crate::identification::manual::model::{
    AcceptedTaskDecision, ManualDecisionInput, ManualDecisionKind, ManualIdentificationContext,
    PendingDecisionDispatch, TaskDecisionState,
};
use crate::identification::model::{MediaKind, ParsedIdentityHint};
use crate::platform::outbox::{OutboxNotifier, OutboxWriter};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::id::new_id;
use crate::tasks::events::{
    TASK_DECISION_ACCEPTED, TaskDecisionAcceptedPayload, TaskDecisionEventKind,
};
use crate::tasks::processing::model::{DecisionCheckpoint, DecisionDispatch};

const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;
const MAX_PAYLOAD_BYTES: usize = 16_384;

#[derive(Clone)]
/// 不可变人工决定与精确可复用反馈的事务存储。
pub struct ManualDecisionStore {
    pool: SqlitePool,
    notifier: OutboxNotifier,
}

impl ManualDecisionStore {
    #[must_use]
    /// 使用独立通知器创建存储；事件会持久化但不会唤醒共享监听者。
    pub fn new(pool: SqlitePool) -> Self {
        Self::new_with_notifier(pool, OutboxNotifier::new())
    }

    #[must_use]
    /// 使用共享通知器创建存储，在决定事务提交后唤醒 outbox/SSE 交付。
    pub fn new_with_notifier(pool: SqlitePool, notifier: OutboxNotifier) -> Self {
        Self { pool, notifier }
    }

    /// 仅在每个选择器分量完全相等且规则启用时加载可复用反馈。
    /// 相互冲突的精确规则会有意禁用自动复用，交由人工复核。
    ///
    /// # Errors
    ///
    /// 持久化身份格式无效或数据库访问失败时返回内部错误。
    pub async fn exact_feedback_context(
        &self,
        account_id: Uuid,
        hint: &ParsedIdentityHint,
    ) -> Result<Option<ManualIdentificationContext>, AppError> {
        let mut episodes = hint.episodes.clone();
        episodes.sort_unstable();
        episodes.dedup();
        let episodes_json = to_string(&episodes).map_err(internal)?;
        let row = sqlx::query(
            "SELECT id,provider_id FROM identification_feedback
             WHERE account_id=? AND enabled=1 AND selector_version=1 AND media_type=?
               AND normalized_title=? AND year IS ? AND season IS ? AND episodes_json=?
             ORDER BY created_at_us DESC,id DESC LIMIT 1",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(media_type(hint.media_kind))
        .bind(&hint.normalized_title)
        .bind(hint.year.map(i64::from))
        .bind(hint.season.map(i64::from))
        .bind(episodes_json)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let provider_id: String = row.get("provider_id");
        let conflicting = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
                SELECT 1 FROM identification_feedback
                WHERE account_id=? AND enabled=1 AND selector_version=1 AND media_type=?
                  AND normalized_title=? AND year IS ? AND season IS ? AND episodes_json=?
                  AND provider_id!=?
             )",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(media_type(hint.media_kind))
        .bind(&hint.normalized_title)
        .bind(hint.year.map(i64::from))
        .bind(hint.season.map(i64::from))
        .bind(to_string(&episodes).map_err(internal)?)
        .bind(&provider_id)
        .fetch_one(&self.pool)
        .await
        .map_err(internal)?;
        if conflicting != 0 {
            return Ok(None);
        }
        Ok(Some(ManualIdentificationContext::ExactFeedback {
            feedback_id: decode_uuid(&row.get::<Vec<u8>, _>("id"))?,
            provider_id,
        }))
    }

    /// 仅为决定所绑定的处理任务解析不可变已接受决定。普通视频决定属于规划阶段，因此不返回
    /// 识别上下文。
    ///
    /// # Errors
    ///
    /// 绑定未知或属于其他任务时返回未找到；已存负载格式错误或数据库失败时返回内部错误。
    pub async fn decision_context_for_task(
        &self,
        task_id: Uuid,
        decision_id: Uuid,
    ) -> Result<Option<ManualIdentificationContext>, AppError> {
        let row = sqlx::query(
            "SELECT payload_json FROM identification_task_decisions
             WHERE id=? AND task_id=?",
        )
        .bind(decision_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "manual decision not found"))?;
        let input: ManualDecisionInput =
            serde_json::from_str(&row.get::<String, _>("payload_json")).map_err(internal)?;
        Ok(match input {
            ManualDecisionInput::SelectProviderCandidate {
                media_kind,
                provider_id,
                ..
            } => Some(ManualIdentificationContext::SelectedProvider {
                decision_id,
                media_kind,
                provider_id,
            }),
            ManualDecisionInput::RematchWithHints { hint, .. } => {
                Some(ManualIdentificationContext::Rematch { decision_id, hint })
            }
            ManualDecisionInput::SelectGenericVideo { .. } => None,
        })
    }

    /// 在处理任务所属账户内解析精确且启用的反馈。
    ///
    /// # Errors
    ///
    /// 任务未知时返回未找到；精确选择器失败与 [`Self::exact_feedback_context`] 相同。
    pub async fn exact_feedback_for_task(
        &self,
        task_id: Uuid,
        hint: &ParsedIdentityHint,
    ) -> Result<Option<ManualIdentificationContext>, AppError> {
        let account = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT account_id FROM tasks_processing_tasks WHERE id=?",
        )
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "processing task not found"))?;
        self.exact_feedback_context(decode_uuid(&account)?, hint)
            .await
    }

    /// 返回一批有界、有序且等待下游应用的已接受决定。
    ///
    /// # Errors
    ///
    /// 数量不在 `1..=200` 时返回校验错误；持久化身份或类别无效、数据库失败时返回内部错误。
    pub async fn pending_dispatches(
        &self,
        limit: u32,
    ) -> Result<Vec<PendingDecisionDispatch>, AppError> {
        if limit == 0 || limit > 200 {
            return Err(validation(
                "manual decision dispatch limit is outside bounds",
            ));
        }
        let rows = sqlx::query(
            "SELECT decision.account_id,decision.task_id,decision.id,decision.kind
             FROM identification_decision_dispatches dispatch
             JOIN identification_task_decisions decision ON decision.id=dispatch.decision_id
             WHERE dispatch.state='accepted'
               AND (dispatch.next_attempt_at_us IS NULL OR dispatch.next_attempt_at_us<=?)
             ORDER BY decision.created_at_us,decision.id LIMIT ?",
        )
        .bind(Utc::now().timestamp_micros())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.into_iter()
            .map(|row| {
                let account_id = decode_uuid(&row.get::<Vec<u8>, _>("account_id"))?;
                let task_id = decode_uuid(&row.get::<Vec<u8>, _>("task_id"))?;
                let decision_id = decode_uuid(&row.get::<Vec<u8>, _>("id"))?;
                let kind = ManualDecisionKind::parse(&row.get::<String, _>("kind"))
                    .ok_or_else(|| stored("manual decision kind"))?;
                let dispatch = match kind {
                    ManualDecisionKind::SelectProviderCandidate => DecisionDispatch::Reidentify {
                        task_id,
                        decision_id,
                        checkpoint: DecisionCheckpoint::ManualDecisionPending,
                    },
                    ManualDecisionKind::RematchWithHints => DecisionDispatch::Reidentify {
                        task_id,
                        decision_id,
                        checkpoint: DecisionCheckpoint::RematchPending,
                    },
                    ManualDecisionKind::SelectGenericVideo => DecisionDispatch::PlanningRequested {
                        task_id,
                        decision_id,
                    },
                };
                Ok(PendingDecisionDispatch {
                    account_id,
                    dispatch,
                })
            })
            .collect()
    }

    /// 在所有幂等消费者提交后，把一项已接受派发标记为已应用。
    ///
    /// # Errors
    ///
    /// 决定未知时返回未找到；状态无效或数据库失败时返回内部错误。重复标记已应用决定为空操作。
    pub async fn mark_applied(&self, decision_id: Uuid, now_us: i64) -> Result<(), AppError> {
        let result = sqlx::query(
            "UPDATE identification_decision_dispatches
             SET state='applied',next_attempt_at_us=NULL,last_error_code=NULL,updated_at_us=?
             WHERE decision_id=? AND state='accepted'",
        )
        .bind(now_us)
        .bind(decision_id.as_bytes().as_slice())
        .execute(&self.pool)
        .await
        .map_err(internal)?;
        if result.rows_affected() == 1 {
            return Ok(());
        }
        match sqlx::query_scalar::<_, String>(
            "SELECT state FROM identification_decision_dispatches WHERE decision_id=?",
        )
        .bind(decision_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .as_deref()
        {
            Some("applied") => Ok(()),
            Some(_) => Err(stored("manual decision dispatch state")),
            None => Err(AppError::new(
                ErrorCode::NotFound,
                "manual decision dispatch not found",
            )),
        }
    }

    /// 接受一项绑定案例版本的决定；精确重放时返回原始回执。
    ///
    /// # Errors
    ///
    /// 有界输入无效时返回校验错误，案例属于其他账户或非活动时返回未找到，案例版本过期时返回
    /// 版本冲突，幂等键被不同正文复用时返回请求冲突；原子持久化无法完成时返回内部错误。
    #[allow(clippy::too_many_lines)]
    pub async fn accept(
        &self,
        account_id: Uuid,
        case_id: Uuid,
        expected_version: i64,
        key: &str,
        input: &ManualDecisionInput,
        now_us: i64,
    ) -> Result<AcceptedTaskDecision, AppError> {
        validate_request(expected_version, key, input)?;
        let payload = to_string(input).map_err(internal)?;
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(validation("manual decision payload exceeds bounds"));
        }
        let request_digest: [u8; 32] = Sha256::digest(payload.as_bytes()).into();
        let key_digest: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        timestamp(now_us)?;

        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        if let Some(replayed) =
            replay(&mut tx, account_id, case_id, &key_digest, &request_digest).await?
        {
            tx.commit().await.map_err(internal)?;
            return Ok(replayed);
        }

        let case = sqlx::query(
            "SELECT review.task_id,review.attempt_id,review.status,review.version,
                    review.title_hint,task.status AS task_status
             FROM identification_review_cases review
             JOIN tasks_processing_tasks task ON task.id=review.task_id
             WHERE review.id=? AND review.account_id=?",
        )
        .bind(case_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "review case not found"))?;
        if case.get::<String, _>("status") != "active" {
            return rollback_with(
                tx,
                AppError::new(ErrorCode::NotFound, "review case is not active"),
            )
            .await;
        }
        if case.get::<String, _>("task_status") != "waiting-confirmation" {
            return rollback_with(
                tx,
                AppError::new(
                    ErrorCode::TaskInvalidState,
                    "processing task is no longer waiting for confirmation",
                ),
            )
            .await;
        }
        let version: i64 = case.get("version");
        if version != expected_version {
            return rollback_with(
                tx,
                AppError::new(
                    ErrorCode::ConfigVersionConflict,
                    "review case version changed",
                ),
            )
            .await;
        }
        let pending = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
                SELECT 1 FROM identification_task_decisions decision
                JOIN identification_decision_dispatches dispatch
                  ON dispatch.decision_id=decision.id
                WHERE decision.case_id=? AND dispatch.state='accepted'
             )",
        )
        .bind(case_id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        if pending != 0 {
            return rollback_with(
                tx,
                AppError::new(
                    ErrorCode::TaskInvalidState,
                    "review case already has a pending manual decision",
                ),
            )
            .await;
        }
        let next_version = version
            .checked_add(1)
            .ok_or_else(|| validation("review case version exceeds bounds"))?;
        let task_id = decode_uuid(&case.get::<Vec<u8>, _>("task_id"))?;
        let attempt_id = decode_uuid(&case.get::<Vec<u8>, _>("attempt_id"))?;
        let title_hint: Option<String> = case.get("title_hint");
        let decision_id = new_id();
        let kind = input.kind();

        sqlx::query(
            "INSERT INTO identification_task_decisions
             (id,account_id,task_id,case_id,case_version,kind,payload_json,request_digest,
              idempotency_key_sha256,created_at_us) VALUES (?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(decision_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .bind(case_id.as_bytes().as_slice())
        .bind(next_version)
        .bind(kind.as_str())
        .bind(&payload)
        .bind(request_digest.as_slice())
        .bind(key_digest.as_slice())
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "INSERT INTO identification_decision_dispatches
             (decision_id,state,attempt_count,updated_at_us) VALUES (?,'accepted',0,?)",
        )
        .bind(decision_id.as_bytes().as_slice())
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        persist_feedback(
            &mut tx,
            account_id,
            decision_id,
            attempt_id,
            title_hint.as_deref(),
            input,
            now_us,
        )
        .await?;
        let updated = sqlx::query(
            "UPDATE identification_review_cases SET version=?,updated_at_us=?
             WHERE id=? AND account_id=? AND status='active' AND version=?",
        )
        .bind(next_version)
        .bind(now_us)
        .bind(case_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(version)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        if updated.rows_affected() != 1 {
            return rollback_with(
                tx,
                AppError::new(
                    ErrorCode::ConfigVersionConflict,
                    "review case version changed",
                ),
            )
            .await;
        }
        OutboxWriter::write(
            &mut tx,
            TASK_DECISION_ACCEPTED,
            task_id,
            &TaskDecisionAcceptedPayload {
                case_id,
                decision_id,
                kind: event_kind(kind),
                case_version: next_version,
            },
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        Ok(AcceptedTaskDecision {
            id: decision_id,
            task_id,
            case_id,
            case_version: next_version,
            kind,
            state: TaskDecisionState::Accepted,
            created_at: timestamp(now_us)?,
        })
    }
}

async fn replay(
    tx: &mut Transaction<'_, Sqlite>,
    account_id: Uuid,
    case_id: Uuid,
    key_digest: &[u8; 32],
    request_digest: &[u8; 32],
) -> Result<Option<AcceptedTaskDecision>, AppError> {
    let row = sqlx::query(
        "SELECT d.id,d.task_id,d.case_id,d.case_version,d.kind,d.request_digest,d.created_at_us,
                dispatch.state
         FROM identification_task_decisions d
         JOIN identification_decision_dispatches dispatch ON dispatch.decision_id=d.id
         WHERE d.account_id=? AND d.case_id=? AND d.idempotency_key_sha256=?",
    )
    .bind(account_id.as_bytes().as_slice())
    .bind(case_id.as_bytes().as_slice())
    .bind(key_digest.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.get::<Vec<u8>, _>("request_digest").as_slice() != request_digest {
        return Err(AppError::new(
            ErrorCode::RequestConflict,
            "idempotency key is bound to a different manual decision",
        ));
    }
    Ok(Some(AcceptedTaskDecision {
        id: decode_uuid(&row.get::<Vec<u8>, _>("id"))?,
        task_id: decode_uuid(&row.get::<Vec<u8>, _>("task_id"))?,
        case_id: decode_uuid(&row.get::<Vec<u8>, _>("case_id"))?,
        case_version: row.get("case_version"),
        kind: ManualDecisionKind::parse(&row.get::<String, _>("kind"))
            .ok_or_else(|| stored("manual decision kind"))?,
        state: TaskDecisionState::parse(&row.get::<String, _>("state"))
            .ok_or_else(|| stored("manual decision state"))?,
        created_at: timestamp(row.get("created_at_us"))?,
    }))
}

async fn persist_feedback(
    tx: &mut Transaction<'_, Sqlite>,
    account_id: Uuid,
    decision_id: Uuid,
    attempt_id: Uuid,
    title_hint: Option<&str>,
    input: &ManualDecisionInput,
    now_us: i64,
) -> Result<(), AppError> {
    let ManualDecisionInput::SelectProviderCandidate {
        media_kind,
        provider_id,
        save_feedback: true,
    } = input
    else {
        return Ok(());
    };
    let title = title_hint
        .filter(|value| bounded_text(value, 200))
        .ok_or_else(|| validation("exact feedback requires a bounded title selector"))?;
    let facts = sqlx::query(
        "SELECT kind,normalized_value FROM identification_evidence
         WHERE attempt_id=? AND source IN ('filename','nfo') AND strength!='conflicting'
         ORDER BY ordinal ASC",
    )
    .bind(attempt_id.as_bytes().as_slice())
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?;
    let mut year = None;
    let mut season = None;
    let mut episodes = BTreeSet::new();
    for row in facts {
        let kind: String = row.get("kind");
        let value: String = row.get("normalized_value");
        if kind == "year" && year.is_none() {
            year = value
                .parse::<u16>()
                .ok()
                .filter(|value| (1870..=2200).contains(value));
        } else if kind == "episode"
            && let Some((parsed_season, episode)) = parse_episode(&value)
            && season.is_none_or(|existing| existing == parsed_season)
        {
            season = Some(parsed_season);
            episodes.insert(episode);
        }
    }
    let episodes = episodes.into_iter().take(32).collect::<Vec<_>>();
    let episodes_json = to_string(&episodes).map_err(internal)?;
    sqlx::query(
        "INSERT INTO identification_feedback
         (id,account_id,source_decision_id,selector_version,media_type,normalized_title,year,
          season,episodes_json,provider,provider_id,enabled,created_at_us)
         VALUES (?,?,?,1,?,?,?,?,?,'tmdb',?,1,?)",
    )
    .bind(new_id().as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
    .bind(decision_id.as_bytes().as_slice())
    .bind(media_type(*media_kind))
    .bind(title)
    .bind(year.map(i64::from))
    .bind(season.map(i64::from))
    .bind(episodes_json)
    .bind(provider_id)
    .bind(now_us)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(())
}

fn validate_request(
    expected_version: i64,
    key: &str,
    input: &ManualDecisionInput,
) -> Result<(), AppError> {
    if expected_version < 1 {
        return Err(AppError::new(
            ErrorCode::ConfigVersionConflict,
            "review case version must be positive",
        ));
    }
    if key.is_empty() || key.len() > MAX_IDEMPOTENCY_KEY_BYTES || key.chars().any(char::is_control)
    {
        return Err(validation("idempotency key is outside bounds"));
    }
    match input {
        ManualDecisionInput::SelectProviderCandidate { provider_id, .. } => {
            if !bounded_text(provider_id, 64)
                || provider_id
                    .parse::<i64>()
                    .ok()
                    .is_none_or(|value| value <= 0)
            {
                return Err(validation("provider identity is outside bounds"));
            }
        }
        ManualDecisionInput::RematchWithHints { hint, .. } => {
            if !bounded_text(&hint.normalized_title, 200)
                || hint.year.is_some_and(|year| !(1870..=2200).contains(&year))
                || hint.season.is_some_and(|season| season > 999)
                || hint.episodes.len() > 32
                || hint.episodes.iter().any(|episode| *episode > 9999)
                || hint.episodes.iter().copied().collect::<BTreeSet<_>>().len()
                    != hint.episodes.len()
                || (hint.media_kind == MediaKind::Movie
                    && (hint.season.is_some() || !hint.episodes.is_empty()))
                || (!hint.episodes.is_empty() && hint.season.is_none())
            {
                return Err(validation("manual identity hint is outside bounds"));
            }
        }
        ManualDecisionInput::SelectGenericVideo {
            display_title,
            group_hint,
            ..
        } => {
            if !bounded_text(display_title, 200)
                || group_hint
                    .as_deref()
                    .is_some_and(|value| !bounded_optional_text(value, 200))
            {
                return Err(validation("generic video intent is outside bounds"));
            }
        }
    }
    Ok(())
}

fn bounded_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
}

fn bounded_optional_text(value: &str, max: usize) -> bool {
    value.chars().count() <= max && !value.chars().any(char::is_control)
}

fn parse_episode(value: &str) -> Option<(u16, u16)> {
    let (season, episode) = value.strip_prefix('S')?.split_once('E')?;
    Some((season.parse().ok()?, episode.parse().ok()?))
}

const fn media_type(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Movie => "movie",
        MediaKind::Episode => "tv",
    }
}

const fn event_kind(kind: ManualDecisionKind) -> TaskDecisionEventKind {
    match kind {
        ManualDecisionKind::SelectProviderCandidate => {
            TaskDecisionEventKind::SelectProviderCandidate
        }
        ManualDecisionKind::RematchWithHints => TaskDecisionEventKind::RematchWithHints,
        ManualDecisionKind::SelectGenericVideo => TaskDecisionEventKind::SelectGenericVideo,
    }
}

async fn rollback_with(
    tx: Transaction<'_, Sqlite>,
    error: AppError,
) -> Result<AcceptedTaskDecision, AppError> {
    tx.rollback().await.map_err(internal)?;
    Err(error)
}

fn timestamp(value: i64) -> Result<String, AppError> {
    Utc.timestamp_micros(value)
        .single()
        .map(|time| time.to_rfc3339_opts(SecondsFormat::Micros, true))
        .ok_or_else(|| validation("manual decision timestamp is outside bounds"))
}

fn decode_uuid(bytes: &[u8]) -> Result<Uuid, AppError> {
    Uuid::from_slice(bytes).map_err(|_| stored("manual decision UUID"))
}

fn validation(message: &str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn stored(message: &str) -> AppError {
    AppError::new(ErrorCode::Internal, message)
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}
