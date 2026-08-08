import { expect, test } from "@playwright/test";

const accountId = "019f0000-0000-7000-8000-000000000001";
const connectionId = "019f0000-0000-7000-8000-000000000060";
const taskId = "019f0000-0000-7000-8000-000000000062";
const session = { account: { id: accountId, administrator_name: "admin" }, csrf_token: "m".repeat(43), version: "v1" };
const connection = {
  id: connectionId, kind: "qbittorrent", display_name: "Primary qBit", base_url: "https://download.test/qbit",
  enabled: true, config_version: 1,
  capabilities: { manual_add: true, task_monitoring: true, product_version: "5.1.2", api_version: "2.11.4" },
  health: "healthy", checked_at: "2026-07-24T05:00:00Z", failure_code: null, updated_at: "2026-07-24T05:00:00Z",
};
const task = {
  id: taskId, connection_id: connectionId, connection_display_name: "Primary qBit", display_name: "Ubuntu ISO",
  status: "queued", remote_status: null, progress_basis_points: 0, failure_code: null, retry_at: null,
  linked: false, projection_version: 1, created_at: "2026-07-24T05:01:00Z", updated_at: "2026-07-24T05:01:00Z",
};

test("tests then saves a connection and replays a secret-safe download create", async ({ page }, testInfo) => {
  if (testInfo.project.name === "chromium-mobile") await page.setViewportSize({ width: 390, height: 844 });
  let saved = false;
  let created = false;
  const writes: Array<{ key?: string; body: Record<string, unknown> }> = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === "/api/v1/events") return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    if (path === "/api/v1/downloader-connections/connection-tests") {
      return json({ reachable: true, health: "healthy", capabilities: connection.capabilities, failure_code: null, checked_at: connection.checked_at });
    }
    if (path === "/api/v1/downloader-connections" && request.method() === "POST") {
      saved = true;
      return json(connection, 201);
    }
    if (path === "/api/v1/downloader-connections") return json({ items: saved ? [connection] : [], next_cursor: null });
    if (path === "/api/v1/download-tasks" && request.method() === "POST") {
      writes.push({ key: request.headers()["idempotency-key"], body: request.postDataJSON() as Record<string, unknown> });
      if (writes.length === 1) return route.abort("connectionreset");
      created = true;
      return json(task, 202);
    }
    if (path === "/api/v1/download-tasks") return json({ items: created ? [task] : [], next_cursor: null });
    return route.fulfill({ status: 404, body: "" });
  });

  await page.goto("/connections/downloaders");
  await page.getByLabel("显示名").fill("Primary qBit");
  await page.getByLabel("基地址").fill("https://download.test/qbit");
  await page.getByLabel("用户名").fill("admin");
  await page.getByLabel("密码").fill("PASSWORD_MUST_NOT_RENDER");
  await page.getByRole("button", { name: "仅测试，不保存" }).click();
  await expect(page.getByText(/测试结果：可连接/)).toBeVisible();
  await expect(page.getByLabel("用户名")).toHaveValue("");
  await expect(page.getByLabel("密码")).toHaveValue("");
  expect(saved).toBe(false);

  await page.getByLabel("用户名").fill("admin");
  await page.getByLabel("密码").fill("password");
  await page.getByRole("button", { name: "保存连接" }).click();
  await expect(page.getByText("Primary qBit").first()).toBeVisible();
  await page.getByRole("link", { name: "查看下载任务" }).click();
  await page.getByLabel("下载器连接").selectOption(connectionId);
  await page.getByLabel("显示名").fill("Ubuntu ISO");
  const secretSource = `magnet:?xt=urn:btih:${"A".repeat(200)}&dn=SOURCE_MUST_NOT_RENDER`;
  await page.getByLabel("Magnet 或 HTTPS torrent URL").fill(secretSource);
  await page.getByRole("button", { name: "创建下载任务" }).click();
  await expect(page.getByText("Ubuntu ISO").last()).toBeVisible();
  await expect(page.getByLabel("Magnet 或 HTTPS torrent URL")).toHaveValue("");
  await expect(page.locator("body")).not.toContainText("SOURCE_MUST_NOT_RENDER");
  expect(writes).toHaveLength(2);
  expect(writes[0]?.key).toBe(writes[1]?.key);
  expect(writes[0]?.body).toEqual(writes[1]?.body);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
});
