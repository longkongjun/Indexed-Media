//! `media-capability.v1` 能力协议的 Rust 信封类型。
//!
//! 本模块只定义 AI 控制面（DSH）与 Core 之间的四类消息信封与事件分页：能力调用、能力
//! 结果、结构化错误和能力事件。JSON Schema（`contracts/capability/v1/`）是唯一事实源；
//! 本模块类型与其逐字段对齐（`deny_unknown_fields` 对应 `additionalProperties: false`），
//! 双端契约测试回放 golden 示例防漂移。
//!
//! 信封层的 `input` 与 `result` 是受具体 capability 约束的自由对象：网关在分发前必须按
//! 各能力的输入 schema 再次校验，不信任调用方已完成校验。能力专属子 schema 随各能力
//! 实现逐个冻结，不在本模块固化。
//!
//! KMP 可消费性约束：字段命名 camelCase、可空性显式、判别使用常量字符串字段；本类型
//! 的序列化形态必须保持 `kotlinx.serialization` 可直接映射。

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

/// 协议版本常量；不随模型版本隐式变化。
pub const PROTOCOL_VERSION: &str = "media-capability.v1";

/// 反序列化时强制 `protocol_version` 等于当前协议版本；未知版本稳定拒绝。
fn deserialize_protocol_version<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value != PROTOCOL_VERSION {
        return Err(serde::de::Error::custom(format!(
            "unknown protocol version: {value} (expected {PROTOCOL_VERSION})"
        )));
    }
    Ok(value)
}

/// 已登记的业务能力名；调用只能指向该清单内的能力。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CapabilityName {
    #[serde(rename = "media.scan")]
    MediaScan,
    #[serde(rename = "media.task-status")]
    MediaTaskStatus,
    #[serde(rename = "media.match-candidates")]
    MediaMatchCandidates,
    #[serde(rename = "media.create-plan")]
    MediaCreatePlan,
    #[serde(rename = "media.request-approval")]
    MediaRequestApproval,
    #[serde(rename = "media.apply-plan")]
    MediaApplyPlan,
    #[serde(rename = "media.retry")]
    MediaRetry,
    #[serde(rename = "media.cancel")]
    MediaCancel,
    #[serde(rename = "media.rollback")]
    MediaRollback,
    #[serde(rename = "media.sync-external")]
    MediaSyncExternal,
}

impl CapabilityName {
    /// 返回线上的稳定能力名。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MediaScan => "media.scan",
            Self::MediaTaskStatus => "media.task-status",
            Self::MediaMatchCandidates => "media.match-candidates",
            Self::MediaCreatePlan => "media.create-plan",
            Self::MediaRequestApproval => "media.request-approval",
            Self::MediaApplyPlan => "media.apply-plan",
            Self::MediaRetry => "media.retry",
            Self::MediaCancel => "media.cancel",
            Self::MediaRollback => "media.rollback",
            Self::MediaSyncExternal => "media.sync-external",
        }
    }
}

/// 调用触发者身份；记录 DSH、人工用户或系统。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct CapabilityActor {
    /// 触发者类别。
    pub kind: CapabilityActorKind,
    /// 触发者稳定标识。
    pub id: String,
}

/// 触发者类别枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityActorKind {
    /// AI 控制面。
    Ai,
    /// 人工用户。
    User,
    /// 系统调度。
    System,
}

/// 执行约束：副作用等级、幂等键、期望版本与截止时间。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct CapabilityExecution {
    /// 副作用等级；A1 阶段网关只接受只读与计划模式。
    pub mode: CapabilityExecutionMode,
    /// 同一请求重放不得产生第二次副作用。
    pub idempotency_key: String,
    /// 调用方所依据的事实版本；过期时 Core 稳定拒绝。
    pub expected_revision: Option<String>,
    /// 请求截止毫秒数；阻止无限等待与隐式后台执行。
    pub deadline_ms: i64,
}

/// 副作用等级枚举；第一阶段不开放不可逆写入。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityExecutionMode {
    /// 只读。
    ReadOnly,
    /// 仅生成计划。
    PlanOnly,
    /// 需要人工审批。
    ApprovalRequired,
    /// 可逆写入（A3 起开放）。
    ReversibleWrite,
}

