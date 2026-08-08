use std::sync::Arc;

use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::discovery::organization_projection::{
    OrganizationInboxProjection, OrganizationSourceProjection,
};
use crate::organization::fs::{
    CompensationOutcome, FileLocator, FileOperationError, FileOperationSpec, NfoOperationSpec,
    OrganizationFs, ProcessingStopToken, VerifiedOperation, VerifiedOperationKind,
};
use crate::platform::task_runtime::TaskClock;
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::processing::model::ProcessingLease;

use super::journal_store::{
    JournalKind, JournalRecord, JournalStatus, JournalStore, LocalNfoStatus, LocalResultStatus,
    LocalResultView,
};
use super::nfo::{NfoDecision, NfoGenerator};
use super::plan_store::{OrganizationPlanRecord, OrganizationPlanStore};
use super::planner::PlanAuthorization;

/// 一次 executor 推进后的稳定应用结论。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionOutcome {
    /// 当前文件 operation 已 verified，并已形成 `LocalResult`。
    Completed(LocalResultView),
    /// 暂时性 I/O 后保持 journal 可恢复。
    RecoveryPending { reason: String },
    /// 当前状态无法证明重放安全，需要人工处理。
    ManualReview(Option<LocalResultView>),
    /// 计划授权或来源事实阻止执行。
    Paused { reason: String },
    /// 协作停止已在安全边界生效。
    Stopped,
    /// 已核对结果已补偿。
    Compensated(LocalResultView),
}

/// 只绑定一个 task 当前 `LocalResult` 版本的 rollback 命令。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RollbackCommand {
    /// `ProcessingTask` ID。
    pub task_id: Uuid,
    /// 调用方确认的当前结果版本。
    pub result_version: i64,
    /// 一次用户意图内复用的幂等键。
    pub idempotency_key: String,
}

#[derive(Clone)]
/// journal-first 单 operation 执行、恢复和受控补偿编排器。
pub struct OrganizationExecutor {
    account_id: Uuid,
    plans: OrganizationPlanStore,
    journals: JournalStore,
    sources: OrganizationInboxProjection,
    fs: Arc<dyn OrganizationFs>,
    clock: Arc<dyn TaskClock>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExecutionScope {
    ThroughNfo,
    FileOnly,
}

impl OrganizationExecutor {
    #[must_use]
    /// 使用同一数据库与 deployment-root FS 依赖创建 executor。
    pub fn new(
        account_id: Uuid,
        plans: OrganizationPlanStore,
        journals: JournalStore,
        fs: Arc<dyn OrganizationFs>,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        let sources = OrganizationInboxProjection::new(journals.pool().clone());
        Self {
            account_id,
            plans,
            journals,
            sources,
            fs,
            clock,
        }
    }

    /// 创建尚未绑定账号的生产模板；执行前必须通过 [`Self::for_account`] 作用域化。
    #[must_use]
    pub fn unscoped(
        plans: OrganizationPlanStore,
        journals: JournalStore,
        fs: Arc<dyn OrganizationFs>,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        Self::new(Uuid::nil(), plans, journals, fs, clock)
    }

    /// 返回共享相同能力与 store、但绑定到指定任务账号的执行器副本。
    #[must_use]
    pub fn for_account(&self, account_id: Uuid) -> Self {
        Self {
            account_id,
            plans: self.plans.clone(),
            journals: self.journals.clone(),
            sources: self.sources.clone(),
            fs: self.fs.clone(),
            clock: self.clock.clone(),
        }
    }

    /// 为租约任务准备并推进下一个文件 operation。
    ///
    /// # Errors
    ///
    /// 当前计划/来源投影缺失、数据库失败或持久状态损坏时返回稳定应用错误。
    pub async fn run_next(
        &self,
        lease: &ProcessingLease,
        stop: ProcessingStopToken,
    ) -> Result<ExecutionOutcome, AppError> {
        let plan = self.current_plan(lease.task.id).await?;
        if plan.authorization == PlanAuthorization::Paused {
            return Ok(ExecutionOutcome::Paused {
                reason: "organization.plan-paused".to_owned(),
            });
        }
        let source_projection = self
            .sources
            .source_for_task(self.account_id, lease.task.id)
            .await?;
        let source = FileLocator::new(
            source_projection.root_id.clone(),
            source_projection.relative_path.clone(),
        );
        let Some(observed) = self.fs.observe(&source).map_err(filesystem)? else {
            return Ok(ExecutionOutcome::Paused {
                reason: "organization.source-changed".to_owned(),
            });
        };
        if !source_matches_revision(&observed, &source_projection, &plan) {
            return Ok(ExecutionOutcome::Paused {
                reason: "organization.source-changed".to_owned(),
            });
        }
        let journal = self
            .journals
            .prepare_file(self.account_id, &plan, observed, self.clock.now_us())
            .await?;
        Box::pin(self.advance(&plan, journal, &stop, ExecutionScope::ThroughNfo)).await
    }

