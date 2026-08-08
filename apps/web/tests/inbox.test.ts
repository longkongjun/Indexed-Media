import type { InboxDirectory, MediaFlowClient, ScanTask } from "@mediaflow/api-client-ts";
import { MediaFlowApiError } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { identityClientKey } from "../src/app/client";
import { useInboxDirectories, validateRelativePath } from "../src/features/inbox-directories/useInboxDirectories";
import InboxDetailView from "../src/views/InboxDetailView.vue";
import InboxListView from "../src/views/InboxListView.vue";

const inbox: InboxDirectory = {
  id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c86", root_id: "incoming", relative_path: "camera/uploads",
  health: "available", last_checked_at: "2026-07-18T08:00:00Z",
};
const task: ScanTask = {
  id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c87", inbox_directory_id: inbox.id, status: "queued", recovering: false,
  counts: { visited_directories: 0, observed_files: 0, skipped_entries: 0, errors: 0 },
};

function client(overrides: Partial<MediaFlowClient> = {}): MediaFlowClient {
  const noop = vi.fn();
  return {
    getBootstrapStatus: noop, bootstrap: noop, createSession: noop, getSession: noop, deleteSession: noop,
    listDeploymentRoots: vi.fn(async () => ({ items: [{ id: "incoming", label: "家庭收件区", access: "read-write" }], next_cursor: null })),
    preflightInboxDirectory: vi.fn(async ({ root_id, relative_path }) => ({ root_id, relative_path, readable: true, overlaps_existing: false })),
    listInboxDirectories: vi.fn(async () => ({ items: [], next_cursor: null })), createInboxDirectory: vi.fn(async () => inbox),
    getInboxDirectory: vi.fn(async () => inbox), createScanTask: vi.fn(async () => task), listScanTasks: noop,
    getScanTask: noop, retryScanTask: noop, cancelScanTask: noop, listScanTaskFiles: noop, listScanTaskErrors: noop,
    setCsrfToken: noop, ...overrides,
  } as MediaFlowClient;
}

