import type { MediaFlowClient, ScanTask } from "@mediaflow/api-client-ts";
import { MediaFlowApiError } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { identityClientKey } from "../src/app/client";
import { taskEventSourceFactoryKey, type EventSourceLike } from "../src/features/scan-tasks/useTaskEvents";
import { useScanTask, useScanTasks } from "../src/features/scan-tasks/useScanTasks";
import ScanTaskDetailView from "../src/views/ScanTaskDetailView.vue";

const id = "018f0f10-8bc1-7a5e-8e5a-2dc913d23c87";
const task = (status: ScanTask["status"] = "running"): ScanTask => ({
  id, inbox_directory_id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c86", status, recovering: false,
  counts: { visited_directories: 3, observed_files: 5, skipped_entries: 1, errors: status === "partial-success" ? 2 : 0 },
});
function client(overrides: Partial<MediaFlowClient> = {}): MediaFlowClient {
  return { listScanTasks: vi.fn(async () => ({ items: [task()], next_cursor: "next-safe" })), getScanTask: vi.fn(async () => task()),
    retryScanTask: vi.fn(async () => task("queued")), cancelScanTask: vi.fn(async () => task("cancelled")), ...overrides } as MediaFlowClient;
}

describe("scan task state", () => {
  it("loads cursor lists, exposes route context, and preserves stale content while offline", async () => {
    const api = client();
    const feature = useScanTasks(api, { status: "running", cursor: "cursor-a", scrollKey: "tasks-running-row-3" });
    expect(feature.state.value.kind).toBe("idle");
    await feature.load();
    expect(api.listScanTasks).toHaveBeenCalledWith("cursor-a");
    expect(feature.state.value.kind).toBe("content");
    expect(feature.nextCursor.value).toBe("next-safe");
    expect(feature.returnQuery.value).toEqual({ status: "running", cursor: "cursor-a", context: "tasks-running-row-3" });
    (api.listScanTasks as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new TypeError("offline"));
    await feature.load();
    expect(feature.state.value).toMatchObject({ kind: "offline", stale: true });
    expect(feature.tasks.value).toHaveLength(1);
  });

  it("shows valid task facts and updates only the affected task for retry/cancel with unique keys", async () => {
    const api = client();
    const detail = useScanTask(api, id);
    await detail.load();
    expect(detail.task.value?.counts.observed_files).toBe(5);
    expect(detail.canCancel.value).toBe(true);
    expect(detail.canRetry.value).toBe(false);
    await detail.cancel();
    expect(api.cancelScanTask).toHaveBeenCalledWith(id, expect.stringMatching(/^[0-9a-f-]{36}$/i));
    expect(detail.task.value?.status).toBe("cancelled");
    expect(detail.canCancel.value).toBe(false);

    const partialApi = client({ getScanTask: vi.fn(async () => task("partial-success")) });
    const partial = useScanTask(partialApi, id);
    await partial.load();
    expect(partial.canRetry.value).toBe(true);
    await partial.retry();
    expect(partialApi.retryScanTask).toHaveBeenCalledWith(id, expect.stringMatching(/^[0-9a-f-]{36}$/i));
  });

  it("prevents duplicate writes and never invents percentage or ETA facts", async () => {
    let resolve!: (value: ScanTask) => void;
    const cancelScanTask = vi.fn(() => new Promise<ScanTask>((done) => { resolve = done; }));
    const detail = useScanTask(client({ cancelScanTask }), id);
    await detail.load();
    const first = detail.cancel();
    const second = detail.cancel();
    expect(cancelScanTask).toHaveBeenCalledTimes(1);
    expect(detail.pendingAction.value).toBe("cancel");
    resolve(task("cancelled"));
    await Promise.all([first, second]);
    expect(JSON.stringify(detail.task.value)).not.toMatch(/percent|eta/i);
  });

  it("reloads one reactive list feature with new filter/cursor context and retains its 401 handler", async () => {
    const onFailure = vi.fn(); const api = client(); const feature = useScanTasks(api, {}, onFailure);
    await feature.load({ status: "running", cursor: "cursor-a", scrollKey: "row-a" });
    expect(api.listScanTasks).toHaveBeenLastCalledWith("cursor-a");
    expect(feature.returnQuery.value).toEqual({ status: "running", cursor: "cursor-a", context: "row-a" });
    const unauthorized = new MediaFlowApiError(401, undefined);
    (api.listScanTasks as ReturnType<typeof vi.fn>).mockRejectedValueOnce(unauthorized);
    await feature.load({ status: "failed", cursor: "cursor-b", scrollKey: "row-b" });
    expect(onFailure).toHaveBeenLastCalledWith(unauthorized);
    expect(feature.returnQuery.value).toEqual({ status: "failed", cursor: "cursor-b", context: "row-b" });
  });

  it("guards retry/cancel offline and replays the exact action with one idempotency key", async () => {
    const cancelScanTask = vi.fn().mockRejectedValueOnce(new TypeError("response lost")).mockRejectedValueOnce(new TypeError("still offline")).mockResolvedValueOnce(task("cancelled"));
    const getScanTask = vi.fn().mockResolvedValueOnce(task());
    const api = client({ cancelScanTask, getScanTask }); const detail = useScanTask(api, id); await detail.load();
    await detail.cancel();
    expect(getScanTask).toHaveBeenCalledTimes(1);
    expect(cancelScanTask).toHaveBeenCalledTimes(2);
    expect(detail.actionError.value?.message).toContain("结果尚未确认");
    await detail.cancel();
    expect(cancelScanTask).toHaveBeenCalledTimes(3);
    expect(cancelScanTask.mock.calls[1]?.[1]).toBe(cancelScanTask.mock.calls[0]?.[1]);
    expect(cancelScanTask.mock.calls[2]?.[1]).toBe(cancelScanTask.mock.calls[0]?.[1]);

    const offline = useScanTask(client(), id); await offline.load();
    (offline.client.getScanTask as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new TypeError("offline")); await offline.load();
    expect(offline.canCancel.value).toBe(false); await offline.cancel();
    expect(offline.client.cancelScanTask).not.toHaveBeenCalled();
  });

  it.each(["retry", "cancel"] as const)("treats post-commit 500 as ambiguous for %s, reconciles, and replays the same key", async (kind) => {
    const initial = kind === "retry" ? task("partial-success") : task("running");
    const success = kind === "retry" ? task("queued") : task("cancelled");
    const write = vi.fn()
      .mockRejectedValueOnce(new MediaFlowApiError(500, { error: { code: "internal.error", message: "audit failed", request_id: id } }))
      .mockResolvedValueOnce(success);
    const getScanTask = vi.fn().mockResolvedValue(initial);
    const api = client({ getScanTask, ...(kind === "retry" ? { retryScanTask: write } : { cancelScanTask: write }) });
    const detail = useScanTask(api, id); await detail.load();
    await detail[kind]();
    expect(getScanTask).toHaveBeenCalledTimes(1);
    expect(write).toHaveBeenCalledTimes(2);
    expect(write.mock.calls[1]?.[1]).toBe(write.mock.calls[0]?.[1]);
    expect(detail.task.value?.status).toBe(success.status);
  });

  it("renders and focuses retry/cancel action errors while preserving the REST snapshot", async () => {
    setActivePinia(createPinia());
    const cancelScanTask = vi.fn(async () => { throw new MediaFlowApiError(409, { error: { code: "task.invalid_state", message: "/host/private", request_id: id } }); });
    const api = client({ cancelScanTask, getSession: vi.fn(async () => ({ account: { id, administrator_name: "admin" }, csrf_token: "a".repeat(43), version: "v1" as const })) });
    class Source implements EventSourceLike { addEventListener() {} removeEventListener() {} close() {} }
    const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/scan-tasks/:id", name: "scan-task", component: ScanTaskDetailView }, { path: "/scan-tasks", name: "scan-tasks", component: { template: "<p>tasks</p>" } }, { path: "/scan-tasks/:id/files", name: "scan-task-files", component: { template: "<p>files</p>" } }, { path: "/login", name: "login", component: { template: "<p>login</p>" } }] });
    await router.push(`/scan-tasks/${id}`); const wrapper = mount(ScanTaskDetailView, { attachTo: document.body, global: { plugins: [router], provide: { [identityClientKey as symbol]: api, [taskEventSourceFactoryKey as symbol]: () => new Source() } } }); await flushPromises();
    await wrapper.get("button").trigger("click"); await flushPromises();
    const summary = wrapper.get("[data-action-error]");
    expect(summary.text()).toContain("当前任务状态不允许"); expect(summary.text()).not.toContain("/host/private");
    expect(document.activeElement).toBe(summary.element); expect(wrapper.text()).toContain("已观察文件");
    wrapper.unmount();
  });
});
