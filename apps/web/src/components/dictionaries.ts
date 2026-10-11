/**
 * 状态词表与事件类型共享字典（status-badge 规格 v1.0 §3 + copy-patterns §4）。
 *
 * 三端同源的词表在 Web 侧的唯一入口：扫描任务状态、capability 事件类型、通知 kind。
 * 标签不得出现同义变体；tone/shape 供徽章双编码（形状符 + 中文词）。
 * 语义权威：页面状态见 packages/ui-contract；状态词表见 ui-contract copy-templates word_lists。
 */
import type { ProcessingTask, ProcessingTaskOrganization, ScanTask } from "@mediaflow/api-client-ts";

/** 计划/ journal 的生成 schema 类型（客户端按 schema 逐名导出，这两个经组织投影索引访问取得）。 */
type OrganizationPlanOperation = NonNullable<ProcessingTaskOrganization["plan"]>["operation"];
type OrganizationJournalEntry = ProcessingTaskOrganization["journals"][number];

/** 徽章语义色调（status-badge 规格 tone 五值）。 */
export type BadgeTone = "success" | "warning" | "danger" | "info" | "neutral";

/** 一个状态词的完整展示定义：中文标签 + 色调 + 形状符（双编码）。 */
export interface BadgeEntry {
  label: string;
  tone: BadgeTone;
  shape: string;
}

/** 扫描任务状态词表（词表对齐 status-badge §2；cancelled 不在核心 12 词表，保留准确词）。 */
export const SCAN_STATUS_BADGES: Record<ScanTask["status"], BadgeEntry> = {
  queued: { label: "排队", tone: "neutral", shape: "○" },
  running: { label: "运行中", tone: "info", shape: "◌" },
  "partial-success": { label: "部分成功", tone: "warning", shape: "◐" },
  completed: { label: "已完成", tone: "success", shape: "◆" },
  failed: { label: "失败", tone: "danger", shape: "▲" },
  cancelled: { label: "已取消", tone: "neutral", shape: "○" },
};

/**
 * capability 事件类型词表（11 类；S-3 债收口：AiSessionPanel 与 CapabilityEventsView 共用）。
 * tone 映射按 status-badge §3：完成/推进类 → info；失败类 → danger；等待审批/输入 → warning。
 */
export const CAPABILITY_EVENT_TYPE_BADGES: Record<string, BadgeEntry> = {
  "task.accepted": { label: "任务已接受", tone: "info", shape: "◌" },
  "task.progressed": { label: "任务推进", tone: "info", shape: "◌" },
  "task.waiting_input": { label: "等待输入", tone: "warning", shape: "◐" },
  "task.waiting_approval": { label: "等待审批", tone: "warning", shape: "◐" },
  "capability.completed": { label: "能力完成", tone: "success", shape: "◆" },
  "capability.failed": { label: "能力失败", tone: "danger", shape: "▲" },
  "task.retryable": { label: "可重试", tone: "warning", shape: "◐" },
  "task.paused": { label: "已暂停", tone: "neutral", shape: "○" },
  "task.succeeded": { label: "任务成功", tone: "success", shape: "◆" },
  "task.failed": { label: "任务失败", tone: "danger", shape: "▲" },
  "external_sync.retryable": { label: "外部同步可重试", tone: "warning", shape: "◐" },
};

/** capability 事件类型的徽章定义；未知类型回退为中性原文展示。 */
export function capabilityEventBadge(eventType: string): BadgeEntry {
  return CAPABILITY_EVENT_TYPE_BADGES[eventType] ?? { label: eventType, tone: "neutral", shape: "○" };
}

/** capability 调用结果状态词表（事件投影的 state 字段；词表对齐 status-badge §2）。 */
const CAPABILITY_STATE_BADGES: Record<string, BadgeEntry> = {
  succeeded: { label: "成功", tone: "success", shape: "◆" },
  failed: { label: "失败", tone: "danger", shape: "▲" },
  waiting_approval: { label: "等待审批", tone: "warning", shape: "◐" },
  needs_approval: { label: "等待审批", tone: "warning", shape: "◐" },
  paused: { label: "已暂停", tone: "neutral", shape: "○" },
};

/** capability 调用状态的徽章定义；未知状态回退为中性原文展示。 */
export function capabilityStateBadge(state: string): BadgeEntry {
  return CAPABILITY_STATE_BADGES[state] ?? { label: state, tone: "neutral", shape: "○" };
}

/** 处理任务状态词表（任务中心列表/详情；值域与筛选枚举一致，词表对齐 status-badge §2）。 */
const PROCESSING_STATUS_BADGES: Record<string, BadgeEntry> = {
  queued: { label: "排队", tone: "neutral", shape: "○" },
  running: { label: "运行中", tone: "info", shape: "◌" },
  "waiting-confirmation": { label: "待确认", tone: "warning", shape: "◐" },
  paused: { label: "已暂停", tone: "neutral", shape: "○" },
  "partial-success": { label: "部分成功", tone: "warning", shape: "◐" },
  completed: { label: "已完成", tone: "success", shape: "◆" },
  failed: { label: "失败", tone: "danger", shape: "▲" },
  cancelled: { label: "已取消", tone: "neutral", shape: "○" },
};

/** 处理任务状态的徽章定义；未知状态回退为中性原文展示。 */
export function processingStatusBadge(status: string): BadgeEntry {
  return PROCESSING_STATUS_BADGES[status] ?? { label: status, tone: "neutral", shape: "○" };
}

/** 通知 kind 词表（notification 规格 §2；新 kind 须同步本字典）。 */
export const NOTIFICATION_KIND_LABELS: Record<string, string> = {
  "review.waiting": "待审确认",
  "organization.paused": "整理已暂停",
  "organization.rolled-back": "整理已回滚",
  "system.notice": "系统通知",
};

/** 通知 kind 的中文标签；未知 kind 原文展示。 */
export function notificationKindLabel(kind: string): string {
  return NOTIFICATION_KIND_LABELS[kind] ?? kind;
}

/** 整理计划固定操作词表（plan-preview 规格 §2：操作类型如实展示，硬链接不得表述为移动）。 */
export const ORGANIZATION_OPERATION_LABELS: Record<OrganizationPlanOperation, string> = {
  move: "移动",
  copy: "复制",
  hardlink: "硬链接",
};

/** 整理 journal 操作类型词表（含执行期步骤：来源清理与 NFO 写入）。 */
export const ORGANIZATION_JOURNAL_KIND_LABELS: Record<OrganizationJournalEntry["kind"], string> = {
  move: "移动",
  copy: "复制",
  hardlink: "硬链接",
  "source-removal": "来源清理",
  nfo: "NFO 写入",
};

/** 整理 journal 状态词表（task-timeline 规格：每行结果标记）。 */
export const ORGANIZATION_JOURNAL_STATUS_LABELS: Record<OrganizationJournalEntry["status"], string> = {
  prepared: "已准备",
  executing: "执行中",
  applied: "已应用",
  verified: "已验证",
  compensated: "已补偿",
  "manual-review": "待人工复核",
};

/** 处理任务阶段词表（task-timeline 规格：识别→计划→文件操作→NFO→完成）。 */
export const PROCESSING_STAGE_LABELS: Record<ProcessingTask["stage"], string> = {
  identification: "识别",
  planning: "计划",
  "file-operation": "文件操作",
  nfo: "NFO",
  completion: "完成",
};