describe("inbox directory flow", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("lists safe deployment-root facts and requires matching preflight confirmation before create", async () => {
    const api = client();
    const feature = useInboxDirectories(api);
    await feature.load();
    expect(feature.state.value.kind).toBe("empty");
    expect(feature.roots.value).toEqual([{ id: "incoming", label: "家庭收件区", access: "read-write" }]);

    feature.form.rootId = "incoming";
    feature.form.relativePath = "camera/uploads";
    await feature.preflight();
    expect(feature.confirmation.value).toMatchObject({ rootLabel: "家庭收件区", relativePath: "camera/uploads", access: "read-write", readable: true, overlapsExisting: false });
    expect(feature.canCreate.value).toBe(true);
    await expect(feature.create()).resolves.toEqual(inbox);
    expect(api.createInboxDirectory).toHaveBeenCalledWith({ root_id: "incoming", relative_path: "camera/uploads" });
  });

  it("invalidates stale preflight and rejects unsafe relative forms without API access", async () => {
    const api = client();
    const feature = useInboxDirectories(api);
    await feature.load();
    feature.form.rootId = "incoming";
    feature.form.relativePath = "camera";
    await feature.preflight();
    feature.form.relativePath = "changed";
    expect(feature.canCreate.value).toBe(false);
    await expect(feature.create()).rejects.toThrow("需要重新预检");

    for (const value of ["", "/etc", "C:/Users", "../secret", "ok/../secret", "folder\\escape"]) {
      expect(validateRelativePath(value)).not.toBeNull();
    }
    feature.form.relativePath = "../secret";
    await feature.preflight();
    expect(api.preflightInboxDirectory).toHaveBeenCalledTimes(1);
  });

  it("retains safe fields and maps overlap, symlink, permission and unavailable failures", async () => {
    const cases = [
      ["inbox.overlap", "与现有收件目录重叠"], ["path.symlink_forbidden", "符号链接"],
      ["path.escape", "超出能力根"], ["root.unavailable", "能力根暂不可用"],
    ] as const;
    for (const [code, copy] of cases) {
      const api = client({ preflightInboxDirectory: vi.fn(async () => { throw new MediaFlowApiError(409, { error: { code, message: "/host/private", request_id: inbox.id } }); }) });
      const feature = useInboxDirectories(api);
      await feature.load();
      feature.form.rootId = "incoming";
      feature.form.relativePath = "camera";
      await feature.preflight();
      expect(feature.form.relativePath).toBe("camera");
      expect(feature.formError.value?.message).toContain(copy);
      expect(feature.formError.value?.message).not.toContain("/host/private");
      expect(feature.formError.value?.field).toBe(code === "root.unavailable" ? "root-id" : "relative-path");
    }
  });

  it("prevents duplicate start-scan writes and routes to the returned task", async () => {
    let resolve!: (value: ScanTask) => void;
    const createScanTask = vi.fn(() => new Promise<ScanTask>((done) => { resolve = done; }));
    const api = client({ createScanTask });
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: "/inbox-directories/:id", component: InboxDetailView }, { path: "/scan-tasks/:id", name: "scan-task", component: { template: "<p>task</p>" } },
    ] });
    await router.push(`/inbox-directories/${inbox.id}`);
    const wrapper = mount(InboxDetailView, { global: { plugins: [router], provide: { [identityClientKey as symbol]: api } } });
    await flushPromises();
    await wrapper.get("[data-start-scan]").trigger("click");
    await wrapper.get("[data-start-scan]").trigger("click");
    expect(createScanTask).toHaveBeenCalledTimes(1);
    expect((createScanTask as unknown as ReturnType<typeof vi.fn>).mock.calls[0]?.[1]).toMatch(/^[0-9a-f-]{36}$/i);
    resolve(task);
    await flushPromises();
    expect(router.currentRoute.value.fullPath).toBe(`/scan-tasks/${task.id}`);
  });

  it("disables and guards create when a confirmed form becomes offline", async () => {
    const api = client(); const feature = useInboxDirectories(api);
    await feature.load(); feature.form.rootId = "incoming"; feature.form.relativePath = "camera"; await feature.preflight();
    (api.listInboxDirectories as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new TypeError("offline"));
    await feature.load();
    expect(feature.state.value.kind).toBe("offline");
    expect(feature.canCreate.value).toBe(false);
    await expect(feature.create()).rejects.toThrow("离线");
    expect(api.createInboxDirectory).not.toHaveBeenCalled();
  });

  it("reacts to inbox cursor query changes and browser back without replacing feature state", async () => {
    const inboxB = { ...inbox, id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c91", relative_path: "page-b" };
    const listInboxDirectories = vi.fn(async (cursor?: string) => ({ items: [cursor === "cursor-b" ? inboxB : inbox], next_cursor: cursor ? null : "cursor-b" }));
    const api = client({ listInboxDirectories }); const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/inbox-directories", component: InboxListView }, { path: "/inbox-directories/:id", name: "inbox-directory", component: { template: "<p>detail</p>" } }, { path: "/login", name: "login", component: { template: "<p>login</p>" } }] });
    await router.push("/inbox-directories"); const wrapper = mount(InboxListView, { global: { plugins: [router], provide: { [identityClientKey as symbol]: api } } }); await flushPromises();
    expect(wrapper.text()).toContain("camera/uploads");
    await router.push({ path: "/inbox-directories", query: { cursor: "cursor-b" } }); await flushPromises();
    expect(wrapper.text()).toContain("page-b"); expect(listInboxDirectories).toHaveBeenLastCalledWith("cursor-b");
    router.back(); await flushPromises();
    expect(wrapper.text()).toContain("camera/uploads");
    wrapper.unmount();
  });

  it("replays start-scan with the same key after response loss and retains it while replay remains ambiguous", async () => {
    const createScanTask = vi.fn().mockRejectedValueOnce(new TypeError("response lost")).mockRejectedValueOnce(new TypeError("still offline")).mockResolvedValueOnce(task);
    const api = client({ createScanTask }); const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/inbox-directories/:id", component: InboxDetailView }, { path: "/scan-tasks/:id", name: "scan-task", component: { template: "<p>task</p>" } }] });
    await router.push(`/inbox-directories/${inbox.id}`); const wrapper = mount(InboxDetailView, { global: { plugins: [router], provide: { [identityClientKey as symbol]: api } } }); await flushPromises();
    await wrapper.get("[data-start-scan]").trigger("click"); await flushPromises();
    expect(createScanTask).toHaveBeenCalledTimes(2); expect(wrapper.get("[data-error-summary]").text()).toContain("结果尚未确认");
    expect(createScanTask.mock.calls[1]?.[1]).toBe(createScanTask.mock.calls[0]?.[1]);
    await wrapper.get("[data-start-scan]").trigger("click"); await flushPromises();
    expect(createScanTask).toHaveBeenCalledTimes(3);
    expect(createScanTask.mock.calls[2]?.[1]).toBe(createScanTask.mock.calls[0]?.[1]);
    expect(router.currentRoute.value.fullPath).toBe(`/scan-tasks/${task.id}`);
  });

  it("replays the exact post-commit create result instead of matching an older active inbox task", async () => {
    const older = { ...task, id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c90", status: "running" as const };
    const completed = { ...task, id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c91", status: "completed" as const };
    const createScanTask = vi.fn()
      .mockRejectedValueOnce(new MediaFlowApiError(500, { error: { code: "internal.error", message: "audit failed", request_id: inbox.id } }))
      .mockResolvedValueOnce(completed);
    const listScanTasks = vi.fn(async () => ({ items: [older], next_cursor: null }));
    const api = client({ createScanTask, listScanTasks });
    const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/inbox-directories/:id", component: InboxDetailView }, { path: "/scan-tasks/:id", name: "scan-task", component: { template: "<p>task</p>" } }] });
    await router.push(`/inbox-directories/${inbox.id}`);
    const wrapper = mount(InboxDetailView, { global: { plugins: [router], provide: { [identityClientKey as symbol]: api } } }); await flushPromises();
    await wrapper.get("[data-start-scan]").trigger("click"); await flushPromises();
    expect(listScanTasks).not.toHaveBeenCalled();
    expect(createScanTask).toHaveBeenCalledTimes(2);
    expect(createScanTask.mock.calls[1]?.[1]).toBe(createScanTask.mock.calls[0]?.[1]);
    expect(router.currentRoute.value.fullPath).toBe(`/scan-tasks/${completed.id}`);
    expect(router.currentRoute.value.fullPath).not.toBe(`/scan-tasks/${older.id}`);
    wrapper.unmount();
  });
});
