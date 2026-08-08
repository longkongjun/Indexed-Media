#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::discovery::capability::DeploymentRootsFingerprint;
use crate::discovery::model::{RelativePath, RootAccess, RootId};
use crate::identification::organization_port::{
    OrganizationIdentityPort, SqliteOrganizationIdentityPort,
};
use crate::identity::model::AuthenticatedSession;
use crate::identity::service::IdentityService;
use crate::organization::coordinator::SqliteOrganizationPlanningPort;
use crate::organization::executor::{OrganizationExecutor, RollbackCommand};
use crate::organization::fs::{FileLocator, OrganizationFs};
use crate::organization::journal_store::{
    JournalKind, JournalRecord, JournalStatus, JournalStore, LocalResultStatus, LocalResultView,
};
use crate::organization::plan_service::{OrganizationPlanService, OrganizationPlanningPort};
use crate::organization::plan_store::{OrganizationPlanRecord, OrganizationPlanStore};
use crate::organization::planner::{OrganizationLocation, PlanAuthorization, PlanningRiskCode};
use crate::platform::audit;
use crate::platform::db::Db;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::request_security::{CsrfGuard, SessionGuard, validate_same_origin};
use crate::platform::task_runtime::{SystemTaskClock, TaskClock};
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::processing::model::{
    ProcessingStage, ProcessingStatus, ProcessingTaskAction, ProcessingTaskView,
};
use crate::tasks::processing::service::ProcessingTaskService;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
/// Organization 详情的服务端计算状态。
pub enum OrganizationTaskState {
    /// 任务存在但尚无计划。
    NotPlanned,
    /// 当前计划可审查或已排队执行。
    Planned,
    /// 当前计划或安全事实要求人工授权/修正。
    Paused,
    /// journal 或 Catalog 尚未收敛。
    RecoveryPending,
    /// 媒体文件可用但仍有可重试的附加步骤。
    PartialSuccess,
    /// 本地结果已完成或已安全补偿。
    Completed,
    /// 外部变化使自动重放或补偿不再安全。
    ManualReview,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 当前 organization 投影允许的显式用户动作。
pub enum OrganizationTaskAction {
    /// 以当前事实创建下一不可变计划版本。
    Recalculate,
    /// 对当前 paused 计划授予一次性执行权。
    Execute,
    /// 从最近持久检查点创建恢复 attempt。
    Retry,
    /// 在下一个安全检查点取消任务。
    Cancel,
    /// 补偿仍与 verified journal 一致的本次产物。
    Rollback,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 能力根内、不会泄露宿主绝对路径的位置投影。
pub struct OrganizationLocationView {
    /// deployment root 逻辑 ID。
    pub root_id: RootId,
    /// 根内规范化相对路径。
    pub relative_path: RelativePath,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 不可变计划的有界公开投影。
pub struct OrganizationPlanView {
    /// 计划稳定 ID。
    pub id: Uuid,
    /// 任务内计划版本。
    pub version: i64,
    /// 目标聚合稳定 ID。
    pub target_id: Uuid,
    /// 已核对来源逻辑位置。
    pub source: OrganizationLocationView,
    /// 计划目标逻辑位置。
    pub destination: OrganizationLocationView,
    /// 固定文件操作。
    pub operation: super::model::OrganizationOperation,
    /// 固定命名结果说明。
    pub naming: String,
    /// 当前授权结论。
    pub authorization: PlanAuthorization,
    /// 阻止自动执行的稳定风险码。
    pub risk_codes: Vec<PlanningRiskCode>,
    /// 创建时间 UTC RFC3339。
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 一项 journal 的安全恢复投影。
pub struct OrganizationJournalView {
    /// journal 稳定 ID。
    pub id: Uuid,
    /// 不可变计划 operation ID。
    pub operation_id: Uuid,
    /// 文件/NFO 操作类型。
    pub kind: &'static str,
    /// 当前持久状态。
    pub status: &'static str,
    /// 有来源的 operation 才返回逻辑来源。
    pub source: Option<OrganizationLocationView>,
    /// 逻辑目标位置。
    pub destination: OrganizationLocationView,
    /// 乐观并发投影版本。
    pub projection_version: i64,
    /// 最近更新时间 UTC RFC3339。
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// `LocalResult` 的有界公开投影。
pub struct OrganizationLocalResultView {
    /// 结果稳定 ID。
    pub id: Uuid,
    /// 乐观并发版本。
    pub version: i64,
    /// 当前结果状态。
    pub status: &'static str,
    /// NFO 结论。
    pub nfo_status: &'static str,
    /// 已提交 Catalog 时的稳定媒体 ID。
    pub catalog_media_item_id: Option<Uuid>,
    /// 尚未完成的稳定动作名。
    pub remaining_actions: Vec<String>,
    /// 最近更新时间 UTC RFC3339。
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 一个 `ProcessingTask` 的完整安全 organization 详情。
pub struct ProcessingTaskOrganizationView {
    /// `ProcessingTask` 稳定 ID。
    pub task_id: Uuid,
    /// 服务端计算的 organization 状态。
    pub state: OrganizationTaskState,
    /// 当前最高版本计划；尚未规划时为空。
    pub plan: Option<OrganizationPlanView>,
    /// 按创建时间和 ID 稳定排序的 journal。
    pub journals: Vec<OrganizationJournalView>,
    /// 当前 LocalResult；尚无已核对文件时为空。
    pub local_result: Option<OrganizationLocalResultView>,
    /// 当前安全可用动作。
    pub allowed_actions: Vec<OrganizationTaskAction>,
}

#[derive(Clone)]
/// 组合 tasks、planner、journal、FS 和 rollback 的 organization 应用服务。
pub struct OrganizationTaskService {
    processing: ProcessingTaskService,
    plans: OrganizationPlanService,
    plan_store: OrganizationPlanStore,
    journals: JournalStore,
    executor: OrganizationExecutor,
    config_guard: Option<Arc<DeploymentRootsGuard>>,
}

struct DeploymentRootsGuard {
    path: PathBuf,
    fingerprint: DeploymentRootsFingerprint,
}

impl OrganizationTaskService {
    /// 使用显式 planner/executor 依赖创建服务，供隔离测试与生产装配复用。
    #[must_use]
    pub fn new(
        pool: sqlx::SqlitePool,
        plans: OrganizationPlanService,
        executor: OrganizationExecutor,
        notifier: OutboxNotifier,
    ) -> Self {
        Self {
            processing: ProcessingTaskService::new_with_notifier(pool.clone(), notifier.clone()),
            plans,
            plan_store: OrganizationPlanStore::new(pool.clone()),
            journals: JournalStore::new_with_notifier(pool, notifier),
            executor,
            config_guard: None,
        }
    }

