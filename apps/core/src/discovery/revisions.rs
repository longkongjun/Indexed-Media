use async_trait::async_trait;
use sqlx::Row;
use uuid::Uuid;

use crate::discovery::auxiliary::{AuxiliaryReason, classify_auxiliary};
use crate::discovery::observations::FileObservation;
use crate::discovery::policy::load_or_ensure_policy;
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::id::new_id;

/// 产生文件元数据观察的入口。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationSource {
    /// 用户或系统触发的 M2 扫描。
    Scan,
    /// 文件系统监听器提示。
    Watcher,
    /// 启动或周期完整对账。
    Reconcile,
}

impl ObservationSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Watcher => "watcher",
            Self::Reconcile => "reconcile",
        }
    }
}

/// 当前 revision 的持久状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevisionStatus {
    /// 尚未同时满足年龄和重复观察门禁。
    Observing,
    /// 已满足稳定门禁并确保下游处理请求。
    Stable,
    /// 最新对账没有发现该文件。
    Missing,
    /// 同一路径已经被不同不可变事实替换。
    Superseded,
    /// 显式辅助视频标记使其不进入识别。
    SkippedAuxiliary,
}

impl RevisionStatus {
    fn parse(value: &str) -> Result<Self, AppError> {
        match value {
            "observing" => Ok(Self::Observing),
            "stable" => Ok(Self::Stable),
            "missing" => Ok(Self::Missing),
            "superseded" => Ok(Self::Superseded),
            "skipped-auxiliary" => Ok(Self::SkippedAuxiliary),
            _ => Err(AppError::new(
                ErrorCode::Internal,
                "stored revision status is invalid",
            )),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Observing => "observing",
            Self::Stable => "stable",
            Self::Missing => "missing",
            Self::Superseded => "superseded",
            Self::SkippedAuxiliary => "skipped-auxiliary",
        }
    }
}

/// 一次原子观察后的 revision 投影。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevisionOutcome {
    /// 稳定的不可变 revision ID。
    pub revision_id: Uuid,
    /// 观察后的状态。
    pub status: RevisionStatus,
    /// 达到最小间隔的相同事实观察次数。
    pub matching_observations: i64,
    /// 本次事务是否首次创建了下游处理请求。
    pub processing_requested: bool,
    /// 辅助视频跳过原因；其他状态为 `None`。
    pub skip_reason: Option<AuxiliaryReason>,
}

#[async_trait]
/// 扫描、watcher 与对账共享的 revision 写入契约。
pub trait RevisionObserver: Send + Sync {
    /// 原子观察一个文件并推进其稳定性投影。
    ///
    /// # Errors
    ///
    /// 收件箱不存在、值无法持久化或数据库事务失败时返回 [`AppError`]。
    async fn observe(
        &self,
        observation: FileObservation,
        source: ObservationSource,
        observed_at_us: i64,
    ) -> Result<RevisionOutcome, AppError>;
}

#[derive(Clone)]
/// 持久化不可变文件 revision 及其可恢复状态投影。
pub struct RevisionService {
    pool: sqlx::SqlitePool,
}

impl RevisionService {
    #[must_use]
    /// 创建 revision 服务。
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    /// 将当前路径标记为缺失，同时保留不可变 revision 历史。
    ///
    /// # Errors
    ///
    /// 数据库失败或持久化 ID 损坏时返回 [`AppError`]。未知路径返回 `Ok(false)`。
    pub async fn mark_missing(
        &self,
        inbox_id: Uuid,
        relative_path_bytes: &[u8],
        observed_at_us: i64,
    ) -> Result<bool, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let revision = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT current_revision_id FROM discovery_tracked_files
             WHERE inbox_directory_id=? AND relative_path_bytes=?",
        )
        .bind(inbox_id.as_bytes().as_slice())
        .bind(relative_path_bytes)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?;
        let Some(revision) = revision else {
            tx.rollback().await.map_err(internal)?;
            return Ok(false);
        };
        let revision_id = parse_uuid(&revision)?;
        sqlx::query(
            "UPDATE discovery_tracked_files
             SET missing_at_us=?,version=version+1,updated_at_us=?
             WHERE inbox_directory_id=? AND relative_path_bytes=?",
        )
        .bind(observed_at_us)
        .bind(observed_at_us)
        .bind(inbox_id.as_bytes().as_slice())
        .bind(relative_path_bytes)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "UPDATE discovery_file_revision_states
             SET status='missing',missing_at_us=?,next_check_at_us=NULL,
                 version=version+1,updated_at_us=? WHERE revision_id=?",
        )
        .bind(observed_at_us)
        .bind(observed_at_us)
        .bind(revision_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(true)
    }
}

