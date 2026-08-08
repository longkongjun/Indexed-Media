import type { MediaFlowClient, SessionResponse } from "@mediaflow/api-client-ts";
import { MediaFlowApiError } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createRouter, createMemoryHistory } from "vue-router";
import { identityClientKey } from "../src/app/client";
import { safeReturnLocation, useSessionStore } from "../src/app/session";
import LoginView from "../src/views/LoginView.vue";
import SetupView from "../src/views/SetupView.vue";
import { createAppRouter } from "../src/router";

const session: SessionResponse = {
  account: { id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c86", administrator_name: "admin" },
  csrf_token: "b".repeat(43),
  version: "v1",
};

function apiError(status: number, code: "auth.invalid_credentials" | "auth.rate_limited" | "bootstrap.invalid_secret" | "bootstrap.already_completed" | "internal.error", details?: Record<string, string | number | boolean | null>) {
  return new MediaFlowApiError(status, {
    error: { code, message: code, request_id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c86", details },
  });
}

function client(overrides: Partial<MediaFlowClient> = {}): MediaFlowClient {
  return {
    getBootstrapStatus: vi.fn(), bootstrap: vi.fn(), createSession: vi.fn(), getSession: vi.fn(),
    deleteSession: vi.fn(), listDeploymentRoots: vi.fn(), preflightInboxDirectory: vi.fn(),
    listInboxDirectories: vi.fn(), createInboxDirectory: vi.fn(), getInboxDirectory: vi.fn(),
    createScanTask: vi.fn(), listScanTasks: vi.fn(), getScanTask: vi.fn(), retryScanTask: vi.fn(),
    cancelScanTask: vi.fn(), listScanTaskFiles: vi.fn(), listScanTaskErrors: vi.fn(),
    getTmdbIntegration: vi.fn(), testTmdbConnection: vi.fn(), putTmdbIntegration: vi.fn(), deleteTmdbIntegration: vi.fn(),
    getDiscoveryPolicy: vi.fn(), putDiscoveryPolicy: vi.fn(), listProcessingTasks: vi.fn(), getProcessingTask: vi.fn(),
    getProcessingTaskIdentification: vi.fn(), retryProcessingTask: vi.fn(), cancelProcessingTask: vi.fn(),
    listOrganizationTargets: vi.fn(), preflightOrganizationTarget: vi.fn(), createOrganizationTarget: vi.fn(),
    getOrganizationTarget: vi.fn(), updateOrganizationTarget: vi.fn(), deleteOrganizationTarget: vi.fn(),
    getProcessingTaskOrganization: vi.fn(), recalculateProcessingTaskOrganization: vi.fn(),
    executeProcessingTaskOrganization: vi.fn(), rollbackProcessingTaskOrganization: vi.fn(),
    listReviewCases: vi.fn(), getReviewCase: vi.fn(), searchReviewCandidates: vi.fn(),
    submitReviewDecision: vi.fn(), listMediaItems: vi.fn(), getMediaItem: vi.fn(), setCsrfToken: vi.fn(),
    listDownloaderConnections: vi.fn(), createDownloaderConnection: vi.fn(), getDownloaderConnection: vi.fn(),
    updateDownloaderConnection: vi.fn(), deleteDownloaderConnection: vi.fn(), testDownloaderConnection: vi.fn(),
    listDownloadTasks: vi.fn(), createDownloadTask: vi.fn(), getDownloadTask: vi.fn(),
    listAutomationSources: vi.fn(), createAutomationSource: vi.fn(), getAutomationSource: vi.fn(),
    updateAutomationSource: vi.fn(), deleteAutomationSource: vi.fn(), testAutomationSource: vi.fn(),
    rotateAutomationWebhookSecret: vi.fn(), listAutomationEvents: vi.fn(), getAutomationEvent: vi.fn(),
    retryAutomationEvent: vi.fn(), cancelAutomationEvent: vi.fn(), getIdentificationEnhancer: vi.fn(),
    testIdentificationEnhancer: vi.fn(), putIdentificationEnhancer: vi.fn(),
    ...overrides,
  };
}

async function harness(component: typeof LoginView | typeof SetupView, api: MediaFlowClient, path: string) {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/setup", name: "setup", component: SetupView },
      { path: "/login", name: "login", component: LoginView },
      { path: "/inbox-directories", name: "inbox-directories", component: { template: "<p>收件目录</p>" } },
      { path: "/tasks", name: "tasks", component: { template: "<p>任务</p>" } },
    ],
  });
  await router.push(path);
  await router.isReady();
  const wrapper = mount(component, { attachTo: document.body, global: { plugins: [router], provide: { [identityClientKey as symbol]: api } } });
  return { router, wrapper };
}

