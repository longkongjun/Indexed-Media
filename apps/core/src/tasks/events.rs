use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::automation::model::{AutomationAction, AutomationEventStatus, AutomationFailureCode};
use crate::connectors::downloader::model::{DownloadTaskStatus, DownloaderFailureCode};
use crate::connectors::downloader::port::RemoteDownloadStatus;
use crate::connectors::model::{IntegrationFailureCode, IntegrationHealth};
use crate::identification::decision::DecisionLevel;
use crate::tasks::model::{ScanCounts, ScanStatus};
use crate::tasks::processing::model::{ProcessingReason, ProcessingStage, ProcessingStatus};

/// 累计扫描计数的 SSE/发件箱类型。
pub const TASK_PROGRESS: &str = "task.progress";
/// 任务生命周期转换的 SSE/发件箱类型。
pub const TASK_STATE_CHANGED: &str = "task.state-changed";
/// 收件箱 watcher/对账健康投影变化的 SSE/发件箱类型。
pub const INBOX_DISCOVERY_HEALTH_CHANGED: &str = "inbox.discovery-health-changed";
/// 单文件处理任务生命周期转换的 SSE/outbox 类型。
pub const PROCESSING_TASK_STATE_CHANGED: &str = "processing-task.state-changed";
/// 一个不可变识别决策提交后的最小刷新事件类型。
pub const PROCESSING_TASK_IDENTIFICATION_DECIDED: &str = "processing-task.identification-decided";
/// 下游投影可幂等消费的已提交不可变人工决定事件类型。
pub const TASK_DECISION_ACCEPTED: &str = "task-decision.accepted";
/// 已核对本地结果创建或更新正式 Catalog 投影的事件类型。
pub const CATALOG_MEDIA_CHANGED: &str = "catalog.media-changed";
/// 整理目标安全配置实际变化后的最小刷新事件类型。
pub const ORGANIZATION_TARGET_CHANGED: &str = "organization-target.changed";
/// 本地整理结果实际变化后的最小刷新事件类型。
pub const ORGANIZATION_RESULT_CHANGED: &str = "organization-result.changed";
/// 内置集成脱敏健康投影变化的事件类型。
pub const INTEGRATION_HEALTH_CHANGED: &str = "integration.health-changed";
/// 下载任务公开投影实际变化时的最小刷新事件类型。
pub const DOWNLOAD_TASK_CHANGED: &str = "download-task.changed";
/// Automation event state, attempt, or downstream relation changed.
pub const AUTOMATION_EVENT_CHANGED: &str = "automation-event.changed";
/// Local identification enhancer configuration or health changed.
pub const IDENTIFICATION_ENHANCER_CHANGED: &str = "identification-enhancer.changed";
/// 表示保留策略已移除所请求历史记录的合成 SSE 类型。
pub const STREAM_GAP: &str = "stream.gap";
/// 当前序列化任务事件信封架构版本。
pub const SCHEMA_VERSION: &str = "1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type")]
/// 从持久化发件箱发往 SSE 客户端的、带标签的 schema-v1 事件。
///
/// 持久化事件 ID 单调递增且为 JavaScript 安全整数。重放时会合成 `StreamGap`，指示客户端从
/// `minimum_available_id` 重新开始。
pub enum TaskEventEnvelope {
    #[serde(rename = "task.progress")]
    /// 报告扫描任务当前的聚合计数。
    Progress {
        /// 单调递增的发件箱事件标识符。
        id: i64,
        /// 序列化事件信封架构版本。
        schema_version: String,
        /// 事件持久化时的 UTC 时间戳。
        occurred_at: String,
        /// 进度发生变化的扫描任务。
        task_id: Uuid,
        /// 扫描器报告的当前计数。
        payload: ScanCounts,
    },
    #[serde(rename = "task.state-changed")]
    /// 报告扫描任务状态转换。
    StateChanged {
        /// 单调递增的发件箱事件标识符。
        id: i64,
        /// 序列化事件信封架构版本。
        schema_version: String,
        /// 事件持久化时的 UTC 时间戳。
        occurred_at: String,
        /// 状态发生变化的扫描任务。
        task_id: Uuid,
        /// 转换后的状态和恢复详情。
        payload: TaskStateChangedPayload,
    },
    #[serde(rename = "inbox.discovery-health-changed")]
    /// 报告一个收件箱 watcher 与完整对账的综合健康变化。
    InboxDiscoveryHealthChanged {
        /// 单调递增的发件箱事件标识符。
        id: i64,
        /// 序列化事件信封架构版本。
        schema_version: String,
        /// 事件持久化时的 UTC 时间戳。
        occurred_at: String,
        /// 非任务事件固定为空。
        task_id: Option<Uuid>,
        /// 脱敏的收件箱健康投影。
        payload: InboxDiscoveryHealthChangedPayload,
    },
    #[serde(rename = "processing-task.state-changed")]
    /// 报告持久化单文件处理任务的状态或阶段转换。
    ProcessingTaskStateChanged {
        /// 单调递增的 outbox 事件标识符。
        id: i64,
        /// 序列化事件信封 Schema 版本。
        schema_version: String,
        /// 事件持久化时的 UTC RFC3339 时间。
        occurred_at: String,
        /// 状态发生变化的处理任务 UUID。
        task_id: Uuid,
        /// 转换后的公开任务状态投影。
        payload: ProcessingTaskStateChangedPayload,
    },
    #[serde(rename = "processing-task.identification-decided")]
    /// 报告已提交识别决策，但不内嵌证据或候选正文。
    ProcessingTaskIdentificationDecided {
        /// 单调递增的 outbox 事件标识符。
        id: i64,
        /// 序列化事件信封 Schema 版本。
        schema_version: String,
        /// 事件持久化时的 UTC RFC3339 时间。
        occurred_at: String,
        /// 完成识别决策的处理任务 UUID。
        task_id: Uuid,
        /// 供客户端按标识重新读取详情的最小刷新负载。
        payload: ProcessingTaskIdentificationDecidedPayload,
    },
    #[serde(rename = "task-decision.accepted")]
    /// 仅用稳定标识报告一个已接受人工决定。
    TaskDecisionAccepted {
        /// 单调递增的 outbox 事件标识符。
        id: i64,
        /// 序列化事件信封 Schema 版本。
        schema_version: String,
        /// 事件持久化时的 UTC RFC3339 时间。
        occurred_at: String,
        /// 接受人工决定的处理任务 UUID。
        task_id: Uuid,
        /// 不含人工输入正文的最小决定回执。
        payload: TaskDecisionAcceptedPayload,
    },
    #[serde(rename = "catalog.media-changed")]
    /// 报告一个正式本地媒体投影的最小刷新键。
    CatalogMediaChanged {
        /// 单调递增的 outbox 事件标识符。
        id: i64,
        /// 序列化事件信封 Schema 版本。
        schema_version: String,
        /// 事件持久化时的 UTC RFC3339 时间。
        occurred_at: String,
        /// 目录变化不是任务事件，因此线协议中固定为 `None`；来源任务从目录详情读取。
        task_id: Option<Uuid>,
        /// 供客户端重新读取目录项目的最小刷新负载。
        payload: CatalogMediaChangedPayload,
    },
    #[serde(rename = "organization-target.changed")]
    /// 报告一个整理目标聚合的最小刷新键。
    OrganizationTargetChanged {
        /// 单调递增的 outbox 事件标识符。
        id: i64,
        /// 序列化事件信封 Schema 版本。
        schema_version: String,
        /// 事件持久化时的 UTC RFC3339 时间。
        occurred_at: String,
        /// 目标变化不是处理任务事件，固定为 `None`。
        task_id: Option<Uuid>,
        /// 不含路径、规则正文或宿主信息的目标刷新负载。
        payload: OrganizationTargetChangedPayload,
    },
    #[serde(rename = "organization-result.changed")]
    /// 报告一个本地整理结果的最小刷新键。
    OrganizationResultChanged {
        /// 单调递增的 outbox 事件标识符。
        id: i64,
        /// 序列化事件信封 Schema 版本。
        schema_version: String,
        /// 事件持久化时的 UTC RFC3339 时间。
        occurred_at: String,
        /// 产生该结果的 `ProcessingTask` UUID。
        task_id: Uuid,
        /// 不含路径、NFO 正文或标题的结果刷新负载。
        payload: OrganizationResultChangedPayload,
    },
    #[serde(rename = "integration.health-changed")]
    /// 报告不含凭据或诊断正文的 TMDB 脱敏健康投影。
    IntegrationHealthChanged {
        /// 单调递增的 outbox 事件标识符。
        id: i64,
        /// 序列化事件信封 Schema 版本。
        schema_version: String,
        /// 事件持久化时的 UTC RFC3339 时间。
        occurred_at: String,
        /// 集成健康变化不是任务事件，因此固定为 `None`。
        task_id: Option<Uuid>,
        /// 可安全向管理员公开的健康与失败分类。
        payload: IntegrationHealthChangedPayload,
    },
    #[serde(rename = "download-task.changed")]
    /// 报告下载任务状态、远端状态、进度或失败分类的实际变化。
    DownloadTaskChanged {
        /// 单调递增的 outbox 事件标识符。
        id: i64,
        /// 序列化事件信封 Schema 版本。
        schema_version: String,
        /// 事件持久化时的 UTC RFC3339 时间。
        occurred_at: String,
        /// 公开投影发生变化的下载任务 UUID。
        task_id: Uuid,
        /// 不含下载源、远端路径或诊断正文的最小投影。
        payload: DownloadTaskChangedPayload,
    },
    #[serde(rename = "automation-event.changed")]
    /// Reports a minimal redacted automation event projection change.
    AutomationEventChanged {
        /// Monotonic outbox event identifier.
        id: i64,
        /// Serialized envelope schema version.
        schema_version: String,
        /// UTC time when the change committed.
        occurred_at: String,
        /// Automation events are not scan tasks, so this is always absent.
        task_id: Option<Uuid>,
        /// Stable identifiers and status only; no action payload.
        payload: AutomationEventChangedPayload,
    },
    #[serde(rename = "identification-enhancer.changed")]
    /// Reports a minimal local enhancer configuration or fallback-health change.
    IdentificationEnhancerChanged {
        id: i64,
        schema_version: String,
        occurred_at: String,
        task_id: Option<Uuid>,
        payload: IdentificationEnhancerChangedPayload,
    },
    #[serde(rename = "stream.gap")]
    /// 表示一个或多个历史事件已无法重放。
    StreamGap {
        /// 单调递增的发件箱事件标识符。
        id: i64,
        /// 序列化事件信封架构版本。
        schema_version: String,
        /// 事件持久化时的 UTC 时间戳。
        occurred_at: String,
        /// 间隙限定于任务时受影响的任务，否则为空。
        task_id: Option<Uuid>,
        /// 用于重新同步的最旧保留事件标识符。
        payload: StreamGapPayload,
    },
}