#[async_trait]
impl RevisionObserver for RevisionService {
    async fn observe(
        &self,
        observation: FileObservation,
        source: ObservationSource,
        observed_at_us: i64,
    ) -> Result<RevisionOutcome, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let outcome = observe_in_connection(&mut tx, &observation, source, observed_at_us).await?;
        tx.commit().await.map_err(internal)?;
        Ok(outcome)
    }
}

pub(crate) async fn observe_in_connection(
    connection: &mut sqlx::SqliteConnection,
    observation: &FileObservation,
    source: ObservationSource,
    observed_at_us: i64,
) -> Result<RevisionOutcome, AppError> {
    let size_bytes = i64::try_from(observation.size_bytes).map_err(internal)?;
    let policy =
        load_or_ensure_policy(connection, observation.inbox_directory_id, observed_at_us).await?;
    let tracked = sqlx::query(
        "SELECT id,current_revision_id FROM discovery_tracked_files
         WHERE inbox_directory_id=? AND relative_path_bytes=?",
    )
    .bind(observation.inbox_directory_id.as_bytes().as_slice())
    .bind(&observation.relative_path_bytes)
    .fetch_optional(&mut *connection)
    .await
    .map_err(internal)?;
    let (tracked_id, current_revision_id) = if let Some(row) = tracked {
        let tracked_id = parse_uuid(row.get::<Vec<u8>, _>("id").as_slice())?;
        let current = row
            .get::<Option<Vec<u8>>, _>("current_revision_id")
            .map(|bytes| parse_uuid(&bytes))
            .transpose()?;
        (tracked_id, current)
    } else {
        let tracked_id = new_id();
        sqlx::query(
            "INSERT INTO discovery_tracked_files
             (id,inbox_directory_id,relative_path_bytes,relative_path_display,current_revision_id,
              last_observed_at_us,missing_at_us,version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,NULL,?,NULL,1,?,?)",
        )
        .bind(tracked_id.as_bytes().as_slice())
        .bind(observation.inbox_directory_id.as_bytes().as_slice())
        .bind(&observation.relative_path_bytes)
        .bind(&observation.relative_path_display)
        .bind(observed_at_us)
        .bind(observed_at_us)
        .bind(observed_at_us)
        .execute(&mut *connection)
        .await
        .map_err(internal)?;
        (tracked_id, None)
    };

    if let Some(revision_id) = current_revision_id {
        let facts = sqlx::query(
            "SELECT identity_snapshot,size_bytes,modified_at_ns
             FROM discovery_file_revisions WHERE id=?",
        )
        .bind(revision_id.as_bytes().as_slice())
        .fetch_one(&mut *connection)
        .await
        .map_err(internal)?;
        if facts.get::<Vec<u8>, _>("identity_snapshot") == observation.identity_snapshot
            && facts.get::<i64, _>("size_bytes") == size_bytes
            && facts.get::<i64, _>("modified_at_ns") == observation.modified_at_ns
        {
            return advance_existing(
                connection,
                tracked_id,
                revision_id,
                observation,
                source,
                observed_at_us,
            )
            .await;
        }
        sqlx::query(
            "UPDATE discovery_file_revision_states
             SET status=CASE WHEN status='missing' THEN status ELSE 'superseded' END,
                 next_check_at_us=NULL,version=version+1,updated_at_us=?
             WHERE revision_id=?",
        )
        .bind(observed_at_us)
        .bind(revision_id.as_bytes().as_slice())
        .execute(&mut *connection)
        .await
        .map_err(internal)?;
    }

    create_or_reactivate_revision(
        connection,
        tracked_id,
        observation,
        source,
        observed_at_us,
        StabilityPolicySnapshot {
            version: policy.config_version,
            minimum_age_seconds: policy.minimum_age_seconds,
            observation_interval_seconds: policy.stable_observation_interval_seconds,
        },
    )
    .await
}

