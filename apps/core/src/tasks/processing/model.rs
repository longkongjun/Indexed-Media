use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 一个不可变文件 revision 的持久化处理生命周期状态。
pub enum ProcessingStatus {
    /// 等待已注册阶段处理器领取。
    Queued,
    /// 由持有可续租租约的 worker 执行。
    Running,
    /// 识别需要管理员明确作出复核决定。
    WaitingConfirmation,
    /// 可恢复依赖暂时阻止流程推进。
    Paused,
    /// 处理已在最近提交的检查点停止。
    Cancelled,
    /// 必需本地工作已完成，但至少一个可选产物失败。
    PartialSuccess,
    /// 全部本地处理成功完成。
    Completed,
    /// 处理因不可恢复失败而终止。
    Failed,
}

impl ProcessingStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingConfirmation => "waiting-confirmation",
            Self::Paused => "paused",
            Self::Cancelled => "cancelled",
            Self::PartialSuccess => "partial-success",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 任务中心用于划分导航集合的稳定视图。
pub enum TaskCenterView {
    #[default]
    /// 仅包含 `waiting-confirmation` 或 `paused` 状态的任务。
    Pending,
    /// 包含 `queued` 或 `running` 状态的任务。
    Running,
    /// 不区分状态的全部任务。
    All,
    /// 仅包含 `partial-success`、`completed` 或 `failed` 状态的任务，不包含 `cancelled`。
    Completed,
}