describe("identity views", () => {
  beforeEach(() => { document.body.innerHTML = ""; setActivePinia(createPinia()); });

  it("accepts explicit processing, scan and inbox return routes while rejecting prefix lookalikes", () => {
    expect(safeReturnLocation("/tasks/task-1?view=pending")).toBe("/tasks/task-1?view=pending");
    expect(safeReturnLocation("/review-cases/case-1")).toBe("/review-cases/case-1");
    expect(safeReturnLocation("/media/media-1?type=movie")).toBe("/media/media-1?type=movie");
    expect(safeReturnLocation("/scan-tasks/task-1?context=task-1")).toBe("/scan-tasks/task-1?context=task-1");
    expect(safeReturnLocation("/inbox-directories/inbox-1")).toBe("/inbox-directories/inbox-1");
    expect(safeReturnLocation("/downloads/download-1")).toBe("/downloads/download-1");
    expect(safeReturnLocation("/connections/downloaders/connection-1")).toBe("/connections/downloaders/connection-1");
    expect(safeReturnLocation("/organization/targets/target-1?tab=rules")).toBe("/organization/targets/target-1?tab=rules");
    expect(safeReturnLocation("/automation/sources/source-1?tab=events")).toBe("/automation/sources/source-1?tab=events");
    expect(safeReturnLocation("/automation/events/event-1")).toBe("/automation/events/event-1");
    expect(safeReturnLocation("/scan-tasks-evil/task-1")).toBeNull();
  });

  it("submits setup once, never stores secrets, and redirects to login", async () => {
    let resolveBootstrap!: () => void;
    const bootstrap = vi.fn(() => new Promise<Awaited<ReturnType<MediaFlowClient["bootstrap"]>>>((resolve) => {
      resolveBootstrap = () => resolve({ account: session.account, version: "v1" });
    }));
    const api = client({ bootstrap });
    const { wrapper, router } = await harness(SetupView, api, "/setup");
    await wrapper.get("#bootstrap-secret").setValue("one-time-secret");
    await wrapper.get("#administrator-name").setValue("admin");
    await wrapper.get("#new-password").setValue("correct horse battery staple");

    await wrapper.get("form").trigger("submit");
    await wrapper.get("form").trigger("submit");
    expect(bootstrap).toHaveBeenCalledTimes(1);
    expect(wrapper.get("button[type=submit]").attributes("disabled")).toBeDefined();
    resolveBootstrap();
    await flushPromises();

    expect(router.currentRoute.value.name).toBe("login");
    expect(JSON.stringify(useSessionStore().$state)).not.toContain("one-time-secret");
    expect(JSON.stringify(useSessionStore().$state)).not.toContain("correct horse");
  });

  it("focuses a stable setup error summary and associates the secret field", async () => {
    const api = client({ bootstrap: vi.fn(async () => { throw apiError(401, "bootstrap.invalid_secret"); }) });
    const { wrapper } = await harness(SetupView, api, "/setup");
    await wrapper.get("#bootstrap-secret").setValue("wrong");
    await wrapper.get("#administrator-name").setValue("admin");
    await wrapper.get("#new-password").setValue("correct horse battery staple");
    await wrapper.get("form").trigger("submit");
    await flushPromises();

    const summary = wrapper.get("[data-error-summary]");
    expect(summary.text()).toContain("引导密钥无效或已过期");
    expect(summary.attributes("tabindex")).toBe("-1");
    expect(document.activeElement).toBe(summary.element);
    expect(wrapper.get("#bootstrap-secret").attributes("aria-describedby")).toContain("setup-error");
    expect(wrapper.get("#new-password").attributes("aria-describedby")).toBe("password-requirements");
    expect(wrapper.get("#new-password").attributes("aria-invalid")).toBe("false");
  });

  it("refetches bootstrap state and enters login when another browser completed setup", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const getBootstrapStatus = vi.fn()
      .mockResolvedValueOnce({ requires_initialization: true, version: "v1" as const })
      .mockResolvedValueOnce({ requires_initialization: false, version: "v1" as const });
    const api = client({
      getBootstrapStatus,
      bootstrap: vi.fn(async () => { throw apiError(409, "bootstrap.already_completed"); }),
    });
    const router = createAppRouter({ pinia, getBootstrapStatus, getSession: api.getSession });
    await router.push("/setup");
    const wrapper = mount(SetupView, { attachTo: document.body, global: {
      plugins: [pinia, router],
      provide: { [identityClientKey as symbol]: api },
    } });
    await wrapper.get("#bootstrap-secret").setValue("one-time-secret");
    await wrapper.get("#administrator-name").setValue("admin");
    await wrapper.get("#new-password").setValue("correct horse battery staple");

    await wrapper.get("form").trigger("submit");
    await flushPromises();

    expect(router.currentRoute.value.name).toBe("login");
    expect(getBootstrapStatus).toHaveBeenCalledTimes(2);
    wrapper.unmount();
  });

  it("clears setup credentials and links the summary to the missing administrator name", async () => {
    const { wrapper } = await harness(SetupView, client(), "/setup");
    await wrapper.get("#bootstrap-secret").setValue("one-time-secret");
    await wrapper.get("#new-password").setValue("correct horse battery staple");

    await wrapper.get("form").trigger("submit");
    await flushPromises();

    expect((wrapper.get("#bootstrap-secret").element as HTMLInputElement).value).toBe("");
    expect((wrapper.get("#new-password").element as HTMLInputElement).value).toBe("");
    expect(wrapper.findAll("[data-error-summary] a").map((link) => link.attributes("href")))
      .toEqual(["#administrator-name", "#bootstrap-secret", "#new-password"]);
    expect(wrapper.get("#administrator-name").attributes("aria-invalid")).toBe("true");
    expect(wrapper.get("#administrator-name").attributes("aria-describedby")).toContain("setup-administrator-name-error");
    expect(wrapper.get("#bootstrap-secret").attributes("aria-invalid")).toBe("true");
    expect(wrapper.get("#bootstrap-secret").attributes("aria-describedby")).toContain("setup-bootstrap-secret-error");
    expect(wrapper.get("#new-password").attributes("aria-invalid")).toBe("true");
    expect(wrapper.get("[data-error-summary]").text()).toContain("已清空，请重新输入");
  });

  it("preserves the administrator name, clears the password, and distinguishes login failures", async () => {
    const cases = [
      [apiError(401, "auth.invalid_credentials"), "管理员名称或密码不正确", true],
      [apiError(429, "auth.rate_limited", { retry_after_seconds: 42 }), "尝试次数过多，请在 42 秒后重试", false],
      [apiError(500, "internal.error"), "登录服务暂时不可用", false],
      [new TypeError("fetch failed"), "无法连接 MediaFlow Core", false],
    ] as const;
    for (const [failure, message, fieldFailure] of cases) {
      setActivePinia(createPinia());
      const api = client({ createSession: vi.fn(async () => { throw failure; }) });
      const { wrapper } = await harness(LoginView, api, "/login");
      await wrapper.get("#login-administrator-name").setValue("admin");
      await wrapper.get("#login-password").setValue("not-right");
      await wrapper.get("form").trigger("submit");
      await flushPromises();
      expect((wrapper.get("#login-administrator-name").element as HTMLInputElement).value).toBe("admin");
      expect((wrapper.get("#login-password").element as HTMLInputElement).value).toBe("");
      expect(wrapper.get("[data-error-summary]").text()).toContain(message);
      expect(document.activeElement).toBe(wrapper.get("[data-error-summary]").element);
      expect(wrapper.get("#login-password").attributes("aria-describedby"))
        .toBe(fieldFailure ? "login-error" : undefined);
      wrapper.unmount();
    }
  });

  it("prevents duplicate login and enters a saved safe location", async () => {
    let resolveLogin!: (value: SessionResponse) => void;
    const createSession = vi.fn(() => new Promise<SessionResponse>((resolve) => { resolveLogin = resolve; }));
    const api = client({ createSession });
    const store = useSessionStore();
    store.configureClient(api);
    store.saveReturnLocation("/tasks?state=running");
    const { wrapper, router } = await harness(LoginView, api, "/login");
    await wrapper.get("#login-administrator-name").setValue("admin");
    await wrapper.get("#login-password").setValue("correct horse battery staple");
    await wrapper.get("form").trigger("submit");
    await wrapper.get("form").trigger("submit");
    expect(createSession).toHaveBeenCalledTimes(1);
    resolveLogin(session);
    await flushPromises();

    expect(router.currentRoute.value.fullPath).toBe("/tasks?state=running");
    expect(store.csrfToken).toBe(session.csrf_token);
    expect(api.setCsrfToken).toHaveBeenCalledWith(session.csrf_token);
  });

  it("links a password-only login validation error to the password field", async () => {
    const { wrapper } = await harness(LoginView, client(), "/login");
    await wrapper.get("#login-administrator-name").setValue("admin");

    await wrapper.get("form").trigger("submit");
    await flushPromises();

    expect(wrapper.get("[data-error-summary] a").attributes("href")).toBe("#login-password");
    expect(wrapper.get("#login-password").attributes("aria-invalid")).toBe("true");
    expect(wrapper.get("#login-password").attributes("aria-describedby")).toContain("login-password-error");
    expect(wrapper.get("#login-administrator-name").attributes("aria-invalid")).toBe("false");
  });

  it("reports that a valid login password was cleared while preserving the original error first", async () => {
    const { wrapper } = await harness(LoginView, client(), "/login");
    await wrapper.get("#login-password").setValue("correct horse battery staple");

    await wrapper.get("form").trigger("submit");
    await flushPromises();

    expect((wrapper.get("#login-password").element as HTMLInputElement).value).toBe("");
    expect(wrapper.findAll("[data-error-summary] a").map((link) => link.attributes("href")))
      .toEqual(["#login-administrator-name", "#login-password"]);
    expect(wrapper.get("#login-password").attributes("aria-invalid")).toBe("true");
    expect(wrapper.get("#login-password").attributes("aria-describedby")).toContain("login-password-error");
    expect(wrapper.get("[data-error-summary]").text()).toContain("密码已清空，请重新输入");
  });

  it("clears setup credentials immediately and ignores completion after unmount", async () => {
    let resolveBootstrap!: (value: Awaited<ReturnType<MediaFlowClient["bootstrap"]>>) => void;
    const api = client({ bootstrap: vi.fn(() => new Promise<Awaited<ReturnType<MediaFlowClient["bootstrap"]>>>((resolve) => { resolveBootstrap = resolve; })) });
    const { wrapper, router } = await harness(SetupView, api, "/setup");
    const replace = vi.spyOn(router, "replace");
    await wrapper.get("#bootstrap-secret").setValue("one-time-secret");
    await wrapper.get("#administrator-name").setValue("admin");
    await wrapper.get("#new-password").setValue("correct horse battery staple");

    await wrapper.get("form").trigger("submit");
    expect((wrapper.get("#bootstrap-secret").element as HTMLInputElement).value).toBe("");
    expect((wrapper.get("#new-password").element as HTMLInputElement).value).toBe("");
    wrapper.unmount();
    resolveBootstrap({ account: session.account, version: "v1" });
    await flushPromises();

    expect(replace).not.toHaveBeenCalled();
  });

  it("clears login password immediately and ignores a session completed after unmount", async () => {
    let resolveLogin!: (value: SessionResponse) => void;
    const api = client({ createSession: vi.fn(() => new Promise<SessionResponse>((resolve) => { resolveLogin = resolve; })) });
    const store = useSessionStore();
    const { wrapper, router } = await harness(LoginView, api, "/login");
    const replace = vi.spyOn(router, "replace");
    await wrapper.get("#login-administrator-name").setValue("admin");
    await wrapper.get("#login-password").setValue("correct horse battery staple");

    await wrapper.get("form").trigger("submit");
    expect((wrapper.get("#login-password").element as HTMLInputElement).value).toBe("");
    wrapper.unmount();
    resolveLogin(session);
    await flushPromises();

    expect(replace).not.toHaveBeenCalled();
    expect(store.account).toBeNull();
    expect(store.csrfToken).toBeNull();
  });

  it("clears state and replaces protected history on logout even when Core is offline", async () => {
    const api = client({ deleteSession: vi.fn(async () => { throw new TypeError("offline"); }) });
    const store = useSessionStore();
    store.configureClient(api);
    store.establish(session);
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: "/login", name: "login", component: { template: "<p>登录</p>" } },
      { path: "/tasks", name: "tasks", component: { template: "<p>敏感任务</p>" } },
    ] });
    await router.push("/tasks");
    await store.logout(router);

    expect(router.currentRoute.value.name).toBe("login");
    expect(store.account).toBeNull();
    expect(store.csrfToken).toBeNull();
    await router.back();
    await flushPromises();
    expect(router.currentRoute.value.name).toBe("login");
  });
});
