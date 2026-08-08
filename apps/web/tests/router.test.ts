import type { BootstrapStatusResponse, SessionResponse } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia, type Pinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useConnectivityStore } from "../src/app/connectivity";
import { useSessionStore } from "../src/app/session";
import App from "../src/App.vue";
import { createAppRouter, type RouterDependencies } from "../src/router";

const session: SessionResponse = {
  account: { id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c86", administrator_name: "admin" },
  csrf_token: "a".repeat(43),
  version: "v1",
};

function status(requiresInitialization: boolean): BootstrapStatusResponse {
  return { requires_initialization: requiresInitialization, version: "v1" };
}

let pinia: Pinia;

function appRouter(dependencies: Omit<RouterDependencies, "pinia">, owner = pinia) {
  return createAppRouter({ ...dependencies, pinia: owner });
}

describe("identity router", () => {
  beforeEach(() => { pinia = createPinia(); setActivePinia(pinia); });

  it("routes an uninitialized business request to setup without looking up a session", async () => {
    const getSession = vi.fn(async () => session);
    const router = appRouter({
      getBootstrapStatus: async () => status(true),
      getSession,
    });

    await router.push("/tasks?state=running");
    await router.isReady();

    expect(router.currentRoute.value.name).toBe("setup");
    expect(getSession).not.toHaveBeenCalled();
  });

  it("routes an initialized setup request to login", async () => {
    const router = appRouter({
      getBootstrapStatus: async () => status(false),
      getSession: async () => { throw new Error("public routes do not need a session"); },
    });

    await router.push("/setup");
    await router.isReady();

    expect(router.currentRoute.value.name).toBe("login");
  });

  it("refreshes bootstrap state when successful setup moves to login", async () => {
    const getBootstrapStatus = vi.fn()
      .mockResolvedValueOnce(status(true))
      .mockResolvedValueOnce(status(false));
    const router = appRouter({ getBootstrapStatus, getSession: async () => session });
    await router.push("/setup");

    await router.push("/login");

    expect(router.currentRoute.value.name).toBe("login");
    expect(getBootstrapStatus).toHaveBeenCalledTimes(2);
  });

  it("allows an authenticated session to enter every protected business route", async () => {
    const router = appRouter({
      getBootstrapStatus: async () => status(false),
      getSession: async () => session,
    });

    for (const path of [
      "/inbox-directories", "/inbox-directories/inbox-1", "/tasks", "/tasks/task-1",
      "/review-cases/case-1",
      "/media", "/media/media-1",
      "/connections/downloaders", "/connections/downloaders/connection-1",
      "/downloads", "/downloads/download-1",
      "/automation/sources", "/automation/sources/source-1", "/automation/events/event-1",
      "/organization/targets", "/organization/targets/target-1",
      "/scan-tasks", "/scan-tasks/task-1", "/scan-tasks/task-1/files",
    ]) {
      await router.push(path);
      expect(router.currentRoute.value.fullPath).toBe(path);
      expect(router.currentRoute.value.meta.requiresAuth).toBe(true);
    }
    expect(useSessionStore().account?.administrator_name).toBe("admin");
  });

  it("redirects an old scan detail link only when it carries explicit scan context", async () => {
    const router = appRouter({
      getBootstrapStatus: async () => status(false),
      getSession: async () => session,
    });

    await router.push("/tasks/task-1?scan_context=scan-task&status=running");
    expect(router.currentRoute.value.fullPath).toBe("/scan-tasks/task-1?status=running");

    await router.push("/tasks/task-1");
    expect(router.currentRoute.value.name).toBe("task");
  });

  it("clears sensitive state and saves only a safe return location when the session expires", async () => {
    const client = { setCsrfToken: vi.fn() };
    const store = useSessionStore();
    store.configureClient(client);
    store.establish(session);
    const router = appRouter({
      getBootstrapStatus: async () => status(false),
      getSession: async () => { throw Object.assign(new Error("expired"), { status: 401 }); },
    });

    await router.push("/tasks/task-1?from=inbox");
    await router.isReady();

    expect(router.currentRoute.value.name).toBe("login");
    expect(store.account).toBeNull();
    expect(store.csrfToken).toBeNull();
    expect(store.returnLocation).toBe("/tasks/task-1?from=inbox");
    expect(client.setCsrfToken).toHaveBeenLastCalledWith(null);
  });

  it.each(["https://evil.example/tasks", "//evil.example/tasks", "javascript:alert(1)", "/tasks#secret", "/login"])(
    "rejects unsafe or non-business return location %s",
    (location) => {
      const store = useSessionStore();
      store.saveReturnLocation(location);
      expect(store.returnLocation).toBeNull();
    },
  );

  it("exposes a retryable offline state instead of treating bootstrap failure as uninitialized", async () => {
    const router = appRouter({
      getBootstrapStatus: async () => { throw new TypeError("fetch failed"); },
      getSession: async () => session,
    });

    await router.push("/tasks");

    const connectivity = useConnectivityStore();
    expect(connectivity.state).toBe("offline");
    expect(connectivity.pendingLocation).toBe("/tasks");
    expect(router.currentRoute.value.name).not.toBe("setup");
  });

  it("renders the offline state and retries the pending protected location", async () => {
    pinia = createPinia();
    setActivePinia(pinia);
    let attempts = 0;
    const router = appRouter({
      getBootstrapStatus: async () => {
        attempts += 1;
        if (attempts === 1) throw new TypeError("fetch failed");
        return status(false);
      },
      getSession: async () => session,
    }, pinia);
    await router.push("/tasks");
    const wrapper = mount(App, { global: { plugins: [pinia, router] } });

    expect(wrapper.text()).toContain("Offline");
    expect(wrapper.text()).toContain("重试连接");
    await wrapper.get("button").trigger("click");
    await flushPromises();

    expect(router.currentRoute.value.name).toBe("tasks");
    expect(wrapper.text()).not.toContain("Offline");
    wrapper.unmount();
  });

  it("does not restore a protected page from an old cookie when back follows an offline logout", async () => {
    const getSession = vi.fn(async () => session);
    const router = appRouter({
      getBootstrapStatus: async () => status(false),
      getSession,
    });
    await router.push("/inbox-directories");
    await router.push("/tasks");
    const store = useSessionStore();
    await store.logout(router, async () => { throw new TypeError("offline"); });
    const navigated = new Promise<void>((resolve) => {
      const remove = router.afterEach(() => { remove(); resolve(); });
    });

    router.back();
    await navigated;

    expect(router.currentRoute.value.name).toBe("login");
    expect(getSession).toHaveBeenCalledTimes(2);
    expect(store.account).toBeNull();
  });

  it("rechecks bootstrap state after a session connectivity failure before retrying", async () => {
    const getBootstrapStatus = vi.fn()
      .mockResolvedValueOnce(status(false))
      .mockResolvedValueOnce(status(true));
    const getSession = vi.fn(async () => { throw new TypeError("Core offline"); });
    const router = appRouter({ getBootstrapStatus, getSession });
    await router.push("/tasks");
    expect(useConnectivityStore(pinia).state).toBe("offline");

    await router.replace("/tasks");

    expect(router.currentRoute.value.name).toBe("setup");
    expect(getBootstrapStatus).toHaveBeenCalledTimes(2);
    expect(getSession).toHaveBeenCalledTimes(1);
  });

  it("keeps each router guard isolated to its owning Pinia instance", async () => {
    const piniaA = createPinia();
    const piniaB = createPinia();
    const storeA = useSessionStore(piniaA);
    const storeB = useSessionStore(piniaB);
    storeA.establish(session);
    storeB.establish({ ...session, account: { ...session.account, administrator_name: "other" } });
    setActivePinia(piniaB);
    const routerA = appRouter({
      getBootstrapStatus: async () => status(false),
      getSession: async () => { throw Object.assign(new Error("expired"), { status: 401 }); },
    }, piniaA);

    await routerA.push("/tasks?owner=a");

    expect(storeA.account).toBeNull();
    expect(storeA.returnLocation).toBe("/tasks?owner=a");
    expect(storeB.account?.administrator_name).toBe("other");
    expect(storeB.returnLocation).toBeNull();
    expect(useConnectivityStore(piniaA).state).toBe("online");
    expect(useConnectivityStore(piniaB).state).toBe("loading");
  });
});
