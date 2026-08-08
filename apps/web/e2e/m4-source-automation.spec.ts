import { expect, test } from "@playwright/test";

const accountId = "019f0000-0000-7000-8000-000000000001";
const sourceId = "019f0000-0000-7000-8000-000000000070";
const eventId = "019f0000-0000-7000-8000-000000000071";
const connectionId = "019f0000-0000-7000-8000-000000000060";
const inboxId = "019f0000-0000-7000-8000-000000000080";
const session = { account: { id: accountId, administrator_name: "admin" }, csrf_token: "m".repeat(43), version: "v1" };
const source = {
  id: sourceId, kind: "rss", display_name: "Release feed", enabled: true,
  downloader_connection_id: connectionId, inbox_directory_id: null,
  endpoint_summary: "feeds.example.test", poll_interval_seconds: 900, allowed_actions: [],
  secret_fingerprint: null, config_version: 2, health: "healthy", checked_at: "2026-07-24T08:00:00Z",
  failure_code: null, projection_version: 3, updated_at: "2026-07-24T08:00:00Z",
};
const failedEvent = {
  id: eventId, source_id: sourceId, source_display_name: "Failed feed", action: "create-download",
  status: "failed", downstream_kind: null, downstream_id: null, result_count: null,
  failure_code: "integration.unavailable", attempt_count: 2, retry_at: null,
  allowed_actions: ["retry"], projection_version: 2,
  created_at: "2026-07-24T08:00:00Z", updated_at: "2026-07-24T08:01:00Z",
};
const reconcileEvent = {
  ...failedEvent, id: "019f0000-0000-7000-8000-000000000073", source_display_name: "Completion map",
  action: "reconcile-inbox", status: "completed", downstream_kind: "reconcile-request",
  downstream_id: "019f0000-0000-7000-8000-000000000074", result_count: 0,
  failure_code: null, attempt_count: 1, allowed_actions: [],
};

