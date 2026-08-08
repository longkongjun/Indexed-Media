use chrono::{SecondsFormat, TimeZone as _, Utc};
use sha2::{Digest as _, Sha256};
use sqlx::{Row as _, SqlitePool};
use uuid::Uuid;

use crate::discovery::model::{FileIdentity, RelativePath, RootId};
use crate::platform::outbox::OutboxNotifier;
use crate::shared::error::{AppError, ErrorCode};

use super::fs::{AppliedFile, FileLocator, ObservedFile};
use super::model::OrganizationOperation;
use super::plan_store::OrganizationPlanRecord;
use super::planner::PlannedOperationKind;

/// journal 中持久化的具体文件操作类别。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalKind {
    /// 流式复制。
    Copy,
    /// 同设备移动，或跨设备 move 的父 operation。
    Move,
    /// 同设备硬链接。
    Hardlink,
    /// 跨设备 move 在目标核对后的来源删除子步骤。
    SourceRemoval,
    /// 缺失 NFO 创建。
    Nfo,
}

impl JournalKind {
    #[must_use]
    /// 返回数据库与公开投影共用的稳定值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Hardlink => "hardlink",
            Self::SourceRemoval => "source-removal",
            Self::Nfo => "nfo",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "copy" => Some(Self::Copy),
            "move" => Some(Self::Move),
            "hardlink" => Some(Self::Hardlink),
            "source-removal" => Some(Self::SourceRemoval),
            "nfo" => Some(Self::Nfo),
            _ => None,
        }
    }
}

impl From<OrganizationOperation> for JournalKind {
    fn from(value: OrganizationOperation) -> Self {
        match value {
            OrganizationOperation::Copy => Self::Copy,
            OrganizationOperation::Move => Self::Move,
            OrganizationOperation::Hardlink => Self::Hardlink,
        }
    }
}

/// 一个文件 operation 的持久恢复状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalStatus {
    /// 已持久化全部执行输入，尚未声明开始 I/O。
    Prepared,
    /// 执行声明已提交，文件系统结果尚未持久化。
    Executing,
    /// 实际文件结果已持久化，等待重新观察核对。
    Applied,
    /// 文件结果已重新观察并核对。
    Verified,
    /// 已核对结果已安全补偿。
    Compensated,
    /// 无法证明重放或补偿安全。
    ManualReview,
}

impl JournalStatus {
    #[must_use]
    /// 返回持久稳定值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Executing => "executing",
            Self::Applied => "applied",
            Self::Verified => "verified",
            Self::Compensated => "compensated",
            Self::ManualReview => "manual-review",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "prepared" => Some(Self::Prepared),
            "executing" => Some(Self::Executing),
            "applied" => Some(Self::Applied),
            "verified" => Some(Self::Verified),
            "compensated" => Some(Self::Compensated),
            "manual-review" => Some(Self::ManualReview),
            _ => None,
        }
    }
}

/// 从数据库恢复的一项 journal 聚合。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalRecord {
    /// journal 稳定 ID。
    pub id: Uuid,
    /// 不可变计划 operation ID；跨设备来源删除使用确定性子 ID。
    pub operation_id: Uuid,
    /// 可选父 operation ID。
    pub parent_operation_id: Option<Uuid>,
    /// 具体文件语义。
    pub kind: JournalKind,
    /// 当前状态。
    pub status: JournalStatus,
    /// 所属任务。
    pub task_id: Uuid,
    /// 所属计划。
    pub plan_id: Uuid,
    /// 所属计划版本。
    pub plan_version: i64,
    /// 执行前来源事实；NFO 可为空。
    pub expected_source: Option<ObservedFile>,
    /// 逻辑目标位置。
    pub destination: FileLocator,
    /// 实际 applied 结果；prepared/executing 可为空。
    pub applied: Option<AppliedFile>,
    /// NFO journal 准备时目标是否已存在；其他类别为空。
    pub nfo_preexisting: Option<bool>,
    /// NFO journal 核对的是已有字节还是本次生成产物。
    pub nfo_outcome: Option<LocalNfoStatus>,
    /// 稳定暂停/人工处理原因。
    pub reason: Option<String>,
    /// 乐观并发投影版本。
    pub projection_version: i64,
    /// 最近更新时间 Unix epoch 微秒。
    pub updated_at_us: i64,
}

/// `LocalResult` 的用户可见状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalResultStatus {
    /// 文件已核对，但仍有后续步骤。
    PartialSuccess,
    /// 当前 change 要求的本地步骤全部完成。
    Completed,
    /// 文件结果已安全补偿。
    Compensated,
    /// 外部变化使自动处理不再安全。
    ManualReview,
}

impl LocalResultStatus {
    #[must_use]
    /// 返回持久稳定值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PartialSuccess => "partial-success",
            Self::Completed => "completed",
            Self::Compensated => "compensated",
            Self::ManualReview => "manual-review",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "partial-success" => Some(Self::PartialSuccess),
            "completed" => Some(Self::Completed),
            "compensated" => Some(Self::Compensated),
            "manual-review" => Some(Self::ManualReview),
            _ => None,
        }
    }
}

/// `LocalResult` 的 NFO 结论。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalNfoStatus {
    /// 当前计划未请求 NFO。
    NotRequested,
    /// 已有 NFO 保持不变。
    Preserved,
    /// 缺失 NFO 已生成。
    Generated,
    /// 文件已完成但 NFO 失败。
    Failed,
}

impl LocalNfoStatus {
    /// 返回持久稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not-requested",
            Self::Preserved => "preserved",
            Self::Generated => "generated",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "not-requested" => Some(Self::NotRequested),
            "preserved" => Some(Self::Preserved),
            "generated" => Some(Self::Generated),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// 不含宿主路径、摘要或配置正文的 `LocalResult` 公开投影。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalResultView {
    /// 结果稳定 ID。
    pub id: Uuid,
    /// 乐观并发结果版本。
    pub version: i64,
    /// 当前结果状态。
    pub status: LocalResultStatus,
    /// NFO 结论。
    pub nfo_status: LocalNfoStatus,
    /// Task 8 完成阶段写入的可选 Catalog ID。
    pub catalog_media_item_id: Option<Uuid>,
    /// 有界稳定剩余动作。
    pub remaining_actions: Vec<String>,
    /// UTC RFC3339 更新时间。
    pub updated_at: String,
}

