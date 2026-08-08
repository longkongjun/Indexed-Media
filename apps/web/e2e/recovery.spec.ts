import { expect, test, type Page } from "@playwright/test";
import { m2Fixtures } from "../../../packages/test-fixtures/src/index.ts";

const { recovery, session } = m2Fixtures;

interface BrowserEventSourceController {
  emit(type: string, data: string): void;
  fail(): void;
  reopen(): void;
}

async function installControlledSameOriginEventSource(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const sources: ControlledEventSource[] = [];

    class ControlledEventSource {
      private readonly listeners = new Map<string, Set<EventListener>>();
      private closed = false;
      private reconnecting = false;

      constructor(readonly url: string) {
        if (url !== "/api/v1/events") throw new Error("M2 recovery SSE must stay same-origin");
        sources.push(this);
        setTimeout(() => this.reopen(), 0);
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

      emitNamed(type: string, data: string): void {
        if (this.reconnecting) return;
        this.dispatch(type, new MessageEvent(type, { data }));
      }

      fail(): void {
        if (this.closed) return;
        this.reconnecting = true;
        this.dispatch("error", new Event("error"));
      }

      reopen(): void {
        if (this.closed) return;
        this.reconnecting = false;
        this.dispatch("open", new Event("open"));
      }
    }

    const controller: BrowserEventSourceController = {
      emit(type, data) {
        for (const source of sources) {
          source.emitNamed(type, data);
        }
      },
      fail() {
        for (const source of sources) source.fail();
      },
      reopen() {
        for (const source of sources) source.reopen();
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

async function reopenRealtime(page: Page): Promise<void> {
  const reopened = await page.evaluate(() => {
    const controller = (window as unknown as {
      __mediaflowM2Events: BrowserEventSourceController & { reopen?: () => void };
    }).__mediaflowM2Events;
    if (!controller.reopen) return false;
    controller.reopen();
    return true;
  });
  expect(reopened).toBe(true);
}

test("browser-level restart consequence keeps the durable task identity and committed REST facts", async ({ page }) => {
  // 此处模拟 Core 中断及恢复在浏览器中的可见后果，不会重启容器。
  await installControlledSameOriginEventSource(page);
  let taskReads = 0;
  let fileReads = 0;
  let errorReads = 0;
  let writeRequests = 0;
  let terminalAvailable = false;

  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (request.method() !== "GET") writeRequests += 1;
    const json = (body: unknown, status = 200) => route.fulfill({
      status,
      contentType: "application/json",
      body: JSON.stringify(body),
    });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === `/api/v1/scan-tasks/${recovery.taskId}`) {
      taskReads += 1;
      return json(terminalAvailable ? recovery.snapshots.terminal : recovery.snapshots.retained);
    }
    if (path === `/api/v1/scan-tasks/${recovery.taskId}/files`) {
      fileReads += 1;
      return json(recovery.files);
    }
    if (path === `/api/v1/scan-tasks/${recovery.taskId}/errors`) {
      errorReads += 1;
      return json(recovery.errors);
    }
    return route.fulfill({ status: 404, body: "" });
  });

  await page.goto(`/scan-tasks/${recovery.taskId}`);
  await expect(page).toHaveURL(new RegExp(`/scan-tasks/${recovery.taskId}$`));
  await expect(page.getByText("已观察文件").locator("..").getByText("2")).toBeVisible();

  await failRealtime(page);
  await expect(page.getByText("正在恢复实时连接")).toBeVisible();
  await expect(page.getByText("已观察文件").locator("..").getByText("2")).toBeVisible();
  await reopenRealtime(page);
  await expect(page.getByText("正在恢复实时连接")).toBeHidden();

  await emit(page, "task.state-changed", {
    ...m2Fixtures.stateChanged,
    id: 10,
    task_id: recovery.taskId,
    payload: { status: recovery.snapshots.recovering.status, recovering: true },
  });
  await emit(page, "task.progress", {
    ...m2Fixtures.progress,
    id: 11,
    task_id: recovery.taskId,
    payload: recovery.snapshots.recovering.counts,
  });
  await expect(page.getByText("任务正在恢复")).toBeVisible();
  await expect(page.getByText("已观察文件").locator("..").getByText("4")).toBeVisible();

  await emit(page, "task.progress", {
    ...m2Fixtures.progress,
    id: 12,
    task_id: recovery.taskId,
    payload: recovery.snapshots.terminal.counts,
  });
  terminalAvailable = true;
  await emit(page, "task.state-changed", {
    ...m2Fixtures.stateChanged,
    id: 13,
    task_id: recovery.taskId,
    payload: { status: recovery.snapshots.terminal.status, recovering: false },
  });
  await expect(page.locator('[data-status="partial-success"]')).toContainText("部分成功");
  await expect(page.getByText("已观察文件").locator("..").getByText("6")).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/scan-tasks/${recovery.taskId}$`));

  await page.getByRole("link", { name: "查看原始文件" }).click();
  await expect(page.locator("[data-wide-results]:visible, [data-mobile-results]:visible").getByText(recovery.files.items[0].relative_path, { exact: true })).toBeVisible();
  await expect(page.getByText(recovery.errors.items[0].relative_path)).toBeVisible();
  expect(taskReads).toBe(2);
  expect(fileReads).toBe(1);
  expect(errorReads).toBe(1);
  expect(writeRequests).toBe(0);
});

test("stream gap performs one targeted REST truth refresh and stale named events cannot regress it", async ({ page }) => {
  await installControlledSameOriginEventSource(page);
  let latestAvailable = false;
  const reads = { task: 0, files: 0, errors: 0, list: 0 };

  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === `/api/v1/scan-tasks/${recovery.taskId}`) {
      reads.task += 1;
      return json(latestAvailable ? recovery.snapshots.terminal : recovery.snapshots.retained);
    }
    if (path === `/api/v1/scan-tasks/${recovery.taskId}/files`) {
      reads.files += 1;
      return json(recovery.files);
    }
    if (path === `/api/v1/scan-tasks/${recovery.taskId}/errors`) {
      reads.errors += 1;
      return json(recovery.errors);
    }
    if (path === "/api/v1/scan-tasks") reads.list += 1;
    return route.fulfill({ status: 404, body: "" });
  });

  await page.goto(`/scan-tasks/${recovery.taskId}`);
  await expect(page.getByText("已观察文件").locator("..").getByText("2")).toBeVisible();
  expect(reads).toEqual({ task: 1, files: 0, errors: 0, list: 0 });

  latestAvailable = true;
  const gap = { ...m2Fixtures.gap, id: 20, payload: { minimum_available_id: 21 } };
  await emit(page, "stream.gap", gap);
  await expect(page.getByText("已观察文件").locator("..").getByText("6")).toBeVisible();
  await expect(page.locator('[data-status="partial-success"]')).toContainText("部分成功");

  await emit(page, "task.progress", {
    ...m2Fixtures.progress,
    id: 19,
    task_id: recovery.taskId,
    payload: { visited_directories: 0, observed_files: 0, skipped_entries: 0, errors: 0 },
  });
  await emit(page, "task.state-changed", {
    ...m2Fixtures.stateChanged,
    id: 18,
    task_id: recovery.taskId,
    payload: { status: "running", recovering: true },
  });
  await emit(page, "stream.gap", gap);

  await expect(page.getByText("已观察文件").locator("..").getByText("6")).toBeVisible();
  await expect(page.locator('[data-status="partial-success"]')).toContainText("部分成功");
  const [filesResponse, errorsResponse] = await Promise.all([
    page.waitForResponse((response) => new URL(response.url()).pathname === `/api/v1/scan-tasks/${recovery.taskId}/files`),
    page.waitForResponse((response) => new URL(response.url()).pathname === `/api/v1/scan-tasks/${recovery.taskId}/errors`),
    page.getByRole("link", { name: "查看原始文件" }).click(),
  ]);
  expect(filesResponse.status()).toBe(200);
  expect(errorsResponse.status()).toBe(200);
  const filesPayload = await filesResponse.json() as typeof recovery.files;
  const errorsPayload = await errorsResponse.json() as typeof recovery.errors;
  expect(filesPayload.items[0]).toMatchObject({
    id: recovery.files.items[0].id,
    relative_path: recovery.files.items[0].relative_path,
  });
  expect(errorsPayload.items[0]).toMatchObject({
    id: recovery.errors.items[0].id,
    relative_path: recovery.errors.items[0].relative_path,
  });
  await expect(page.locator("[data-wide-results]:visible, [data-mobile-results]:visible").getByText(recovery.files.items[0].relative_path, { exact: true })).toBeVisible();
  await expect(page.getByText(recovery.errors.items[0].relative_path, { exact: true })).toBeVisible();
  await expect.poll(() => reads).toEqual({ task: 2, files: 1, errors: 1, list: 0 });
  await expect(page).toHaveURL(new RegExp(`/scan-tasks/${recovery.taskId}/files\\?view=files$`));
});