impl TaskCenterView {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::All => "all",
            Self::Completed => "completed",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 可由索引支持并绑定进游标的任务中心过滤条件。
pub struct ProcessingTaskFilter {
    /// 决定任务所属导航集合的基础视图。
    pub view: TaskCenterView,
    /// 仅返回指定能力阶段；`None` 不限制阶段。
    pub stage: Option<ProcessingStage>,
    /// 仅返回指定执行状态；`None` 不限制状态。
    pub status: Option<ProcessingStatus>,
    /// 仅返回指定收件目录的任务。
    pub inbox_directory_id: Option<Uuid>,
    /// 对规范化相对路径执行包含匹配的查询文本。
    pub query: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 服务端根据当前任务状态推导出的安全操作。
pub enum ProcessingTaskAction {
    /// 打开并处理当前活动复核案例。
    Review,
    /// 从已提交检查点创建新的恢复尝试。
    Retry,
    /// 取消任务；`queued`、`paused` 或 `waiting-confirmation` 会在请求事务内直接转为
    /// `cancelled`，仅 `running` 会先记录取消请求并在后续协作式停止点生效。
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 与当前页基于同一不可变任务中心快照计算的导航计数。
pub struct TaskCenterSummary {
    /// 快照中属于 pending 视图的任务数。
    pub pending: u64,
    /// 快照中属于 running 视图的任务数。
    pub running: u64,
    /// 快照中的任务总数。
    pub all: u64,
    /// 快照中属于 completed 视图的任务数。
    pub completed: u64,
    /// 将页面、计数与后续游标绑定到一起的快照版本。
    pub snapshot_version: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 快照稳定的任务中心页面及其匹配导航计数。
pub struct ProcessingTaskPageView {
    /// 按稳定排序返回的公开任务投影。
    pub items: Vec<ProcessingTaskView>,
    /// 继续读取同一快照的游标；已到末页时为 `None`。
    pub next_cursor: Option<String>,
    /// 对同一过滤快照计算的各导航集合计数。
    pub summary: TaskCenterSummary,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 与执行状态相互独立、面向用户的能力阶段。
pub enum ProcessingStage {
    /// 从本地证据与提供方候选确定媒体身份。
    Identification,
    /// 根据已确认身份规划本地文件与元数据操作。
    Planning,
    /// 执行已核对的本地文件操作。
    FileOperation,
    /// 生成或更新本地 NFO 产物。
    Nfo,
    /// 汇总结果并提交正式本地投影。
    Completion,
}

impl ProcessingStage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Identification => "identification",
            Self::Planning => "planning",
            Self::FileOperation => "file-operation",
            Self::Nfo => "nfo",
            Self::Completion => "completion",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 最近一次原子提交的处理边界，用于取消与恢复。
pub enum ProcessingCheckpoint {
    /// 尚未提交任何处理阶段结果。
    Pending,
    /// 身份识别已完成，可进入规划或复核。
    IdentificationComplete,
    /// 已接受确认身份或普通视频决定，等待规划。
    PlanningRequested,
    /// 当前不可变整理计划已持久化。
    PlanPrepared,
    /// 当前计划已通过自动或一次性授权门禁。
    ExecutionAuthorized,
    /// 当前计划因规则或安全风险暂停。
    PlanningPaused,
    /// 文件 operation journal 已在副作用前持久化。
    FileOperationPrepared,
    /// 文件 operation 已进入可观察、可恢复的执行状态。
    FileOperationExecuting,
    /// 文件 operation 已通过目标事实核验。
    FileOperationVerified,
    /// 文件 operation 无法证明安全重放，需要人工处理。
    FileOperationManualReview,
    /// 文件结果已核验，等待生成或保留 NFO。
    NfoPending,
    /// NFO 已通过本地事实核验。
    NfoVerified,
    /// NFO 失败但已核验媒体文件仍可使用。
    NfoFailed,
    /// 可提交 Catalog 的本地结果已持久化。
    LocalResultPrepared,
    /// Catalog 已幂等接收当前本地结果。
    CatalogCommitted,
    /// 已持久化复核案例，等待管理员决定。
    WaitingConfirmation,
    /// 外部依赖阻塞已持久化，可按重试时间恢复。
    DependencyBlocked,
    /// 辅助文件已按策略跳过且该结论已提交。
    SkippedAuxiliary,
    /// 协作式取消已在安全边界提交。
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 应用一项不可变人工决定时使用的持久化子检查点。
pub enum DecisionCheckpoint {
    /// 已接受人工决定，等待下游协调器领取。
    ManualDecisionPending,
    /// 已安排使用人工线索重新识别。
    RematchPending,
    /// 管理员已确认普通视频路径。
    GenericVideoSelected,
    /// 识别结果已交给后续规划阶段。
    PlanningRequested,
}

impl DecisionCheckpoint {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ManualDecisionPending => "manual-decision-pending",
            Self::RematchPending => "rematch-pending",
            Self::GenericVideoSelected => "generic-video-selected",
            Self::PlanningRequested => "planning-requested",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 由已接受人工决定推导出的有界跨模块命令。
pub enum DecisionDispatch {
    /// 以已核验选择或修正线索重新执行识别。
    Reidentify {
        /// 需要恢复的处理任务 UUID。
        task_id: Uuid,
        /// 作为本次派发幂等依据的人工决定 UUID。
        decision_id: Uuid,
        /// 恢复前必须持久化的决定子检查点。
        checkpoint: DecisionCheckpoint,
    },
    /// 普通视频决定无需提供方识别，可直接进入规划。
    PlanningRequested {
        /// 需要推进到规划阶段的处理任务 UUID。
        task_id: Uuid,
        /// 作为本次派发幂等依据的人工决定 UUID。
        decision_id: Uuid,
    },
}

impl DecisionDispatch {
    #[must_use]
    /// 返回派发命令绑定的处理任务 UUID。
    pub const fn task_id(&self) -> Uuid {
        match self {
            Self::Reidentify { task_id, .. } | Self::PlanningRequested { task_id, .. } => *task_id,
        }
    }

