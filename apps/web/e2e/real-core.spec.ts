import { expect, test } from "@playwright/test";
import type { ProcessingTaskPage, SessionResponse } from "@mediaflow/api-client-ts";
import { startRealCore } from "./real-core-harness";

// Keep the existing fixture-only suite fast. Run this file with real-core.config.ts explicitly.
test.skip(!process.env.MEDIAFLOW_REAL_CORE, "Set MEDIAFLOW_REAL_CORE=1 for disposable real Core integration");

test("real Core persists Web bootstrap, scans, paused task retry and restart recovery", async ({ page }, testInfo) => {
  const core = await startRealCore();
  const browserErrors: string[] = [];
  const http: Array<{ method: string; path: string; status: number }> = [];
  page.on("pageerror", (error) => browserErrors.push(error.message));
  page.on("response", (response) => {
    const path = new URL(response.url()).pathname;
    if (path.startsWith("/api/")) http.push({ method: response.request().method(), path, status: response.status() });
  });
  const at = (path: string) => `${core.baseURL}${path}`;
  let databaseFacts: unknown;
  try {
    await page.goto(at("/"));
    await expect(page.getByRole("heading", { name: "创建管理员" })).toBeVisible();
    await page.getByLabel("一次性引导密钥").fill(core.bootstrapSecret);
    await page.getByLabel("管理员名称").fill("local-admin");
    await page.getByLabel("密码", { exact: true }).fill(core.password);
    await page.getByRole("button", { name: "创建管理员" }).click();
    await expect(page.getByRole("heading", { name: "管理员登录" })).toBeVisible();
    await page.getByLabel("管理员名称").fill("local-admin");
    await page.getByLabel("密码", { exact: true }).fill(core.password);
    await page.getByRole("button", { name: "登录", exact: true }).click();
    await expect(page).not.toHaveURL(/login/);
    await page.getByRole("button", { name: testInfo.project.name === "chromium-mobile" ? "更多" : "账户与系统", exact: true }).click();
    await page.getByRole("button", { name: "退出登录" }).click();
    await expect(page.getByRole("heading", { name: "管理员登录" })).toBeVisible();
    await page.getByLabel("管理员名称").fill("local-admin");
    await page.getByLabel("密码", { exact: true }).fill(core.password);
    await page.getByRole("button", { name: "登录", exact: true }).click();
    await expect(page).not.toHaveURL(/login/);
    await page.goto(at("/media"));
    await expect(page.getByText("尚无已整理媒体", { exact: true })).toBeVisible();

    await page.goto(at("/inbox-directories"));
    await page.getByRole("button", { name: "添加收件目录" }).click();
    await page.getByLabel("能力根", { exact: true }).selectOption("incoming");
    await page.getByLabel("根内相对路径", { exact: true }).fill("inbox");
    await page.getByRole("button", { name: "预检目录" }).click();
    await expect(page.getByRole("heading", { name: "确认扫描范围" })).toBeVisible();
    await page.getByRole("button", { name: "确认并创建" }).click();
    await expect(page).toHaveURL(/inbox-directories\/[0-9a-f-]+$/);
    const inboxURL = page.url();
    const inboxId = new URL(inboxURL).pathname.split("/").at(-1)!;
    // Shorten only the public stability policy; scanner/revision/task identity stay production-created.
    const policyStatus = await page.evaluate(async (id) => {
      const sessionResponse = await fetch("/api/v1/session");
      if (!sessionResponse.ok) throw new Error(`Session read failed: ${String(sessionResponse.status)}`);
      const session = await sessionResponse.json() as SessionResponse;
      const policy = await fetch(`/api/v1/inbox-directories/${id}/discovery-policy`, {
        method: "PUT",
        headers: { "Content-Type": "application/json", "X-CSRF-Token": session.csrf_token, "If-Match": "1" },
        body: JSON.stringify({ minimum_age_seconds: 0, stable_observation_interval_seconds: 1, reconcile_interval_seconds: 60, watcher_enabled: false }),
      });
      return policy.status;
    }, inboxId);
    expect(policyStatus).toBe(200);
    // GET /session rotates CSRF; reload lets the application acquire its own fresh token.
    await page.reload();
    await page.getByRole("button", { name: "开始扫描" }).click();
    await expect(page.locator('[data-status="completed"]')).toBeVisible();
    const firstScanURL = page.url();
    await page.getByRole("link", { name: "查看原始文件" }).click();
    await expect(page.locator("[data-wide-results]:visible, [data-mobile-results]:visible").getByText("inbox/Local.Journey.2026.mkv", { exact: true })).toBeVisible();
    // The production scanner requires two stable observations (one-second policy above).
    await expect.poll(async () => Date.now(), { intervals: [1_100] }).toBeGreaterThan(Date.now() + 1_000);
    await page.goto(inboxURL);
    await page.getByRole("button", { name: "开始扫描" }).click();
    await expect(page.locator('[data-status="completed"]')).toBeVisible();
    await expect.poll(async () => {
      const tasks = await page.evaluate(async () => {
        const response = await fetch("/api/v1/processing-tasks?view=all");
        if (!response.ok) throw new Error(`Task read failed: ${String(response.status)}`);
        return await response.json() as ProcessingTaskPage;
      });
      return tasks.items.find((task) => task.relative_path === "inbox/Local.Journey.2026.mkv")?.status;
    }, { timeout: 20_000 }).toBe("paused");
    await page.goto(at("/tasks?view=all"));
    await page.getByRole("link", { name: "inbox/Local.Journey.2026.mkv", exact: true }).click();
    await expect(page.getByRole("heading", { name: "处理任务详情" })).toBeVisible();
    const taskURL = page.url();
    const taskId = new URL(taskURL).pathname.split("/").at(-1)!;
    await expect(page.getByRole("button", { name: "重试处理" })).toBeEnabled();
    const retry = page.waitForResponse((response) => response.url().endsWith("/attempts") && response.request().method() === "POST");
    await page.getByRole("button", { name: "重试处理" }).click();
    expect((await retry).status()).toBe(202);
    await expect(page.getByText("尝试次数").locator("..")).toContainText("2");
    await expect(page.getByRole("button", { name: "重试处理" })).toBeEnabled();

    await core.restart();
    await page.reload();
    await expect(page).toHaveURL(taskURL);
    await expect(page.getByText("尝试次数").locator("..")).toContainText("2");
    await expect(page.getByRole("button", { name: "重试处理" })).toBeEnabled();
    await page.goto(firstScanURL);
    await expect(page.locator('[data-status="completed"]')).toBeVisible();
    await page.getByRole("link", { name: "查看原始文件" }).click();
    await expect(page.locator("[data-wide-results]:visible, [data-mobile-results]:visible").getByText("inbox/Local.Journey.2026.mkv", { exact: true })).toBeVisible();
    databaseFacts = core.databaseFacts(taskId);
    expect(databaseFacts).toMatchObject({ quick_check: "ok", task: { status: "paused", attempt_count: 2 }, attempts: 2, manual_scans: [{ status: "completed", count: 2 }], linked_revisions: 1 });
    expect(await core.assertSourceUnchanged()).toBe(true);
    expect(browserErrors).toEqual([]);
    expect(http.some((request) => request.path === "/api/v1/system/bootstrap" && request.status === 201)).toBe(true);
    expect(http.some((request) => request.path === "/api/v1/sessions" && request.status === 201)).toBe(true);
  } finally {
    try {
      await testInfo.attach("real-core-journey.json", { body: JSON.stringify({ binary_sha256: core.binarySha256, web_transport: core.webTransport, web_distribution: core.webDistribution, browserErrors, http, databaseFacts, sourceUnchanged: await core.assertSourceUnchanged(), seeded_database_rows: 0, metadata_provider: "unconfigured; production task pauses; no approval claimed" }, null, 2), contentType: "application/json" });
    } finally {
      try { await page.close(); } finally { await core.close(); }
    }
  }
});