test("configures sources and enhancer, recovers an event, and preserves drafts offline", async ({ page }, testInfo) => {
  if (testInfo.project.name === "chromium-mobile") await page.setViewportSize({ width: 390, height: 844 });
  let offline = false;
  let sourceTests = 0;
  let enhancerTests = 0;
  let sourceWrites = 0;
  let event = { ...failedEvent };
  const sources = [{ ...source }];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (offline && path.startsWith("/api/v1/automation-sources/")) return route.abort("internetdisconnected");
    if (path === "/api/v1/downloader-connections") return json({ items: [{
      id: connectionId, kind: "qbittorrent", display_name: "Primary qBit", base_url: "https://download.test",
      enabled: true, config_version: 1, capabilities: null, health: "healthy", checked_at: null,
      failure_code: null, updated_at: source.updated_at,
    }], next_cursor: null });
    if (path === "/api/v1/deployment-roots") return json({ items: [{ id: "incoming", label: "Incoming", access: "read-write" }], next_cursor: null });
    if (path === "/api/v1/inbox-directories") return json({ items: [{ id: inboxId, root_id: "incoming", relative_path: "incoming", health: "available", last_checked_at: source.updated_at }], next_cursor: null });
    if (path === "/api/v1/automation-sources/connection-tests") {
      sourceTests += 1;
      if (sourceTests === 1) return json({ error: { code: "validation.failed", message: "RAW_FEED_BODY_MUST_NOT_RENDER", request_id: sourceId } }, 422);
      return json({ reachable: true, health: "healthy", detected_format: "rss-2.0", item_count: 4, ignored_item_count: 1, failure_code: null, checked_at: source.updated_at });
    }
    if (path === "/api/v1/automation-sources" && request.method() === "POST") {
      sourceWrites += 1;
      const body = request.postDataJSON() as { kind: string; display_name: string };
      if (body.kind === "webhook") {
        const webhook = { ...source, id: "019f0000-0000-7000-8000-000000000075", kind: "webhook", display_name: body.display_name, endpoint_summary: "/api/v1/source-webhooks/…/events", secret_fingerprint: "…7a2f", allowed_actions: ["download.create"] };
        sources.unshift(webhook);
        return json({ source: webhook, secret: "SECRET_ONCE_MUST_DISAPPEAR" }, 201);
      }
      const saved = { ...source, display_name: body.display_name };
      sources[0] = saved;
      return json(saved, 201);
    }
    if (path === "/api/v1/automation-sources") return json({ items: sources, next_cursor: null });
    if (path === `/api/v1/automation-sources/${sourceId}`) return json(sources.find((item) => item.id === sourceId) ?? source);
    if (path === "/api/v1/automation-events") return json({ items: [event, reconcileEvent], next_cursor: null });
    if (path === `/api/v1/automation-events/${eventId}/retries`) {
      event = { ...event, status: "completed", failure_code: null, allowed_actions: [], projection_version: 3 };
      return json(event);
    }
    if (path === `/api/v1/automation-events/${eventId}`) return json(event);
    if (path === "/api/v1/identification-enhancer/connection-tests") {
      enhancerTests += 1;
      return enhancerTests === 1
        ? json({ reachable: false, health: "unavailable", adapter_version: "ollama-v1", model_available: false, fallback_code: "provider.timeout", checked_at: source.updated_at })
        : json({ reachable: true, health: "healthy", adapter_version: "ollama-v1", model_available: true, fallback_code: null, checked_at: source.updated_at });
    }
    if (path === "/api/v1/identification-enhancer" && request.method() === "PUT") return json({ kind: "ollama", enabled: true, endpoint_summary: "http://127.0.0.1:11434", model: "qwen3:4b", timeout_ms: 3000, config_version: 2, health: "unavailable", checked_at: source.updated_at, fallback_code: "provider.timeout", projection_version: 2, updated_at: source.updated_at });
    if (path === "/api/v1/identification-enhancer") return json({ kind: "ollama", enabled: false, endpoint_summary: "http://127.0.0.1:11434", model: "qwen3:4b", timeout_ms: 3000, config_version: 1, health: "degraded", checked_at: null, fallback_code: "automation.source-disabled", projection_version: 1, updated_at: source.updated_at });
    return route.fulfill({ status: 404, body: "" });
  });

  await page.goto("/automation/sources");
  const sourceForm = page.locator("section.form-card").filter({ hasText: "添加 RSS / Atom" });
  await sourceForm.getByLabel("显示名").fill("Release feed updated");
  await sourceForm.getByLabel("Feed URL").fill("https://private.example.test/feed?token=FEED_SECRET_MUST_NOT_RENDER");
  await sourceForm.getByLabel("下载器连接").selectOption(connectionId);
  await sourceForm.getByRole("button", { name: "仅测试，不保存" }).click();
  await expect(sourceForm.locator("[data-error-summary]")).toBeFocused();
  await expect(page.locator("body")).not.toContainText("RAW_FEED_BODY_MUST_NOT_RENDER");
  expect(sourceWrites).toBe(0);
  await sourceForm.getByRole("button", { name: "仅测试，不保存" }).click();
  await expect(sourceForm.getByText(/测试结果：可读取/)).toBeVisible();
  await sourceForm.getByRole("button", { name: "保存来源" }).click();
  await expect(page.getByText("Release feed updated").first()).toBeVisible();
  await expect(sourceForm.getByLabel("Feed URL")).toHaveValue("");

  await page.getByRole("button", { name: /签名 Webhook/ }).first().click();
  const webhookForm = page.locator("section.form-card").filter({ hasText: "添加 签名 Webhook" });
  await webhookForm.getByLabel("显示名").fill("Home webhook");
  await webhookForm.getByRole("button", { name: "保存来源" }).click();
  await expect(page.getByText("SECRET_ONCE_MUST_DISAPPEAR")).toBeVisible();
  await page.getByLabel("我已安全保存").check();
  await page.getByRole("button", { name: "完成并清除" }).click();
  await expect(page.locator("body")).not.toContainText("SECRET_ONCE_MUST_DISAPPEAR");

  const enhancer = page.locator("#identification-enhancer");
  await enhancer.getByLabel("启用本地提示").check();
  await enhancer.getByRole("button", { name: "仅测试，不保存" }).click();
  await expect(enhancer.getByText(/确定性识别仍在运行 · provider.timeout/)).toBeVisible();
  await expect(enhancer.getByRole("button", { name: "保存模型配置" })).toBeDisabled();
  await enhancer.getByRole("button", { name: "仅测试，不保存" }).click();
  await enhancer.getByRole("button", { name: "保存模型配置" }).click();
  await expect(enhancer.getByText(/确定性识别仍在运行/)).toBeVisible();

  await expect(page.getByText("对账完成，未发现新文件")).toBeVisible();
  await page.getByRole("link", { name: /Failed feed/ }).click();
  await page.getByRole("button", { name: "重试原事件" }).click();
  await expect(page.getByText("completed")).toBeVisible();
  await page.getByRole("link", { name: "返回工作台" }).click();

  await page.getByLabel("显示名").first().fill("Offline draft remains");
  offline = true;
  await page.getByRole("button", { name: "刷新此来源" }).first().click();
  await expect(page.getByText("Offline")).toBeVisible();
  await expect(page.getByLabel("显示名").first()).toHaveValue("Offline draft remains");
  await expect(page.getByRole("button", { name: "保存来源" })).toBeDisabled();
  await expect(page.locator("body")).not.toContainText("FEED_SECRET_MUST_NOT_RENDER");
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
});
