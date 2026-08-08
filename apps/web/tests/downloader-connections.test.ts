import type { DownloaderConnection, MediaFlowClient } from "@mediaflow/api-client-ts";
import { MediaFlowApiError } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { identityClientKey } from "../src/app/client";
import { useDownloaderConnections } from "../src/features/downloader-connections/useDownloaderConnections";
import DownloaderConnectionsView from "../src/views/DownloaderConnectionsView.vue";

const connection: DownloaderConnection = {
  id: "019f0000-0000-7000-8000-000000000060",
  kind: "qbittorrent",
  display_name: "Primary qBit",
  base_url: "https://download.test/qbit",
  enabled: true,
  config_version: 1,
  capabilities: { manual_add: true, task_monitoring: true, product_version: "5.1.2", api_version: "2.11.4" },
  health: "healthy",
  checked_at: "2026-07-24T05:00:00Z",
  failure_code: null,
  updated_at: "2026-07-24T05:00:00Z",
};

function client(overrides: Partial<MediaFlowClient> = {}): MediaFlowClient {
  return {
    listDownloaderConnections: vi.fn(async () => ({ items: [connection], next_cursor: null })),
    testDownloaderConnection: vi.fn(async () => ({ reachable: true, health: "healthy", capabilities: connection.capabilities, failure_code: null, checked_at: connection.checked_at! })),
    createDownloaderConnection: vi.fn(async () => connection),
    getDownloaderConnection: vi.fn(async () => connection),
    updateDownloaderConnection: vi.fn(async () => connection),
    deleteDownloaderConnection: vi.fn(async () => undefined),
    setCsrfToken: vi.fn(),
    ...overrides,
  } as unknown as MediaFlowClient;
}

describe("downloader connection management", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("tests without saving, then saves only after credentials are re-entered", async () => {
    const api = client();
    const feature = useDownloaderConnections(api);
    Object.assign(feature.form, {
      kind: "qbittorrent", displayName: "Home qBit", baseUrl: "https://nas.test/qbit",
      username: "admin", password: "PASSWORD_MUST_NOT_REMAIN", enabled: true,
    });
    await feature.testCandidate();
    expect(api.testDownloaderConnection).toHaveBeenCalledTimes(1);
    expect(api.createDownloaderConnection).not.toHaveBeenCalled();
    expect(feature.form.password).toBe("");
    feature.form.username = "admin";
    feature.form.password = "new-password";
    await feature.save();
    expect(api.createDownloaderConnection).toHaveBeenCalledTimes(1);
    expect(feature.form.password).toBe("");
    expect(feature.items.value[0]?.id).toBe(connection.id);
  });

  it("keeps non-secret draft fields and focuses a safe conflict while clearing credentials", async () => {
    const api = client({
      createDownloaderConnection: vi.fn(async () => {
        throw new MediaFlowApiError(409, { error: { code: "request.conflict", message: "private upstream body", request_id: connection.id } });
      }),
    });
    const feature = useDownloaderConnections(api);
    Object.assign(feature.form, {
      kind: "transmission", displayName: "Basement", baseUrl: "http://nas.test:9091",
      username: "private-user", password: "private-password", enabled: true,
    });
    await feature.save();
    expect(feature.form.displayName).toBe("Basement");
    expect(feature.form.baseUrl).toBe("http://nas.test:9091");
    expect(feature.form.username).toBe("");
    expect(feature.form.password).toBe("");
    expect(feature.formError.value?.message).toContain("稍后刷新");
    expect(feature.formError.value?.message).not.toContain("private upstream body");
  });

  it("retains the last connection projection and disables writes while offline", async () => {
    const list = vi.fn()
      .mockResolvedValueOnce({ items: [connection], next_cursor: null })
      .mockRejectedValueOnce(new TypeError("offline"));
    const feature = useDownloaderConnections(client({ listDownloaderConnections: list }));
    await feature.load();
    await feature.load();
    expect(feature.items.value).toEqual([connection]);
    expect(feature.state.value).toMatchObject({ kind: "offline", stale: true });
    expect(feature.canWrite.value).toBe(false);
  });

  it("renders redacted connection facts and never renders credential values", async () => {
    const api = client();
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: "/connections/downloaders", component: DownloaderConnectionsView },
      { path: "/connections/downloaders/:id", name: "downloader-connection", component: { template: "<main>detail</main>" } },
    ] });
    await router.push("/connections/downloaders");
    const wrapper = mount(DownloaderConnectionsView, { global: { plugins: [router], provide: { [identityClientKey as symbol]: api } } });
    await flushPromises();
    expect(wrapper.text()).toContain("Primary qBit");
    expect(wrapper.text()).toContain("healthy");
    expect(wrapper.text()).not.toContain("password");
    wrapper.unmount();
  });
});
