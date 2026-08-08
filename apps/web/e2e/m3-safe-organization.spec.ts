import { expect, test } from "@playwright/test";

const accountId = "019f0000-0000-7000-8000-000000000001";
const targetId = "019f0000-0000-7000-8000-000000000071";
const taskId = "019f0000-0000-7000-8000-000000000081";
const inboxId = "019f0000-0000-7000-8000-000000000084";
const revisionId = "019f0000-0000-7000-8000-000000000085";
const planId = "019f0000-0000-7000-8000-000000000083";
const resultId = "019f0000-0000-7000-8000-000000000082";
const journalId = "019f0000-0000-7000-8000-000000000087";
const operationId = "019f0000-0000-7000-8000-000000000088";
const session = {
  account: { id: accountId, administrator_name: "admin" },
  csrf_token: "o".repeat(43),
  version: "v1",
};
const longDestination = `Movies/${"long-segment/".repeat(28)}Arrival (2016).mkv`;

const target = {
  id: targetId,
  kind: "movie",
  display_name: "电影安全库",
  root_id: "library",
  relative_path: "Movies",
  operation: "copy",
  naming_pattern: "movie",
  nfo_policy: "generate-missing",
  automatic: true,
  enabled: true,
  rules: [{ media_kind: "movie", inbox_directory_id: null, explicit_tag: "trusted", enabled: true }],
  config_version: 1,
  updated_at: "2026-07-24T09:00:00Z",
};

const task = {
  id: taskId,
  inbox_directory_id: inboxId,
  file_revision_id: revisionId,
  relative_path: "ready/Arrival.2016.mkv",
  status: "paused",
  stage: "planning",
  checkpoint: "planning-paused",
  decision_checkpoint: null,
  current_task_decision_id: null,
  reason: "organization.plan-paused",
  recovering: false,
  attempt_count: 2,
  next_retry_at: null,
  allowed_actions: ["retry", "cancel"],
  updated_at: "2026-07-24T09:00:00Z",
};

function projection(state: "paused" | "partial" | "compensated") {
  const paused = state === "paused";
  const compensated = state === "compensated";
  return {
    task_id: taskId,
    state: paused ? "paused" : compensated ? "completed" : "partial-success",
    plan: {
      id: planId,
      version: 3,
      target_id: targetId,
      source: { root_id: "incoming", relative_path: "ready/Arrival.2016.mkv" },
      destination: { root_id: "library", relative_path: longDestination },
      operation: "copy",
      naming: "Arrival (2016)",
      authorization: paused ? "paused" : "one-time",
      risk_codes: paused ? ["organization.rule-not-matched"] : [],
      created_at: "2026-07-24T09:00:00Z",
    },
    journals: paused ? [] : [{
      id: journalId,
      operation_id: operationId,
      kind: "copy",
      status: compensated ? "compensated" : "verified",
      source: { root_id: "incoming", relative_path: "ready/Arrival.2016.mkv" },
      destination: { root_id: "library", relative_path: longDestination },
      projection_version: compensated ? 5 : 4,
      updated_at: "2026-07-24T09:02:00Z",
    }],
    local_result: paused ? null : {
      id: resultId,
      version: compensated ? 6 : 5,
      status: compensated ? "compensated" : "partial-success",
      nfo_status: "failed",
      catalog_media_item_id: null,
      remaining_actions: compensated ? [] : ["nfo"],
      updated_at: "2026-07-24T09:04:00Z",
    },
    allowed_actions: paused
      ? ["recalculate", "execute", "retry", "cancel"]
      : compensated ? [] : ["retry", "rollback"],
  };
}

