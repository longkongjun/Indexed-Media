import { expect, test } from "@playwright/test";

const inboxId = "018f0f10-8bc1-7a5e-8e5a-2dc913d23c86";
const taskId = "018f0f10-8bc1-7a5e-8e5a-2dc913d23c87";
const account = { account: { id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c85", administrator_name: "admin" }, csrf_token: "a".repeat(43), version: "v1" };

async function installDeterministicEventSource(page: import("@playwright/test").Page) {
  await page.addInitScript(() => {
    class DeterministicEventSource {
      listeners = new Map<string, Set<EventListener>>();
      constructor(readonly url: string) {
        if (url !== "/api/v1/events") throw new Error("SSE must stay same-origin");
        setTimeout(() => this.emit("open", new Event("open")), 0);
        setTimeout(() => this.emit("task.progress", new MessageEvent("task.progress", { data: JSON.stringify({ id: 7, type: "task.progress", schema_version: "1", occurred_at: "2026-07-18T08:00:00Z", task_id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c87", payload: { visited_directories: 4, observed_files: 9, skipped_entries: 2, errors: 1 } }) })), 25);
        setTimeout(() => this.emit("task.state-changed", new MessageEvent("task.state-changed", { data: JSON.stringify({ id: 8, type: "task.state-changed", schema_version: "1", occurred_at: "2026-07-18T08:00:01Z", task_id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c87", payload: { status: "partial-success", recovering: false } }) })), 50);
      }
      addEventListener(type: string, listener: EventListener) { const group = this.listeners.get(type) ?? new Set(); group.add(listener); this.listeners.set(type, group); }
      removeEventListener(type: string, listener: EventListener) { this.listeners.get(type)?.delete(listener); }
      emit(type: string, event: Event) { for (const listener of this.listeners.get(type) ?? []) listener(event); }
      close() {}
    }
    Object.defineProperty(window, "EventSource", { value: DeterministicEventSource });
  });
}

test("adds an inbox, scans live, keeps partial results, pages safely, and disables offline writes", async ({ page, context }, testInfo) => {
  await installDeterministicEventSource(page);
  let created = false;
  let offline = false;
  let taskReads = 0;
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request(); const url = new URL(request.url()); const path = url.pathname;
    if (offline && !["/api/v1/system/bootstrap-status", "/api/v1/session"].includes(path)) return route.abort("internetdisconnected");
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(account);
    if (path === "/api/v1/deployment-roots") return json({ items: [{ id: "incoming", label: "家庭收件区", access: "read-write" }], next_cursor: null });
    if (path === "/api/v1/inbox-directories/preflight") return json({ root_id: "incoming", relative_path: "camera/uploads", readable: true, overlaps_existing: false });
    if (path === "/api/v1/inbox-directories" && request.method() === "POST") { created = true; return json({ id: inboxId, root_id: "incoming", relative_path: "camera/uploads", health: "available", last_checked_at: "2026-07-18T08:00:00Z" }, 201); }
    if (path === "/api/v1/inbox-directories") return json(url.searchParams.get("cursor") === "inbox-next" ? { items: [{ id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c92", root_id: "incoming", relative_path: "inbox-page-two", health: "available", last_checked_at: "2026-07-18T08:00:00Z" }], next_cursor: null } : { items: created ? [{ id: inboxId, root_id: "incoming", relative_path: "camera/uploads", health: "available", last_checked_at: "2026-07-18T08:00:00Z" }] : [], next_cursor: created ? "inbox-next" : null });
    if (path === `/api/v1/inbox-directories/${inboxId}`) return json({ id: inboxId, root_id: "incoming", relative_path: "camera/uploads", health: "available", last_checked_at: "2026-07-18T08:00:00Z" });
    if (path === `/api/v1/inbox-directories/${inboxId}/scan-tasks`) { expect(request.headers()["idempotency-key"]).toMatch(/^[0-9a-f-]{36}$/i); return json({ id: taskId, inbox_directory_id: inboxId, status: "queued", recovering: false, counts: { visited_directories: 0, observed_files: 0, skipped_entries: 0, errors: 0 } }, 202); }
    if (path === `/api/v1/scan-tasks/${taskId}/files`) return json(url.searchParams.get("cursor") === "file-next" ? { items: [{ id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c93", relative_path: "camera/uploads/file-page-two.mkv", size_bytes: 84, modified_at: "2026-07-18T08:00:00Z" }], next_cursor: null } : { items: [{ id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c88", relative_path: "camera/uploads/movie.mkv", size_bytes: 42, modified_at: "2026-07-18T08:00:00Z" }], next_cursor: "file-next" });
    if (path === `/api/v1/scan-tasks/${taskId}/errors`) return json(url.searchParams.get("cursor") === "error-next" ? { items: [{ id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c94", code: "entry.permission_denied", relative_path: "camera/uploads/error-page-two.mkv", occurrences: 1 }], next_cursor: null } : { items: [{ id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c89", code: "entry.permission_denied", relative_path: "camera/uploads/broken.mkv", occurrences: 1 }], next_cursor: "error-next" });
    if (path === `/api/v1/scan-tasks/${taskId}`) {
      taskReads += 1;
      return json(taskReads === 1
        ? { id: taskId, inbox_directory_id: inboxId, status: "running", recovering: false, counts: { visited_directories: 1, observed_files: 2, skipped_entries: 0, errors: 0 } }
        : { id: taskId, inbox_directory_id: inboxId, status: "partial-success", recovering: false, counts: { visited_directories: 4, observed_files: 9, skipped_entries: 2, errors: 1 } });
    }
    if (path === "/api/v1/scan-tasks") return json(url.searchParams.get("cursor") === "task-next" ? { items: [{ id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c95", inbox_directory_id: inboxId, status: "completed", recovering: false, counts: { visited_directories: 8, observed_files: 22, skipped_entries: 0, errors: 0 } }], next_cursor: null } : { items: created ? [{ id: taskId, inbox_directory_id: inboxId, status: "partial-success", recovering: false, counts: { visited_directories: 4, observed_files: 9, skipped_entries: 2, errors: 1 } }] : [], next_cursor: created ? "task-next" : null });
    return route.fulfill({ status: 404, body: "" });
  });

  await page.goto("/inbox-directories");
  await expect(page.getByRole("heading", { name: "收件目录" })).toBeVisible();
  await page.getByRole("button", { name: "添加收件目录" }).focus(); await page.keyboard.press("Enter");
  await page.getByLabel("能力根").focus(); await page.getByLabel("能力根").selectOption("incoming");
  await expect(page.getByLabel("能力根")).toHaveValue("incoming");
  await page.getByLabel("根内相对路径").focus(); await page.keyboard.type("camera/uploads");
  await page.getByRole("button", { name: "预检目录" }).focus(); await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { name: "确认扫描范围" })).toBeVisible();
  await page.getByRole("button", { name: "确认并创建" }).focus(); await page.keyboard.press("Enter");
  await page.getByRole("button", { name: "开始扫描" }).focus(); await page.keyboard.press("Enter");
  await expect(page.getByText("已观察文件").locator("..").getByText("9")).toBeVisible();
  await expect(page.locator('[data-status="partial-success"]')).toContainText("部分成功");
  await page.getByRole("link", { name: "查看原始文件" }).focus(); await page.keyboard.press("Enter");
  const resultStructure = testInfo.project.name === "chromium-mobile" ? page.locator("[data-mobile-results]") : page.locator("[data-wide-results]");
  await expect(resultStructure.getByText("camera/uploads/movie.mkv")).toBeVisible();
  await expect(page.getByText("camera/uploads/broken.mkv")).toBeVisible();
  const filesRegion = page.getByRole("region", { name: "原始文件" });
  await filesRegion.getByRole("button", { name: "继续查看更多" }).click(); await expect(resultStructure.getByText("camera/uploads/file-page-two.mkv")).toBeVisible();
  await page.goBack(); await expect(resultStructure.getByText("camera/uploads/movie.mkv")).toBeVisible();
  await page.goForward(); await expect(resultStructure.getByText("camera/uploads/file-page-two.mkv")).toBeVisible(); await page.goBack();
  const errorsRegion = page.getByRole("region", { name: "扫描错误" });
  await errorsRegion.getByRole("button", { name: "继续查看更多" }).click(); await expect(page.getByText("camera/uploads/error-page-two.mkv")).toBeVisible();
  await page.goBack(); await expect(page.getByText("camera/uploads/broken.mkv")).toBeVisible();
  await page.getByRole("link", { name: "返回任务详情" }).click();
  await page.getByRole("link", { name: "返回扫描任务" }).click();
  await expect(page).toHaveURL(/\/scan-tasks/);
  await page.getByRole("button", { name: "继续查看更多" }).click(); await expect(page.getByText("22 个文件")).toBeVisible();
  await page.goBack(); await expect(page.getByText("9 个文件")).toBeVisible(); await page.goForward(); await expect(page.getByText("22 个文件")).toBeVisible();
  expect(await page.locator("nav").allTextContents()).not.toContain(expect.stringMatching(/识别|Jellyfin|即将推出/));

  await page.goto("/inbox-directories");
  await page.getByRole("button", { name: "继续查看更多" }).click(); await expect(page.getByText("inbox-page-two")).toBeVisible();
  await page.goBack(); await expect(page.getByText("camera/uploads")).toBeVisible(); await page.goForward(); await expect(page.getByText("inbox-page-two")).toBeVisible(); await page.goBack();
  offline = true;
  await page.reload();
  await expect(page.getByText("Offline")).toBeVisible();
  await expect(page.getByRole("button", { name: "添加收件目录" })).toBeDisabled();
});

test("native EventSource receives the same named frames emitted by Core", async ({ page }) => {
  const progress = JSON.stringify({ id: 31, type: "task.progress", schema_version: "1", occurred_at: "2026-07-18T08:00:00Z", task_id: taskId, payload: { visited_directories: 6, observed_files: 17, skipped_entries: 2, errors: 1 } });
  const terminal = JSON.stringify({ id: 32, type: "task.state-changed", schema_version: "1", occurred_at: "2026-07-18T08:00:01Z", task_id: taskId, payload: { status: "partial-success", recovering: false } });
  let taskReads = 0;
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/v1/events") return route.fulfill({ status: 200, headers: { "Content-Type": "text/event-stream", "Cache-Control": "no-cache" }, body: `id: 31\nevent: task.progress\ndata: ${progress}\n\nid: 32\nevent: task.state-changed\ndata: ${terminal}\n\n` });
    let body;
    if (path === "/api/v1/system/bootstrap-status") body = { requires_initialization: false, version: "v1" };
    else if (path === "/api/v1/session") body = account;
    else {
      taskReads += 1;
      body = taskReads === 1
        ? { id: taskId, inbox_directory_id: inboxId, status: "running", recovering: false, counts: { visited_directories: 1, observed_files: 2, skipped_entries: 0, errors: 0 } }
        : { id: taskId, inbox_directory_id: inboxId, status: "partial-success", recovering: false, counts: { visited_directories: 6, observed_files: 17, skipped_entries: 2, errors: 1 } };
    }
    return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
  });
  await page.goto(`/scan-tasks/${taskId}`);
  await expect(page.getByText("已观察文件").locator("..").getByText("17")).toBeVisible();
  await expect(page.locator('[data-status="partial-success"]')).toContainText("部分成功");
});