#[derive(Clone, Copy)]
struct StabilityPolicySnapshot {
    version: i64,
    minimum_age_seconds: i64,
    observation_interval_seconds: i64,
}

async fn create_or_reactivate_revision(
    connection: &mut sqlx::SqliteConnection,
    tracked_id: Uuid,
    observation: &FileObservation,
    source: ObservationSource,
    observed_at_us: i64,
    policy: StabilityPolicySnapshot,
) -> Result<RevisionOutcome, AppError> {
    let proposed_revision = new_id();
    let size_bytes = i64::try_from(observation.size_bytes).map_err(internal)?;
    sqlx::query(
        "INSERT INTO discovery_file_revisions
         (id,tracked_file_id,identity_snapshot,size_bytes,modified_at_ns,policy_version,
          minimum_age_seconds,stable_observation_interval_seconds,created_at_us)
         VALUES (?,?,?,?,?,?,?,?,?)
         ON CONFLICT(tracked_file_id,identity_snapshot,size_bytes,modified_at_ns) DO NOTHING",
    )
    .bind(proposed_revision.as_bytes().as_slice())
    .bind(tracked_id.as_bytes().as_slice())
    .bind(&observation.identity_snapshot)
    .bind(size_bytes)
    .bind(observation.modified_at_ns)
    .bind(policy.version)
    .bind(policy.minimum_age_seconds)
    .bind(policy.observation_interval_seconds)
    .bind(observed_at_us)
    .execute(&mut *connection)
    .await
    .map_err(internal)?;
    let revision_bytes = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT id FROM discovery_file_revisions
         WHERE tracked_file_id=? AND identity_snapshot=? AND size_bytes=? AND modified_at_ns=?",
    )
    .bind(tracked_id.as_bytes().as_slice())
    .bind(&observation.identity_snapshot)
    .bind(size_bytes)
    .bind(observation.modified_at_ns)
    .fetch_one(&mut *connection)
    .await
    .map_err(internal)?;
    let revision_id = parse_uuid(&revision_bytes)?;
    let skip_reason = classify_auxiliary(&observation.relative_path_display);
    let status = if skip_reason.is_some() {
        RevisionStatus::SkippedAuxiliary
    } else {
        RevisionStatus::Observing
    };
    let next_check_at_us = if status == RevisionStatus::Observing {
        Some(next_check_at(
            observation.modified_at_ns,
            policy.minimum_age_seconds,
            observed_at_us,
            policy.observation_interval_seconds,
            1,
        ))
    } else {
        None
    };
    sqlx::query(
        "INSERT INTO discovery_file_revision_states
         (revision_id,status,matching_observations,first_observed_at_us,
          last_counted_observed_at_us,last_observed_at_us,next_check_at_us,stable_at_us,
          missing_at_us,last_observation_source,skip_reason,version,updated_at_us)
         VALUES (?,?,1,?,?,?,?,NULL,NULL,?,?,1,?)
         ON CONFLICT(revision_id) DO UPDATE SET
           status=excluded.status,matching_observations=1,
           first_observed_at_us=excluded.first_observed_at_us,
           last_counted_observed_at_us=excluded.last_counted_observed_at_us,
           last_observed_at_us=excluded.last_observed_at_us,
           next_check_at_us=excluded.next_check_at_us,stable_at_us=NULL,missing_at_us=NULL,
           last_observation_source=excluded.last_observation_source,
           skip_reason=excluded.skip_reason,version=discovery_file_revision_states.version+1,
           updated_at_us=excluded.updated_at_us",
    )
    .bind(revision_id.as_bytes().as_slice())
    .bind(status.as_str())
    .bind(observed_at_us)
    .bind(observed_at_us)
    .bind(observed_at_us)
    .bind(next_check_at_us)
    .bind(source.as_str())
    .bind(skip_reason.map(AuxiliaryReason::as_str))
    .bind(observed_at_us)
    .execute(&mut *connection)
    .await
    .map_err(internal)?;
    sqlx::query(
        "UPDATE discovery_tracked_files
         SET relative_path_display=?,current_revision_id=?,last_observed_at_us=?,missing_at_us=NULL,
             version=version+1,updated_at_us=? WHERE id=?",
    )
    .bind(&observation.relative_path_display)
    .bind(revision_id.as_bytes().as_slice())
    .bind(observed_at_us)
    .bind(observed_at_us)
    .bind(tracked_id.as_bytes().as_slice())
    .execute(&mut *connection)
    .await
    .map_err(internal)?;
    Ok(RevisionOutcome {
        revision_id,
        status,
        matching_observations: 1,
        processing_requested: false,
        skip_reason,
    })
}