    /// 以 `SQLite` 窄端口和 capability FS 创建生产服务。
    #[must_use]
    pub fn production(
        pool: sqlx::SqlitePool,
        notifier: OutboxNotifier,
        fs: Arc<dyn OrganizationFs>,
        root_access: BTreeMap<RootId, RootAccess>,
    ) -> Self {
        let identities: Arc<dyn OrganizationIdentityPort> =
            Arc::new(SqliteOrganizationIdentityPort::new(pool.clone()));
        let planning: Arc<dyn OrganizationPlanningPort> = Arc::new(
            SqliteOrganizationPlanningPort::new(pool.clone(), identities, fs.clone(), root_access),
        );
        let clock: Arc<dyn TaskClock> = Arc::new(SystemTaskClock);
        let plans = OrganizationPlanService::new(planning, pool.clone());
        let executor = OrganizationExecutor::unscoped(
            OrganizationPlanStore::new(pool.clone()),
            JournalStore::new_with_notifier(pool.clone(), notifier.clone()),
            fs,
            clock,
        );
        Self::new(pool, plans, executor, notifier)
    }

    /// 绑定服务创建时的 deployment-roots 精确指纹。
    #[must_use]
    pub fn with_config_guard(
        mut self,
        path: PathBuf,
        fingerprint: DeploymentRootsFingerprint,
    ) -> Self {
        self.config_guard = Some(Arc::new(DeploymentRootsGuard { path, fingerprint }));
        self
    }

