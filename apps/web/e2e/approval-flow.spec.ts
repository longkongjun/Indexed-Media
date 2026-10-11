import { expect, test, type Page } from "@playwright/test";
import { m2Fixtures } from "../../../packages/test-fixtures/src/index.ts";

/**
 * 审批链端到端旅程（W3 出口验收，approval-flow 模式 ③-⑦）：
 * needs_approval 横幅 → 计划全影响预览 → 二次确认批准（Web 人工授权，唯一授权产生方）
 * → 时间线留痕 → 会话深链入口与导航未读角标。
 * 会话内不出现任何批准/拒绝按钮（分工红线）。
 */

const { session } = m2Fixtures;
const taskId = "019f0000-0000-7000-8000-000000000110";
const planId = "019f0000-0000-7000-8000-000000000111";
const targetId = "019f0000-0000-7000-8000-000000000112";

const pausedTask = {
  id: taskId,
  inbox_directory_id: "019f0000-0000-7000-8000-000000000120",
  file_revision_id: "019f0000-0000-7000-8000-000000000121",
  relative_path: "incoming/Blade Runner 2049.mkv",
  status: "paused",
  stage: "planning",
  checkpoint: "planning-paused",
  decision_checkpoint: "planning-requested",
  current_task_decision_id: null,
  reason: "organization.plan-paused",
  recovering: false,
  attempt_count: 1,
  next_retry_at: null,
  allowed_actions: ["cancel"],
  updated_at: "2026-10-11T10:00:00Z",
};

const pausedProjection = {
  task_id: taskId,
  state: "paused",
  plan: {
    id: planId,
    version: 2,
    target_id: targetId,
    source: { root_id: "incoming", relative_path: "Blade Runner 2049.mkv" },
    destination: { root_id: "library", relative_path: "Movies/Blade Runner 2049 (2017)/Blade Runner 2049 (2017).mkv" },
    operation: "copy",
    naming: "Blade Runner 2049 (2017)",
    authorization: "paused",
    risk_codes: ["organization.rule-not-matched"],
    created_at: "2026-10-11T10:00:00Z",
  },
  journals: [],
  local_result: null,
  allowed_actions: ["recalculate", "execute", "retry", "cancel"],
};

const executedProjection = {
  task_id: taskId,
  state: "partial-success",
  plan: { ...pausedProjection.plan, authorization: "one-time" },
  journals: [
    {
      id: "019f0000-0000-7000-8000-000000000131",
      operation_id: "019f0000-0000-7000-8000-000000000132",
      kind: "copy",
      status: "verified",
      source: pausedProjection.plan.source,
      destination: pausedProjection.plan.destination,
      projection_version: 3,
      updated_at: "2026-10-11T10:05:00Z",
    },
  ],
  local_result: {
    id: "019f0000-0000-7000-8000-000000000133",
    version: 3,
    status: "partial-success",
    nfo_status: "failed",
    catalog_media_item_id: null,
    remaining_actions: ["nfo"],
    updated_at: "2026-10-11T10:06:00Z",
  },
  allowed_actions: ["retry", "rollback"],
};

const executedTask = { ...pausedTask, status: "partial-success", stage: "nfo", checkpoint: "nfo-failed", allowed_actions: ["retry", "cancel"] };

const capabilityEvents = {
  items: [
    {
      sequence: 41,
      event_id: "019f0000-0000-7000-8000-000000000141",
      event_type: "task.waiting_approval",
      occurred_at: "2026-10-11T10:01:00Z",
      task_id: taskId,
      request_id: "019f0000-0000-7000-8000-000000000142",
      state: "needs_approval",
      error_code: null,
    },
  ],
  next_cursor: null,
};

interface BrowserEventSourceController {
  emit(type: string, data: string): void;
  fail(): void;
}

async function installControlledSameOriginEventSource(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const sources: ControlledEventSource[] = [];

    class ControlledEventSource {
      private readonly listeners = new Map<string, Set<EventListener>>();
      private closed = false;

      constructor(readonly url: string) {
        if (url !== "/api/v1/events") throw new Error("SSE must stay same-origin");
        sources.push(this);
        setTimeout(() => this.dispatch("open", new Event("open")), 0);
      }

      addEventListener(type: string, listener: EventListener): void {
        const group = this.listeners.get(type) ?? new Set<EventListener>();
        group.add(listener);
        this.listeners.set(type, group);
      }

      removeEventListener(type: string, listener: EventListener): void {
        this.listeners.get(type)?.delete(listener);
      }

      close(): void {
        this.closed = true;
      }

      dispatch(type: string, event: Event): void {
        if (this.closed) return;
        for (const listener of this.listeners.get(type) ?? []) listener(event);
      }
    }

    const controller: BrowserEventSourceController = {
      emit(type: string) {
        for (const source of sources) source.dispatch(type, new MessageEvent(type));
      },
      fail() {
        for (const source of sources) source.dispatch("error", new Event("error"));
      },
    };

    Object.defineProperty(window, "EventSource", { configurable: true, value: ControlledEventSource });
    Object.defineProperty(window, "__mediaflowApprovalEvents", { configurable: true, value: controller });
  });
}