async fn advance_existing(
    connection: &mut sqlx::SqliteConnection,
    tracked_id: Uuid,
    revision_id: Uuid,
    observation: &FileObservation,
    source: ObservationSource,
    observed_at_us: i64,
) -> Result<RevisionOutcome, AppError> {
    let row = sqlx::query(
        "SELECT s.status,s.matching_observations,s.last_counted_observed_at_us,
                s.last_observed_at_us,s.skip_reason,r.minimum_age_seconds,
                r.stable_observation_interval_seconds,r.modified_at_ns
         FROM discovery_file_revision_states s
         JOIN discovery_file_revisions r ON r.id=s.revision_id
         WHERE s.revision_id=?",
    )
    .bind(revision_id.as_bytes().as_slice())
    .fetch_one(&mut *connection)
    .await
    .map_err(internal)?;
    let previous_status = RevisionStatus::parse(row.get::<String, _>("status").as_str())?;
    let mut status = previous_status;
    let previous_last_observed: i64 = row.get("last_observed_at_us");
    let last_counted: i64 = row.get("last_counted_observed_at_us");
    let interval_seconds: i64 = row.get("stable_observation_interval_seconds");
    let minimum_age_seconds: i64 = row.get("minimum_age_seconds");
    let modified_at_ns: i64 = row.get("modified_at_ns");
    let mut matching: i64 = row.get("matching_observations");
    let skip_reason = parse_skip_reason(row.get::<Option<String>, _>("skip_reason").as_deref())?;

    if status == RevisionStatus::Missing {
        status = if skip_reason.is_some() {
            RevisionStatus::SkippedAuxiliary
        } else {
            RevisionStatus::Observing
        };
        matching = 1;
    } else if status == RevisionStatus::Observing
        && observed_at_us >= last_counted.saturating_add(seconds_to_us(interval_seconds))
    {
        matching = matching.saturating_add(1);
    }
    let effective_last_observed = previous_last_observed.max(observed_at_us);
    let counted_at = if status == RevisionStatus::Observing
        && (matching == 1 && previous_status == RevisionStatus::Missing
            || observed_at_us >= last_counted.saturating_add(seconds_to_us(interval_seconds)))
    {
        observed_at_us
    } else {
        last_counted
    };
    if status == RevisionStatus::Observing
        && matching >= 2
        && effective_last_observed >= age_due_at(modified_at_ns, minimum_age_seconds)
    {
        status = RevisionStatus::Stable;
    }
    let next_check = if status == RevisionStatus::Observing {
        Some(next_check_at(
            modified_at_ns,
            minimum_age_seconds,
            counted_at,
            interval_seconds,
            matching,
        ))
    } else {
        None
    };
    let processing_requested = if status == RevisionStatus::Stable {
        ensure_processing_request(connection, revision_id, observed_at_us).await?
    } else {
        false
    };
    persist_revision_state(
        connection,
        revision_id,
        StateAdvance {
            status,
            previous_status,
            matching,
            counted_at,
            effective_last_observed,
            next_check,
            source,
            observed_at_us,
        },
    )
    .await?;
    update_tracked_observation(
        connection,
        tracked_id,
        &observation.relative_path_display,
        effective_last_observed,
    )
    .await?;
    Ok(RevisionOutcome {
        revision_id,
        status,
        matching_observations: matching,
        processing_requested,
        skip_reason,
    })
}

struct StateAdvance {
    status: RevisionStatus,
    previous_status: RevisionStatus,
    matching: i64,
    counted_at: i64,
    effective_last_observed: i64,
    next_check: Option<i64>,
    source: ObservationSource,
    observed_at_us: i64,
}