/// AI 控制面发起的能力调用信封。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct CapabilityCall {
    /// 协议版本；反序列化时强制等于当前版本。
    #[serde(deserialize_with = "deserialize_protocol_version")]
    pub protocol_version: String,
    /// 跨重试与重启的请求关联键。
    pub request_id: Uuid,
    /// 可选的 DSH 会话关联。
    pub session_id: Option<Uuid>,
    /// 可选的任务关联。
    pub task_id: Option<Uuid>,
    /// 目标能力。
    pub capability: CapabilityName,
    /// 触发者。
    pub actor: CapabilityActor,
    /// 受能力输入 schema 约束的自由对象。
    pub input: Map<String, Value>,
    /// 执行约束。
    pub execution: CapabilityExecution,
}

/// 能力结果状态；AI 只能依据该结构化枚举选择下一步。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityResultStatus {
    /// 已成功完成。
    Succeeded,
    /// 请求已接受，异步推进。
    Accepted,
    /// 等待补充输入。
    NeedsInput,
    /// 等待人工审批。
    NeedsApproval,
    /// 可安全重试。
    Retryable,
    /// 安全暂停。
    Paused,
    /// 稳定拒绝。
    Rejected,
    /// 最终失败。
    Failed,
    /// 已取消。
    Cancelled,
}

/// 结果信封中的证据引用。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct CapabilityEvidenceRef {
    /// 稳定证据标识。
    pub id: String,
    /// 证据类别。
    pub kind: CapabilityEvidenceKind,
}

impl CapabilityEvidenceRef {
    /// 事件 `evidence_refs` 使用的 `kind:id` 引用前缀。
    #[must_use]
    pub fn kind_id_prefix(&self) -> &'static str {
        match self.kind {
            CapabilityEvidenceKind::Candidate => "candidate",
            CapabilityEvidenceKind::Plan => "plan",
            CapabilityEvidenceKind::Decision => "decision",
            CapabilityEvidenceKind::Journal => "journal",
            CapabilityEvidenceKind::Task => "task",
            CapabilityEvidenceKind::ReviewCase => "review-case",
            CapabilityEvidenceKind::MediaItem => "media-item",
        }
    }
}

/// 证据类别枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityEvidenceKind {
    /// 识别候选。
    Candidate,
    /// 整理计划。
    Plan,
    /// 人工或确定性决定。
    Decision,
    /// 文件操作 journal。
    Journal,
    /// 处理任务。
    Task,
    /// 待确认记录。
    ReviewCase,
    /// 正式媒体记录。
    MediaItem,
}

/// AI 被允许选择的下一步动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityNextAction {
    /// 询问用户。
    AskUser,
    /// 重试。
    Retry,
    /// 重新规划。
    Replan,
    /// 请求审批。
    RequestApproval,
    /// 取消。
    Cancel,
    /// 停止并报告。
    StopAndReport,
}

/// Core 对能力调用的结果信封。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct CapabilityResult {
    /// 协议版本；反序列化时强制等于当前版本。
    #[serde(deserialize_with = "deserialize_protocol_version")]
    pub protocol_version: String,
    /// 对应调用的请求关联键。
    pub request_id: Uuid,
    /// 可选的任务关联。
    pub task_id: Option<Uuid>,
    /// 结构化结果状态。
    pub status: CapabilityResultStatus,
    /// 受能力结果 schema 约束的自由对象。
    pub result: Option<Map<String, Value>>,
    /// 支撑结论的证据引用。
    pub evidence: Vec<CapabilityEvidenceRef>,
    /// 允许的下一步动作。
    pub next_actions: Vec<CapabilityNextAction>,
    /// 失败或拒绝时的结构化错误。
    pub error: Option<CapabilityError>,
}

/// 结构化错误的机器分类。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityErrorCategory {
    /// 输入校验失败。
    Validation,
    /// 权限不足。
    Permission,
    /// 事实冲突（如过期 revision）。
    Conflict,
    /// 前置条件不满足。
    Precondition,
    /// 目标不存在。
    NotFound,
    /// 外部 provider 失败。
    Provider,
    /// 暂时性故障。
    Transient,
    /// 内部一致性损坏。
    Consistency,
    /// 能力不支持。
    Unsupported,
    /// 未分类内部错误。
    Internal,
}

