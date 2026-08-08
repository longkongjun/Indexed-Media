import type { DownloadTask, MediaFlowClient } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { identityClientKey } from "../src/app/client";
import { useDownloadTasks } from "../src/features/download-tasks/useDownloadTasks";
import DownloadTaskListView from "../src/views/DownloadTaskListView.vue";

const task: DownloadTask = {
  id: "019f0000-0000-7000-8000-000000000062",
  connection_id: "019f0000-0000-7000-8000-000000000060",
  connection_display_name: "Primary qBit",
  display_name: "Ubuntu ISO",
  status: "monitoring",
  remote_status: "downloading",
  progress_basis_points: 5_000,
  failure_code: null,
  retry_at: null,
  linked: true,
  projection_version: 3,
  created_at: "2026-07-24T05:00:00Z",
  updated_at: "2026-07-24T05:01:00Z",
};

function client(overrides: Partial<MediaFlowClient> = {}): MediaFlowClient {
  return {
    listDownloadTasks: vi.fn(async () => ({ items: [task], next_cursor: null })),
    createDownloadTask: vi.fn(async () => task),
    getDownloadTask: vi.fn(async () => task),
    listDownloaderConnections: vi.fn(async () => ({ items: [{
      id: task.connection_id, kind: "qbittorrent", display_name: "Primary qBit", base_url: "https://download.test/qbit",
      enabled: true, config_version: 1, capabilities: null, health: "healthy", checked_at: null, failure_code: null,
      updated_at: task.updated_at,
    }], next_cursor: null })),
    setCsrfToken: vi.fn(),
    ...overrides,
  } as unknown as MediaFlowClient;
}

describe("download task management", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("retries a lost create response with the same idempotency key and never exposes the source", async () => {
    const create = vi.fn()
      .mockRejectedValueOnce(new TypeError("response lost"))
      .mockResolvedValueOnce(task);
    const api = client({ createDownloadTask: create });
    const feature = useDownloadTasks(api);
    Object.assign(feature.form, {
      connectionId: task.connection_id,
      displayName: "Ubuntu ISO",
      source: `magnet:?xt=urn:btih:${"A".repeat(80)}&dn=SOURCE_MUST_NOT_RENDER`,
    });
    await feature.create();
    expect(create).toHaveBeenCalledTimes(2);
    expect(create.mock.calls[1]?.[1]).toBe(create.mock.calls[0]?.[1]);
    expect(feature.form.source).toBe("");
    expect(JSON.stringify(feature.items.value)).not.toContain("SOURCE_MUST_NOT_RENDER");
  });

  it("passes stable server filters and retains the last projection while offline", async () => {
    const list = vi.fn()
      .mockResolvedValueOnce({ items: [task], next_cursor: "next" })
      .mockRejectedValueOnce(new TypeError("offline"));
    const feature = useDownloadTasks(client({ listDownloadTasks: list }), { connectionId: task.connection_id, status: "monitoring", query: "Ubuntu" });
    await feature.load();
    expect(list).toHaveBeenCalledWith({ connectionId: task.connection_id, status: "monitoring", query: "Ubuntu" });
    await feature.load();
    expect(feature.items.value).toEqual([task]);
    expect(feature.state.value).toMatchObject({ kind: "offline", stale: true });
    expect(feature.canWrite.value).toBe(false);
  });

  it("keeps a long 390px-style source inside the input and clears it without rendering it", async () => {
    const api = client();
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: "/downloads", component: DownloadTaskListView },
      { path: "/downloads/:id", name: "download-task", component: { template: "<main>detail</main>" } },
    ] });
    await router.push("/downloads");
    const wrapper = mount(DownloadTaskListView, { global: { plugins: [router], provide: { [identityClientKey as symbol]: api } } });
    await flushPromises();
    const source = `magnet:?xt=urn:btih:${"B".repeat(200)}&dn=LONG_SOURCE_MUST_NOT_RENDER`;
    await wrapper.get("#download-source").setValue(source);
    expect(wrapper.text()).not.toContain("LONG_SOURCE_MUST_NOT_RENDER");
    expect(wrapper.get("#download-source").attributes("autocomplete")).toBe("off");
    wrapper.unmount();
  });
});