    /// 只推进媒体文件 operation；若计划包含 NFO，则在持久化待办结果后停止。
    ///
    /// # Errors
    ///
    /// 当前计划/来源投影缺失、数据库失败或持久状态损坏时返回稳定应用错误。
    pub async fn run_file_stage(
        &self,
        lease: &ProcessingLease,
        stop: ProcessingStopToken,
    ) -> Result<ExecutionOutcome, AppError> {
        let plan = self.current_plan(lease.task.id).await?;
        if plan.authorization == PlanAuthorization::Paused {
            return Ok(ExecutionOutcome::Paused {
                reason: "organization.plan-paused".to_owned(),
            });
        }
        let source_projection = self
            .sources
            .source_for_task(self.account_id, lease.task.id)
            .await?;
        let source = FileLocator::new(
            source_projection.root_id.clone(),
            source_projection.relative_path.clone(),
        );
        let Some(observed) = self.fs.observe(&source).map_err(filesystem)? else {
            return Ok(ExecutionOutcome::Paused {
                reason: "organization.source-changed".to_owned(),
            });
        };
        if !source_matches_revision(&observed, &source_projection, &plan) {
            return Ok(ExecutionOutcome::Paused {
                reason: "organization.source-changed".to_owned(),
            });
        }
        let journal = self
            .journals
            .prepare_file(self.account_id, &plan, observed, self.clock.now_us())
            .await?;
        Box::pin(self.advance(&plan, journal, &stop, ExecutionScope::FileOnly)).await
    }