    /// 返回 deployment-roots 是否仍与服务创建时相同。
    #[must_use]
    pub fn configuration_is_current(&self) -> bool {
        self.config_guard
            .as_ref()
            .is_none_or(|guard| guard.fingerprint.matches_path(&guard.path))
    }

    /// 读取任务拥有的安全 organization 聚合；尚无计划时返回 `not-planned`。
    ///
    /// # Errors
    ///
    /// 任务不属于账号、数据库或持久投影无效时返回稳定应用错误。
    pub async fn get(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<ProcessingTaskOrganizationView, AppError> {
        let task = self.processing.get(account_id, task_id).await?;
        let (plan, journals, result) = tokio::try_join!(
            self.plan_store.current(account_id, task_id),
            self.journals.for_task(account_id, task_id),
            self.journals.result_for_task(account_id, task_id),
        )?;
        to_task_view(&task, plan, &journals, result)
    }

    /// 以幂等键创建下一计划版本，并从 planning 安全恢复点重新排队。
    ///
    /// # Errors
    ///
    /// 任务已产生副作用、正在运行、规划失败或幂等键冲突时返回稳定应用错误。
    pub async fn recalculate(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: &str,
    ) -> Result<ProcessingTaskOrganizationView, AppError> {
        let task = self.commandable_planning_task(account_id, task_id).await?;
        self.plans
            .recalculate(account_id, task_id, idempotency_key)
            .await?;
        self.resume_planning(account_id, &task, idempotency_key)
            .await?;
        self.get(account_id, task_id).await
    }

    /// 为精确当前计划版本保存一次性授权，并从 planning 恢复点重新排队。
    ///
    /// # Errors
    ///
    /// 计划版本过期、风险不可覆盖、任务已开始执行或幂等键冲突时返回稳定错误。
    pub async fn execute(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        plan_version: i64,
        idempotency_key: &str,
    ) -> Result<ProcessingTaskOrganizationView, AppError> {
        let task = self.commandable_planning_task(account_id, task_id).await?;
        self.plans
            .authorize_once(account_id, task_id, plan_version, idempotency_key)
            .await?;
        self.resume_planning(account_id, &task, idempotency_key)
            .await?;
        self.get(account_id, task_id).await
    }

    /// 对当前结果执行版本绑定且可重放的安全补偿。
    ///
    /// # Errors
    ///
    /// 结果版本过期、文件事实改变、补偿不安全或数据库失败时返回稳定应用错误。
    pub async fn rollback(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        result_version: i64,
        idempotency_key: &str,
    ) -> Result<ProcessingTaskOrganizationView, AppError> {
        self.processing.get(account_id, task_id).await?;
        self.executor
            .for_account(account_id)
            .rollback(RollbackCommand {
                task_id,
                result_version,
                idempotency_key: idempotency_key.to_owned(),
            })
            .await?;
        self.get(account_id, task_id).await
    }

    async fn commandable_planning_task(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<ProcessingTaskView, AppError> {
        let task = self.processing.get(account_id, task_id).await?;
        let (journals, result) = tokio::try_join!(
            self.journals.for_task(account_id, task_id),
            self.journals.result_for_task(account_id, task_id),
        )?;
        if task.stage != ProcessingStage::Planning
            || task.status == ProcessingStatus::Running
            || !journals.is_empty()
            || result.is_some()
        {
            return Err(AppError::new(
                ErrorCode::TaskInvalidState,
                "organization planning command is not safe in the current state",
            ));
        }
        Ok(task)
    }

    async fn resume_planning(
        &self,
        account_id: Uuid,
        task: &ProcessingTaskView,
        idempotency_key: &str,
    ) -> Result<(), AppError> {
        match task.status {
            ProcessingStatus::Queued => Ok(()),
            ProcessingStatus::Paused | ProcessingStatus::Failed | ProcessingStatus::Cancelled => {
                self.processing
                    .retry(account_id, task.id, idempotency_key, now_us())
                    .await?;
                Ok(())
            }
            ProcessingStatus::Running
            | ProcessingStatus::WaitingConfirmation
            | ProcessingStatus::PartialSuccess
            | ProcessingStatus::Completed => Err(AppError::new(
                ErrorCode::TaskInvalidState,
                "organization planning task cannot be resumed",
            )),
        }
    }
}

#[derive(Clone)]
struct OrganizationTaskHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    tasks: OrganizationTaskService,
}

impl OrganizationTaskHttpState {
    async fn ensure_ready(&self) -> Result<(), AppError> {
        if !self.config.readiness_issues().is_empty() || !self.tasks.configuration_is_current() {
            return Err(not_ready());
        }
        match tokio::time::timeout(
            std::time::Duration::from_millis(250),
            sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(self.db.pool()),
        )
        .await
        {
            Ok(Ok(1)) => Ok(()),
            _ => Err(not_ready()),
        }
    }