#[derive(Clone)]
/// version-checked journal、LocalResult 与 rollback receipt 的事务边界。
pub struct JournalStore {
    pool: SqlitePool,
    notifier: OutboxNotifier,
}

impl JournalStore {
    #[must_use]
    /// 使用已有连接池创建 store。
    pub fn new(pool: SqlitePool) -> Self {
        Self::new_with_notifier(pool, OutboxNotifier::new())
    }

    /// 使用共享通知器创建 store，使结果触发器事件提交后立即唤醒 SSE。
    #[must_use]
    pub fn new_with_notifier(pool: SqlitePool, notifier: OutboxNotifier) -> Self {
        Self { pool, notifier }
    }

    pub(crate) const fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 为计划中的唯一文件 operation 幂等创建 prepared journal。
    ///
    /// # Errors
    ///
    /// 计划缺失文件步骤、来源/目标不匹配、数据库失败或已有 journal 输入不一致时返回错误。
    pub async fn prepare_file(
        &self,
        account_id: Uuid,
        plan: &OrganizationPlanRecord,
        expected_source: ObservedFile,
        now_us: i64,
    ) -> Result<JournalRecord, AppError> {
        if expected_source.locator().root_id() != &plan.draft.source.root_id
            || expected_source.locator().relative_path() != &plan.draft.source.relative_path
        {
            return Err(conflict("organization source no longer matches plan"));
        }
        let operation = plan
            .operations
            .iter()
            .find(|operation| operation.kind == PlannedOperationKind::File)
            .ok_or_else(|| invalid("organization plan file operation is missing"))?;
        let destination = FileLocator::new(
            operation.destination.root_id.clone(),
            operation.destination.relative_path.clone(),
        );
        let kind = JournalKind::from(plan.draft.operation);
        let modified_at_ns = i64::try_from(expected_source.modified_at_ns())
            .map_err(|_| invalid("source modified time is out of range"))?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database)?;
        if let Some(existing) =
            journal_by_operation_on(&mut tx, account_id, plan.id, operation.id).await?
        {
            if existing.kind != kind
                || existing.expected_source.as_ref() != Some(&expected_source)
                || existing.destination != destination
            {
                return Err(conflict("organization journal input changed"));
            }
            tx.commit().await.map_err(database)?;
            return Ok(existing);
        }
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO organization_file_operation_journals
             (id,account_id,task_id,plan_id,plan_version,operation_id,parent_operation_id,
              kind,status,source_root_id,source_relative_path,expected_source_identity,
              expected_source_size,expected_source_modified_at_ns,destination_root_id,
              destination_relative_path,applied_target_identity,applied_size,applied_sha256,
              reason,projection_version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,NULL,?,'prepared',?,?,?,?,?,?,?,NULL,NULL,NULL,NULL,1,?,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(plan.task_id.as_bytes().as_slice())
        .bind(plan.id.as_bytes().as_slice())
        .bind(plan.version)
        .bind(operation.id.as_bytes().as_slice())
        .bind(kind.as_str())
        .bind(expected_source.locator().root_id().as_str())
        .bind(expected_source.locator().relative_path().as_str())
        .bind(expected_source.identity().snapshot_bytes())
        .bind(i64::try_from(expected_source.size_bytes()).map_err(invalid)?)
        .bind(modified_at_ns)
        .bind(destination.root_id().as_str())
        .bind(destination.relative_path().as_str())
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        self.get(account_id, id)
            .await?
            .ok_or_else(|| invalid("prepared organization journal is missing"))
    }

    /// 为跨设备 move 幂等创建确定性来源删除子 journal。
    ///
    /// # Errors
    ///
    /// 父 journal 未 verified/缺少来源或 applied 摘要、数据库失败或既有输入不一致时返回错误。
    pub async fn prepare_source_removal(
        &self,
        account_id: Uuid,
        parent: &JournalRecord,
        now_us: i64,
    ) -> Result<JournalRecord, AppError> {
        if parent.kind != JournalKind::Move || parent.status != JournalStatus::Verified {
            return Err(conflict("move journal is not ready for source removal"));
        }
        let source = parent
            .expected_source
            .as_ref()
            .ok_or_else(|| invalid("move source facts are missing"))?;
        if parent.applied.is_none() {
            return Err(invalid("move applied facts are missing"));
        }
        let operation_id = source_removal_operation_id(parent.operation_id);
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database)?;
        if let Some(existing) =
            journal_by_operation_on(&mut tx, account_id, parent.plan_id, operation_id).await?
        {
            if existing.parent_operation_id != Some(parent.operation_id)
                || existing.kind != JournalKind::SourceRemoval
                || existing.expected_source.as_ref() != Some(source)
            {
                return Err(conflict("source-removal journal input changed"));
            }
            tx.commit().await.map_err(database)?;
            return Ok(existing);
        }
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO organization_file_operation_journals
             (id,account_id,task_id,plan_id,plan_version,operation_id,parent_operation_id,
              kind,status,source_root_id,source_relative_path,expected_source_identity,
              expected_source_size,expected_source_modified_at_ns,destination_root_id,
              destination_relative_path,applied_target_identity,applied_size,applied_sha256,
              reason,projection_version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,?,'source-removal','prepared',?,?,?,?,?,?,?,NULL,NULL,NULL,NULL,1,?,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(parent.task_id.as_bytes().as_slice())
        .bind(parent.plan_id.as_bytes().as_slice())
        .bind(parent.plan_version)
        .bind(operation_id.as_bytes().as_slice())
        .bind(parent.operation_id.as_bytes().as_slice())
        .bind(source.locator().root_id().as_str())
        .bind(source.locator().relative_path().as_str())
        .bind(source.identity().snapshot_bytes())
        .bind(i64::try_from(source.size_bytes()).map_err(invalid)?)
        .bind(i64::try_from(source.modified_at_ns()).map_err(invalid)?)
        .bind(parent.destination.root_id().as_str())
        .bind(parent.destination.relative_path().as_str())
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        self.get(account_id, id)
            .await?
            .ok_or_else(|| invalid("source-removal journal is missing"))
    }