test("needs_approval banner to authorized execution leaves a verifiable timeline trail", async ({ page }) => {
  await installControlledSameOriginEventSource(page);
  let executed = false;
  const executions: Array<{ key: string | undefined; body: unknown }> = [];

  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === "/api/v1/capability-events") return json(capabilityEvents);
    if (path === "/api/v1/notifications") return json({ items: [], next_cursor: null, unread_count: 2 });
    if (path === `/api/v1/processing-tasks/${taskId}`) return json(executed ? executedTask : pausedTask);
    if (path === `/api/v1/processing-tasks/${taskId}/organization`) return json(executed ? executedProjection : pausedProjection);
    if (path === `/api/v1/processing-tasks/${taskId}/organization/executions`) {
      executions.push({ key: request.headers()["idempotency-key"], body: request.postDataJSON() });
      executed = true;
      return json(executedProjection);
    }
    return route.fulfill({ status: 404, body: "" });
  });

  // ③ 会话：等待审批事件升级为审批深链入口（无任何批准按钮）。
  await page.goto("/");
  await expect(page.getByText("AI 摘要，以任务详情页为准")).toBeVisible();
  const approvalEntry = page.locator("[data-approval-entry]");
  await expect(approvalEntry).toBeVisible();
  await expect(approvalEntry).toContainText("等待审批");
  await expect(approvalEntry).toContainText("到任务审批");
  // 导航未读角标：宽屏在侧栏；窄屏先展开"更多"面板。
  if ((page.viewportSize()?.width ?? 0) < 768) {
    await page.getByRole("button", { name: "更多" }).click();
  }
  await expect(page.locator(".nav-badge:visible").first()).toBeVisible();

  // ④ 横幅 → 全影响预览。
  await approvalEntry.click();
  await expect(page.getByRole("heading", { name: "处理任务详情" })).toBeVisible();
  const banner = page.getByRole("alert").filter({ hasText: "1 个整理计划等待审批" });
  await expect(banner).toBeVisible();
  await banner.getByRole("button", { name: "查看完整计划并审批" }).click();
  const preview = page.locator("[data-plan-preview]");
  await expect(preview).toBeVisible();
  await expect(preview).toContainText("Blade Runner 2049.mkv");
  await expect(preview).toContainText("Movies/Blade Runner 2049 (2017)/Blade Runner 2049 (2017).mkv");
  await expect(preview).toContainText("风险提示");
  // 时间线：计划节点 ⚠ 等待审批。
  await expect(page.locator("[data-timeline-state='attention']").filter({ hasText: "计划" })).toContainText("等待审批");

  // ⑤ 二次确认批准：焦点默认在"取消"（防误批），版本绑定 plan_version=2。
  await preview.getByRole("button", { name: "批准并执行计划 v2" }).click();
  const dialog = page.getByRole("alertdialog", { name: "确认批准并执行" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("button", { name: "取消" })).toBeFocused();
  await dialog.getByRole("button", { name: "确认执行计划 v2" }).click();

  // ⑥ 执行留痕：journal 行、时间线推进、结果可核对。
  await expect(page.getByText(/重试只继续这些步骤/)).toBeVisible();
  await expect(page.locator("[data-plan-preview]")).toContainText("已验证");
  expect(executions).toHaveLength(1);
  expect(executions[0]?.key).toMatch(/^[0-9a-f-]{36}$/i);
  expect(executions[0]?.body).toEqual({ plan_version: 2 });
  await expect(page.locator("[data-timeline-state='attention']").filter({ hasText: "NFO" })).toContainText("失败");
  // 批准后横幅消失（计划不再等待审批）。
  await expect(banner).toBeHidden();

  // 分工红线：会话面板内无任何批准/拒绝按钮。
  await expect(page.locator(".ai-session-panel").getByRole("button", { name: /批准|拒绝/ })).toHaveCount(0);
});
