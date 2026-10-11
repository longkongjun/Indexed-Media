// 共享字典完整性：词表值域、tone 域、双编码完备（status-badge 规格 + copy-patterns 词表）。
import { describe, expect, it } from "vitest";
import {
  CAPABILITY_EVENT_TYPE_BADGES,
  NOTIFICATION_KIND_LABELS,
  SCAN_STATUS_BADGES,
  capabilityEventBadge,
  capabilityStateBadge,
  notificationKindLabel,
  processingStatusBadge,
  type BadgeEntry,
} from "../src/components/dictionaries";

const TONES = new Set(["success", "warning", "danger", "info", "neutral"]);

function expectValidEntry(entry: BadgeEntry): void {
  expect(entry.label.length).toBeGreaterThan(0);
  expect(TONES.has(entry.tone)).toBe(true);
  expect(entry.shape.length).toBeGreaterThan(0);
}

describe("共享状态字典", () => {
  it("扫描任务状态：六态全覆盖且词表对齐（排队/运行中/部分成功…）", () => {
    const entries = Object.values(SCAN_STATUS_BADGES);
    expect(Object.keys(SCAN_STATUS_BADGES).sort()).toEqual(["cancelled", "completed", "failed", "partial-success", "queued", "running"]);
    for (const entry of entries) expectValidEntry(entry);
    expect(SCAN_STATUS_BADGES.queued.label).toBe("排队");
    expect(SCAN_STATUS_BADGES.running.label).toBe("运行中");
    expect(SCAN_STATUS_BADGES.failed.tone).toBe("danger");
  });

  it("capability 事件类型：11 类全覆盖，tone 映射符合规格（完成 info/success、失败 danger、审批 warning）", () => {
    expect(Object.keys(CAPABILITY_EVENT_TYPE_BADGES)).toHaveLength(11);
    for (const entry of Object.values(CAPABILITY_EVENT_TYPE_BADGES)) expectValidEntry(entry);
    expect(CAPABILITY_EVENT_TYPE_BADGES["task.failed"]!.tone).toBe("danger");
    expect(CAPABILITY_EVENT_TYPE_BADGES["task.waiting_approval"]!.tone).toBe("warning");
    expect(capabilityEventBadge("unknown.event").label).toBe("unknown.event");
  });

  it("capability 调用状态与处理任务状态：已知值域映射 + 未知回退原文", () => {
    expect(capabilityStateBadge("succeeded").label).toBe("成功");
    expect(capabilityStateBadge("waiting_approval").tone).toBe("warning");
    expect(processingStatusBadge("waiting-confirmation").label).toBe("待确认");
    expect(processingStatusBadge("never-seen").label).toBe("never-seen");
  });

  it("通知 kind 词表：待审确认为 review.waiting（notification 规格）", () => {
    expect(NOTIFICATION_KIND_LABELS["review.waiting"]).toBe("待审确认");
    expect(notificationKindLabel("future.kind")).toBe("future.kind");
  });
});