    #[must_use]
    /// 返回生成派发命令的不可变人工决定 UUID。
    pub const fn decision_id(&self) -> Uuid {
        match self {
            Self::Reidentify { decision_id, .. } | Self::PlanningRequested { decision_id, .. } => {
                *decision_id
            }
        }
    }
}

impl ProcessingCheckpoint {
    /// 返回数据库与公开协议共用的稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::IdentificationComplete => "identification-complete",
            Self::PlanningRequested => "planning-requested",
            Self::PlanPrepared => "plan-prepared",
            Self::ExecutionAuthorized => "execution-authorized",
            Self::PlanningPaused => "planning-paused",
            Self::FileOperationPrepared => "file-operation-prepared",
            Self::FileOperationExecuting => "file-operation-executing",
            Self::FileOperationVerified => "file-operation-verified",
            Self::FileOperationManualReview => "file-operation-manual-review",
            Self::NfoPending => "nfo-pending",
            Self::NfoVerified => "nfo-verified",
            Self::NfoFailed => "nfo-failed",
            Self::LocalResultPrepared => "local-result-prepared",
            Self::CatalogCommitted => "catalog-committed",
            Self::WaitingConfirmation => "waiting-confirmation",
            Self::DependencyBlocked => "dependency-blocked",
            Self::SkippedAuxiliary => "skipped-auxiliary",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 通过处理投影和事件暴露的稳定、脱敏原因。
pub enum ProcessingReason {
    #[serde(rename = "identification.ambiguous")]
    /// 识别证据互相冲突，无法自动选择候选。
    IdentificationAmbiguous,
    #[serde(rename = "identification.confirmed-external-id")]
    /// 显式外部 ID 已由提供方核验并确认身份。
    IdentificationConfirmedExternalId,
    #[serde(rename = "identification.confirmed-title-year")]
    /// 标题与年份证据共同确认身份。
    IdentificationConfirmedTitleYear,
    #[serde(rename = "identification.multiple-strong-candidates")]
    /// 同时存在多个满足强确认条件的候选。
    IdentificationMultipleStrongCandidates,
    #[serde(rename = "identification.no-candidate")]
    /// 提供方没有返回可用候选。
    IdentificationNoCandidate,
    #[serde(rename = "identification.probable-title")]
    /// 标题匹配但证据不足以自动确认。
    IdentificationProbableTitle,
    #[serde(rename = "identification.provider-unavailable")]
    /// 提供方暂时不可用，可在稍后恢复。
    IdentificationProviderUnavailable,
    #[serde(rename = "identification.provider-unauthorized")]
    /// 提供方拒绝了当前配置的凭据。
    IdentificationProviderUnauthorized,
    #[serde(rename = "identification.revision-changed")]
    /// 处理期间文件 revision 被更新事实替换。
    IdentificationRevisionChanged,
    #[serde(rename = "auxiliary.sample")]
    /// 文件按策略识别为 sample 辅助内容。
    AuxiliarySample,
    #[serde(rename = "auxiliary.trailer")]
    /// 文件按策略识别为 trailer 辅助内容。
    AuxiliaryTrailer,
    #[serde(rename = "auxiliary.extra")]
    /// 文件按策略识别为其他附加内容。
    AuxiliaryExtra,
    #[serde(rename = "watcher.unavailable")]
    /// 文件系统 watcher 当前不可用。
    WatcherUnavailable,
    #[serde(rename = "reconcile.failed")]
    /// 最近一次完整事实对账失败。
    ReconcileFailed,
    #[serde(rename = "integration.unconfigured")]
    /// 所需内置集成尚未配置凭据。
    IntegrationUnconfigured,
    #[serde(rename = "integration.healthy")]
    /// 最近一次权威集成检查成功。
    IntegrationHealthy,
    #[serde(rename = "integration.unauthorized")]
    /// 内置集成凭据被提供方拒绝。
    IntegrationUnauthorized,
    #[serde(rename = "integration.rate-limited")]
    /// 提供方要求在有界时间后重试。
    IntegrationRateLimited,
    #[serde(rename = "integration.unavailable")]
    /// 集成因超时、网络或安全解析失败暂时不可用。
    IntegrationUnavailable,
    #[serde(rename = "organization.plan-paused")]
    /// 当前计划未通过规则、profile 或安全门禁。
    OrganizationPlanPaused,
    #[serde(rename = "organization.io-temporary")]
    /// 文件系统暂时失败，可从已持久化 journal 恢复。
    OrganizationIoTemporary,
    #[serde(rename = "organization.manual-review")]
    /// 文件事实无法证明可安全自动恢复。
    OrganizationManualReview,
    #[serde(rename = "organization.nfo-failed")]
    /// 媒体文件已核验，但 NFO 产物暂时失败。
    OrganizationNfoFailed,
    #[serde(rename = "organization.catalog-unavailable")]
    /// Catalog 暂时未接受已持久化本地结果。
    OrganizationCatalogUnavailable,
}

impl ProcessingReason {
    /// 返回数据库与公开协议共用的稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdentificationAmbiguous => "identification.ambiguous",
            Self::IdentificationConfirmedExternalId => "identification.confirmed-external-id",
            Self::IdentificationConfirmedTitleYear => "identification.confirmed-title-year",
            Self::IdentificationMultipleStrongCandidates => {
                "identification.multiple-strong-candidates"
            }
            Self::IdentificationNoCandidate => "identification.no-candidate",
            Self::IdentificationProbableTitle => "identification.probable-title",
            Self::IdentificationProviderUnavailable => "identification.provider-unavailable",
            Self::IdentificationProviderUnauthorized => "identification.provider-unauthorized",
            Self::IdentificationRevisionChanged => "identification.revision-changed",
            Self::AuxiliarySample => "auxiliary.sample",
            Self::AuxiliaryTrailer => "auxiliary.trailer",
            Self::AuxiliaryExtra => "auxiliary.extra",
            Self::WatcherUnavailable => "watcher.unavailable",
            Self::ReconcileFailed => "reconcile.failed",
            Self::IntegrationUnconfigured => "integration.unconfigured",
            Self::IntegrationHealthy => "integration.healthy",
            Self::IntegrationUnauthorized => "integration.unauthorized",
            Self::IntegrationRateLimited => "integration.rate-limited",
            Self::IntegrationUnavailable => "integration.unavailable",
            Self::OrganizationPlanPaused => "organization.plan-paused",
            Self::OrganizationIoTemporary => "organization.io-temporary",
            Self::OrganizationManualReview => "organization.manual-review",
            Self::OrganizationNfoFailed => "organization.nfo-failed",
            Self::OrganizationCatalogUnavailable => "organization.catalog-unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 创建一次持久化处理尝试的原因。
pub enum ProcessingAttemptReason {
    /// 文件 revision 首次进入处理流程。
    Initial,
    /// 管理员对可重试任务发起手动重试。
    ManualRetry,
    /// 暂时性依赖恢复后自动创建新尝试。
    DependencyRecovery,
    /// 进程终止或租约过期后恢复未完成任务。
    LeaseRecovery,
    /// 新文件 revision 取代了处理中绑定的旧 revision。
    RevisionReplaced,
}

impl ProcessingAttemptReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::ManualRetry => "manual-retry",
            Self::DependencyRecovery => "dependency-recovery",
            Self::LeaseRecovery => "lease-recovery",
            Self::RevisionReplaced => "revision-replaced",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 不含主机绝对路径的公开处理任务投影。
pub struct ProcessingTaskView {
    /// 单文件处理任务的稳定 UUID。
    pub id: Uuid,
    /// 文件所属收件目录 UUID。
    pub inbox_directory_id: Uuid,
    /// 当前任务绑定的不可变文件 revision UUID。
    pub file_revision_id: Uuid,
    /// 文件相对于已验证收件目录的路径。
    pub relative_path: String,
    /// 任务当前的持久化执行状态。
    pub status: ProcessingStatus,
    /// 当前面向用户的处理能力阶段。
    pub stage: ProcessingStage,
    /// 最近一次原子提交的恢复边界。
    pub checkpoint: ProcessingCheckpoint,
    /// 人工决定应用流程的可选持久化子检查点。
    pub decision_checkpoint: Option<DecisionCheckpoint>,
    /// 当前正在应用的不可变人工决定 UUID。
    pub current_task_decision_id: Option<Uuid>,
    /// 当前不可变整理计划 UUID；尚未规划时为 `None`。
    pub organization_plan_id: Option<Uuid>,
    /// 当前本地整理结果 UUID；文件尚未核验时为 `None`。
    pub organization_result_id: Option<Uuid>,
    /// Catalog 已接收结果后返回的正式媒体项目 UUID。
    pub catalog_media_item_id: Option<Uuid>,
    /// 解释当前等待、暂停或结果状态的脱敏原因。
    pub reason: Option<ProcessingReason>,
    /// 当前投影是否由恢复路径创建。
    pub recovering: bool,
    /// 为该任务创建过的持久化处理尝试总数。
    pub attempt_count: u64,
    /// 依赖阻塞允许重试的 UTC RFC3339 时间。
    pub next_retry_at: Option<String>,
    /// 服务端根据当前状态推导的安全操作集合。
    pub allowed_actions: Vec<ProcessingTaskAction>,
    /// 投影最近更新的 UTC RFC3339 时间。
    pub updated_at: String,
}

#[derive(Clone, Debug)]
/// 提交一次运行中处理尝试所需的乐观并发权限凭据。
pub struct ProcessingLease {
    /// 领取租约时读取的任务公开投影。
    pub task: ProcessingTaskView,
    /// 当前持久化处理尝试的 UUID。
    pub attempt_id: Uuid,
    /// 必须与提交调用方一致的租约所有者标识。
    pub owner: String,
    /// 每次续租或提交都会校验并递增的乐观版本。
    pub version: i64,
    /// 租约失效的 Unix epoch 微秒时间；过期后不得提交。
    pub expires_at_us: i64,
}