impl TaskEventEnvelope {
    #[must_use]
    /// 返回分配给此事件的单调递增标识符。
    pub const fn id(&self) -> i64 {
        match self {
            Self::Progress { id, .. }
            | Self::StateChanged { id, .. }
            | Self::InboxDiscoveryHealthChanged { id, .. }
            | Self::ProcessingTaskStateChanged { id, .. }
            | Self::ProcessingTaskIdentificationDecided { id, .. }
            | Self::TaskDecisionAccepted { id, .. }
            | Self::CatalogMediaChanged { id, .. }
            | Self::OrganizationTargetChanged { id, .. }
            | Self::OrganizationResultChanged { id, .. }
            | Self::IntegrationHealthChanged { id, .. }
            | Self::DownloadTaskChanged { id, .. }
            | Self::AutomationEventChanged { id, .. }
            | Self::IdentificationEnhancerChanged { id, .. }
            | Self::StreamGap { id, .. } => *id,
        }
    }

    #[must_use]
    /// 返回 SSE 协议使用的稳定事件类型字符串。
    pub const fn event_type(&self) -> &'static str {
        match self {
            Self::Progress { .. } => TASK_PROGRESS,
            Self::StateChanged { .. } => TASK_STATE_CHANGED,
            Self::InboxDiscoveryHealthChanged { .. } => INBOX_DISCOVERY_HEALTH_CHANGED,
            Self::ProcessingTaskStateChanged { .. } => PROCESSING_TASK_STATE_CHANGED,
            Self::ProcessingTaskIdentificationDecided { .. } => {
                PROCESSING_TASK_IDENTIFICATION_DECIDED
            }
            Self::TaskDecisionAccepted { .. } => TASK_DECISION_ACCEPTED,
            Self::CatalogMediaChanged { .. } => CATALOG_MEDIA_CHANGED,
            Self::OrganizationTargetChanged { .. } => ORGANIZATION_TARGET_CHANGED,
            Self::OrganizationResultChanged { .. } => ORGANIZATION_RESULT_CHANGED,
            Self::IntegrationHealthChanged { .. } => INTEGRATION_HEALTH_CHANGED,
            Self::DownloadTaskChanged { .. } => DOWNLOAD_TASK_CHANGED,
            Self::AutomationEventChanged { .. } => AUTOMATION_EVENT_CHANGED,
            Self::IdentificationEnhancerChanged { .. } => IDENTIFICATION_ENHANCER_CHANGED,
            Self::StreamGap { .. } => STREAM_GAP,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// 下载任务 outbox/SSE 使用的最小脱敏投影。
pub struct DownloadTaskChangedPayload {
    /// 单调递增的任务投影版本。
    pub projection_version: i64,
    /// 当前本地任务状态。
    pub status: DownloadTaskStatus,
    /// 已关联任务的规范化远端状态。
    pub remote_status: Option<RemoteDownloadStatus>,
    /// 0 到 10000 的完成度基点。
    pub progress_basis_points: u16,
    /// 失败或等待重试时的稳定脱敏分类。
    pub failure_code: Option<DownloaderFailureCode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// Minimal automation event outbox/SSE projection.
pub struct AutomationEventChangedPayload {
    pub automation_event_id: Uuid,
    pub projection_version: i64,
    pub status: AutomationEventStatus,
    pub action: AutomationAction,
    pub failure_code: Option<AutomationFailureCode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// Minimal refresh payload without endpoint, model, prompt, or response content.
pub struct IdentificationEnhancerChangedPayload {
    pub projection_version: i64,
    pub enabled: bool,
    pub health: IntegrationHealth,
    pub fallback_code: Option<AutomationFailureCode>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 正式媒体目录投影相对于上一版本的变化类别。
pub enum CatalogMediaChange {
    /// 首次创建该媒体项目投影。
    Created,
    /// 已存在媒体项目被新的已核对结果更新。
    Updated,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// Catalog 投影提交后用于客户端重新读取的最小事件负载。
pub struct CatalogMediaChangedPayload {
    /// 已创建或更新的正式媒体项目 UUID。
    pub media_item_id: Uuid,
    /// 账户目录投影的单调版本，可用于检测刷新顺序。
    pub projection_version: i64,
    /// 本次事务创建还是更新了媒体项目。
    pub change: CatalogMediaChange,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 整理目标聚合的变化类别。
pub enum OrganizationTargetChange {
    /// 首次创建。
    Created,
    /// 配置版本更新。
    Updated,
    /// 聚合删除。
    Deleted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// 整理目标变化的最小安全刷新负载。
pub struct OrganizationTargetChangedPayload {
    /// 目标稳定 UUID。
    pub target_id: Uuid,
    /// 变化对应的配置版本。
    pub config_version: i64,
    /// 创建、更新或删除。
    pub change: OrganizationTargetChange,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 本地整理结果事件允许的稳定状态集合。
pub enum OrganizationResultStatus {
    /// 文件可用但仍有后续动作。
    PartialSuccess,
    /// 当前计划的本地工作已完成。
    Completed,
    /// 已安全补偿。
    Compensated,
    /// 需要人工处理外部变化。
    ManualReview,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// 本地整理结果变化的最小安全刷新负载。
pub struct OrganizationResultChangedPayload {
    /// 本地结果稳定 UUID。
    pub result_id: Uuid,
    /// 产生该结果的 `ProcessingTask` UUID。
    pub processing_task_id: Uuid,
    /// 单调结果版本。
    pub version: i64,
    /// 当前结果状态。
    pub status: OrganizationResultStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 最小人工决定事件携带的稳定命令类别。
pub enum TaskDecisionEventKind {
    /// 选择并重新核验一个提供方候选。
    SelectProviderCandidate,
    /// 使用管理员修正线索重新匹配。
    RematchWithHints,
    /// 明确按普通视频处理。
    SelectGenericVideo,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// 一项已提交人工决定的最小公开投影。
pub struct TaskDecisionAcceptedPayload {
    /// 决定所消费的复核案例 UUID。
    pub case_id: Uuid,
    /// 不可变人工决定的 UUID。
    pub decision_id: Uuid,
    /// 已接受命令的稳定类别。
    pub kind: TaskDecisionEventKind,
    /// 接受决定时匹配的案例乐观并发版本。
    pub case_version: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// 一项已提交识别决策的最小公开投影。
pub struct ProcessingTaskIdentificationDecidedPayload {
    /// 新持久化识别决策的 UUID。
    pub decision_id: Uuid,
    /// 决策的结论等级。
    pub level: DecisionLevel,
    /// 面向客户端的主要脱敏原因。
    pub reason: ProcessingReason,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// TMDB 健康变化的最小脱敏事件投影。
pub struct IntegrationHealthChangedPayload {
    /// 集成的稳定类别；当前为 `tmdb`。
    pub kind: String,
    /// 最新权威连接检查得到的安全健康分类。
    pub health: IntegrationHealth,
    /// 可安全持久化和展示的稳定失败码。
    pub failure_code: Option<IntegrationFailureCode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// 处理生命周期事件携带的公开状态投影。
pub struct ProcessingTaskStateChangedPayload {
    /// 事务提交后的任务执行状态。
    pub status: ProcessingStatus,
    /// 当前面向用户的能力阶段。
    pub stage: ProcessingStage,
    /// 本次状态是否由租约或依赖恢复路径产生。
    pub recovering: bool,
    /// 解释暂停、等待或降级状态的稳定原因。
    pub reason: Option<ProcessingReason>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 收件箱 watcher/对账面向客户端的综合健康等级。
pub enum InboxDiscoveryHealth {
    /// watcher 或完整对账最近成功，最终正确性路径可用。
    Healthy,
    /// watcher 提示不可靠或对账失败，已安排恢复对账。
    Degraded,
    /// 能力根或收件箱当前不可安全访问。
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 收件箱健康变化的稳定脱敏原因。
pub enum InboxDiscoveryHealthReason {
    /// watcher 当前不可用或根能力失效。
    #[serde(rename = "watcher.unavailable")]
    WatcherUnavailable,
    /// 最近一次完整对账失败。
    #[serde(rename = "reconcile.failed")]
    ReconcileFailed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// 收件箱健康变化事件的有界公开负载。
pub struct InboxDiscoveryHealthChangedPayload {
    /// 健康投影所属收件箱。
    pub inbox_directory_id: Uuid,
    /// 综合健康等级。
    pub health: InboxDiscoveryHealth,
    /// 当前是否有活动 watcher。
    pub watcher_active: bool,
    /// 可选稳定失败原因。
    pub reason: Option<InboxDiscoveryHealthReason>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// 原子转换后立即可见的任务生命周期状态。
pub struct TaskStateChangedPayload {
    /// 新的持久化任务状态。
    pub status: ScanStatus,
    /// 此排队/运行任务是否在过期租约后恢复。
    pub recovering: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// 可安全恢复保留事件重放的游标。
pub struct StreamGapPayload {
    /// 持久化发件箱中仍存在的最旧事件 ID。
    pub minimum_available_id: i64,
}