    async fn guard_get(&self, headers: &HeaderMap) -> Result<AuthenticatedSession, AppError> {
        self.ensure_ready().await?;
        SessionGuard::authenticate(headers, &self.identity).await
    }

    async fn guard_write(&self, headers: &HeaderMap) -> Result<AuthenticatedSession, AppError> {
        self.ensure_ready().await?;
        let session = SessionGuard::authenticate_without_sliding(headers, &self.identity).await?;
        if let Err(error) = validate_same_origin(headers, &self.config.public_origin) {
            self.identity.audit_denial("origin").await?;
            return Err(error);
        }
        if let Err(error) = CsrfGuard::validate(headers, &session) {
            self.identity.audit_denial("csrf").await?;
            return Err(error);
        }
        Ok(session)
    }
}

/// 构建 organization 详情、重算、一次性执行与回滚命令路由。
pub fn router(config: AppConfig, db: &Db, tasks: OrganizationTaskService) -> Router {
    Router::new()
        .route(
            "/api/v1/processing-tasks/{processingTaskId}/organization",
            get(get_organization),
        )
        .route(
            "/api/v1/processing-tasks/{processingTaskId}/organization/recalculations",
            post(recalculate_organization),
        )
        .route(
            "/api/v1/processing-tasks/{processingTaskId}/organization/executions",
            post(execute_organization),
        )
        .route(
            "/api/v1/processing-tasks/{processingTaskId}/organization/rollbacks",
            post(rollback_organization),
        )
        .with_state(Arc::new(OrganizationTaskHttpState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            config,
            db: db.clone(),
            tasks,
        }))
}

async fn get_organization(
    State(state): State<Arc<OrganizationTaskHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<ProcessingTaskOrganizationView>, AppError> {
    let session = state.guard_get(&headers).await?;
    let task_id = task_id(&uri, None)?;
    Ok(Json(state.tasks.get(session.account.id, task_id).await?))
}

async fn recalculate_organization(
    State(state): State<Arc<OrganizationTaskHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_write(&headers).await?;
    let task_id = task_id(&uri, Some("recalculations"))?;
    let result = state
        .tasks
        .recalculate(session.account.id, task_id, idempotency_key(&headers)?)
        .await?;
    audit_success(&state, "organization.recalculate", task_id).await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

async fn execute_organization(
    State(state): State<Arc<OrganizationTaskHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    payload: Result<Json<PlanVersionRequest>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_write(&headers).await?;
    let task_id = task_id(&uri, Some("executions"))?;
    let key = idempotency_key(&headers)?;
    let Json(request) = payload.map_err(|_| validation("invalid organization execution JSON"))?;
    let result = state
        .tasks
        .execute(
            session.account.id,
            task_id,
            positive_version(request.plan_version)?,
            key,
        )
        .await?;
    audit_success(&state, "organization.execute", task_id).await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

async fn rollback_organization(
    State(state): State<Arc<OrganizationTaskHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    payload: Result<Json<ResultVersionRequest>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_write(&headers).await?;
    let task_id = task_id(&uri, Some("rollbacks"))?;
    let key = idempotency_key(&headers)?;
    let Json(request) = payload.map_err(|_| validation("invalid organization rollback JSON"))?;
    let result = state
        .tasks
        .rollback(
            session.account.id,
            task_id,
            positive_version(request.result_version)?,
            key,
        )
        .await?;
    audit_success(&state, "organization.rollback", task_id).await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanVersionRequest {
    plan_version: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultVersionRequest {
    result_version: i64,
}

async fn audit_success(
    state: &OrganizationTaskHttpState,
    action: &str,
    task_id: Uuid,
) -> Result<(), AppError> {
    audit::record(
        state.db.pool(),
        action,
        "success",
        Some(&task_id.to_string()),
    )
    .await
}

fn task_id(uri: &Uri, suffix: Option<&str>) -> Result<Uuid, AppError> {
    let base = uri
        .path()
        .strip_prefix("/api/v1/processing-tasks/")
        .ok_or_else(|| validation("invalid organization processing task ID"))?;
    let value = match suffix {
        Some(suffix) => base.strip_suffix(&format!("/organization/{suffix}")),
        None => base.strip_suffix("/organization"),
    }
    .filter(|value| !value.is_empty() && !value.contains('/'))
    .ok_or_else(|| validation("invalid organization processing task ID"))?;
    Uuid::parse_str(value).map_err(|_| validation("invalid organization processing task ID"))
}

fn idempotency_key(headers: &HeaderMap) -> Result<&str, AppError> {
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| validation("organization idempotency key missing"))?;
    if key.is_empty() || key.len() > 255 || key.chars().any(char::is_control) {
        return Err(validation("organization idempotency key is outside bounds"));
    }
    Ok(key)
}

fn positive_version(version: i64) -> Result<i64, AppError> {
    (version > 0)
        .then_some(version)
        .ok_or_else(|| validation("organization command version is invalid"))
}

fn to_task_view(
    task: &ProcessingTaskView,
    plan: Option<OrganizationPlanRecord>,
    journals: &[JournalRecord],
    result: Option<LocalResultView>,
) -> Result<ProcessingTaskOrganizationView, AppError> {
    let state = organization_state(task, plan.as_ref(), journals, result.as_ref());
    let allowed_actions = organization_actions(task, plan.as_ref(), journals, result.as_ref());
    Ok(ProcessingTaskOrganizationView {
        task_id: task.id,
        state,
        plan: plan.map(plan_view).transpose()?,
        journals: journals
            .iter()
            .map(journal_view)
            .collect::<Result<_, _>>()?,
        local_result: result.map(result_view),
        allowed_actions,
    })
}

fn organization_state(
    task: &ProcessingTaskView,
    plan: Option<&OrganizationPlanRecord>,
    journals: &[JournalRecord],
    result: Option<&LocalResultView>,
) -> OrganizationTaskState {
    if result.is_some_and(|result| result.status == LocalResultStatus::ManualReview)
        || journals
            .iter()
            .any(|journal| journal.status == JournalStatus::ManualReview)
    {
        return OrganizationTaskState::ManualReview;
    }
    if let Some(result) = result {
        return match result.status {
            LocalResultStatus::PartialSuccess => OrganizationTaskState::PartialSuccess,
            LocalResultStatus::Completed if task.status == ProcessingStatus::Completed => {
                OrganizationTaskState::Completed
            }
            LocalResultStatus::Completed => OrganizationTaskState::RecoveryPending,
            LocalResultStatus::Compensated => OrganizationTaskState::Completed,
            LocalResultStatus::ManualReview => OrganizationTaskState::ManualReview,
        };
    }
    if journals.iter().any(|journal| {
        matches!(
            journal.status,
            JournalStatus::Prepared
                | JournalStatus::Executing
                | JournalStatus::Applied
                | JournalStatus::Verified
        )
    }) {
        return OrganizationTaskState::RecoveryPending;
    }
    match plan {
        Some(plan) if plan.authorization == PlanAuthorization::Paused => {
            OrganizationTaskState::Paused
        }
        Some(_) => OrganizationTaskState::Planned,
        None => OrganizationTaskState::NotPlanned,
    }
}

fn organization_actions(
    task: &ProcessingTaskView,
    plan: Option<&OrganizationPlanRecord>,
    journals: &[JournalRecord],
    result: Option<&LocalResultView>,
) -> Vec<OrganizationTaskAction> {
    let mut actions = BTreeSet::new();
    let planning_safe = task.stage == ProcessingStage::Planning
        && task.status != ProcessingStatus::Running
        && journals.is_empty()
        && result.is_none();
    if planning_safe {
        actions.insert(OrganizationTaskAction::Recalculate);
    }
    if planning_safe
        && plan.is_some_and(|plan| {
            plan.authorization == PlanAuthorization::Paused
                && plan.draft.risk_codes.iter().all(|risk| {
                    matches!(
                        risk,
                        PlanningRiskCode::RuleNotMatched | PlanningRiskCode::AutomaticDisabled
                    )
                })
        })
    {
        actions.insert(OrganizationTaskAction::Execute);
    }
    for action in &task.allowed_actions {
        match action {
            ProcessingTaskAction::Retry => {
                actions.insert(OrganizationTaskAction::Retry);
            }
            ProcessingTaskAction::Cancel => {
                actions.insert(OrganizationTaskAction::Cancel);
            }
            ProcessingTaskAction::Review => {}
        }
    }
    if result.is_some_and(|result| {
        matches!(
            result.status,
            LocalResultStatus::Completed | LocalResultStatus::PartialSuccess
        )
    }) && journals.iter().any(|journal| {
        journal.status == JournalStatus::Verified
            && matches!(
                journal.kind,
                JournalKind::Copy | JournalKind::Move | JournalKind::Hardlink
            )
    }) {
        actions.insert(OrganizationTaskAction::Rollback);
    }
    actions.into_iter().collect()
}

fn plan_view(plan: OrganizationPlanRecord) -> Result<OrganizationPlanView, AppError> {
    Ok(OrganizationPlanView {
        id: plan.id,
        version: plan.version,
        target_id: plan.draft.target.id,
        source: location_view(&plan.draft.source),
        destination: location_view(&plan.draft.destination),
        operation: plan.draft.operation,
        naming: plan.draft.naming,
        authorization: plan.authorization,
        risk_codes: plan.draft.risk_codes,
        created_at: timestamp(plan.created_at_us)?,
    })
}

fn journal_view(journal: &JournalRecord) -> Result<OrganizationJournalView, AppError> {
    Ok(OrganizationJournalView {
        id: journal.id,
        operation_id: journal.operation_id,
        kind: journal.kind.as_str(),
        status: journal.status.as_str(),
        source: journal
            .expected_source
            .as_ref()
            .map(|source| locator_view(source.locator())),
        destination: locator_view(&journal.destination),
        projection_version: journal.projection_version,
        updated_at: timestamp(journal.updated_at_us)?,
    })
}

fn result_view(result: LocalResultView) -> OrganizationLocalResultView {
    OrganizationLocalResultView {
        id: result.id,
        version: result.version,
        status: result.status.as_str(),
        nfo_status: result.nfo_status.as_str(),
        catalog_media_item_id: result.catalog_media_item_id,
        remaining_actions: result.remaining_actions,
        updated_at: result.updated_at,
    }
}

fn location_view(location: &OrganizationLocation) -> OrganizationLocationView {
    OrganizationLocationView {
        root_id: location.root_id.clone(),
        relative_path: location.relative_path.clone(),
    }
}

fn locator_view(location: &FileLocator) -> OrganizationLocationView {
    OrganizationLocationView {
        root_id: location.root_id().clone(),
        relative_path: location.relative_path().clone(),
    }
}

fn timestamp(value: i64) -> Result<String, AppError> {
    chrono::DateTime::from_timestamp_micros(value)
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "invalid organization timestamp"))
}

fn now_us() -> i64 {
    chrono::Utc::now().timestamp_micros()
}

fn validation(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn not_ready() -> AppError {
    AppError::new(
        ErrorCode::NotReady,
        "organization task routes are not ready",
    )
}
