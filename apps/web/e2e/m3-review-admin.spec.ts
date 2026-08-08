import { expect, test } from "@playwright/test";

const accountId = "019f0000-0000-7000-8000-000000000001";
const inboxId = "019f0000-0000-7000-8000-000000000002";
const taskId = "019f0000-0000-7000-8000-000000000043";
const revisionId = "019f0000-0000-7000-8000-000000000044";
const caseId = "019f0000-0000-7000-8000-000000000048";
const decisionId = "019f0000-0000-7000-8000-000000000049";
const mediaId = "019f0000-0000-7000-8000-000000000050";
const libraryId = "019f0000-0000-7000-8000-000000000051";
const session = { account: { id: accountId, administrator_name: "admin" }, csrf_token: "m".repeat(43), version: "v1" };
const processingTask = {
  id: taskId, inbox_directory_id: inboxId, file_revision_id: revisionId,
  relative_path: "incoming/Dune.2021.mkv", status: "waiting-confirmation", stage: "identification",
  checkpoint: "waiting-confirmation", decision_checkpoint: null, current_task_decision_id: null,
  reason: "identification.ambiguous", recovering: false, attempt_count: 1, next_retry_at: null,
  allowed_actions: ["review", "retry", "cancel"], updated_at: "2026-07-23T08:00:00Z",
};
const reviewCase = {
  id: caseId, task_id: taskId, file_revision_id: revisionId, inbox_directory_id: inboxId,
  relative_path: processingTask.relative_path, level: "ambiguous", reason: "identification.multiple-strong-candidates",
  title_hint: "Dune", version: 3,
  allowed_actions: ["select-provider-candidate", "rematch-with-hints", "select-generic-video"],
  latest_task_decision: null, updated_at: "2026-07-23T08:00:00Z",
};
const identification = {
  review_case_id: caseId,
  task: processingTask,
  revision: { id: revisionId, inbox_directory_id: inboxId, relative_path: processingTask.relative_path, size_bytes: 42, modified_at: "2026-07-23T07:55:00Z", stability: "stable" },
  decision: { id: "019f0000-0000-7000-8000-000000000047", level: "ambiguous", reason: "identification.multiple-strong-candidates", candidate_id: null, retry_at: null, decided_at: "2026-07-23T08:00:00Z" },
  evidence: [{ id: "019f0000-0000-7000-8000-000000000045", source: "filename", kind: "title", value: "Dune", strength: "strong", reason: "filename.title" }],
  candidates: [], evidence_truncated: false, candidates_truncated: false,
};
const candidate = { provider: "tmdb", media_type: "movie", provider_id: "438631", title: "Dune", original_title: "Dune", year: 2021, locale: "zh-CN" };

test("reviews a candidate with one replay-safe intent and keeps the layout bounded", async ({ page }) => {
  const writes: Array<{ key: string | undefined; body: unknown; version: string | undefined }> = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === "/api/v1/processing-tasks") return json({ items: [processingTask], next_cursor: null, summary: { pending: 1, running: 0, all: 1, completed: 0, snapshot_version: 9 } });
    if (path === `/api/v1/processing-tasks/${taskId}`) return json(processingTask);
    if (path === `/api/v1/processing-tasks/${taskId}/identification`) return json(identification);
    if (path === `/api/v1/review-cases/${caseId}` && request.method() === "GET") return json(reviewCase);
    if (path === `/api/v1/review-cases/${caseId}/candidates`) return json({ items: [candidate] });
    if (path === `/api/v1/review-cases/${caseId}/decisions`) {
      writes.push({ key: request.headers()["idempotency-key"], version: request.headers()["if-match"], body: request.postDataJSON() });
      if (writes.length === 1) return route.abort("connectionreset");
      return json({ id: decisionId, task_id: taskId, review_case_id: caseId, case_version: 3, kind: "select-provider-candidate", state: "accepted", created_at: "2026-07-23T09:00:00Z" }, 202);
    }
    return route.fulfill({ status: 404, body: "" });
  });

  await page.goto("/tasks?view=pending");
  await expect(page.getByRole("heading", { name: "任务中心" })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "任务视图" })).toContainText("待处理1");
  await page.getByRole("link", { name: processingTask.relative_path }).click();
  await page.getByRole("link", { name: "进入人工确认" }).click();
  await expect(page.getByRole("heading", { name: "人工识别确认" })).toBeVisible();
  await expect(page.getByRole("region", { name: "识别证据" })).toContainText("Dune");

  await page.getByRole("button", { name: "搜索候选" }).click();
  await page.getByLabel(/Dune · movie · 2021/).check();
  await expect(page.getByLabel(/保存为精确反馈/)).not.toBeChecked();
  await page.getByRole("button", { name: "确认提交" }).click();
  await expect(page.getByText(/人工决定已接受/)).toBeVisible();
  await expect(page.locator("body")).not.toContainText("已整理");

  expect(writes).toHaveLength(2);
  expect(writes[0]?.key).toMatch(/^[0-9a-f-]{36}$/i);
  expect(writes[0]?.key).toBe(writes[1]?.key);
  expect(writes[0]?.version).toBe("3");
  expect(writes[0]?.body).toEqual({ kind: "select-provider-candidate", provider: "tmdb", media_type: "movie", provider_id: "438631", save_feedback: false });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
});