async fn persist_revision_state(
    connection: &mut sqlx::SqliteConnection,
    revision_id: Uuid,
    advance: StateAdvance,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE discovery_file_revision_states
         SET status=?,matching_observations=?,
             first_observed_at_us=CASE WHEN ?='missing' THEN ? ELSE first_observed_at_us END,
             last_counted_observed_at_us=?,
             last_observed_at_us=?,next_check_at_us=?,stable_at_us=CASE
               WHEN ?='stable' THEN COALESCE(stable_at_us,?) ELSE NULL END,
             missing_at_us=NULL,last_observation_source=?,version=version+1,updated_at_us=?
         WHERE revision_id=?",
    )
    .bind(advance.status.as_str())
    .bind(advance.matching)
    .bind(advance.previous_status.as_str())
    .bind(advance.observed_at_us)
    .bind(advance.counted_at)
    .bind(advance.effective_last_observed)
    .bind(advance.next_check)
    .bind(advance.status.as_str())
    .bind(advance.observed_at_us)
    .bind(advance.source.as_str())
    .bind(advance.effective_last_observed)
    .bind(revision_id.as_bytes().as_slice())
    .execute(&mut *connection)
    .await
    .map(|_| ())
    .map_err(internal)
}

async fn update_tracked_observation(
    connection: &mut sqlx::SqliteConnection,
    tracked_id: Uuid,
    relative_path_display: &str,
    observed_at_us: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE discovery_tracked_files
         SET relative_path_display=?,last_observed_at_us=?,missing_at_us=NULL,
             version=version+1,updated_at_us=? WHERE id=?",
    )
    .bind(relative_path_display)
    .bind(observed_at_us)
    .bind(observed_at_us)
    .bind(tracked_id.as_bytes().as_slice())
    .execute(&mut *connection)
    .await
    .map(|_| ())
    .map_err(internal)
}

async fn ensure_processing_request(
    connection: &mut sqlx::SqliteConnection,
    revision_id: Uuid,
    observed_at_us: i64,
) -> Result<bool, AppError> {
    let request_id = new_id();
    sqlx::query(
        "INSERT INTO discovery_processing_requests (id,revision_id,status,created_at_us)
         VALUES (?,?,'pending',?) ON CONFLICT(revision_id) DO NOTHING",
    )
    .bind(request_id.as_bytes().as_slice())
    .bind(revision_id.as_bytes().as_slice())
    .bind(observed_at_us)
    .execute(&mut *connection)
    .await
    .map(|result| result.rows_affected() == 1)
    .map_err(internal)
}

fn next_check_at(
    modified_at_ns: i64,
    minimum_age_seconds: i64,
    last_counted_at_us: i64,
    observation_interval_seconds: i64,
    matching_observations: i64,
) -> i64 {
    let age_due = age_due_at(modified_at_ns, minimum_age_seconds);
    if matching_observations >= 2 {
        age_due
    } else {
        age_due.max(last_counted_at_us.saturating_add(seconds_to_us(observation_interval_seconds)))
    }
}

fn age_due_at(modified_at_ns: i64, minimum_age_seconds: i64) -> i64 {
    let modified_us =
        modified_at_ns.div_euclid(1_000) + i64::from(modified_at_ns.rem_euclid(1_000) != 0);
    modified_us.saturating_add(seconds_to_us(minimum_age_seconds))
}

const fn seconds_to_us(seconds: i64) -> i64 {
    seconds.saturating_mul(1_000_000)
}

fn parse_uuid(bytes: &[u8]) -> Result<Uuid, AppError> {
    Uuid::from_slice(bytes).map_err(|error| AppError::with_source(ErrorCode::Internal, error))
}

fn parse_skip_reason(value: Option<&str>) -> Result<Option<AuxiliaryReason>, AppError> {
    match value {
        None => Ok(None),
        Some("sample") => Ok(Some(AuxiliaryReason::Sample)),
        Some("trailer") => Ok(Some(AuxiliaryReason::Trailer)),
        Some("extra") => Ok(Some(AuxiliaryReason::Extra)),
        Some(_) => Err(AppError::new(
            ErrorCode::Internal,
            "stored auxiliary reason is invalid",
        )),
    }
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