    /// 在媒体文件 verified 后，为唯一 NFO operation 幂等创建 prepared journal。
    ///
    /// `existing` 是 journal 创建前对目标的只读观察；其完整身份和摘要会与
    /// `nfo_preexisting` 一起持久化，使 executing 恢复能够区分“原本存在”与
    /// “本次写入响应丢失”。
    ///
    /// # Errors
    ///
    /// 父文件未 verified、计划没有匹配 NFO 输入/operation、目标事实不一致或数据库失败时返回错误。
    pub async fn prepare_nfo(
        &self,
        account_id: Uuid,
        plan: &OrganizationPlanRecord,
        parent: &JournalRecord,
        existing: Option<&AppliedFile>,
        now_us: i64,
    ) -> Result<JournalRecord, AppError> {
        if parent.plan_id != plan.id
            || parent.status != JournalStatus::Verified
            || matches!(parent.kind, JournalKind::Nfo | JournalKind::SourceRemoval)
        {
            return Err(conflict("file journal is not ready for NFO"));
        }
        let input = plan
            .draft
            .nfo_input
            .as_ref()
            .ok_or_else(|| invalid("organization plan NFO input is missing"))?;
        let operation = plan
            .operations
            .iter()
            .find(|operation| operation.kind == PlannedOperationKind::EnsureMissingNfo)
            .ok_or_else(|| invalid("organization plan NFO operation is missing"))?;
        let destination = FileLocator::new(
            operation.destination.root_id.clone(),
            operation.destination.relative_path.clone(),
        );
        if destination.relative_path().as_str().rsplit('/').next() != Some(input.file_name.as_str())
            || existing.is_some_and(|file| file.locator() != &destination)
        {
            return Err(conflict("organization NFO target no longer matches plan"));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database)?;
        if let Some(stored) =
            journal_by_operation_on(&mut tx, account_id, plan.id, operation.id).await?
        {
            if stored.kind != JournalKind::Nfo
                || stored.parent_operation_id != Some(parent.operation_id)
                || stored.destination != destination
                || stored.nfo_preexisting != Some(existing.is_some())
                || (existing.is_some() && stored.applied.as_ref() != existing)
            {
                return Err(conflict("organization NFO journal input changed"));
            }
            tx.commit().await.map_err(database)?;
            return Ok(stored);
        }
        let id = Uuid::now_v7();
        let identity = existing.map(|file| file.identity().snapshot_bytes());
        let size = existing
            .map(|file| i64::try_from(file.size_bytes()).map_err(invalid))
            .transpose()?;
        let sha256 = existing.map(|file| file.sha256().to_vec());
        sqlx::query(
            "INSERT INTO organization_file_operation_journals
             (id,account_id,task_id,plan_id,plan_version,operation_id,parent_operation_id,
              kind,status,source_root_id,source_relative_path,expected_source_identity,
              expected_source_size,expected_source_modified_at_ns,destination_root_id,
              destination_relative_path,applied_target_identity,applied_size,applied_sha256,
              nfo_preexisting,nfo_outcome,reason,projection_version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,?,'nfo','prepared',NULL,NULL,NULL,NULL,NULL,?,?,?,?,?, ?,NULL,NULL,1,?,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(plan.task_id.as_bytes().as_slice())
        .bind(plan.id.as_bytes().as_slice())
        .bind(plan.version)
        .bind(operation.id.as_bytes().as_slice())
        .bind(parent.operation_id.as_bytes().as_slice())
        .bind(destination.root_id().as_str())
        .bind(destination.relative_path().as_str())
        .bind(identity)
        .bind(size)
        .bind(sha256)
        .bind(i64::from(existing.is_some()))
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        self.get(account_id, id)
            .await?
            .ok_or_else(|| invalid("prepared organization NFO journal is missing"))
    }

    /// 返回任务的全部 journal，按创建时间和 ID 稳定排序。
    ///
    /// # Errors
    ///
    /// 数据库或持久字段无效时返回错误。
    pub async fn for_task(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<Vec<JournalRecord>, AppError> {
        let rows = sqlx::query(
            "SELECT * FROM organization_file_operation_journals
             WHERE account_id=? AND task_id=? ORDER BY created_at_us,id",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(database)?;
        rows.iter().map(decode_journal).collect()
    }

    /// 按账户和 journal ID 读取记录。
    ///
    /// # Errors
    ///
    /// 数据库或持久字段无效时返回错误。
    pub async fn get(&self, account_id: Uuid, id: Uuid) -> Result<Option<JournalRecord>, AppError> {
        let row = sqlx::query(
            "SELECT * FROM organization_file_operation_journals WHERE account_id=? AND id=?",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database)?;
        row.as_ref().map(decode_journal).transpose()
    }

    /// 读取一个计划内的稳定 operation journal。
    ///
    /// # Errors
    ///
    /// 数据库或持久字段无效时返回错误。
    pub async fn by_operation(
        &self,
        account_id: Uuid,
        plan_id: Uuid,
        operation_id: Uuid,
    ) -> Result<Option<JournalRecord>, AppError> {
        let mut connection = self.pool.acquire().await.map_err(database)?;
        journal_by_operation_on(&mut connection, account_id, plan_id, operation_id).await
    }

    /// 将 prepared journal 版本检查地推进到 executing。
    ///
    /// # Errors
    ///
    /// 跳步、陈旧版本或数据库失败时返回资源冲突/内部错误。
    pub async fn mark_executing(
        &self,
        id: Uuid,
        version: i64,
        now_us: i64,
    ) -> Result<JournalRecord, AppError> {
        self.transition(
            id,
            version,
            JournalStatus::Prepared,
            JournalStatus::Executing,
            None,
            now_us,
        )
        .await
    }

    /// 保存实际 applied 文件事实并推进到 applied。
    ///
    /// # Errors
    ///
    /// 跳步、陈旧版本或数据库失败时返回错误。
    pub async fn mark_applied(
        &self,
        id: Uuid,
        version: i64,
        applied: &AppliedFile,
        now_us: i64,
    ) -> Result<JournalRecord, AppError> {
        let result = sqlx::query(
            "UPDATE organization_file_operation_journals
             SET status='applied',applied_target_identity=?,applied_size=?,applied_sha256=?,
                 reason=NULL,projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND status='executing' AND projection_version=?",
        )
        .bind(applied.identity().snapshot_bytes())
        .bind(i64::try_from(applied.size_bytes()).map_err(invalid)?)
        .bind(applied.sha256().as_slice())
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(version)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        self.updated(result.rows_affected(), id).await
    }

    /// 保存 NFO 的实际文件事实与 preserved/generated 结论并推进到 applied。
    ///
    /// # Errors
    ///
    /// journal 不是 executing NFO、结论无效、版本陈旧或数据库失败时返回错误。
    pub async fn mark_nfo_applied(
        &self,
        id: Uuid,
        version: i64,
        applied: &AppliedFile,
        outcome: LocalNfoStatus,
        now_us: i64,
    ) -> Result<JournalRecord, AppError> {
        if !matches!(
            outcome,
            LocalNfoStatus::Preserved | LocalNfoStatus::Generated
        ) {
            return Err(invalid("organization NFO outcome is invalid"));
        }
        let result = sqlx::query(
            "UPDATE organization_file_operation_journals
             SET status='applied',applied_target_identity=?,applied_size=?,applied_sha256=?,
                 nfo_outcome=?,reason=NULL,projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND kind='nfo' AND status='executing' AND projection_version=?",
        )
        .bind(applied.identity().snapshot_bytes())
        .bind(i64::try_from(applied.size_bytes()).map_err(invalid)?)
        .bind(applied.sha256().as_slice())
        .bind(outcome.as_str())
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(version)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        self.updated(result.rows_affected(), id).await
    }

    /// 保存来源删除已应用但尚待重新观察的边界。
    ///
    /// # Errors
    ///
    /// journal 不是 executing source-removal、版本陈旧或数据库失败时返回错误。
    pub async fn mark_source_removed(
        &self,
        id: Uuid,
        version: i64,
        now_us: i64,
    ) -> Result<JournalRecord, AppError> {
        let result = sqlx::query(
            "UPDATE organization_file_operation_journals
             SET status='applied',reason=NULL,projection_version=projection_version+1,
                 updated_at_us=?
             WHERE id=? AND kind='source-removal' AND status='executing'
               AND projection_version=?",
        )
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(version)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        self.updated(result.rows_affected(), id).await
    }

    /// 将具有 applied 事实的 journal 推进到 verified。
    ///
    /// # Errors
    ///
    /// 跳步、陈旧版本或数据库失败时返回错误。
    pub async fn verify(
        &self,
        id: Uuid,
        version: i64,
        now_us: i64,
    ) -> Result<JournalRecord, AppError> {
        self.transition(
            id,
            version,
            JournalStatus::Applied,
            JournalStatus::Verified,
            None,
            now_us,
        )
        .await
    }

    /// 将无法安全恢复的非终态 journal 保存为 manual-review。
    ///
    /// # Errors
    ///
    /// journal 已终态、版本陈旧、reason 无效或数据库失败时返回错误。
    pub async fn manual_review(
        &self,
        id: Uuid,
        version: i64,
        reason: &str,
        now_us: i64,
    ) -> Result<JournalRecord, AppError> {
        if reason.is_empty() || reason.len() > 100 {
            return Err(invalid("organization journal reason is invalid"));
        }
        let result = sqlx::query(
            "UPDATE organization_file_operation_journals
             SET status='manual-review',reason=?,projection_version=projection_version+1,
                 updated_at_us=?
             WHERE id=? AND status IN ('prepared','executing','applied','verified')
               AND projection_version=?",
        )
        .bind(reason)
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(version)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        self.updated(result.rows_affected(), id).await
    }

    /// 为 verified 文件 journal 幂等创建 `LocalResult`。
    ///
    /// # Errors
    ///
    /// journal 未核对、数据库失败或持久结果无效时返回错误。
    pub async fn ensure_completed_result(
        &self,
        account_id: Uuid,
        journal: &JournalRecord,
        now_us: i64,
    ) -> Result<LocalResultView, AppError> {
        if journal.status != JournalStatus::Verified {
            return Err(conflict("organization journal is not verified"));
        }
        sqlx::query(
            "INSERT INTO organization_local_results
             (id,account_id,task_id,plan_id,plan_version,file_journal_id,status,nfo_status,
              catalog_media_item_id,remaining_actions_json,version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,'completed','not-requested',NULL,'[]',1,?,?)
             ON CONFLICT(plan_id) DO UPDATE SET
               status='completed',remaining_actions_json='[]',version=version+1,
               updated_at_us=excluded.updated_at_us
             WHERE organization_local_results.status!='completed'",
        )
        .bind(Uuid::now_v7().as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(journal.task_id.as_bytes().as_slice())
        .bind(journal.plan_id.as_bytes().as_slice())
        .bind(journal.plan_version)
        .bind(journal.id.as_bytes().as_slice())
        .bind(now_us)
        .bind(now_us)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        self.notifier.notify_after_commit();
        self.result_for_plan(account_id, journal.plan_id)
            .await?
            .ok_or_else(|| invalid("organization local result is missing"))
    }

    /// 为目标已 verified、来源删除未完成的跨设备 move 保存 partial-success。
    ///
    /// # Errors
    ///
    /// 父 move journal 未 verified、数据库失败或持久结果无效时返回错误。
    pub async fn ensure_partial_result(
        &self,
        account_id: Uuid,
        parent: &JournalRecord,
        now_us: i64,
    ) -> Result<LocalResultView, AppError> {
        if parent.kind != JournalKind::Move || parent.status != JournalStatus::Verified {
            return Err(conflict("move journal is not verified"));
        }
        sqlx::query(
            "INSERT INTO organization_local_results
             (id,account_id,task_id,plan_id,plan_version,file_journal_id,status,nfo_status,
              catalog_media_item_id,remaining_actions_json,version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,'partial-success','not-requested',NULL,
                     '[\"source-removal\"]',1,?,?)
             ON CONFLICT(plan_id) DO NOTHING",
        )
        .bind(Uuid::now_v7().as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(parent.task_id.as_bytes().as_slice())
        .bind(parent.plan_id.as_bytes().as_slice())
        .bind(parent.plan_version)
        .bind(parent.id.as_bytes().as_slice())
        .bind(now_us)
        .bind(now_us)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        self.notifier.notify_after_commit();
        self.result_for_plan(account_id, parent.plan_id)
            .await?
            .ok_or_else(|| invalid("organization partial result is missing"))
    }

    /// 在 verified 媒体文件之后保存只剩 NFO 的 partial-success。
    ///
    /// 已有 source-removal partial result 会单调切换到 NFO；重复调用不会增加版本。
    ///
    /// # Errors
    ///
    /// 文件 journal 未 verified、数据库失败或持久结果无效时返回错误。
    pub async fn ensure_nfo_pending_result(
        &self,
        account_id: Uuid,
        file_journal: &JournalRecord,
        now_us: i64,
    ) -> Result<LocalResultView, AppError> {
        if file_journal.status != JournalStatus::Verified
            || matches!(
                file_journal.kind,
                JournalKind::Nfo | JournalKind::SourceRemoval
            )
        {
            return Err(conflict("file journal is not verified for NFO"));
        }
        sqlx::query(
            "INSERT INTO organization_local_results
             (id,account_id,task_id,plan_id,plan_version,file_journal_id,status,nfo_status,
              catalog_media_item_id,remaining_actions_json,version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,'partial-success','not-requested',NULL,'[\"nfo\"]',1,?,?)
             ON CONFLICT(plan_id) DO UPDATE SET
               status='partial-success',
               nfo_status=CASE WHEN organization_local_results.nfo_status='failed'
                               THEN 'failed' ELSE 'not-requested' END,
               remaining_actions_json='[\"nfo\"]',version=version+1,
               updated_at_us=excluded.updated_at_us
             WHERE organization_local_results.status='partial-success'
               AND organization_local_results.remaining_actions_json!='[\"nfo\"]'",
        )
        .bind(Uuid::now_v7().as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(file_journal.task_id.as_bytes().as_slice())
        .bind(file_journal.plan_id.as_bytes().as_slice())
        .bind(file_journal.plan_version)
        .bind(file_journal.id.as_bytes().as_slice())
        .bind(now_us)
        .bind(now_us)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        self.notifier.notify_after_commit();
        self.result_for_plan(account_id, file_journal.plan_id)
            .await?
            .ok_or_else(|| invalid("organization NFO pending result is missing"))
    }

    /// 记录 NFO 暂时失败；文件结果保持 partial-success，重试只剩 `nfo`。
    ///
    /// # Errors
    ///
    /// 结果缺失、数据库失败或持久结果无效时返回错误。
    pub async fn mark_nfo_failed_result(
        &self,
        account_id: Uuid,
        plan_id: Uuid,
        now_us: i64,
    ) -> Result<LocalResultView, AppError> {
        sqlx::query(
            "UPDATE organization_local_results
             SET status='partial-success',nfo_status='failed',
                 remaining_actions_json='[\"nfo\"]',version=version+1,updated_at_us=?
             WHERE account_id=? AND plan_id=?
               AND status='partial-success'
               AND (nfo_status!='failed' OR remaining_actions_json!='[\"nfo\"]')",
        )
        .bind(now_us)
        .bind(account_id.as_bytes().as_slice())
        .bind(plan_id.as_bytes().as_slice())
        .execute(&self.pool)
        .await
        .map_err(database)?;
        self.notifier.notify_after_commit();
        self.result_for_plan(account_id, plan_id)
            .await?
            .ok_or_else(|| invalid("organization NFO failure result is missing"))
    }

    /// 以 verified NFO journal 的 preserved/generated 结论完成本地结果。
    ///
    /// # Errors
    ///
    /// journal 未 verified、缺少 NFO 结论、数据库失败或结果缺失时返回错误。
    pub async fn finish_nfo_result(
        &self,
        account_id: Uuid,
        journal: &JournalRecord,
        now_us: i64,
    ) -> Result<LocalResultView, AppError> {
        if journal.kind != JournalKind::Nfo || journal.status != JournalStatus::Verified {
            return Err(conflict("organization NFO journal is not verified"));
        }
        let outcome = journal
            .nfo_outcome
            .filter(|outcome| {
                matches!(
                    outcome,
                    LocalNfoStatus::Preserved | LocalNfoStatus::Generated
                )
            })
            .ok_or_else(|| invalid("organization NFO outcome is missing"))?;
        sqlx::query(
            "UPDATE organization_local_results
             SET status='completed',nfo_status=?,remaining_actions_json='[]',
                 version=version+1,updated_at_us=?
             WHERE account_id=? AND plan_id=?
               AND (status!='completed' OR nfo_status!=? OR remaining_actions_json!='[]')",
        )
        .bind(outcome.as_str())
        .bind(now_us)
        .bind(account_id.as_bytes().as_slice())
        .bind(journal.plan_id.as_bytes().as_slice())
        .bind(outcome.as_str())
        .execute(&self.pool)
        .await
        .map_err(database)?;
        self.notifier.notify_after_commit();
        self.result_for_plan(account_id, journal.plan_id)
            .await?
            .ok_or_else(|| invalid("organization completed NFO result is missing"))
    }

    /// 将已有 `LocalResult` 收敛为 manual-review。
    ///
    /// # Errors
    ///
    /// 结果缺失、数据库失败或持久结果无效时返回错误。
    pub async fn mark_result_manual(
        &self,
        account_id: Uuid,
        plan_id: Uuid,
        now_us: i64,
    ) -> Result<LocalResultView, AppError> {
        let result = sqlx::query(
            "UPDATE organization_local_results
             SET status='manual-review',version=version+1,updated_at_us=?
             WHERE account_id=? AND plan_id=? AND status!='manual-review'",
        )
        .bind(now_us)
        .bind(account_id.as_bytes().as_slice())
        .bind(plan_id.as_bytes().as_slice())
        .execute(&self.pool)
        .await
        .map_err(database)?;
        self.notifier.notify_after_commit();
        if result.rows_affected() > 1 {
            return Err(invalid("multiple organization results were updated"));
        }
        self.result_for_plan(account_id, plan_id)
            .await?
            .ok_or_else(|| invalid("organization local result is missing"))
    }

    /// 读取任务当前 `LocalResult`。
    ///
    /// # Errors
    ///
    /// 数据库或持久字段无效时返回错误。
    pub async fn result_for_task(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<Option<LocalResultView>, AppError> {
        let row = sqlx::query(
            "SELECT * FROM organization_local_results
             WHERE account_id=? AND task_id=? ORDER BY plan_version DESC LIMIT 1",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database)?;
        row.as_ref().map(decode_result).transpose()
    }

    /// 幂等记录 Catalog 已接收当前本地结果；相同 ID 重放不增加版本。
    ///
    /// # Errors
    ///
    /// 结果不存在、版本变化、Catalog ID 冲突或数据库失败时返回稳定应用错误。
    pub async fn mark_catalog_committed(
        &self,
        account_id: Uuid,
        result_id: Uuid,
        expected_version: i64,
        media_item_id: Uuid,
        now_us: i64,
    ) -> Result<LocalResultView, AppError> {
        let current =
            sqlx::query("SELECT * FROM organization_local_results WHERE account_id=? AND id=?")
                .bind(account_id.as_bytes().as_slice())
                .bind(result_id.as_bytes().as_slice())
                .fetch_optional(&self.pool)
                .await
                .map_err(database)?
                .as_ref()
                .map(decode_result)
                .transpose()?
                .ok_or_else(|| {
                    AppError::new(ErrorCode::NotFound, "organization result not found")
                })?;
        if current.catalog_media_item_id == Some(media_item_id) {
            return Ok(current);
        }
        if current.catalog_media_item_id.is_some() {
            return Err(conflict("organization result catalog binding changed"));
        }
        if current.version != expected_version {
            return Err(AppError::new(
                ErrorCode::ConfigVersionConflict,
                "organization result version changed",
            ));
        }
        let updated = sqlx::query(
            "UPDATE organization_local_results
             SET catalog_media_item_id=?,version=version+1,updated_at_us=?
             WHERE id=? AND account_id=? AND version=? AND catalog_media_item_id IS NULL",
        )
        .bind(media_item_id.as_bytes().as_slice())
        .bind(now_us)
        .bind(result_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(expected_version)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        if updated.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::ConfigVersionConflict,
                "organization result version changed",
            ));
        }
        self.notifier.notify_after_commit();
        let row =
            sqlx::query("SELECT * FROM organization_local_results WHERE account_id=? AND id=?")
                .bind(account_id.as_bytes().as_slice())
                .bind(result_id.as_bytes().as_slice())
                .fetch_optional(&self.pool)
                .await
                .map_err(database)?;
        row.as_ref()
            .map(decode_result)
            .transpose()?
            .ok_or_else(|| invalid("updated organization result is missing"))
    }

    /// 在任何补偿 I/O 前查询并验证 rollback 幂等回执。
    ///
    /// # Errors
    ///
    /// key 已绑定不同请求、输入无效、数据库失败或持久结果损坏时返回错误。
    pub async fn rollback_replay(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        result_version: i64,
        idempotency_key: &str,
    ) -> Result<Option<LocalResultView>, AppError> {
        if idempotency_key.is_empty() || idempotency_key.len() > 255 {
            return Err(invalid("rollback idempotency key is invalid"));
        }
        let key_hash = Sha256::digest(
            [
                b"mediaflow.organization.rollback.v1\0".as_slice(),
                idempotency_key.as_bytes(),
            ]
            .concat(),
        );
        let request_digest = rollback_digest(task_id, result_version);
        let row = sqlx::query(
            "SELECT r.request_digest,l.* FROM organization_rollback_receipts r
             JOIN organization_local_results l ON l.id=r.result_id
             WHERE r.account_id=? AND r.idempotency_key_sha256=?",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(key_hash.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database)?;
        let Some(row) = row else {
            return Ok(None);
        };
        if row.get::<Vec<u8>, _>("request_digest") != request_digest {
            return Err(AppError::new(
                ErrorCode::RequestConflict,
                "rollback key conflict",
            ));
        }
        Ok(Some(decode_result(&row)?))
    }

    /// 原子保存 rollback 的 journal/result 状态与幂等回执。
    ///
    /// 相同 key 与请求精确重放返回同一结果；不同请求返回冲突。
    ///
    /// # Errors
    ///
    /// 结果版本陈旧、key 冲突、journal 不匹配或数据库失败时返回错误。
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn finish_rollback(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        expected_result_version: i64,
        idempotency_key: &str,
        journal: &JournalRecord,
        compensated_additional: &[JournalRecord],
        status: LocalResultStatus,
        now_us: i64,
    ) -> Result<LocalResultView, AppError> {
        if idempotency_key.is_empty() || idempotency_key.len() > 255 {
            return Err(invalid("rollback idempotency key is invalid"));
        }
        if !matches!(
            status,
            LocalResultStatus::Compensated | LocalResultStatus::ManualReview
        ) {
            return Err(invalid("rollback result status is invalid"));
        }
        if compensated_additional.iter().any(|additional| {
            additional.plan_id != journal.plan_id
                || additional.kind != JournalKind::Nfo
                || additional.nfo_outcome != Some(LocalNfoStatus::Generated)
                || additional.status != JournalStatus::Verified
        }) {
            return Err(invalid("rollback additional journal is invalid"));
        }
        let key_hash = Sha256::digest(
            [
                b"mediaflow.organization.rollback.v1\0".as_slice(),
                idempotency_key.as_bytes(),
            ]
            .concat(),
        );
        let request_digest = rollback_digest(task_id, expected_result_version);
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database)?;
        if let Some((stored_digest, result_id)) = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
            "SELECT request_digest,result_id FROM organization_rollback_receipts
             WHERE account_id=? AND idempotency_key_sha256=?",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(key_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database)?
        {
            if stored_digest != request_digest {
                return Err(AppError::new(
                    ErrorCode::RequestConflict,
                    "rollback key conflict",
                ));
            }
            let result_id = Uuid::from_slice(&result_id).map_err(invalid)?;
            let row = sqlx::query("SELECT * FROM organization_local_results WHERE id=?")
                .bind(result_id.as_bytes().as_slice())
                .fetch_one(&mut *tx)
                .await
                .map_err(database)?;
            let result = decode_result(&row)?;
            tx.commit().await.map_err(database)?;
            return Ok(result);
        }
        let result = sqlx::query(
            "UPDATE organization_local_results
             SET status=?,version=version+1,updated_at_us=?
             WHERE account_id=? AND task_id=? AND version=? AND file_journal_id=?",
        )
        .bind(status.as_str())
        .bind(now_us)
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .bind(expected_result_version)
        .bind(journal.id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if result.rows_affected() != 1 {
            return Err(conflict("organization result version changed"));
        }
        let journal_status = if status == LocalResultStatus::Compensated {
            "compensated"
        } else {
            "manual-review"
        };
        let journal_result = sqlx::query(
            "UPDATE organization_file_operation_journals
             SET status=?,reason=?,projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND projection_version=? AND status='verified'",
        )
        .bind(journal_status)
        .bind((status == LocalResultStatus::ManualReview).then_some("rollback.manual-review"))
        .bind(now_us)
        .bind(journal.id.as_bytes().as_slice())
        .bind(journal.projection_version)
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if journal_result.rows_affected() != 1 {
            return Err(conflict("organization journal version changed"));
        }
        if status == LocalResultStatus::Compensated {
            sqlx::query(
                "UPDATE organization_file_operation_journals
                 SET status='compensated',reason=NULL,
                     projection_version=projection_version+1,updated_at_us=?
                 WHERE plan_id=? AND parent_operation_id=? AND kind='source-removal'
                   AND status='verified'",
            )
            .bind(now_us)
            .bind(journal.plan_id.as_bytes().as_slice())
            .bind(journal.operation_id.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(database)?;
        }
        for additional in compensated_additional {
            let updated = sqlx::query(
                "UPDATE organization_file_operation_journals
                 SET status='compensated',reason=NULL,
                     projection_version=projection_version+1,updated_at_us=?
                 WHERE id=? AND projection_version=? AND status='verified'",
            )
            .bind(now_us)
            .bind(additional.id.as_bytes().as_slice())
            .bind(additional.projection_version)
            .execute(&mut *tx)
            .await
            .map_err(database)?;
            if updated.rows_affected() != 1 {
                return Err(conflict("additional rollback journal version changed"));
            }
        }
        let result_id = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT id FROM organization_local_results WHERE account_id=? AND task_id=?",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(database)?;
        sqlx::query(
            "INSERT INTO organization_rollback_receipts
             (account_id,task_id,idempotency_key_sha256,request_digest,result_id,created_at_us)
             VALUES (?,?,?,?,?,?)",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .bind(key_hash.as_slice())
        .bind(request_digest.as_slice())
        .bind(&result_id)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        let row = sqlx::query("SELECT * FROM organization_local_results WHERE id=?")
            .bind(&result_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(database)?;
        let result = decode_result(&row)?;
        tx.commit().await.map_err(database)?;
        self.notifier.notify_after_commit();
        Ok(result)
    }

    async fn result_for_plan(
        &self,
        account_id: Uuid,
        plan_id: Uuid,
    ) -> Result<Option<LocalResultView>, AppError> {
        let row = sqlx::query(
            "SELECT * FROM organization_local_results WHERE account_id=? AND plan_id=?",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(plan_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database)?;
        row.as_ref().map(decode_result).transpose()
    }

    async fn transition(
        &self,
        id: Uuid,
        version: i64,
        from: JournalStatus,
        to: JournalStatus,
        reason: Option<&str>,
        now_us: i64,
    ) -> Result<JournalRecord, AppError> {
        let result = sqlx::query(
            "UPDATE organization_file_operation_journals
             SET status=?,reason=?,projection_version=projection_version+1,updated_at_us=?
             WHERE id=? AND status=? AND projection_version=?",
        )
        .bind(to.as_str())
        .bind(reason)
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(from.as_str())
        .bind(version)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        self.updated(result.rows_affected(), id).await
    }

    async fn updated(&self, rows: u64, id: Uuid) -> Result<JournalRecord, AppError> {
        if rows != 1 {
            return Err(conflict("organization journal transition conflict"));
        }
        let row = sqlx::query("SELECT * FROM organization_file_operation_journals WHERE id=?")
            .bind(id.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .map_err(database)?;
        decode_journal(&row)
    }
}

async fn journal_by_operation_on(
    connection: &mut sqlx::SqliteConnection,
    account_id: Uuid,
    plan_id: Uuid,
    operation_id: Uuid,
) -> Result<Option<JournalRecord>, AppError> {
    let row = sqlx::query(
        "SELECT * FROM organization_file_operation_journals
         WHERE account_id=? AND plan_id=? AND operation_id=?",
    )
    .bind(account_id.as_bytes().as_slice())
    .bind(plan_id.as_bytes().as_slice())
    .bind(operation_id.as_bytes().as_slice())
    .fetch_optional(connection)
    .await
    .map_err(database)?;
    row.as_ref().map(decode_journal).transpose()
}

fn decode_journal(row: &sqlx::sqlite::SqliteRow) -> Result<JournalRecord, AppError> {
    let kind = JournalKind::parse(&row.get::<String, _>("kind"))
        .ok_or_else(|| invalid("organization journal kind is invalid"))?;
    let source_root = row.get::<Option<String>, _>("source_root_id");
    let source_path = row.get::<Option<String>, _>("source_relative_path");
    let expected_source = match (source_root, source_path) {
        (Some(root), Some(path)) => Some(ObservedFile::from_parts(
            FileLocator::new(
                RootId::parse(&root).map_err(invalid)?,
                RelativePath::parse(&path).map_err(invalid)?,
            ),
            decode_identity(&row.get::<Vec<u8>, _>("expected_source_identity"))?,
            u64::try_from(row.get::<i64, _>("expected_source_size")).map_err(invalid)?,
            i128::from(row.get::<i64, _>("expected_source_modified_at_ns")),
        )),
        (None, None) => None,
        _ => {
            return Err(invalid(
                "organization journal source columns are inconsistent",
            ));
        }
    };
    let destination = FileLocator::new(
        RootId::parse(&row.get::<String, _>("destination_root_id")).map_err(invalid)?,
        RelativePath::parse(&row.get::<String, _>("destination_relative_path")).map_err(invalid)?,
    );
    let applied = match row.get::<Option<Vec<u8>>, _>("applied_target_identity") {
        Some(identity) => {
            let sha = row.get::<Vec<u8>, _>("applied_sha256");
            let sha256: [u8; 32] = sha
                .try_into()
                .map_err(|_| invalid("organization journal SHA-256 is invalid"))?;
            Some(AppliedFile::from_parts(
                destination.clone(),
                decode_identity(&identity)?,
                u64::try_from(row.get::<i64, _>("applied_size")).map_err(invalid)?,
                sha256,
            ))
        }
        None => None,
    };
    let nfo_preexisting = row
        .get::<Option<i64>, _>("nfo_preexisting")
        .map(|value| match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid("organization NFO preexisting flag is invalid")),
        })
        .transpose()?;
    let nfo_outcome = row
        .get::<Option<String>, _>("nfo_outcome")
        .map(|value| {
            LocalNfoStatus::parse(&value)
                .filter(|status| {
                    matches!(
                        status,
                        LocalNfoStatus::Preserved | LocalNfoStatus::Generated
                    )
                })
                .ok_or_else(|| invalid("organization journal NFO outcome is invalid"))
        })
        .transpose()?;
    if (kind == JournalKind::Nfo) != nfo_preexisting.is_some()
        || (kind != JournalKind::Nfo && nfo_outcome.is_some())
    {
        return Err(invalid("organization journal NFO fields are inconsistent"));
    }
    Ok(JournalRecord {
        id: row_uuid(row, "id")?,
        operation_id: row_uuid(row, "operation_id")?,
        parent_operation_id: optional_row_uuid(row, "parent_operation_id")?,
        kind,
        status: JournalStatus::parse(&row.get::<String, _>("status"))
            .ok_or_else(|| invalid("organization journal status is invalid"))?,
        task_id: row_uuid(row, "task_id")?,
        plan_id: row_uuid(row, "plan_id")?,
        plan_version: row.get("plan_version"),
        expected_source,
        destination,
        applied,
        nfo_preexisting,
        nfo_outcome,
        reason: row.get("reason"),
        projection_version: row.get("projection_version"),
        updated_at_us: row.get("updated_at_us"),
    })
}

fn decode_result(row: &sqlx::sqlite::SqliteRow) -> Result<LocalResultView, AppError> {
    let updated_at_us: i64 = row.get("updated_at_us");
    let updated_at = Utc
        .timestamp_micros(updated_at_us)
        .single()
        .ok_or_else(|| invalid("organization result timestamp is invalid"))?
        .to_rfc3339_opts(SecondsFormat::Micros, true);
    Ok(LocalResultView {
        id: row_uuid(row, "id")?,
        version: row.get("version"),
        status: LocalResultStatus::parse(&row.get::<String, _>("status"))
            .ok_or_else(|| invalid("organization result status is invalid"))?,
        nfo_status: LocalNfoStatus::parse(&row.get::<String, _>("nfo_status"))
            .ok_or_else(|| invalid("organization NFO status is invalid"))?,
        catalog_media_item_id: optional_row_uuid(row, "catalog_media_item_id")?,
        remaining_actions: serde_json::from_str(&row.get::<String, _>("remaining_actions_json"))
            .map_err(invalid)?,
        updated_at,
    })
}

fn decode_identity(bytes: &[u8]) -> Result<FileIdentity, AppError> {
    if !matches!(bytes.len(), 16 | 25) {
        return Err(invalid("organization file identity is invalid"));
    }
    let device = u64::from_be_bytes(bytes[..8].try_into().map_err(invalid)?);
    let inode = u64::from_be_bytes(bytes[8..16].try_into().map_err(invalid)?);
    let mount_id = if bytes.len() == 25 && bytes[16] == 1 {
        Some(u64::from_be_bytes(
            bytes[17..25].try_into().map_err(invalid)?,
        ))
    } else {
        None
    };
    Ok(FileIdentity {
        device,
        inode,
        mount_id,
    })
}

fn row_uuid(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Uuid, AppError> {
    Uuid::from_slice(&row.get::<Vec<u8>, _>(column)).map_err(invalid)
}

fn optional_row_uuid(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<Option<Uuid>, AppError> {
    row.get::<Option<Vec<u8>>, _>(column)
        .map(|bytes| Uuid::from_slice(&bytes).map_err(invalid))
        .transpose()
}

fn rollback_digest(task_id: Uuid, version: i64) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"mediaflow.organization.rollback.request.v1\0");
    digest.update(task_id.as_bytes());
    digest.update(version.to_be_bytes());
    digest.finalize().into()
}

fn source_removal_operation_id(parent_operation_id: Uuid) -> Uuid {
    let digest = Sha256::digest(
        [
            b"mediaflow.organization.source-removal.v1\0".as_slice(),
            parent_operation_id.as_bytes(),
        ]
        .concat(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn database(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn invalid(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}

fn conflict(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ResourceConflict, message)
}