test("creates a bounded target and safely recovers execution and rollback response loss", async ({ page }) => {
  await page.addInitScript(() => {
    class QuietEventSource {
      addEventListener() {}
      removeEventListener() {}
      close() {}
    }
    Object.defineProperty(window, "EventSource", { value: QuietEventSource });
  });
  let saved = false;
  let organizationState: "paused" | "partial" | "compensated" = "paused";
  const preflights: unknown[] = [];
  const creates: unknown[] = [];
  const executions: Array<{ key: string | undefined; body: unknown }> = [];
  const rollbacks: Array<{ key: string | undefined; body: unknown }> = [];

  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === "/api/v1/deployment-roots") return json({
      items: [
        { id: "incoming", label: "只读收件", access: "read-only" },
        { id: "library", label: "媒体库", access: "read-write" },
      ],
      next_cursor: null,
    });
    if (path === "/api/v1/organization-targets/preflights") {
      preflights.push(request.postDataJSON());
      return json({ root_id: "library", relative_path: "Movies", writable: true, overlaps_existing: false, same_filesystem_hint: true, failure_code: null });
    }
    if (path === "/api/v1/organization-targets" && request.method() === "GET") {
      return json({ items: saved ? [target] : [], next_cursor: null });
    }
    if (path === "/api/v1/organization-targets" && request.method() === "POST") {
      creates.push(request.postDataJSON());
      saved = true;
      return json(target, 201);
    }
    if (path === `/api/v1/processing-tasks/${taskId}`) return json(task);
    if (path === `/api/v1/processing-tasks/${taskId}/organization` && request.method() === "GET") {
      return json(projection(organizationState));
    }
    if (path === `/api/v1/processing-tasks/${taskId}/organization/executions`) {
      executions.push({ key: request.headers()["idempotency-key"], body: request.postDataJSON() });
      if (executions.length === 1) return route.abort("connectionreset");
      organizationState = "partial";
      return json(projection(organizationState), 202);
    }
    if (path === `/api/v1/processing-tasks/${taskId}/organization/rollbacks`) {
      rollbacks.push({ key: request.headers()["idempotency-key"], body: request.postDataJSON() });
      organizationState = "compensated";
      return route.abort("connectionreset");
    }
    return route.fulfill({ status: 404, body: "" });
  });

  await page.goto("/organization/targets");
  await expect(page.getByRole("heading", { name: "整理目标", exact: true })).toBeVisible();
  await page.getByLabel("显示名").fill("电影安全库");
  await page.getByLabel("可写能力根").selectOption("library");
  await expect(page.getByLabel("可写能力根").locator("option")).toHaveCount(2);
  await page.getByLabel("根内相对目录").fill("Movies/../Movies");
  await page.getByLabel("NFO 策略").selectOption("generate-missing");
  await page.getByLabel("允许命中有界规则的低风险计划自动执行").check();
  await page.getByRole("button", { name: "添加规则" }).click();
  await page.getByLabel("显式标签（可选）").fill("trusted");
  await page.getByRole("button", { name: "检查目标" }).click();
  await expect(page.getByText(/预检通过：library\/Movies/)).toBeVisible();
  await page.getByRole("button", { name: "保存目标" }).click();
  await expect(page.getByRole("link", { name: /电影安全库/ })).toBeVisible();
  expect(preflights).toEqual([{ root_id: "library", relative_path: "Movies/../Movies" }]);
  expect(creates).toEqual([expect.objectContaining({
    display_name: "电影安全库", root_id: "library", relative_path: "Movies",
    operation: "copy", nfo_policy: "generate-missing", automatic: true,
    rules: [{ media_kind: "movie", inbox_directory_id: null, explicit_tag: "trusted", enabled: true }],
  })]);

  await page.goto(`/tasks/${taskId}`);
  await expect(page.getByRole("heading", { name: "安全整理" })).toBeVisible();
  await expect(page.getByText(longDestination, { exact: false })).toBeVisible();
  await page.getByRole("button", { name: "授权当前计划执行一次" }).focus();
  await page.keyboard.press("Enter");
  await expect(page.getByText(/操作结果尚未确认/)).toBeVisible();
  expect(executions).toHaveLength(1);

  await page.getByRole("button", { name: "授权当前计划执行一次" }).focus();
  await page.keyboard.press("Enter");
  await expect(page.getByText("NFO：失败")).toBeVisible();
  await expect(page.getByText(/重试只继续这些步骤/)).toBeVisible();
  await expect(page.getByText(/copy · verified/)).toBeVisible();
  expect(executions).toHaveLength(2);
  expect(executions[0]?.key).toMatch(/^[0-9a-f-]{36}$/i);
  expect(executions[0]?.key).toBe(executions[1]?.key);
  expect(executions.map((write) => write.body)).toEqual([{ plan_version: 3 }, { plan_version: 3 }]);

  await page.getByRole("button", { name: "回滚本次操作" }).focus();
  await page.keyboard.press("Enter");
  const rollbackDialog = page.getByRole("alertdialog", { name: "确认安全回滚" });
  await expect(rollbackDialog).toBeVisible();
  await expect(page.getByRole("heading", { name: "确认安全回滚" })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(rollbackDialog.getByRole("button", { name: "取消" })).toBeFocused();
  await page.keyboard.press("Enter");
  expect(rollbacks).toHaveLength(0);
  await page.getByRole("button", { name: "回滚本次操作" }).focus();
  await page.keyboard.press("Enter");
  await page.keyboard.press("Tab");
  await page.keyboard.press("Tab");
  await expect(rollbackDialog.getByRole("button", { name: "确认回滚当前结果版本" })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByText("本地结果：compensated")).toBeVisible();
  await expect(page.getByRole("button", { name: "回滚本次操作" })).toHaveCount(0);
  expect(rollbacks).toHaveLength(1);
  expect(rollbacks[0]?.key).toMatch(/^[0-9a-f-]{36}$/i);
  expect(rollbacks[0]?.body).toEqual({ result_version: 5 });
  await expect(page.locator("body")).not.toContainText("/private/");
  await expect(page.locator("body")).not.toContainText("NFO_MUST_NOT_RENDER");
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
});