    /// 只推进当前计划的 NFO operation；不会重新执行已经核验的媒体文件 operation。
    ///
    /// # Errors
    ///
    /// 当前计划、已核验文件 journal 或本地结果缺失，数据库失败或状态损坏时返回稳定错误。
    pub async fn run_nfo_stage(
        &self,
        lease: &ProcessingLease,
    ) -> Result<ExecutionOutcome, AppError> {
        let plan = self.current_plan(lease.task.id).await?;
        let journals = self
            .journals
            .for_task(self.account_id, lease.task.id)
            .await?;
        if let Some(nfo) = journals
            .iter()
            .find(|journal| journal.plan_id == plan.id && journal.kind == JournalKind::Nfo)
            .cloned()
        {
            return self.advance_nfo(&plan, nfo).await;
        }
        let file = journals
            .iter()
            .find(|journal| {
                journal.plan_id == plan.id
                    && journal.status == JournalStatus::Verified
                    && matches!(
                        journal.kind,
                        JournalKind::Copy | JournalKind::Move | JournalKind::Hardlink
                    )
            })
            .cloned()
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::ResourceConflict,
                    "verified file journal not found",
                )
            })?;
        if plan.draft.nfo_input.is_none() {
            let result = self
                .journals
                .ensure_completed_result(self.account_id, &file, self.clock.now_us())
                .await?;
            return Ok(ExecutionOutcome::Completed(result));
        }
        self.journals
            .ensure_nfo_pending_result(self.account_id, &file, self.clock.now_us())
            .await?;
        let destination = nfo_destination(&plan)?;
        let existing = self.fs.inspect(&destination).map_err(filesystem)?;
        let nfo = self
            .journals
            .prepare_nfo(
                self.account_id,
                &plan,
                &file,
                existing.as_ref(),
                self.clock.now_us(),
            )
            .await?;
        self.advance_nfo(&plan, nfo).await
    }

    /// 从已有 journal 状态重新观察并安全收敛。
    ///
    /// # Errors
    ///
    /// 当前计划/来源投影缺失、数据库失败或持久状态损坏时返回稳定应用错误。
    pub async fn recover(&self, lease: &ProcessingLease) -> Result<ExecutionOutcome, AppError> {
        let plan = self.current_plan(lease.task.id).await?;
        let journals = self
            .journals
            .for_task(self.account_id, lease.task.id)
            .await?;
        let journal = journals
            .iter()
            .find(|journal| {
                journal.plan_id == plan.id
                    && journal.kind == JournalKind::SourceRemoval
                    && !matches!(
                        journal.status,
                        JournalStatus::Verified | JournalStatus::Compensated
                    )
            })
            .cloned()
            .or_else(|| {
                journals
                    .iter()
                    .find(|journal| {
                        journal.plan_id == plan.id
                            && journal.kind == JournalKind::Nfo
                            && !matches!(
                                journal.status,
                                JournalStatus::Verified | JournalStatus::Compensated
                            )
                    })
                    .cloned()
            })
            .or_else(|| {
                journals
                    .iter()
                    .rev()
                    .find(|journal| journal.plan_id == plan.id)
                    .cloned()
            })
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "organization journal not found"))?;
        Box::pin(self.advance(
            &plan,
            journal,
            &ProcessingStopToken::default(),
            ExecutionScope::ThroughNfo,
        ))
        .await
    }

    /// 只补偿仍与 verified journal 一致的当前 `LocalResult`。
    ///
    /// # Errors
    ///
    /// 结果版本陈旧、幂等键冲突、journal 不可补偿或数据库失败时返回稳定错误。
    pub async fn rollback(&self, command: RollbackCommand) -> Result<LocalResultView, AppError> {
        if let Some(replay) = self
            .journals
            .rollback_replay(
                self.account_id,
                command.task_id,
                command.result_version,
                &command.idempotency_key,
            )
            .await?
        {
            return Ok(replay);
        }
        let result = self
            .journals
            .result_for_task(self.account_id, command.task_id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "organization result not found"))?;
        if !rollback_candidate(&result, command.result_version) {
            return Err(AppError::new(
                ErrorCode::ConfigVersionConflict,
                "organization result version changed",
            ));
        }
        let journals = self
            .journals
            .for_task(self.account_id, command.task_id)
            .await?;
        let journal = journals
            .iter()
            .find(|journal| {
                journal.status == JournalStatus::Verified
                    && matches!(
                        journal.kind,
                        JournalKind::Copy | JournalKind::Move | JournalKind::Hardlink
                    )
            })
            .cloned()
            .ok_or_else(|| {
                AppError::new(ErrorCode::ResourceConflict, "verified journal missing")
            })?;
        let mut compensated_additional = Vec::new();
        if let Some(nfo) = journals.iter().find(|candidate| {
            candidate.status == JournalStatus::Verified
                && candidate.kind == JournalKind::Nfo
                && candidate.nfo_outcome == Some(LocalNfoStatus::Generated)
        }) {
            let compensation = self
                .fs
                .compensate(&verified_operation(nfo)?)
                .map_err(file_operation)?;
            if compensation == CompensationOutcome::ManualReview {
                self.journals
                    .manual_review(
                        nfo.id,
                        nfo.projection_version,
                        "rollback.manual-review",
                        self.clock.now_us(),
                    )
                    .await?;
                return self
                    .journals
                    .finish_rollback(
                        self.account_id,
                        command.task_id,
                        command.result_version,
                        &command.idempotency_key,
                        &journal,
                        &[],
                        LocalResultStatus::ManualReview,
                        self.clock.now_us(),
                    )
                    .await;
            }
            compensated_additional.push(nfo.clone());
        }
        let compensation = self
            .fs
            .compensate(&verified_operation(&journal)?)
            .map_err(file_operation)?;
        let status = match compensation {
            CompensationOutcome::Removed
            | CompensationOutcome::Restored
            | CompensationOutcome::AlreadyAbsent => LocalResultStatus::Compensated,
            CompensationOutcome::ManualReview => LocalResultStatus::ManualReview,
        };
        self.journals
            .finish_rollback(
                self.account_id,
                command.task_id,
                command.result_version,
                &command.idempotency_key,
                &journal,
                &compensated_additional,
                status,
                self.clock.now_us(),
            )
            .await
    }

    async fn current_plan(&self, task_id: Uuid) -> Result<OrganizationPlanRecord, AppError> {
        self.plans
            .current(self.account_id, task_id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "organization plan not found"))
    }

    async fn advance(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
        stop: &ProcessingStopToken,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        if journal.kind == JournalKind::SourceRemoval {
            return self.advance_source_removal(plan, journal, scope).await;
        }
        if journal.kind == JournalKind::Nfo {
            return self.advance_nfo(plan, journal).await;
        }
        match journal.status {
            JournalStatus::Prepared => {
                let executing = self
                    .journals
                    .mark_executing(journal.id, journal.projection_version, self.clock.now_us())
                    .await?;
                self.execute(plan, executing, stop, scope).await
            }
            JournalStatus::Executing => {
                Box::pin(self.recover_executing(plan, journal, stop, scope)).await
            }
            JournalStatus::Applied => self.verify_applied(plan, journal, scope).await,
            JournalStatus::Verified => self.completed(plan, journal, scope).await,
            JournalStatus::ManualReview => Ok(ExecutionOutcome::ManualReview(
                self.journals
                    .result_for_task(self.account_id, journal.task_id)
                    .await?,
            )),
            JournalStatus::Compensated => {
                let result = self
                    .journals
                    .result_for_task(self.account_id, journal.task_id)
                    .await?
                    .ok_or_else(|| AppError::new(ErrorCode::Internal, "result missing"))?;
                Ok(ExecutionOutcome::Compensated(result))
            }
        }
    }

    async fn execute(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
        stop: &ProcessingStopToken,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        let source = journal
            .expected_source
            .clone()
            .ok_or_else(|| AppError::new(ErrorCode::Internal, "journal source missing"))?;
        let operation =
            FileOperationSpec::new(journal.operation_id, source, journal.destination.clone());
        let applied = match plan.draft.operation {
            super::model::OrganizationOperation::Copy => self.fs.copy_no_clobber(&operation, stop),
            super::model::OrganizationOperation::Move => self.fs.move_no_clobber(&operation),
            super::model::OrganizationOperation::Hardlink => {
                self.fs.hardlink_no_clobber(&operation)
            }
        };
        match applied {
            Ok(applied) => {
                let applied = self
                    .journals
                    .mark_applied(
                        journal.id,
                        journal.projection_version,
                        &applied,
                        self.clock.now_us(),
                    )
                    .await?;
                self.verify_applied(plan, applied, scope).await
            }
            Err(FileOperationError::IoTemporary) if stop.is_stopped() => {
                Ok(ExecutionOutcome::Stopped)
            }
            Err(FileOperationError::CompositeMoveRequired) => {
                self.execute_composite_move(plan, journal, &operation, stop, scope)
                    .await
            }
            Err(FileOperationError::IoTemporary) => Ok(ExecutionOutcome::RecoveryPending {
                reason: "organization.io-temporary".to_owned(),
            }),
            Err(error) => self.manual_review(journal, error.stable_reason()).await,
        }
    }

    async fn recover_executing(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
        stop: &ProcessingStopToken,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        let target = self.fs.inspect(&journal.destination).map_err(filesystem)?;
        if let Some(target) = target {
            if executing_target_matches(&self.fs, &journal, &target)? {
                let applied = self
                    .journals
                    .mark_applied(
                        journal.id,
                        journal.projection_version,
                        &target,
                        self.clock.now_us(),
                    )
                    .await?;
                return self.verify_applied(plan, applied, scope).await;
            }
            return self.manual_review(journal, "target-exists").await;
        }
        let Some(expected) = journal.expected_source.as_ref() else {
            return self.manual_review(journal, "source-changed").await;
        };
        if self
            .fs
            .observe(expected.locator())
            .map_err(filesystem)?
            .as_ref()
            != Some(expected)
        {
            return self.manual_review(journal, "source-changed").await;
        }
        self.execute(plan, journal, stop, scope).await
    }

    async fn verify_applied(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        let expected = journal
            .applied
            .as_ref()
            .ok_or_else(|| AppError::new(ErrorCode::Internal, "applied facts missing"))?;
        if self
            .fs
            .inspect(&journal.destination)
            .map_err(filesystem)?
            .as_ref()
            != Some(expected)
        {
            return self.manual_review(journal, "target-changed").await;
        }
        let verified = self
            .journals
            .verify(journal.id, journal.projection_version, self.clock.now_us())
            .await?;
        self.completed(plan, verified, scope).await
    }

    async fn completed(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        if journal.kind == JournalKind::Move {
            let source = journal
                .expected_source
                .as_ref()
                .ok_or_else(|| AppError::new(ErrorCode::Internal, "move source facts missing"))?;
            if let Some(current) = self.fs.observe(source.locator()).map_err(filesystem)? {
                if current != *source {
                    return self.manual_review(journal, "source-changed").await;
                }
                return self.start_source_removal(plan, journal, scope).await;
            }
        }
        self.start_nfo_or_complete(plan, journal, scope).await
    }

    async fn manual_review(
        &self,
        journal: JournalRecord,
        reason: &str,
    ) -> Result<ExecutionOutcome, AppError> {
        let journal = self
            .journals
            .manual_review(
                journal.id,
                journal.projection_version,
                reason,
                self.clock.now_us(),
            )
            .await?;
        let current = self
            .journals
            .result_for_task(self.account_id, journal.task_id)
            .await?;
        let result = if current.is_some() {
            Some(
                self.journals
                    .mark_result_manual(self.account_id, journal.plan_id, self.clock.now_us())
                    .await?,
            )
        } else {
            None
        };
        Ok(ExecutionOutcome::ManualReview(result))
    }

    async fn execute_composite_move(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
        operation: &FileOperationSpec,
        stop: &ProcessingStopToken,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        match self.fs.copy_no_clobber(operation, stop) {
            Ok(applied) => {
                let applied = self
                    .journals
                    .mark_applied(
                        journal.id,
                        journal.projection_version,
                        &applied,
                        self.clock.now_us(),
                    )
                    .await?;
                let expected = applied.applied.as_ref().ok_or_else(|| {
                    AppError::new(ErrorCode::Internal, "composite move applied facts missing")
                })?;
                if self
                    .fs
                    .inspect(&applied.destination)
                    .map_err(filesystem)?
                    .as_ref()
                    != Some(expected)
                {
                    return self.manual_review(applied, "target-changed").await;
                }
                let verified = self
                    .journals
                    .verify(applied.id, applied.projection_version, self.clock.now_us())
                    .await?;
                self.start_source_removal(plan, verified, scope).await
            }
            Err(FileOperationError::IoTemporary) if stop.is_stopped() => {
                Ok(ExecutionOutcome::Stopped)
            }
            Err(FileOperationError::IoTemporary) => Ok(ExecutionOutcome::RecoveryPending {
                reason: "organization.io-temporary".to_owned(),
            }),
            Err(error) => self.manual_review(journal, error.stable_reason()).await,
        }
    }

    async fn start_source_removal(
        &self,
        plan: &OrganizationPlanRecord,
        parent: JournalRecord,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        self.journals
            .ensure_partial_result(self.account_id, &parent, self.clock.now_us())
            .await?;
        let child = self
            .journals
            .prepare_source_removal(self.account_id, &parent, self.clock.now_us())
            .await?;
        self.advance_source_removal(plan, child, scope).await
    }

    async fn advance_source_removal(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        match journal.status {
            JournalStatus::Prepared => {
                let executing = self
                    .journals
                    .mark_executing(journal.id, journal.projection_version, self.clock.now_us())
                    .await?;
                self.execute_source_removal(plan, executing, scope).await
            }
            JournalStatus::Executing => {
                let source = journal.expected_source.as_ref().ok_or_else(|| {
                    AppError::new(ErrorCode::Internal, "source-removal facts missing")
                })?;
                match self.fs.observe(source.locator()).map_err(filesystem)? {
                    None => self.execute_source_removal(plan, journal, scope).await,
                    Some(current) if current == *source => {
                        self.execute_source_removal(plan, journal, scope).await
                    }
                    Some(_) => self.manual_review(journal, "source-changed").await,
                }
            }
            JournalStatus::Applied => self.verify_source_removal(plan, journal, scope).await,
            JournalStatus::Verified => self.finish_source_removal(plan, journal, scope).await,
            JournalStatus::ManualReview => Ok(ExecutionOutcome::ManualReview(
                self.journals
                    .result_for_task(self.account_id, journal.task_id)
                    .await?,
            )),
            JournalStatus::Compensated => {
                let result = self
                    .journals
                    .result_for_task(self.account_id, journal.task_id)
                    .await?
                    .ok_or_else(|| AppError::new(ErrorCode::Internal, "result missing"))?;
                Ok(ExecutionOutcome::Compensated(result))
            }
        }
    }

    async fn execute_source_removal(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        let parent_operation_id = journal.parent_operation_id.ok_or_else(|| {
            AppError::new(ErrorCode::Internal, "source-removal parent is missing")
        })?;
        let parent = self
            .journals
            .by_operation(self.account_id, journal.plan_id, parent_operation_id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::Internal, "move parent journal missing"))?;
        let source = journal
            .expected_source
            .as_ref()
            .ok_or_else(|| AppError::new(ErrorCode::Internal, "source-removal facts missing"))?;
        let sha256 = parent
            .applied
            .as_ref()
            .ok_or_else(|| AppError::new(ErrorCode::Internal, "move applied facts missing"))?
            .sha256();
        match self
            .fs
            .remove_verified_source(journal.operation_id, source, sha256)
        {
            Ok(()) => {
                let applied = self
                    .journals
                    .mark_source_removed(
                        journal.id,
                        journal.projection_version,
                        self.clock.now_us(),
                    )
                    .await?;
                self.verify_source_removal(plan, applied, scope).await
            }
            Err(FileOperationError::IoTemporary) => Ok(ExecutionOutcome::RecoveryPending {
                reason: "organization.io-temporary".to_owned(),
            }),
            Err(error) => self.manual_review(journal, error.stable_reason()).await,
        }
    }

    async fn verify_source_removal(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        let source = journal
            .expected_source
            .as_ref()
            .ok_or_else(|| AppError::new(ErrorCode::Internal, "source-removal facts missing"))?;
        if self
            .fs
            .observe(source.locator())
            .map_err(filesystem)?
            .is_some()
        {
            return self.manual_review(journal, "source-changed").await;
        }
        let verified = self
            .journals
            .verify(journal.id, journal.projection_version, self.clock.now_us())
            .await?;
        self.finish_source_removal(plan, verified, scope).await
    }

    async fn finish_source_removal(
        &self,
        plan: &OrganizationPlanRecord,
        child: JournalRecord,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        let parent_operation_id = child.parent_operation_id.ok_or_else(|| {
            AppError::new(ErrorCode::Internal, "source-removal parent is missing")
        })?;
        let parent = self
            .journals
            .by_operation(self.account_id, child.plan_id, parent_operation_id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::Internal, "move parent journal missing"))?;
        self.start_nfo_or_complete(plan, parent, scope).await
    }

    async fn start_nfo_or_complete(
        &self,
        plan: &OrganizationPlanRecord,
        file_journal: JournalRecord,
        scope: ExecutionScope,
    ) -> Result<ExecutionOutcome, AppError> {
        if plan.draft.nfo_input.is_none() {
            let result = self
                .journals
                .ensure_completed_result(self.account_id, &file_journal, self.clock.now_us())
                .await?;
            return Ok(ExecutionOutcome::Completed(result));
        }
        self.journals
            .ensure_nfo_pending_result(self.account_id, &file_journal, self.clock.now_us())
            .await?;
        if scope == ExecutionScope::FileOnly {
            let result = self
                .journals
                .result_for_task(self.account_id, file_journal.task_id)
                .await?
                .ok_or_else(|| AppError::new(ErrorCode::Internal, "result missing"))?;
            return Ok(ExecutionOutcome::Completed(result));
        }
        let destination = nfo_destination(plan)?;
        let existing = self.fs.inspect(&destination).map_err(filesystem)?;
        let nfo = self
            .journals
            .prepare_nfo(
                self.account_id,
                plan,
                &file_journal,
                existing.as_ref(),
                self.clock.now_us(),
            )
            .await?;
        self.advance_nfo(plan, nfo).await
    }

    async fn advance_nfo(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
    ) -> Result<ExecutionOutcome, AppError> {
        match journal.status {
            JournalStatus::Prepared => {
                let executing = self
                    .journals
                    .mark_executing(journal.id, journal.projection_version, self.clock.now_us())
                    .await?;
                self.execute_nfo(plan, executing).await
            }
            JournalStatus::Executing => self.recover_nfo_executing(plan, journal).await,
            JournalStatus::Applied => self.verify_nfo(journal).await,
            JournalStatus::Verified => self.finish_nfo(journal).await,
            JournalStatus::ManualReview => Ok(ExecutionOutcome::ManualReview(
                self.journals
                    .result_for_task(self.account_id, journal.task_id)
                    .await?,
            )),
            JournalStatus::Compensated => {
                let result = self
                    .journals
                    .result_for_task(self.account_id, journal.task_id)
                    .await?
                    .ok_or_else(|| AppError::new(ErrorCode::Internal, "result missing"))?;
                Ok(ExecutionOutcome::Compensated(result))
            }
        }
    }

    async fn execute_nfo(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
    ) -> Result<ExecutionOutcome, AppError> {
        if journal.nfo_preexisting == Some(true) {
            let existing = journal.applied.clone().ok_or_else(|| {
                AppError::new(ErrorCode::Internal, "preexisting NFO facts missing")
            })?;
            let applied = self
                .journals
                .mark_nfo_applied(
                    journal.id,
                    journal.projection_version,
                    &existing,
                    LocalNfoStatus::Preserved,
                    self.clock.now_us(),
                )
                .await?;
            return self.verify_nfo(applied).await;
        }
        let (file_name, bytes) = generated_nfo(plan)?;
        if journal
            .destination
            .relative_path()
            .as_str()
            .rsplit('/')
            .next()
            != Some(file_name.as_str())
        {
            return self.manual_review(journal, "nfo-plan-changed").await;
        }
        self.publish_generated_nfo(journal, &bytes).await
    }

    async fn publish_generated_nfo(
        &self,
        journal: JournalRecord,
        bytes: &[u8],
    ) -> Result<ExecutionOutcome, AppError> {
        let operation = NfoOperationSpec::new(journal.operation_id, journal.destination.clone());
        match self.fs.write_new_nfo(&operation, bytes) {
            Ok(created) => {
                let applied = self
                    .journals
                    .mark_nfo_applied(
                        journal.id,
                        journal.projection_version,
                        &created,
                        LocalNfoStatus::Generated,
                        self.clock.now_us(),
                    )
                    .await?;
                self.verify_nfo(applied).await
            }
            Err(FileOperationError::TargetExists) => {
                match self.fs.inspect(&journal.destination).map_err(filesystem)? {
                    Some(current)
                        if current.size_bytes()
                            == u64::try_from(bytes.len()).map_err(internal)?
                            && current.sha256() == &<[u8; 32]>::from(Sha256::digest(bytes)) =>
                    {
                        let applied = self
                            .journals
                            .mark_nfo_applied(
                                journal.id,
                                journal.projection_version,
                                &current,
                                LocalNfoStatus::Generated,
                                self.clock.now_us(),
                            )
                            .await?;
                        self.verify_nfo(applied).await
                    }
                    Some(_) | None => self.manual_review(journal, "target-exists").await,
                }
            }
            Err(FileOperationError::IoTemporary) => {
                self.journals
                    .mark_nfo_failed_result(self.account_id, journal.plan_id, self.clock.now_us())
                    .await?;
                Ok(ExecutionOutcome::RecoveryPending {
                    reason: "organization.io-temporary".to_owned(),
                })
            }
            Err(error) => self.manual_review(journal, error.stable_reason()).await,
        }
    }

    async fn recover_nfo_executing(
        &self,
        plan: &OrganizationPlanRecord,
        journal: JournalRecord,
    ) -> Result<ExecutionOutcome, AppError> {
        let current = self.fs.inspect(&journal.destination).map_err(filesystem)?;
        if journal.nfo_preexisting == Some(true) {
            let expected = journal.applied.as_ref().ok_or_else(|| {
                AppError::new(ErrorCode::Internal, "preexisting NFO facts missing")
            })?;
            return if current.as_ref() == Some(expected) {
                let applied = self
                    .journals
                    .mark_nfo_applied(
                        journal.id,
                        journal.projection_version,
                        expected,
                        LocalNfoStatus::Preserved,
                        self.clock.now_us(),
                    )
                    .await?;
                self.verify_nfo(applied).await
            } else {
                self.manual_review(journal, "nfo-changed").await
            };
        }
        let (_, bytes) = generated_nfo(plan)?;
        if let Some(current) = current {
            let expected_sha256: [u8; 32] = Sha256::digest(&bytes).into();
            if current.size_bytes() == u64::try_from(bytes.len()).map_err(internal)?
                && current.sha256() == &expected_sha256
            {
                let applied = self
                    .journals
                    .mark_nfo_applied(
                        journal.id,
                        journal.projection_version,
                        &current,
                        LocalNfoStatus::Generated,
                        self.clock.now_us(),
                    )
                    .await?;
                return self.verify_nfo(applied).await;
            }
            return self.manual_review(journal, "target-exists").await;
        }
        self.publish_generated_nfo(journal, &bytes).await
    }

    async fn verify_nfo(&self, journal: JournalRecord) -> Result<ExecutionOutcome, AppError> {
        let expected = journal
            .applied
            .as_ref()
            .ok_or_else(|| AppError::new(ErrorCode::Internal, "NFO applied facts missing"))?;
        if self
            .fs
            .inspect(&journal.destination)
            .map_err(filesystem)?
            .as_ref()
            != Some(expected)
        {
            return self.manual_review(journal, "nfo-changed").await;
        }
        let verified = self
            .journals
            .verify(journal.id, journal.projection_version, self.clock.now_us())
            .await?;
        self.finish_nfo(verified).await
    }

    async fn finish_nfo(&self, journal: JournalRecord) -> Result<ExecutionOutcome, AppError> {
        let result = self
            .journals
            .finish_nfo_result(self.account_id, &journal, self.clock.now_us())
            .await?;
        Ok(ExecutionOutcome::Completed(result))
    }
}

