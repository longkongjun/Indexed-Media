import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { m2Fixtures } from "../../../packages/test-fixtures/src/index.ts";

const { recovery, session } = m2Fixtures;

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
        if (url !== "/api/v1/events") throw new Error("M2 offline SSE must stay same-origin");
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
      emit(type, data) {
        for (const source of sources) source.dispatch(type, new MessageEvent(type, { data }));
      },
      fail() {
        for (const source of sources) source.dispatch("error", new Event("error"));
      },
    };

    Object.defineProperty(window, "EventSource", { configurable: true, value: ControlledEventSource });
    Object.defineProperty(window, "__mediaflowM2Events", { configurable: true, value: controller });
  });
}

async function emit(page: Page, type: string, event: unknown): Promise<void> {
  await page.evaluate(({ eventType, data }) => {
    (window as unknown as { __mediaflowM2Events: BrowserEventSourceController })
      .__mediaflowM2Events.emit(eventType, JSON.stringify(data));
  }, { eventType: type, data: event });
}

async function failRealtime(page: Page): Promise<void> {
  await page.evaluate(() => {
    (window as unknown as { __mediaflowM2Events: BrowserEventSourceController })
      .__mediaflowM2Events.fail();
  });
}

async function artifactFiles(directory: string): Promise<string[]> {
  let entries;
  try {
    entries = await readdir(directory, { withFileTypes: true });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw error;
  }
  const files: string[] = [];
  for (const entry of entries) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await artifactFiles(path));
    else files.push(path);
  }
  return files;
}

test("offline transport failure preserves rendered facts, disables writes, and restores the same identity", async ({ page }) => {
  await installControlledSameOriginEventSource(page);
  let offline = false;
  let taskReads = 0;
  let writeRequests = 0;

  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (request.method() !== "GET") writeRequests += 1;
    if (offline && (path === "/api/v1/session" || path === `/api/v1/scan-tasks/${recovery.taskId}`)) {
      return route.abort("internetdisconnected");
    }
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === `/api/v1/scan-tasks/${recovery.taskId}`) {
      taskReads += 1;
      return json(recovery.snapshots.retained);
    }
    if (path === `/api/v1/scan-tasks/${recovery.taskId}/files`) return json(recovery.files);
    if (path === `/api/v1/scan-tasks/${recovery.taskId}/errors`) return json(recovery.errors);
    return route.fulfill({ status: 404, body: "" });
  });

  await page.goto(`/scan-tasks/${recovery.taskId}`);
  await expect(page.getByText("已观察文件").locator("..").getByText("2")).toBeVisible();
  await expect(page.getByRole("button", { name: "取消扫描" })).toBeEnabled();

  offline = true;
  await failRealtime(page);
  await emit(page, "stream.gap", { ...m2Fixtures.gap, id: 30, payload: { minimum_available_id: 31 } });
  await expect(page.getByText("Offline")).toBeVisible();
  await expect(page.getByRole("heading", { name: "扫描任务详情" })).toBeVisible();
  await expect(page.getByText("已观察文件").locator("..").getByText("2")).toBeVisible();
  await expect(page.getByRole("button", { name: "取消扫描" })).toBeDisabled();
  await expect(page).toHaveURL(new RegExp(`/scan-tasks/${recovery.taskId}$`));

  offline = false;
  await page.getByRole("button", { name: "重试", exact: true }).click();
  await expect(page.getByText("Offline")).toBeHidden();
  await expect(page.getByRole("button", { name: "取消扫描" })).toBeEnabled();
  await expect(page.getByText("已观察文件").locator("..").getByText("2")).toBeVisible();
  await page.getByRole("link", { name: "查看原始文件" }).click();
  await expect(page.locator("[data-wide-results]:visible, [data-mobile-results]:visible").getByText(recovery.files.items[0].relative_path, { exact: true })).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/scan-tasks/${recovery.taskId}/files`));
  expect(taskReads).toBe(2);
  expect(writeRequests).toBe(0);
});

test("401 clears the in-memory session, saves a safe return path, and leaks no credential material", async ({ page }, testInfo) => {
  await installControlledSameOriginEventSource(page);
  const consoleOutput: string[] = [];
  page.on("console", (message) => consoleOutput.push(message.text()));
  page.on("pageerror", (error) => consoleOutput.push(error.message));
  const returnPath = `/scan-tasks/${recovery.taskId}?context=task-${recovery.taskId}`;
  const submittedPassword = "browser-only-password";
  let sessionUnauthorized = false;
  let loginCsrfHeader: string | undefined;

  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({
      status,
      contentType: "application/json",
      body: JSON.stringify(body),
    });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session" && request.method() === "GET") {
      if (sessionUnauthorized) {
        return json({ error: { code: "session.expired", message: "Session expired", request_id: "019f0000-0000-7000-8000-000000000099", details: {} } }, 401);
      }
      return json(session);
    }
    if (path === "/api/v1/sessions" && request.method() === "POST") {
      loginCsrfHeader = request.headers()["x-csrf-token"];
      sessionUnauthorized = false;
      return json(session, 201);
    }
    if (path === `/api/v1/scan-tasks/${recovery.taskId}`) return json(recovery.snapshots.retained);
    return route.fulfill({ status: 404, body: "" });
  });

  await page.goto(returnPath);
  await expect(page.getByText("已观察文件").locator("..").getByText("2")).toBeVisible();
  sessionUnauthorized = true;
  await failRealtime(page);
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByRole("heading", { name: "管理员登录" })).toBeVisible();
  await expect(page.locator("body")).not.toContainText(session.account.administrator_name);

  const preLoginExposure = await page.evaluate((csrfToken) => ({
    html: document.documentElement.outerHTML,
    url: window.location.href,
    local: Object.values(localStorage),
    session: Object.values(sessionStorage),
    hasToken: document.documentElement.outerHTML.includes(csrfToken),
  }), session.csrf_token);
  expect(preLoginExposure.hasToken).toBe(false);
  expect(JSON.stringify(preLoginExposure)).not.toContain(session.csrf_token);

  await page.getByLabel("管理员名称").fill(session.account.administrator_name);
  await page.getByLabel("密码").fill(submittedPassword);
  await page.getByRole("button", { name: "登录", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`${returnPath.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`));
  await expect(page.getByText("已观察文件").locator("..").getByText("2")).toBeVisible();
  expect(loginCsrfHeader).toBeUndefined();

  const browserExposure = await page.evaluate(() => ({
    html: document.documentElement.outerHTML,
    url: window.location.href,
    local: Object.values(localStorage),
    session: Object.values(sessionStorage),
  }));
  const capturedOutput = consoleOutput.join("\n");
  for (const secret of [session.csrf_token, submittedPassword]) {
    expect(JSON.stringify(browserExposure)).not.toContain(secret);
    expect(capturedOutput).not.toContain(secret);
    for (const artifact of await artifactFiles(testInfo.outputDir)) {
      expect((await readFile(artifact)).includes(Buffer.from(secret))).toBe(false);
    }
  }
});
