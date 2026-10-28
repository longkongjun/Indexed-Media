import { createMediaFlowClient } from "@mediaflow/api-client-ts";
import { createPinia } from "pinia";
import { describe, expect, it, vi } from "vitest";
import { safeReturnLocation } from "../src/app/session";
import { createAppRouter } from "../src/router";

describe("TV pairing entry", () => {
  it("uses an authenticated route with a safe login return location", () => {
    const router = createAppRouter({
      pinia: createPinia(), getBootstrapStatus: vi.fn(), getSession: vi.fn(),
    });
    expect(router.resolve("/devices/tv-pairing").meta.requiresAuth).toBe(true);
    expect(safeReturnLocation("/devices/tv-pairing")).toBe("/devices/tv-pairing");
    expect(safeReturnLocation("/devices/tv-pairing-evil")).toBeNull();
  });

  it("sends the scoped mutation through the existing same-origin CSRF client", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValue(new Response(JSON.stringify({
      pairing_code: "ABCDEFG2", expires_at: "2030-01-01T00:00:00Z", access_mode: "media-read-only", version: "v1",
    }), { status: 201, headers: { "content-type": "application/json" } }));
    const client = createMediaFlowClient({ fetch });
    client.setCsrfToken("scoped-csrf");
    await client.createMediaReadOnlyDevicePairingCode();
    const [path, init] = fetch.mock.calls[0]!;
    expect(path).toBe("/api/v1/device-pairing-codes/media-read-only");
    expect(init?.method).toBe("POST");
    expect(init?.credentials).toBe("same-origin");
    expect(new Headers(init?.headers).get("X-CSRF-Token")).toBe("scoped-csrf");
    expect(init?.body).toBeUndefined();
  });
});

import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach } from "vitest";
import { identityClientKey } from "../src/app/client";
import { useSessionStore } from "../src/app/session";
import TvPairingView from "../src/views/TvPairingView.vue";
import type { DevicePairingCode, MediaFlowClient } from "@mediaflow/api-client-ts";

function pairingHarness(issue: MediaFlowClient["createMediaReadOnlyDevicePairingCode"]) {
  const pinia = createPinia();
  const session = useSessionStore(pinia);
  session.establish({ account: { id: "admin-id", administrator_name: "admin" }, csrf_token: "csrf", version: "v1" });
  const client = createMediaFlowClient();
  client.createMediaReadOnlyDevicePairingCode = issue;
  const wrapper = mount(TvPairingView, { global: { plugins: [pinia], provide: { [identityClientKey as symbol]: client } } });
  return { wrapper, session };
}

function receipt(mode: "administrator" | "media-read-only" = "media-read-only"): DevicePairingCode {
  return { pairing_code: "ABCDEFG2", expires_at: new Date(Date.now() + 600_000).toISOString(), access_mode: mode, version: "v1" };
}

describe("TV pairing page", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("displays only a current read-only receipt and hides it on expiry", async () => {
    const issue = vi.fn(async () => receipt());
    const { wrapper } = pairingHarness(issue);
    await wrapper.get("button").trigger("click");
    await flushPromises();
    expect(issue).toHaveBeenCalledTimes(1);
    expect(wrapper.get("output").text()).toBe("ABCDEFG2");
    expect(wrapper.text()).toContain("有效期内仅可使用一次");
    await vi.advanceTimersByTimeAsync(600_000);
    expect(wrapper.find("output").exists()).toBe(false);
    expect(wrapper.get('[role="alert"]').text()).toContain("已过期");
    wrapper.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("rejects an administrator receipt instead of displaying its code", async () => {
    const { wrapper } = pairingHarness(async () => receipt("administrator"));
    await wrapper.get("button").trigger("click");
    await flushPromises();
    expect(wrapper.find("output").exists()).toBe(false);
    expect(wrapper.get('[role="alert"]').text()).toContain("无效");
    wrapper.unmount();
  });

  it("clears displayed codes and ignores late requests across logout and reauthentication", async () => {
    let resolve: ((value: DevicePairingCode) => void) | undefined;
    const issue = vi.fn<MediaFlowClient["createMediaReadOnlyDevicePairingCode"]>()
      .mockResolvedValueOnce(receipt())
      .mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    const { wrapper, session } = pairingHarness(issue);
    await wrapper.get("button").trigger("click");
    await flushPromises();
    expect(wrapper.find("output").exists()).toBe(true);
    session.resetSensitiveState();
    await wrapper.vm.$nextTick();
    expect(wrapper.find("output").exists()).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
    session.establish({ account: { id: "admin-id", administrator_name: "admin" }, csrf_token: "new-csrf", version: "v1" });
    await wrapper.vm.$nextTick();
    await wrapper.get("button").trigger("click");
    expect(wrapper.get("button").attributes("disabled")).toBeDefined();
    session.resetSensitiveState();
    session.establish({ account: { id: "admin-id", administrator_name: "admin" }, csrf_token: "another-csrf", version: "v1" });
    resolve?.(receipt());
    await flushPromises();
    expect(wrapper.find("output").exists()).toBe(false);
    wrapper.unmount();
  });

  it("does not create timers or leak an error after unmount during a request", async () => {
    let resolve: ((value: DevicePairingCode) => void) | undefined;
    const { wrapper } = pairingHarness(() => new Promise((done) => { resolve = done; }));
    await wrapper.get("button").trigger("click");
    wrapper.unmount();
    resolve?.(receipt());
    await flushPromises();
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("TV pairing browser lifecycle", () => {
  it("hides a code before a document enters browser history cache", async () => {
    const { wrapper } = pairingHarness(async () => receipt());
    await wrapper.get("button").trigger("click");
    await flushPromises();
    expect(wrapper.find("output").exists()).toBe(true);
    window.dispatchEvent(new Event("pagehide"));
    await wrapper.vm.$nextTick();
    expect(wrapper.find("output").exists()).toBe(false);
    wrapper.unmount();
  });
});