fn rollback_candidate(result: &LocalResultView, expected_version: i64) -> bool {
    result.version == expected_version
        && matches!(
            result.status,
            LocalResultStatus::Completed | LocalResultStatus::PartialSuccess
        )
}

fn verified_operation(journal: &JournalRecord) -> Result<VerifiedOperation, AppError> {
    let applied = journal
        .applied
        .clone()
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "journal applied facts missing"))?;
    let kind = match journal.kind {
        JournalKind::Copy => VerifiedOperationKind::Copy,
        JournalKind::Move => VerifiedOperationKind::Move,
        JournalKind::Hardlink => VerifiedOperationKind::Hardlink,
        JournalKind::Nfo if journal.nfo_outcome == Some(LocalNfoStatus::Generated) => {
            VerifiedOperationKind::Nfo
        }
        JournalKind::Nfo | JournalKind::SourceRemoval => {
            return Err(AppError::new(
                ErrorCode::ResourceConflict,
                "journal cannot be compensated independently",
            ));
        }
    };
    Ok(VerifiedOperation::new(
        journal.operation_id,
        kind,
        journal
            .expected_source
            .as_ref()
            .map(|source| source.locator().clone()),
        applied,
    ))
}

fn nfo_destination(plan: &OrganizationPlanRecord) -> Result<FileLocator, AppError> {
    let operation = plan
        .operations
        .iter()
        .find(|operation| operation.kind == super::planner::PlannedOperationKind::EnsureMissingNfo)
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "plan NFO operation missing"))?;
    Ok(FileLocator::new(
        operation.destination.root_id.clone(),
        operation.destination.relative_path.clone(),
    ))
}