/// 错误发生阶段。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityErrorPhase {
    /// 扫描。
    Scan,
    /// 识别。
    Identification,
    /// 计划。
    Planning,
    /// 文件执行。
    Execution,
    /// 外部同步。
    Sync,
}

/// 错误要求的补救动作；AI 必须重新经能力网关发起，不能绕过校验。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRequiredAction {
    /// 询问用户。
    AskUser,
    /// 重新规划。
    Replan,
    /// 重试。
    Retry,
    /// 回滚。
    Rollback,
    /// 无需动作。
    None,
}

/// Core 反馈给 AI 的结构化错误；不携带凭据、完整敏感路径或原始模型输出。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct CapabilityError {
    /// 稳定机器错误码。
    pub error_code: String,
    /// 机器分类。
    pub category: CapabilityErrorCategory,
    /// Core 已证明重试不会重复副作用时才为真。
    pub retryable: bool,
    /// 发生阶段。
    pub phase: CapabilityErrorPhase,
    /// 可选的任务关联。
    pub task_id: Option<Uuid>,
    /// 可选的文件操作关联。
    pub operation_id: Option<String>,
    /// 要求的补救动作。
    pub required_action: CapabilityRequiredAction,
    /// 脱敏诊断信息。
    pub safe_details: Map<String, Value>,
    /// 可追溯的输入、候选或 journal 引用。
    pub evidence_refs: Vec<String>,
}

/// Core 反馈给 AI 的事实事件类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CapabilityEventType {
    /// 任务已接受。
    #[serde(rename = "task.accepted")]
    TaskAccepted,
    /// 任务推进。
    #[serde(rename = "task.progressed")]
    TaskProgressed,
    /// 任务等待输入。
    #[serde(rename = "task.waiting_input")]
    TaskWaitingInput,
    /// 任务等待审批。
    #[serde(rename = "task.waiting_approval")]
    TaskWaitingApproval,
    /// 能力调用完成。
    #[serde(rename = "capability.completed")]
    CapabilityCompleted,
    /// 能力调用失败。
    #[serde(rename = "capability.failed")]
    CapabilityFailed,
    /// 任务可重试。
    #[serde(rename = "task.retryable")]
    TaskRetryable,
    /// 任务暂停。
    #[serde(rename = "task.paused")]
    TaskPaused,
    /// 任务成功。
    #[serde(rename = "task.succeeded")]
    TaskSucceeded,
    /// 任务失败。
    #[serde(rename = "task.failed")]
    TaskFailed,
    /// 外部同步可重试；本地结果不受影响。
    #[serde(rename = "external_sync.retryable")]
    ExternalSyncRetryable,
}

/// Core 反馈给 AI 的事实事件信封；按 sequence 单调，断线按 cursor 补发。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct CapabilityEvent {
    /// 事件稳定标识。
    pub event_id: Uuid,
    /// 单调递增序号；AI 按其去重。
    pub sequence: i64,
    /// 事件类型。
    pub event_type: CapabilityEventType,
    /// 事件时间。
    pub occurred_at: String,
    /// 可选的任务关联。
    pub task_id: Option<Uuid>,
    /// 可选的请求关联。
    pub request_id: Option<Uuid>,
    /// 可选的文件操作关联。
    pub operation_id: Option<String>,
    /// 可选的任务状态快照。
    pub state: Option<String>,
    /// 可选的进度信息。
    pub progress: Option<Map<String, Value>>,
    /// 可选的证据引用。
    pub evidence_refs: Option<Vec<String>>,
    /// 可选的结构化错误。
    pub error: Option<CapabilityError>,
}

/// AI 按事件 cursor 拉取的有序事件页。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct CapabilityEventPage {
    /// 按 sequence 递增的事件列表。
    pub items: Vec<CapabilityEvent>,
    /// 后续拉取游标；`None` 表示暂无更多事件。
    pub next_cursor: Option<String>,
}
