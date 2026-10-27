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
    const violations: string[] = [];
    document.addEventListener("securitypolicyviolation", (event) => violations.push(event.effectiveDirective));
    Object.defineProperty(window, "__approvalCspViolations", { configurable: true, value: violations });
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
      emit(type: string, data: string) {
        for (const source of sources) source.dispatch(type, new MessageEvent(type, { data }));
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
  await expect(dialog.getByRole("button", { name: /^取\s*消$/ })).toBeFocused();
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


test("formal approval and rejection modals preserve focus, themes, busy state and failed version context", async ({ page }) => {
  await installControlledSameOriginEventSource(page);
  let version = 2;
  let executionCount = 0;
  let ambiguous = false;
  let projectionUnavailable = false;
  let cancellationCount = 0;
  let releaseExecution: (() => void) | undefined;
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === "/api/v1/notifications") return json({ items: [], next_cursor: null, unread_count: 0 });
    if (path === "/api/v1/capability-events") return json({ items: [], next_cursor: null });
    if (path === `/api/v1/processing-tasks/${taskId}`) return json(pausedTask);
    if (path === `/api/v1/processing-tasks/${taskId}/organization`) {
      if (projectionUnavailable) return route.abort("failed");
      return json({ ...pausedProjection, plan: { ...pausedProjection.plan, version } });
    }
    if (path === `/api/v1/processing-tasks/${taskId}/organization/executions`) {
      executionCount += 1;
      if (ambiguous) return route.abort("failed");
      await new Promise<void>((resolve) => { releaseExecution = resolve; });
      return route.fulfill({ status: 409, contentType: "application/json", body: JSON.stringify({ error: { code: "organization.target-exists", message: "conflict", request_id: taskId } }) });
    }
    if (path === `/api/v1/processing-tasks/${taskId}/cancel`) {
      cancellationCount += 1;
      return route.fulfill({ status: 409, contentType: "application/json", body: JSON.stringify({ error: { code: "task.invalid_state", message: "conflict", request_id: taskId } }) });
    }
    return route.fulfill({ status: 404, body: "" });
  });
  const response = await page.goto(`/tasks/${taskId}?status=paused`);
  const preview = page.locator("[data-plan-preview]");
  const approve = preview.getByRole("button", { name: "批准并执行计划 v2" });
  const dialog = page.getByRole("alertdialog");
  const backgrounds: string[] = [];
  for (const theme of ["light", "dark"] as const) {
    await page.evaluate((mode) => {
      document.documentElement.dataset.theme = mode;
      document.documentElement.dispatchEvent(new CustomEvent("mfd-theme-change"));
    }, theme);
    await expect(page.locator(".id-theme")).toHaveAttribute("data-theme", theme);
    await approve.click();
    await expect(dialog).toHaveAttribute("aria-modal", "true");
    const cancel = dialog.getByRole("button", { name: /^取\s*消$/ });
    await expect(cancel).toBeFocused();
    const visited: string[] = [];
    for (let step = 0; step < 6; step += 1) {
      await page.keyboard.press("Tab");
      expect(await dialog.evaluate((element) => element.contains(document.activeElement))).toBe(true);
      visited.push(await page.evaluate(() => document.activeElement?.textContent.replace(/\s/g, "") ?? ""));
    }
    expect(visited).toContain("确认执行计划v2");
    expect(visited).toContain("取消");
    backgrounds.push(await dialog.locator(".ant-modal-content").evaluate((element) => getComputedStyle(element).backgroundColor));
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(approve).toBeFocused();
  }
  expect(backgrounds[0]).not.toBe(backgrounds[1]);
  await approve.click();
  await dialog.getByRole("button", { name: "确认执行计划 v2" }).click();
  await expect.poll(() => executionCount).toBe(1);
  await expect(dialog.getByRole("button", { name: /^取\s*消$/ })).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeVisible();
  releaseExecution?.();
  await expect(dialog.getByRole("alert")).toContainText("目标位置已有");
  await expect(dialog.getByRole("alert")).toBeFocused();
  await expect(dialog.getByRole("button", { name: "确认执行计划 v2" })).toBeEnabled();
  expect(executionCount).toBe(1);
  await dialog.getByRole("button", { name: /^取\s*消$/ }).click();
  await expect(approve).toBeFocused();

  const reject = preview.getByRole("button", { name: "拒绝此计划" });
  await reject.click();
  await expect(dialog).toHaveAccessibleName("确认拒绝此计划");
  await dialog.getByRole("button", { name: "确认拒绝并取消任务" }).click();
  await expect(dialog.getByRole("alert")).toContainText("当前任务状态不允许");
  expect(cancellationCount).toBe(1);
  await dialog.getByRole("button", { name: /^取\s*消$/ }).click();
  await expect(reject).toBeFocused();

  await approve.click();
  version = 3;
  await page.evaluate(({ id }) => {
    (window as unknown as { __mediaflowApprovalEvents: BrowserEventSourceController }).__mediaflowApprovalEvents.emit("processing-task.state-changed", JSON.stringify({
      id: 77, type: "processing-task.state-changed", schema_version: "1", occurred_at: "2026-10-27T00:00:00Z", task_id: id,
      payload: { status: "paused", stage: "planning", recovering: false, reason: "organization.plan-paused" },
    }));
  }, { id: taskId });
  await expect(dialog).toBeHidden();
  await expect(preview.getByRole("button", { name: "批准并执行计划 v3" })).toBeVisible();
  expect(executionCount).toBe(1);
  await expect(preview).toBeFocused();

  // 回执未知且权威GET失败：旧内容inert，恢复模态仍必须可取消并返回可用重试入口。
  ambiguous = true;
  projectionUnavailable = true;
  await preview.getByRole("button", { name: "批准并执行计划 v3" }).click();
  await dialog.getByRole("button", { name: "确认执行计划 v3" }).click();
  await expect(dialog.getByRole("alert")).toContainText("操作结果尚未确认");
  await expect(dialog.getByRole("alert")).toBeFocused();
  await expect(dialog.getByRole("button", { name: /^取\s*消$/ })).toBeEnabled();
  await dialog.getByRole("button", { name: /^取\s*消$/ }).click();
  await expect(dialog).toBeHidden();
  expect(executionCount).toBe(2);
  const retry = page.locator("[data-organization-task]").getByRole("button", { name: /重\s*试/ });
  await expect(retry).toBeFocused();
  projectionUnavailable = false;
  await page.keyboard.press("Enter");
  await expect(preview.getByRole("button", { name: "批准并执行计划 v3" })).toBeEnabled();
  expect(executionCount).toBe(2);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  expect(await page.evaluate(() => (window as unknown as { __approvalCspViolations: string[] }).__approvalCspViolations)).toEqual([]);
  const nonce = await page.locator('meta[name="csp-nonce"]').getAttribute("content");
  // 构建产物CSP宿主提供合法nonce；普通Vite保留未替换占位符。
  if (nonce && /^[A-Za-z0-9_-]{43}$/.test(nonce)) {
    expect(response?.headers()["content-security-policy"]).toBe(`default-src 'self'; connect-src 'self'; frame-ancestors 'none'; style-src 'self' 'nonce-${nonce}'`);
    const styles = await page.locator("style[data-css-hash]").evaluateAll((elements) => elements.map((element) => (element as HTMLStyleElement).nonce));
    expect(styles.length).toBeGreaterThan(0);
    expect(styles.every((value) => value === nonce)).toBe(true);
    console.log(JSON.stringify({ cspFixture: "built-web-static-host", contentSecurityPolicy: response?.headers()["content-security-policy"], nonce, styledElements: styles.length, violations: 0 }));
  }
});