fn generated_nfo(plan: &OrganizationPlanRecord) -> Result<(String, Vec<u8>), AppError> {
    let input = plan
        .draft
        .nfo_input
        .as_ref()
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "plan NFO input missing"))?;
    match NfoGenerator.decide(input, None).map_err(internal)? {
        NfoDecision::Generate { file_name, bytes } => Ok((file_name, bytes)),
        NfoDecision::Preserve { .. } | NfoDecision::Skip => Err(AppError::new(
            ErrorCode::Internal,
            "plan NFO input is not generatable",
        )),
    }
}

fn source_matches_revision(
    observed: &super::fs::ObservedFile,
    source: &OrganizationSourceProjection,
    plan: &OrganizationPlanRecord,
) -> bool {
    source.file_revision_id == plan.draft.file_revision_id
        && observed
            .identity()
            .matches_durable_snapshot(&source.identity_snapshot)
        && observed.size_bytes() == source.size_bytes
        && observed.modified_at_ns() == i128::from(source.modified_at_ns)
}

fn executing_target_matches(
    fs: &Arc<dyn OrganizationFs>,
    journal: &JournalRecord,
    target: &super::fs::AppliedFile,
) -> Result<bool, AppError> {
    let Some(source) = journal.expected_source.as_ref() else {
        return Ok(false);
    };
    match journal.kind {
        JournalKind::Copy => Ok(fs
            .inspect(source.locator())
            .map_err(filesystem)?
            .is_some_and(|current| {
                current.identity() == source.identity()
                    && current.size_bytes() == source.size_bytes()
                    && current.size_bytes() == target.size_bytes()
                    && current.sha256() == target.sha256()
            })),
        JournalKind::Move => {
            let current_source = fs.observe(source.locator()).map_err(filesystem)?;
            Ok(current_source
                .as_ref()
                .is_none_or(|current| current == source)
                && target.size_bytes() == source.size_bytes()
                && (target.identity() == source.identity()
                    || fs
                        .inspect(source.locator())
                        .map_err(filesystem)?
                        .is_some_and(|current| current.sha256() == target.sha256())))
        }
        JournalKind::Hardlink => Ok(
            target.identity() == source.identity() && target.size_bytes() == source.size_bytes()
        ),
        JournalKind::SourceRemoval | JournalKind::Nfo => Ok(false),
    }
}

fn filesystem(error: crate::discovery::model::FsBoundaryError) -> AppError {
    AppError::with_source(ErrorCode::OrganizationTargetUnavailable, error)
}

fn file_operation(error: FileOperationError) -> AppError {
    AppError::with_source(ErrorCode::ResourceConflict, error)
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}