test("keeps a rematch draft after a version conflict refresh", async ({ page }) => {
  let caseReads = 0;
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === `/api/v1/review-cases/${caseId}` && request.method() === "GET") {
      caseReads += 1;
      return json({ ...reviewCase, version: caseReads === 1 ? 3 : 4 });
    }
    if (path === `/api/v1/processing-tasks/${taskId}/identification`) return json(identification);
    if (path === `/api/v1/review-cases/${caseId}/decisions`) {
      return json({ error: { code: "request.conflict", message: "The resource version has changed.", request_id: decisionId, details: {} } }, 409);
    }
    return route.fulfill({ status: 404, body: "" });
  });

  await page.goto(`/review-cases/${caseId}`);
  await page.getByLabel("决定类型").selectOption("rematch-with-hints");
  await page.getByLabel("标题").fill("Dune Part Two");
  await page.getByLabel("年份（可选）").fill("2024");
  await page.getByRole("button", { name: "确认提交" }).click();
  await expect(page.getByText(/审核内容已变化/)).toBeVisible();
  await expect(page.getByLabel("标题")).toHaveValue("Dune Part Two");
  await expect(page.getByLabel("年份（可选）")).toHaveValue("2024");
  await expect(page.getByText("版本").locator("..").getByText("4")).toBeVisible();
  expect(caseReads).toBe(2);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
});

test("shows only formal local media and refreshes bounded projections from SSE", async ({ page }) => {
  await page.addInitScript(() => {
    const sources: Array<{ emit: (type: string, data: string) => void }> = [];
    class ControlledEventSource {
      listeners = new Map<string, Set<(event: MessageEvent<string>) => void>>();
      constructor(readonly url: string) { sources.push(this); }
      addEventListener(type: string, listener: (event: MessageEvent<string>) => void) { const listeners = this.listeners.get(type) ?? new Set(); listeners.add(listener); this.listeners.set(type, listeners); }
      removeEventListener(type: string, listener: (event: MessageEvent<string>) => void) { this.listeners.get(type)?.delete(listener); }
      close() {}
      emit(type: string, data: string) { for (const listener of this.listeners.get(type) ?? []) listener(new MessageEvent(type, { data })); }
    }
    Object.defineProperty(window, "EventSource", { value: ControlledEventSource });
    Object.assign(window, { __mediaFlowEmit: (type: string, data: string) => { for (const source of sources) source.emit(type, data); } });
  });
  const mediaItem = { id: mediaId, type: "movie", library_id: libraryId, title: "沙丘", year: 2021, local_status: "partial", artwork_ref: null, updated_at: "2026-07-23T10:00:00Z" };
  const mediaDetail = {
    item: mediaItem,
    metadata: [{ field: "overview", value: null, state: "missing", source_type: null, source_id: null, source_version: null }],
    versions: [{ id: "019f0000-0000-7000-8000-000000000052", label: "4K", files: [{ id: "019f0000-0000-7000-8000-000000000053", source_relative_path: "incoming/Dune.2021.mkv", current_relative_path: "movies/Dune (2021)/Dune (2021).mkv", size_bytes: 1024 }] }],
    children: [{ id: "019f0000-0000-7000-8000-000000000054", parent_id: null, type: "generic-video-item", title: "花絮", ordinal: 1, versions: [] }],
    nfo_status: "failed", related_task_ids: [taskId],
  };
  let listReads = 0;
  let detailReads = 0;
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request(); const url = new URL(request.url()); const path = url.pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === "/api/v1/media-items") {
      listReads += 1;
      expect(url.searchParams.get("type")).toBe("movie");
      expect(url.searchParams.get("library_id")).toBe(libraryId);
      expect(url.searchParams.get("local_status")).toBe("partial");
      expect(url.searchParams.get("q")).toBe("Dune");
      return json({ items: [mediaItem], next_cursor: null });
    }
    if (path === `/api/v1/media-items/${mediaId}`) { detailReads += 1; return json(mediaDetail); }
    return route.fulfill({ status: 404, body: "" });
  });

  const emit = (type: string, value: unknown) => page.evaluate(({ eventType, data }) => {
    (window as typeof window & { __mediaFlowEmit: (type: string, data: string) => void }).__mediaFlowEmit(eventType, JSON.stringify(data));
  }, { eventType: type, data: value });
  const event = (id: number, version: number) => ({ id, type: "catalog.media-changed", schema_version: "1", occurred_at: "2026-07-23T10:00:00Z", task_id: null, payload: { media_item_id: mediaId, projection_version: version, change: "updated" } });

  await page.goto(`/media?type=movie&library_id=${libraryId}&local_status=partial&q=Dune`);
  await expect(page.getByRole("heading", { name: "媒体", exact: true })).toBeVisible();
  await expect(page.getByText("暂无本地封面")).toBeVisible();
  await emit("catalog.media-changed", event(10, 2));
  await expect.poll(() => listReads).toBe(2);
  await emit("catalog.media-changed", event(11, 2));
  await expect.poll(() => listReads).toBe(2);

  await page.getByRole("link", { name: /沙丘/ }).click();
  await expect(page.getByText("元数据缺失")).toBeVisible();
  await expect(page.getByText("NFO：生成失败")).toBeVisible();
  await expect(page.getByText("movies/Dune (2021)/Dune (2021).mkv")).toBeVisible();
  await expect(page.getByText("花絮")).toBeVisible();
  await expect(page.locator("body")).not.toContainText("Jellyfin");
  await expect(page.locator("img")).toHaveCount(0);
  await emit("catalog.media-changed", event(12, 3));
  await expect.poll(() => detailReads).toBe(2);
  await emit("stream.gap", { id: 13, type: "stream.gap", schema_version: "1", occurred_at: "2026-07-23T10:00:01Z", task_id: null, payload: { minimum_available_id: 13 } });
  await expect.poll(() => detailReads).toBe(3);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
});
