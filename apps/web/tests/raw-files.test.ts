import type { MediaFlowClient } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { identityClientKey } from "../src/app/client";
import { useRawFiles } from "../src/features/raw-files/useRawFiles";
import RawFilesView from "../src/views/RawFilesView.vue";

const taskId = "018f0f10-8bc1-7a5e-8e5a-2dc913d23c87";
function client(): MediaFlowClient {
  return {
    listScanTaskFiles: vi.fn(async (_id, cursor) => ({ items: cursor ? [] : [{ id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c89", relative_path: "safe/movie.mkv", size_bytes: 42, modified_at: "2026-07-18T08:00:00Z" }], next_cursor: cursor ? null : "opaque-next" })),
    listScanTaskErrors: vi.fn(async () => ({ items: [{ id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c90", code: "entry.permission_denied", relative_path: "safe/broken.mkv", occurrences: 1 }], next_cursor: null })),
  } as unknown as MediaFlowClient;
}

describe("raw files and scan errors", () => {
  it("loads stable cursor results without fabricating numeric pages", async () => {
    const api = client();
    const feature = useRawFiles(api, taskId, { cursor: null });
    await feature.load();
    expect(feature.state.value.kind).toBe("content");
    expect(feature.files.value[0]?.relative_path).toBe("safe/movie.mkv");
    expect(feature.nextCursor.value).toBe("opaque-next");
    expect(feature.pageLabel.value).toBe("当前结果");
    expect(feature.pageLabel.value).not.toMatch(/\d/);
    await feature.loadErrors();
    expect(feature.errors.value[0]?.code).toBe("entry.permission_denied");
  });

  it("distinguishes empty, error, and offline-with-stale-content", async () => {
    const api = client();
    const feature = useRawFiles(api, taskId, { cursor: null });
    await feature.load();
    (api.listScanTaskFiles as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new TypeError("offline"));
    await feature.load();
    expect(feature.state.value).toMatchObject({ kind: "offline", stale: true });
    expect(feature.files.value).toHaveLength(1);

    (api.listScanTaskFiles as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ items: [], next_cursor: null });
    await feature.load();
    expect(feature.state.value.kind).toBe("empty");
    (api.listScanTaskFiles as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("/host/private"));
    await feature.load();
    expect(feature.state.value.kind).toBe("error");
    expect(feature.errorMessage.value).not.toContain("/host/private");
  });

  it("reacts independently to file/error cursors and browser back while retaining the authenticated handler", async () => {
    setActivePinia(createPinia()); const onFailure = vi.fn();
    const listScanTaskFiles = vi.fn(async (_id: string, cursor?: string) => ({ items: [{ id: `${taskId.slice(0, -2)}${cursor ? "91" : "89"}`, relative_path: cursor ? "page-b.mkv" : "page-a.mkv", size_bytes: 42, modified_at: "2026-07-18T08:00:00Z" }], next_cursor: cursor ? null : "file-b" }));
    const listScanTaskErrors = vi.fn(async (_id: string, cursor?: string) => ({ items: [{ id: `${taskId.slice(0, -2)}${cursor ? "92" : "90"}`, code: "entry.permission_denied", relative_path: cursor ? "error-b.mkv" : "error-a.mkv", occurrences: 1 }], next_cursor: cursor ? null : "error-b" }));
    const api = { ...client(), listScanTaskFiles, listScanTaskErrors };
    const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/scan-tasks/:id/files", component: RawFilesView }, { path: "/scan-tasks/:id", name: "scan-task", component: { template: "<p>task</p>" } }, { path: "/login", name: "login", component: { template: "<p>login</p>" } }] });
    await router.push(`/scan-tasks/${taskId}/files`); const wrapper = mount(RawFilesView, { global: { plugins: [router], provide: { [identityClientKey as symbol]: api } } }); await flushPromises();
    expect(wrapper.text()).toContain("page-a.mkv"); expect(wrapper.text()).toContain("error-a.mkv");
    await router.push({ query: { cursor: "file-b" } }); await flushPromises(); expect(wrapper.text()).toContain("page-b.mkv");
    await router.push({ query: { cursor: "file-b", errorCursor: "error-b" } }); await flushPromises(); expect(wrapper.text()).toContain("error-b.mkv");
    router.back(); await flushPromises(); expect(wrapper.text()).toContain("error-a.mkv");
    router.back(); await flushPromises(); expect(wrapper.text()).toContain("page-a.mkv");
    expect(listScanTaskFiles.mock.calls.map((call) => call[1])).toEqual(expect.arrayContaining([undefined, "file-b"]));
    expect(listScanTaskErrors.mock.calls.map((call) => call[1])).toEqual(expect.arrayContaining([undefined, "error-b"]));

    const feature = useRawFiles(api, taskId, {}, onFailure);
    listScanTaskFiles.mockRejectedValueOnce(Object.assign(new Error("expired"), { status: 401 })); await feature.load("file-c");
    listScanTaskErrors.mockRejectedValueOnce(Object.assign(new Error("expired"), { status: 401 })); await feature.loadErrors("error-c");
    expect(onFailure).toHaveBeenCalledTimes(2); wrapper.unmount();
  });
});
