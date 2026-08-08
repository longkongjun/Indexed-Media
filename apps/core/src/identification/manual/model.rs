use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::identification::model::MediaKind;
use crate::tasks::processing::model::DecisionDispatch;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// 管理员提供的有界结构化身份线索。
pub struct ManualIdentityHint {
    /// 管理员期望重新匹配的媒体类别。
    pub media_kind: MediaKind,
    /// 管理员提供的标题原值；请求边界只做有界文本校验（trim 后非空、拒绝控制字符并限制
    /// 长度），不会改写或规范化标题，决策比较时才执行规范化。
    pub normalized_title: String,
    /// 可选发行或首播年份。
    pub year: Option<u16>,
    /// 剧集身份的季号；电影或未指定时为 `None`。
    pub season: Option<u16>,
    /// 剧集身份的有界集号集合。
    pub episodes: Vec<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一次有界人工重识别所信任的来源上下文。
///
/// 依赖提供方的变体只携带身份选择器，不接受候选展示字段；执行时必须通过已配置提供方
/// 重新获取并核验这些字段。
pub enum ManualIdentificationContext {
    /// 重新核验管理员从当前候选中选择的提供方身份。
    SelectedProvider {
        /// 接受人工决定前的不可变识别决策 UUID。
        decision_id: Uuid,
        /// 管理员选择的媒体类别。
        media_kind: MediaKind,
        /// 需要从提供方重新获取的实体 ID。
        provider_id: String,
    },
    /// 使用管理员修正后的结构化线索重新搜索。
    Rematch {
        /// 触发本次修正的原识别决策 UUID。
        decision_id: Uuid,
        /// 经过请求校验的替代身份线索。
        hint: ManualIdentityHint,
    },
    /// 复用与当前账户、媒体类型、标题、年份及季/集选择器精确匹配且不存在提供方冲突的启用反馈。
    ExactFeedback {
        /// 提供本次身份选择的反馈记录 UUID。
        feedback_id: Uuid,
        /// 反馈中保存且仍需重新核验的提供方 ID。
        provider_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
/// 识别边界接受的穷尽性人工命令。
pub enum ManualDecisionInput {
    /// 从候选中选择一个提供方实体，并可选择为当前本地身份选择器保存精确反馈。
    SelectProviderCandidate {
        /// 所选候选的媒体类别。
        media_kind: MediaKind,
        /// 所选候选的提供方实体 ID；服务端会重新获取其内容。
        provider_id: String,
        /// 是否为当前媒体类型、标题、年份及季/集选择器保存可复用反馈。
        save_feedback: bool,
    },
    /// 使用管理员修正的线索重新执行提供方匹配。
    RematchWithHints {
        /// 替代本地解析结果的结构化身份线索。
        hint: ManualIdentityHint,
        /// 保留调用方的反馈保存意图；当前实现不会为此命令持久化反馈。
        save_feedback: bool,
    },
    /// 明确把文件作为普通视频处理，不创建电影或剧集身份。
    SelectGenericVideo {
        /// 普通视频在本地目录中的展示标题。
        display_title: String,
        /// 可选的本地分组线索；不触发提供方匹配。
        group_hint: Option<String>,
        /// 保留调用方的分组反馈保存意图；当前实现不会持久化分组反馈。
        save_grouping_feedback: bool,
    },
}

impl ManualDecisionInput {
    #[must_use]
    /// 返回该命令用于持久化和事件投影的稳定类别。
    pub const fn kind(&self) -> ManualDecisionKind {
        match self {
            Self::SelectProviderCandidate { .. } => ManualDecisionKind::SelectProviderCandidate,
            Self::RematchWithHints { .. } => ManualDecisionKind::RematchWithHints,
            Self::SelectGenericVideo { .. } => ManualDecisionKind::SelectGenericVideo,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 一项不可变人工决定的稳定类别。
pub enum ManualDecisionKind {
    /// 管理员选择并要求重新核验一个提供方候选。
    SelectProviderCandidate,
    /// 管理员提供修正线索并要求重新匹配。
    RematchWithHints,
    /// 管理员确认按普通视频处理。
    SelectGenericVideo,
}

impl ManualDecisionKind {
    #[must_use]
    /// 返回用于持久化和线协议的稳定 kebab-case 值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SelectProviderCandidate => "select-provider-candidate",
            Self::RematchWithHints => "rematch-with-hints",
            Self::SelectGenericVideo => "select-generic-video",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "select-provider-candidate" => Some(Self::SelectProviderCandidate),
            "rematch-with-hints" => Some(Self::RematchWithHints),
            "select-generic-video" => Some(Self::SelectGenericVideo),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 不可变 `TaskDecision` 对应的、独立可变执行状态。
pub enum TaskDecisionState {
    /// 决定已持久化，等待下游协调器领取。
    Accepted,
    /// 决定已经由下游工作流成功应用。
    Applied,
    /// 决定已被领取，但下游应用最终失败。
    Failed,
}

impl TaskDecisionState {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "accepted" => Some(Self::Accepted),
            "applied" => Some(Self::Applied),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 服务端根据活动 `ReviewCase` 状态推导出的当前允许操作。
pub enum ReviewAction {
    /// 允许选择并重新核验提供方候选。
    SelectProviderCandidate,
    /// 允许提交修正线索重新匹配。
    RematchWithHints,
    /// 允许明确按普通视频处理。
    SelectGenericVideo,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 最近一次已接受不可变人工决定的安全摘要。
pub struct TaskDecisionSummary {
    /// 人工决定的稳定 UUID。
    pub id: Uuid,
    /// 决定采用的命令类别。
    pub kind: ManualDecisionKind,
    /// 决定的当前下游应用状态。
    pub state: TaskDecisionState,
    /// 决定首次持久化的 UTC RFC3339 时间。
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 首次接受和精确幂等重放都会返回的持久回执。
pub struct AcceptedTaskDecision {
    /// 已接受人工决定的稳定 UUID。
    pub id: Uuid,
    /// 该决定绑定的处理任务 UUID。
    pub task_id: Uuid,
    #[serde(rename = "review_case_id")]
    /// 决定所消费的复核案例 UUID。
    pub case_id: Uuid,
    /// 接受决定时匹配的案例乐观并发版本。
    pub case_version: i64,
    /// 已持久化命令的稳定类别。
    pub kind: ManualDecisionKind,
    /// 当前下游应用状态；首次接受通常为 `Accepted`。
    pub state: TaskDecisionState,
    /// 首次接受该决定的 UTC RFC3339 时间。
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 已接受且可供有界下游协调器领取的一项识别决定。
pub struct PendingDecisionDispatch {
    /// 决定所属账户，用于保持后续读取与写入隔离。
    pub account_id: Uuid,
    /// 协调器恢复任务所需的确定性派发内容。
    pub dispatch: DecisionDispatch,
}
