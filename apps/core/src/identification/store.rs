#![allow(clippy::too_many_lines)]

use chrono::{SecondsFormat, TimeZone as _, Utc};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use sqlx::{Row as _, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

use crate::connectors::model::{ProviderError, ProviderMediaKind};
use crate::identification::decision::{
    DecisionCandidate, DecisionLevel, DecisionReason, IdentificationDecisionDraft,
};
use crate::identification::evidence::{
    EvidenceDraft, EvidenceKind, EvidenceSource, EvidenceStrength,
};
use crate::platform::outbox::{OutboxNotifier, OutboxWriter};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::id::new_id;
use crate::tasks::events::{
    PROCESSING_TASK_IDENTIFICATION_DECIDED, PROCESSING_TASK_STATE_CHANGED,
    ProcessingTaskIdentificationDecidedPayload,
};
use crate::tasks::processing::model::{ProcessingLease, ProcessingReason, ProcessingTaskView};
use crate::tasks::processing::store::ProcessingStore;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 识别详情响应使用的安全、不可变文件 revision 摘要。
pub struct FileRevisionSummary {
    /// 不可变文件 revision 的稳定 UUID。
    pub id: Uuid,
    /// 文件所属收件目录 UUID。
    pub inbox_directory_id: Uuid,
    /// 文件相对于已验证收件目录的路径，不暴露主机绝对路径。
    pub relative_path: String,
    /// 观测到的文件长度，单位为字节。
    pub size_bytes: u64,
    /// revision 记录的 UTC RFC3339 修改时间。
    pub modified_at: String,
    /// 创建 revision 时确认的稳定性状态。
    pub stability: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 不含来源字节、哈希或提供方正文的有界证据投影。
pub struct IdentificationEvidenceView {
    /// 持久化证据条目的稳定 UUID。
    pub id: Uuid,
    /// 产生证据的系统或人工来源。
    pub source: EvidenceSource,
    /// 证据参与身份规则的语义类别。
    pub kind: EvidenceKind,
    /// 可安全展示的有界证据值；其规范化程度由来源决定，TMDB 标题在决策比较时才规范化。
    pub value: String,
    /// 证据对当前候选的支持或冲突程度。
    pub strength: EvidenceStrength,
    /// 面向人工复核的有界原因文本。
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
/// 候选详情中由提供方核验的一个剧集集数。
pub struct CandidateEpisodeView {
    /// 集数所属季号。
    pub season_number: u16,
    /// 季内集号。
    pub episode_number: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 不含原始提供方正文的有界 TMDB 候选投影。
pub struct IdentificationCandidateView {
    /// 本次识别尝试内稳定引用候选的 UUID。
    pub id: Uuid,
    /// 候选来源的稳定提供方名称。
    pub provider: &'static str,
    /// 提供方返回并已映射的媒体类别。
    pub media_type: String,
    /// 提供方命名空间中的实体 ID。
    pub provider_id: String,
    /// 按请求 locale 回退规则得到的主要标题。
    pub title: String,
    /// 提供方的原始语言标题；不可用时为 `None`。
    pub original_title: Option<String>,
    /// 发行或首播年份；提供方未给出时为 `None`。
    pub year: Option<u16>,
    /// 实际产生主要本地化字段的 locale。
    pub locale: String,
    /// 仅供 UI 排序的有界分值，不参与自动确认。
    pub ranking_score: u16,
    /// 剧集候选已核验的有界集数集合。
    pub episodes: Vec<CandidateEpisodeView>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 一次识别尝试最新的不可变决策投影。
pub struct IdentificationDecisionView {
    /// 持久化决策的稳定 UUID。
    pub id: Uuid,
    /// 本次尝试的结论等级。
    pub level: DecisionLevel,
    /// 面向任务中心和客户端的主要脱敏原因。
    pub reason: ProcessingReason,
    /// 被唯一选中的候选 UUID；confirmed 或唯一 probable 时为 `Some`，其余为 `None`。
    pub candidate_id: Option<Uuid>,
    /// 暂时性阻塞允许重试的 UTC RFC3339 时间。
    pub retry_at: Option<String>,
    /// 决策事务提交的 UTC RFC3339 时间。
    pub decided_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 汇总任务、revision、证据、候选与最新决策的有界详情响应。
pub struct IdentificationDetailView {
    /// 当前活动复核案例 UUID；无需人工复核时为 `None`。
    pub review_case_id: Option<Uuid>,
    /// 账户隔离的处理任务公开投影。
    pub task: ProcessingTaskView,
    /// 任务所绑定的不可变文件 revision 摘要。
    pub revision: FileRevisionSummary,
    /// 最新已提交决策；尚未完成识别时为 `None`。
    pub decision: Option<IdentificationDecisionView>,
    /// 按确定顺序返回的有界证据页内集合。
    pub evidence: Vec<IdentificationEvidenceView>,
    /// 按显示排名返回的有界候选集合。
    pub candidates: Vec<IdentificationCandidateView>,
    /// 是否仍有证据因响应上限未返回。
    pub evidence_truncated: bool,
    /// 是否仍有候选因响应上限未返回。
    pub candidates_truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 提交时重新校验文件 revision 权威性的生命周期结果。
pub enum IdentificationCommitStatus {
    /// revision 仍为当前事实，决策及事件已经原子提交。
    Decided,
    /// revision 已被替换，本次计算结果被丢弃并安排恢复。
    RevisionChanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 仅在业务事务和 outbox 条目共同提交后才可对外暴露的结果。
pub struct IdentificationCommitOutcome {
    /// revision 权威性检查与事务提交的最终状态。
    pub status: IdentificationCommitStatus,
    /// 新提交决策的 UUID；revision 已变化时为 `None`。
    pub decision_id: Option<Uuid>,
    /// 新建或更新的活动复核案例 UUID；无需复核时为 `None`。
    pub review_case_id: Option<Uuid>,
    /// 已提交决策等级；revision 已变化时为 `None`。
    pub level: Option<DecisionLevel>,
    /// 本次尝试观察到的提供方失败，不进入公开投影。
    pub provider_failure: Option<ProviderError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 正在运行的不可变识别尝试头信息。
pub struct IdentificationAttempt {
    /// 本次识别尝试的稳定 UUID。
    pub id: Uuid,
    /// 尝试所属处理任务 UUID。
    pub task_id: Uuid,
    /// 持有当前处理租约的运行尝试 UUID。
    pub processing_attempt_id: Uuid,
    /// 本次尝试绑定的不可变文件 revision UUID。
    pub file_revision_id: Uuid,
    /// 同一任务内单调递增的识别尝试序号。
    pub ordinal: u32,
}

/// 为一个有效处理租约原子提交的借用、有界识别内容。
pub struct IdentificationCommit<'a> {
    /// 证明任务、所有者、版本和期限的乐观并发租约。
    pub lease: &'a ProcessingLease,
    /// 与本次写入内容对应的识别尝试 UUID。
    pub attempt_id: Uuid,
    /// 需要与候选及决策在同一事务保存的证据。
    pub evidence: &'a [EvidenceDraft],
    /// 需要持久化的有界候选集合。
    pub candidates: &'a [DecisionCandidate],
    /// 需要投影到任务与复核案例的不可变决策。
    pub decision: &'a IdentificationDecisionDraft,
    /// 供复核列表展示的安全标题线索。
    pub title_hint: Option<&'a str>,
    /// 本次事务使用的 Unix epoch 微秒时间。
    pub now_us: i64,
}

#[derive(Clone)]
/// 不可变识别历史及其 `ReviewCase` 投影的事务存储。
pub struct IdentificationStore {
    pool: SqlitePool,
    notifier: OutboxNotifier,
}

impl IdentificationStore {
    #[must_use]
    /// 使用独立通知器创建存储；事件会持久化但不会唤醒共享监听者。
    pub fn new(pool: SqlitePool) -> Self {
        Self::new_with_notifier(pool, OutboxNotifier::new())
    }

    #[must_use]
    /// 使用共享通知器创建存储，在事务提交后唤醒 outbox/SSE 交付。
    pub fn new_with_notifier(pool: SqlitePool, notifier: OutboxNotifier) -> Self {
        Self { pool, notifier }
    }

    /// 读取账户所拥有处理任务的最新有界识别尝试。
    ///
    /// # Errors
    ///
    /// 任务不属于账户时返回未找到；持久化值无效或数据库访问失败时返回内部错误。
    pub async fn detail(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<IdentificationDetailView, AppError> {
        let task = ProcessingStore::new(self.pool.clone())
            .get(account_id, task_id)
            .await?;
        let revision = load_revision(&self.pool, &task).await?;
        let review_case_id = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT id FROM identification_review_cases WHERE task_id=? AND status='active'",
        )
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .map(|value| Uuid::from_slice(&value).map_err(internal))
        .transpose()?;
        let attempt = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT id FROM identification_attempts
             WHERE task_id=? ORDER BY ordinal DESC LIMIT 1",
        )
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        let Some(attempt) = attempt else {
            return Ok(IdentificationDetailView {
                review_case_id,
                task,
                revision,
                decision: None,
                evidence: Vec::new(),
                candidates: Vec::new(),
                evidence_truncated: false,
                candidates_truncated: false,
            });
        };
        let attempt_id = Uuid::from_slice(&attempt).map_err(internal)?;
        let (evidence, evidence_truncated) = load_evidence(&self.pool, attempt_id).await?;
        let (candidates, candidates_truncated) = load_candidates(&self.pool, attempt_id).await?;
        let decision = load_decision(&self.pool, attempt_id).await?;
        Ok(IdentificationDetailView {
            review_case_id,
            task,
            revision,
            decision,
            evidence,
            candidates,
            evidence_truncated,
            candidates_truncated,
        })
    }

    /// 将新的识别尝试精确绑定到一个仍有效的处理尝试。
    ///
    /// # Errors
    ///
    /// 输入无效、租约丢失或持久化约束不满足时返回错误。
    pub async fn begin_attempt(
        &self,
        lease: &ProcessingLease,
        parser_version: &str,
        provider_version: &str,
        rule_version: u16,
        now_us: i64,
    ) -> Result<IdentificationAttempt, AppError> {
        if !(1..=64).contains(&parser_version.len())
            || !(1..=64).contains(&provider_version.len())
            || rule_version == 0
        {
            return Err(validation("identification version is outside bounds"));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        sqlx::query(
            "UPDATE identification_attempts
             SET status='failed',failure_code='task.lease-lost',finished_at_us=?
             WHERE task_id=? AND status='running' AND processing_attempt_id!=?",
        )
        .bind(now_us)
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.attempt_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        let row = sqlx::query(
            "SELECT pa.ordinal
             FROM tasks_processing_tasks t
             JOIN tasks_processing_attempts pa ON pa.id=t.current_attempt_id
             WHERE t.id=? AND t.current_attempt_id=? AND t.file_revision_id=?
               AND t.status='running' AND t.stage='identification' AND t.cancel_requested=0
               AND t.lease_owner=? AND t.lease_expires_at_us>? AND pa.status='running'",
        )
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.attempt_id.as_bytes().as_slice())
        .bind(lease.task.file_revision_id.as_bytes().as_slice())
        .bind(&lease.owner)
        .bind(now_us)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or_else(lease_lost)?;
        let ordinal_i64: i64 = row.get("ordinal");
        let ordinal = u32::try_from(ordinal_i64).map_err(internal)?;
        let id = new_id();
        sqlx::query(
            "INSERT INTO identification_attempts
             (id,task_id,processing_attempt_id,file_revision_id,ordinal,parser_version,
              provider_version,rule_version,status,started_at_us)
             VALUES (?,?,?,?,?,?,?,?,'running',?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(lease.task.id.as_bytes().as_slice())
        .bind(lease.attempt_id.as_bytes().as_slice())
        .bind(lease.task.file_revision_id.as_bytes().as_slice())
        .bind(ordinal_i64)
        .bind(parser_version)
        .bind(provider_version)
        .bind(i64::from(rule_version))
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(IdentificationAttempt {
            id,
            task_id: lease.task.id,
            processing_attempt_id: lease.attempt_id,
            file_revision_id: lease.task.file_revision_id,
            ordinal,
        })
    }

    /// 重新校验 revision 权威性，并在同一事务提交证据、候选、决策、`ReviewCase`、处理检查点
    /// 与最小 outbox 事件。
    ///
    /// # Errors
    ///
    /// 输入无效或租约丢失时返回对应错误；任何数据库/outbox 失败都会回滚本次全部新写入。
    pub async fn commit(
        &self,
        command: IdentificationCommit<'_>,
    ) -> Result<IdentificationCommitOutcome, AppError> {
        validate_commit(&command)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        assert_attempt_authority(&mut tx, &command).await?;
        if !revision_is_current(&mut tx, command.lease.task.id).await? {
            commit_revision_changed(&mut tx, &command).await?;
            tx.commit().await.map_err(internal)?;
            self.notifier.notify_after_commit();
            return Ok(IdentificationCommitOutcome {
                status: IdentificationCommitStatus::RevisionChanged,
                decision_id: None,
                review_case_id: None,
                level: None,
                provider_failure: None,
            });
        }

        insert_evidence(
            &mut tx,
            command.attempt_id,
            command.evidence,
            command.now_us,
        )
        .await?;
        insert_candidates(
            &mut tx,
            command.attempt_id,
            command.candidates,
            command.now_us,
        )
        .await?;
        let decision_id = new_id();
        let reason = public_reason(command.decision);
        sqlx::query(
            "INSERT INTO identification_decisions
             (id,attempt_id,level,reason,selected_candidate_id,retry_at_us,rule_version,decided_at_us)
             VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(decision_id.as_bytes().as_slice())
        .bind(command.attempt_id.as_bytes().as_slice())
        .bind(command.decision.level.as_str())
        .bind(reason.as_str())
        .bind(
            command
                .decision
                .selected_candidate
                .map(|id| id.as_bytes().to_vec()),
        )
        .bind(command.decision.retry_at_us)
        .bind(i64::from(command.decision.rule_version))
        .bind(command.now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        for (index, reason) in command.decision.reasons.iter().enumerate() {
            sqlx::query(
                "INSERT INTO identification_decision_reasons(decision_id,ordinal,reason)
                 VALUES (?,?,?)",
            )
            .bind(decision_id.as_bytes().as_slice())
            .bind(ordinal(index)?)
            .bind(reason.as_str())
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        }
        sqlx::query(
            "UPDATE identification_attempts
             SET status='decided',finished_at_us=? WHERE id=? AND status='running'",
        )
        .bind(command.now_us)
        .bind(command.attempt_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;

        let review_case_id =
            replace_review_case(&mut tx, &command, decision_id, reason, command.now_us).await?;
        commit_processing_checkpoint(&mut tx, &command, reason).await?;
        OutboxWriter::write(
            &mut tx,
            PROCESSING_TASK_IDENTIFICATION_DECIDED,
            command.lease.task.id,
            &ProcessingTaskIdentificationDecidedPayload {
                decision_id,
                level: command.decision.level,
                reason,
            },
            command.now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        Ok(IdentificationCommitOutcome {
            status: IdentificationCommitStatus::Decided,
            decision_id: Some(decision_id),
            review_case_id,
            level: Some(command.decision.level),
            provider_failure: None,
        })
    }
}

async fn load_revision(
    pool: &SqlitePool,
    task: &ProcessingTaskView,
) -> Result<FileRevisionSummary, AppError> {
    let row = sqlx::query(
        "SELECT r.size_bytes,r.modified_at_ns,s.status
         FROM discovery_file_revisions r
         JOIN discovery_file_revision_states s ON s.revision_id=r.id
         WHERE r.id=?",
    )
    .bind(task.file_revision_id.as_bytes().as_slice())
    .fetch_one(pool)
    .await
    .map_err(internal)?;
    let size_bytes = u64::try_from(row.get::<i64, _>("size_bytes")).map_err(internal)?;
    let modified_at_ns: i64 = row.get("modified_at_ns");
    let seconds = modified_at_ns.div_euclid(1_000_000_000);
    let nanos = u32::try_from(modified_at_ns.rem_euclid(1_000_000_000)).map_err(internal)?;
    let modified_at = chrono::DateTime::from_timestamp(seconds, nanos)
        .ok_or_else(|| invalid_stored("revision timestamp"))?
        .to_rfc3339_opts(SecondsFormat::Nanos, true);
    Ok(FileRevisionSummary {
        id: task.file_revision_id,
        inbox_directory_id: task.inbox_directory_id,
        relative_path: task.relative_path.clone(),
        size_bytes,
        modified_at,
        stability: row.get("status"),
    })
}

async fn load_evidence(
    pool: &SqlitePool,
    attempt_id: Uuid,
) -> Result<(Vec<IdentificationEvidenceView>, bool), AppError> {
    let rows = sqlx::query(
        "SELECT id,source,kind,normalized_value,strength,reason
         FROM identification_evidence WHERE attempt_id=? ORDER BY ordinal LIMIT 201",
    )
    .bind(attempt_id.as_bytes().as_slice())
    .fetch_all(pool)
    .await
    .map_err(internal)?;
    let truncated = rows.len() > 200;
    let values = rows
        .iter()
        .take(200)
        .map(|row| {
            Ok(IdentificationEvidenceView {
                id: row_uuid(row, "id")?,
                source: parse_evidence_source(&row.get::<String, _>("source"))?,
                kind: parse_evidence_kind(&row.get::<String, _>("kind"))?,
                value: row.get("normalized_value"),
                strength: parse_evidence_strength(&row.get::<String, _>("strength"))?,
                reason: row.get("reason"),
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok((values, truncated))
}

async fn load_candidates(
    pool: &SqlitePool,
    attempt_id: Uuid,
) -> Result<(Vec<IdentificationCandidateView>, bool), AppError> {
    let rows = sqlx::query(
        "SELECT id,media_type,provider_id,year,locale,original_title,ranking_score
         FROM identification_candidates WHERE attempt_id=? ORDER BY ordinal LIMIT 101",
    )
    .bind(attempt_id.as_bytes().as_slice())
    .fetch_all(pool)
    .await
    .map_err(internal)?;
    let truncated = rows.len() > 100;
    let mut values = Vec::with_capacity(rows.len().min(100));
    for row in rows.iter().take(100) {
        let id = row_uuid(row, "id")?;
        let title = sqlx::query_scalar::<_, String>(
            "SELECT value FROM identification_candidate_titles
             WHERE candidate_id=? AND kind='title' ORDER BY ordinal LIMIT 1",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_one(pool)
        .await
        .map_err(internal)?;
        let episode_rows = sqlx::query(
            "SELECT season_number,episode_number FROM identification_candidate_episodes
             WHERE candidate_id=? ORDER BY season_number,episode_number LIMIT 100",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_all(pool)
        .await
        .map_err(internal)?;
        let episodes = episode_rows
            .iter()
            .map(|episode| {
                Ok(CandidateEpisodeView {
                    season_number: u16::try_from(episode.get::<i64, _>("season_number"))
                        .map_err(internal)?,
                    episode_number: u16::try_from(episode.get::<i64, _>("episode_number"))
                        .map_err(internal)?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        values.push(IdentificationCandidateView {
            id,
            provider: "tmdb",
            media_type: row.get("media_type"),
            provider_id: row.get::<i64, _>("provider_id").to_string(),
            title,
            original_title: row.get("original_title"),
            year: row
                .get::<Option<i64>, _>("year")
                .map(u16::try_from)
                .transpose()
                .map_err(internal)?,
            locale: row.get("locale"),
            ranking_score: u16::try_from(row.get::<i64, _>("ranking_score")).map_err(internal)?,
            episodes,
        });
    }
    Ok((values, truncated))
}

async fn load_decision(
    pool: &SqlitePool,
    attempt_id: Uuid,
) -> Result<Option<IdentificationDecisionView>, AppError> {
    let row = sqlx::query(
        "SELECT id,level,reason,selected_candidate_id,retry_at_us,decided_at_us
         FROM identification_decisions WHERE attempt_id=?",
    )
    .bind(attempt_id.as_bytes().as_slice())
    .fetch_optional(pool)
    .await
    .map_err(internal)?;
    row.map(|row| {
        Ok(IdentificationDecisionView {
            id: row_uuid(&row, "id")?,
            level: parse_decision_level(&row.get::<String, _>("level"))?,
            reason: parse_processing_reason(&row.get::<String, _>("reason"))?,
            candidate_id: row
                .get::<Option<Vec<u8>>, _>("selected_candidate_id")
                .as_deref()
                .map(Uuid::from_slice)
                .transpose()
                .map_err(internal)?,
            retry_at: row
                .get::<Option<i64>, _>("retry_at_us")
                .map(timestamp_us)
                .transpose()?,
            decided_at: timestamp_us(row.get("decided_at_us"))?,
        })
    })
    .transpose()
}

fn parse_evidence_source(value: &str) -> Result<EvidenceSource, AppError> {
    match value {
        "filename" => Ok(EvidenceSource::Filename),
        "nfo" => Ok(EvidenceSource::Nfo),
        "tmdb" => Ok(EvidenceSource::Tmdb),
        "cache" => Ok(EvidenceSource::Cache),
        "system" => Ok(EvidenceSource::System),
        "manual-decision" => Ok(EvidenceSource::ManualDecision),
        "manual-feedback" => Ok(EvidenceSource::ManualFeedback),
        "enhancer" => Ok(EvidenceSource::Enhancer),
        _ => Err(invalid_stored("evidence source")),
    }
}

fn parse_evidence_kind(value: &str) -> Result<EvidenceKind, AppError> {
    match value {
        "external-id" => Ok(EvidenceKind::ExternalId),
        "title" => Ok(EvidenceKind::Title),
        "alias" => Ok(EvidenceKind::Alias),
        "year" => Ok(EvidenceKind::Year),
        "episode" => Ok(EvidenceKind::Episode),
        "media-type" => Ok(EvidenceKind::MediaType),
        "conflict" => Ok(EvidenceKind::Conflict),
        "availability" => Ok(EvidenceKind::Availability),
        _ => Err(invalid_stored("evidence kind")),
    }
}

fn parse_evidence_strength(value: &str) -> Result<EvidenceStrength, AppError> {
    match value {
        "strong" => Ok(EvidenceStrength::Strong),
        "supporting" => Ok(EvidenceStrength::Supporting),
        "conflicting" => Ok(EvidenceStrength::Conflicting),
        _ => Err(invalid_stored("evidence strength")),
    }
}

fn parse_decision_level(value: &str) -> Result<DecisionLevel, AppError> {
    match value {
        "confirmed" => Ok(DecisionLevel::Confirmed),
        "probable" => Ok(DecisionLevel::Probable),
        "ambiguous" => Ok(DecisionLevel::Ambiguous),
        "unidentified" => Ok(DecisionLevel::Unidentified),
        "blocked" => Ok(DecisionLevel::Blocked),
        _ => Err(invalid_stored("decision level")),
    }
}

fn parse_processing_reason(value: &str) -> Result<ProcessingReason, AppError> {
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
        _ => Err(invalid_stored("identification reason")),
    }
}

fn timestamp_us(value: i64) -> Result<String, AppError> {
    Utc.timestamp_micros(value)
        .single()
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Micros, true))
        .ok_or_else(|| invalid_stored("identification timestamp"))
}

fn row_uuid(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Uuid, AppError> {
    Uuid::from_slice(&row.get::<Vec<u8>, _>(column)).map_err(internal)
}

fn invalid_stored(kind: &str) -> AppError {
    AppError::new(ErrorCode::Internal, format!("stored {kind} is invalid"))
}

fn validate_commit(command: &IdentificationCommit<'_>) -> Result<(), AppError> {
    if command.evidence.len() > 256
        || command.candidates.len() > 20
        || command.decision.reasons.is_empty()
        || command.decision.reasons.len() > 32
        || command.decision.rule_version == 0
        || command
            .title_hint
            .is_some_and(|title| title.is_empty() || title.chars().count() > 512)
    {
        return Err(validation("identification commit is outside bounds"));
    }
    if command.decision.level == DecisionLevel::Confirmed
        && command.decision.selected_candidate.is_none()
    {
        return Err(validation("confirmed decision requires a candidate"));
    }
    if let Some(selected) = command.decision.selected_candidate
        && command
            .candidates
            .iter()
            .all(|candidate| candidate.id != selected)
    {
        return Err(validation("selected candidate is not in the attempt"));
    }
    for evidence in command.evidence {
        if evidence.source_version.is_empty()
            || evidence.source_version.len() > 64
            || evidence.normalized_value.is_empty()
            || evidence.normalized_value.len() > 1024
            || evidence.reason.is_empty()
            || evidence.reason.len() > 128
        {
            return Err(validation("identification evidence is outside bounds"));
        }
    }
    for candidate in command.candidates {
        if candidate.identity.provider_id <= 0
            || candidate.titles.is_empty()
            || candidate.titles.len() > 64
            || candidate.aliases.len() > 64
            || candidate.release_dates.len() > 64
            || candidate.episodes.len() > 100
            || candidate.external_ids.len() > 16
            || candidate.provider_version == 0
            || candidate.ranking_score > 100
            || !valid_locale(&candidate.locale)
            || candidate
                .original_title
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.chars().count() > 512)
            || candidate
                .titles
                .iter()
                .chain(&candidate.aliases)
                .any(|value| value.is_empty() || value.chars().count() > 512)
        {
            return Err(validation("identification candidate is outside bounds"));
        }
    }
    Ok(())
}

async fn assert_attempt_authority(
    tx: &mut Transaction<'_, Sqlite>,
    command: &IdentificationCommit<'_>,
) -> Result<(), AppError> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)
         FROM identification_attempts ia
         JOIN tasks_processing_tasks t ON t.id=ia.task_id
         WHERE ia.id=? AND ia.task_id=? AND ia.processing_attempt_id=?
           AND ia.file_revision_id=? AND ia.status='running'
           AND t.current_attempt_id=? AND t.status='running' AND t.cancel_requested=0
           AND t.stage='identification' AND t.lease_owner=? AND t.lease_expires_at_us>?",
    )
    .bind(command.attempt_id.as_bytes().as_slice())
    .bind(command.lease.task.id.as_bytes().as_slice())
    .bind(command.lease.attempt_id.as_bytes().as_slice())
    .bind(command.lease.task.file_revision_id.as_bytes().as_slice())
    .bind(command.lease.attempt_id.as_bytes().as_slice())
    .bind(&command.lease.owner)
    .bind(command.now_us)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    if exists == 1 {
        Ok(())
    } else {
        Err(lease_lost())
    }
}

async fn revision_is_current(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: Uuid,
) -> Result<bool, AppError> {
    let row = sqlx::query(
        "SELECT t.file_revision_id,f.current_revision_id,s.status
         FROM tasks_processing_tasks t
         JOIN discovery_tracked_files f ON f.id=t.discovered_file_id
         JOIN discovery_file_revision_states s ON s.revision_id=t.file_revision_id
         WHERE t.id=?",
    )
    .bind(task_id.as_bytes().as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    let expected: Vec<u8> = row.get("file_revision_id");
    let current: Option<Vec<u8>> = row.get("current_revision_id");
    Ok(current.as_deref() == Some(expected.as_slice())
        && row.get::<String, _>("status") == "stable")
}

async fn commit_revision_changed(
    tx: &mut Transaction<'_, Sqlite>,
    command: &IdentificationCommit<'_>,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE identification_attempts
         SET status='revision-changed',failure_code='identification.revision-changed',finished_at_us=?
         WHERE id=? AND status='running'",
    )
    .bind(command.now_us)
    .bind(command.attempt_id.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    sqlx::query(
        "UPDATE tasks_processing_attempts
         SET status='failed',failure_code='identification.revision-changed',lease_owner=NULL,
             finished_at_us=? WHERE id=? AND status='running'",
    )
    .bind(command.now_us)
    .bind(command.lease.attempt_id.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    assert_task_update(
        tx,
        command,
        "UPDATE tasks_processing_tasks
         SET status='cancelled',checkpoint='cancelled',reason='identification.revision-changed',
             recovering=0,cancel_requested=0,lease_owner=NULL,lease_expires_at_us=NULL,
             next_retry_at_us=NULL,version=version+1,updated_at_us=?
         WHERE id=? AND status='running' AND cancel_requested=0 AND lease_owner=?
           AND lease_expires_at_us>?",
    )
    .await?;
    write_state_event(
        tx,
        command.lease.task.id,
        "cancelled",
        "identification",
        ProcessingReason::IdentificationRevisionChanged,
        command.now_us,
    )
    .await
}

async fn insert_evidence(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: Uuid,
    evidence: &[EvidenceDraft],
    now_us: i64,
) -> Result<(), AppError> {
    for (index, item) in evidence.iter().enumerate() {
        let id = new_id();
        sqlx::query(
            "INSERT INTO identification_evidence
             (id,attempt_id,ordinal,source,source_version,kind,normalized_value,strength,
              reason,source_hash,created_at_us) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(attempt_id.as_bytes().as_slice())
        .bind(ordinal(index)?)
        .bind(item.source.as_str())
        .bind(&item.source_version)
        .bind(item.kind.as_str())
        .bind(&item.normalized_value)
        .bind(item.strength.as_str())
        .bind(&item.reason)
        .bind(item.source_hash.as_slice())
        .bind(now_us)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    }
    Ok(())
}

async fn insert_candidates(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: Uuid,
    candidates: &[DecisionCandidate],
    now_us: i64,
) -> Result<(), AppError> {
    for (index, candidate) in candidates.iter().enumerate() {
        let media_type = match candidate.identity.media_kind {
            ProviderMediaKind::Movie => "movie",
            ProviderMediaKind::Tv => "tv",
        };
        let source_hash = candidate_hash(candidate);
        sqlx::query(
            "INSERT INTO identification_candidates
             (id,attempt_id,ordinal,provider,media_type,provider_id,year,locale,original_title,
              ranking_score,provider_version,source_hash,created_at_us)
             VALUES (?,?,?,'tmdb',?,?,?,?,?,?,?,?,?)",
        )
        .bind(candidate.id.as_bytes().as_slice())
        .bind(attempt_id.as_bytes().as_slice())
        .bind(ordinal(index)?)
        .bind(media_type)
        .bind(candidate.identity.provider_id)
        .bind(candidate.year.map(i64::from))
        .bind(&candidate.locale)
        .bind(&candidate.original_title)
        .bind(i64::from(candidate.ranking_score))
        .bind(i64::from(candidate.provider_version))
        .bind(source_hash.as_slice())
        .bind(now_us)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        for (title_index, title) in candidate.titles.iter().enumerate() {
            insert_candidate_title(tx, candidate.id, "title", title_index, title).await?;
        }
        for (alias_index, alias) in candidate.aliases.iter().enumerate() {
            insert_candidate_title(tx, candidate.id, "alias", alias_index, alias).await?;
        }
        for (date_index, date) in candidate.release_dates.iter().enumerate() {
            sqlx::query(
                "INSERT INTO identification_candidate_release_dates
                 (candidate_id,ordinal,release_date) VALUES (?,?,?)",
            )
            .bind(candidate.id.as_bytes().as_slice())
            .bind(ordinal(date_index)?)
            .bind(date.to_string())
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        }
        for episode in &candidate.episodes {
            sqlx::query(
                "INSERT INTO identification_candidate_episodes
                 (candidate_id,season_number,episode_number) VALUES (?,?,?)",
            )
            .bind(candidate.id.as_bytes().as_slice())
            .bind(i64::from(episode.season))
            .bind(i64::from(episode.episode))
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        }
        for (external_index, external_id) in candidate.external_ids.iter().enumerate() {
            sqlx::query(
                "INSERT INTO identification_candidate_external_ids
                 (candidate_id,ordinal,provider,value,is_default) VALUES (?,?,?,?,?)",
            )
            .bind(candidate.id.as_bytes().as_slice())
            .bind(ordinal(external_index)?)
            .bind(&external_id.provider)
            .bind(&external_id.value)
            .bind(i64::from(external_id.is_default))
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        }
    }
    Ok(())
}

async fn insert_candidate_title(
    tx: &mut Transaction<'_, Sqlite>,
    candidate_id: Uuid,
    kind: &str,
    index: usize,
    value: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO identification_candidate_titles(candidate_id,kind,ordinal,value)
         VALUES (?,?,?,?)",
    )
    .bind(candidate_id.as_bytes().as_slice())
    .bind(kind)
    .bind(ordinal(index)?)
    .bind(value)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(())
}

async fn replace_review_case(
    tx: &mut Transaction<'_, Sqlite>,
    command: &IdentificationCommit<'_>,
    decision_id: Uuid,
    reason: ProcessingReason,
    now_us: i64,
) -> Result<Option<Uuid>, AppError> {
    let active = sqlx::query(
        "SELECT id FROM identification_review_cases WHERE task_id=? AND status='active'",
    )
    .bind(command.lease.task.id.as_bytes().as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    if command.decision.level == DecisionLevel::Blocked {
        return active.map(|row| row_uuid(&row, "id")).transpose();
    }
    let next_version = match active.as_ref() {
        Some(row) => sqlx::query_scalar::<_, i64>(
            "SELECT version FROM identification_review_cases WHERE id=?",
        )
        .bind(row.get::<Vec<u8>, _>("id"))
        .fetch_one(&mut **tx)
        .await
        .map_err(internal)?
        .checked_add(1)
        .ok_or_else(|| validation("review case version exceeds bounds"))?,
        None => 1,
    };
    sqlx::query(
        "UPDATE identification_review_cases
         SET status='closed',closed_at_us=?,updated_at_us=?
         WHERE task_id=? AND status='active'",
    )
    .bind(now_us)
    .bind(now_us)
    .bind(command.lease.task.id.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    if !matches!(
        command.decision.level,
        DecisionLevel::Probable | DecisionLevel::Ambiguous | DecisionLevel::Unidentified
    ) {
        return Ok(None);
    }
    let account_id = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT account_id FROM tasks_processing_tasks WHERE id=?",
    )
    .bind(command.lease.task.id.as_bytes().as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    let id = new_id();
    sqlx::query(
        "INSERT INTO identification_review_cases
         (id,account_id,task_id,attempt_id,decision_id,file_revision_id,inbox_directory_id,
          level,reason,title_hint,status,created_at_us,updated_at_us)
         VALUES (?,?,?,?,?,?,?,?,?,?,'active',?,?)",
    )
    .bind(id.as_bytes().as_slice())
    .bind(account_id)
    .bind(command.lease.task.id.as_bytes().as_slice())
    .bind(command.attempt_id.as_bytes().as_slice())
    .bind(decision_id.as_bytes().as_slice())
    .bind(command.lease.task.file_revision_id.as_bytes().as_slice())
    .bind(command.lease.task.inbox_directory_id.as_bytes().as_slice())
    .bind(command.decision.level.as_str())
    .bind(reason.as_str())
    .bind(command.title_hint)
    .bind(now_us)
    .bind(now_us)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    if next_version > 1 {
        sqlx::query("UPDATE identification_review_cases SET version=? WHERE id=?")
            .bind(next_version)
            .bind(id.as_bytes().as_slice())
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
    }
    Ok(Some(id))
}

async fn commit_processing_checkpoint(
    tx: &mut Transaction<'_, Sqlite>,
    command: &IdentificationCommit<'_>,
    reason: ProcessingReason,
) -> Result<(), AppError> {
    let (status, stage, checkpoint, attempt_status, failure_code, next_retry) =
        match command.decision.level {
            DecisionLevel::Confirmed => (
                "queued",
                "planning",
                "identification-complete",
                "queued",
                None,
                None,
            ),
            DecisionLevel::Probable | DecisionLevel::Ambiguous | DecisionLevel::Unidentified => (
                "waiting-confirmation",
                "identification",
                "waiting-confirmation",
                "succeeded",
                None,
                None,
            ),
            DecisionLevel::Blocked => (
                "paused",
                "identification",
                "dependency-blocked",
                "failed",
                Some(reason.as_str()),
                command.decision.retry_at_us,
            ),
        };
    let decision_checkpoint_update =
        if status != "paused" && command.lease.task.current_task_decision_id.is_some() {
            ",decision_checkpoint=NULL"
        } else {
            ""
        };
    let sql = format!(
        "UPDATE tasks_processing_tasks
         SET status='{status}',stage='{stage}',checkpoint='{checkpoint}',reason=?,recovering=0,
             cancel_requested=0,lease_owner=NULL,lease_expires_at_us=NULL,next_retry_at_us=?,
             version=version+1,updated_at_us=?{decision_checkpoint_update}
         WHERE id=? AND status='running' AND cancel_requested=0 AND lease_owner=?
           AND lease_expires_at_us>?"
    );
    let result = sqlx::query(&sql)
        .bind(reason.as_str())
        .bind(next_retry)
        .bind(command.now_us)
        .bind(command.lease.task.id.as_bytes().as_slice())
        .bind(&command.lease.owner)
        .bind(command.now_us)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    if result.rows_affected() != 1 {
        return Err(lease_lost());
    }
    sqlx::query(
        "UPDATE tasks_processing_attempts
         SET status=?,stage=?,lease_owner=NULL,failure_code=?,finished_at_us=?
         WHERE id=? AND status='running'",
    )
    .bind(attempt_status)
    .bind(stage)
    .bind(failure_code)
    .bind((attempt_status != "queued").then_some(command.now_us))
    .bind(command.lease.attempt_id.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    write_state_event(
        tx,
        command.lease.task.id,
        status,
        stage,
        reason,
        command.now_us,
    )
    .await
}

async fn assert_task_update(
    tx: &mut Transaction<'_, Sqlite>,
    command: &IdentificationCommit<'_>,
    sql: &str,
) -> Result<(), AppError> {
    let result = sqlx::query(sql)
        .bind(command.now_us)
        .bind(command.lease.task.id.as_bytes().as_slice())
        .bind(&command.lease.owner)
        .bind(command.now_us)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(lease_lost())
    }
}

async fn write_state_event(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: Uuid,
    status: &str,
    stage: &str,
    reason: ProcessingReason,
    now_us: i64,
) -> Result<(), AppError> {
    OutboxWriter::write(
        tx,
        PROCESSING_TASK_STATE_CHANGED,
        task_id,
        &serde_json::json!({
            "status": status,
            "stage": stage,
            "recovering": false,
            "reason": reason,
        }),
        now_us,
    )
    .await
    .map(|_| ())
}

fn public_reason(decision: &IdentificationDecisionDraft) -> ProcessingReason {
    match decision.level {
        DecisionLevel::Confirmed
            if decision
                .reasons
                .contains(&DecisionReason::ExternalIdVerified) =>
        {
            ProcessingReason::IdentificationConfirmedExternalId
        }
        DecisionLevel::Confirmed => ProcessingReason::IdentificationConfirmedTitleYear,
        DecisionLevel::Probable => ProcessingReason::IdentificationProbableTitle,
        DecisionLevel::Ambiguous
            if decision
                .reasons
                .contains(&DecisionReason::MultipleStrongCandidates) =>
        {
            ProcessingReason::IdentificationMultipleStrongCandidates
        }
        DecisionLevel::Ambiguous => ProcessingReason::IdentificationAmbiguous,
        DecisionLevel::Unidentified => ProcessingReason::IdentificationNoCandidate,
        DecisionLevel::Blocked
            if decision
                .reasons
                .contains(&DecisionReason::ProviderUnauthorized) =>
        {
            ProcessingReason::IdentificationProviderUnauthorized
        }
        DecisionLevel::Blocked => ProcessingReason::IdentificationProviderUnavailable,
    }
}

fn candidate_hash(candidate: &DecisionCandidate) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.identification.candidate.v1\0");
    hasher.update(candidate.identity.provider_id.to_be_bytes());
    hasher.update([match candidate.identity.media_kind {
        ProviderMediaKind::Movie => 0,
        ProviderMediaKind::Tv => 1,
    }]);
    for value in candidate.titles.iter().chain(&candidate.aliases) {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    if let Some(year) = candidate.year {
        hasher.update(year.to_be_bytes());
    }
    hasher.update(candidate.locale.as_bytes());
    if let Some(original_title) = &candidate.original_title {
        hasher.update(original_title.as_bytes());
    }
    hasher.finalize().into()
}

fn valid_locale(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 5
        && bytes[..2].iter().all(u8::is_ascii_lowercase)
        && bytes[2] == b'-'
        && bytes[3..].iter().all(u8::is_ascii_uppercase)
}

fn ordinal(index: usize) -> Result<i64, AppError> {
    i64::try_from(index + 1).map_err(internal)
}

fn lease_lost() -> AppError {
    AppError::new(
        ErrorCode::TaskLeaseLost,
        "identification lease is no longer authoritative",
    )
}

fn validation(message: &str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
