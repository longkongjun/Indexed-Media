import type { MediaFlowClient } from "@mediaflow/api-client-ts";
import axe from "axe-core";
import { flushPromises, mount } from "@vue/test-utils";
import type { Component } from "vue";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import AppShell from "../src/app/AppShell.vue";
import { identityClientKey } from "../src/app/client";
import LoginView from "../src/views/LoginView.vue";
import SetupView from "../src/views/SetupView.vue";
import InboxListView from "../src/views/InboxListView.vue";
import RawFilesView from "../src/views/RawFilesView.vue";
import MediaListView from "../src/views/MediaListView.vue";
import MediaDetailView from "../src/views/MediaDetailView.vue";
import DownloaderConnectionsView from "../src/views/DownloaderConnectionsView.vue";
import DownloadTaskListView from "../src/views/DownloadTaskListView.vue";
import OrganizationTargetsView from "../src/views/OrganizationTargetsView.vue";
import AutomationSourcesView from "../src/views/AutomationSourcesView.vue";
import ProcessingTaskDetailView from "../src/views/ProcessingTaskDetailView.vue";

const client = {
  bootstrap: vi.fn(), createSession: vi.fn(), setCsrfToken: vi.fn(),
} as unknown as MediaFlowClient;

async function mountWithRouter(component: Component, path: string, api: MediaFlowClient = client) {
  const router = createRouter({ history: createMemoryHistory(), routes: [
    { path: "/setup", component: SetupView }, { path: "/login", component: LoginView },
    { path: "/tasks", name: "tasks", component: { template: "<main><h1>任务</h1></main>" } },
    { path: "/tasks/:id", name: "task", component: { template: "<main><h1>任务</h1></main>" } },
    { path: "/media", name: "media", component: MediaListView },
    { path: "/media/:id", name: "media-item", component: MediaDetailView },
    { path: "/connections/downloaders", name: "downloader-connections", component: DownloaderConnectionsView },
    { path: "/connections/downloaders/:id", name: "downloader-connection", component: { template: "<main><h1>连接详情</h1></main>" } },
    { path: "/downloads", name: "download-tasks", component: DownloadTaskListView },
    { path: "/downloads/:id", name: "download-task", component: { template: "<main><h1>下载详情</h1></main>" } },
    { path: "/organization/targets", name: "organization-targets", component: OrganizationTargetsView },
    { path: "/organization/targets/:id", name: "organization-target", component: { template: "<main><h1>整理目标详情</h1></main>" } },
    { path: "/scan-tasks/:id", name: "scan-task", component: { template: "<main><h1>任务</h1></main>" } },
    { path: "/scan-tasks/:id/files", component: RawFilesView },
    { path: "/inbox-directories", component: InboxListView },
    { path: "/inbox-directories/:id", name: "inbox-directory", component: { template: "<main><h1>收件目录详情</h1></main>" } },
  ] });
  await router.push(path);
  await router.isReady();
  return mount(component, { attachTo: document.body, global: { plugins: [router], provide: { [identityClientKey as symbol]: api } } });
}

describe("Task 7 accessibility", () => {
  beforeEach(() => { document.body.innerHTML = ""; vi.clearAllMocks(); setActivePinia(createPinia()); });

  it.each([[SetupView, "/setup"], [LoginView, "/login"]] as const)("has no automated form violations", async (component, path) => {
    const wrapper = await mountWithRouter(component, path);
    // jsdom 无法计算渲染后的颜色，因此颜色对比度由后续浏览器测试覆盖。
    const result = await axe.run(wrapper.element, { rules: { "color-contrast": { enabled: false } } });
    expect(result.violations).toEqual([]);
    wrapper.unmount();
  });

  it("provides explicit labels, autocomplete, password guidance, and keyboard submission", async () => {
    const wrapper = await mountWithRouter(SetupView, "/setup");
    expect(wrapper.get('label[for="bootstrap-secret"]')).toBeTruthy();
    expect(wrapper.get("#bootstrap-secret").attributes("autocomplete")).toBe("off");
    expect(wrapper.get("#administrator-name").attributes("autocomplete")).toBe("username");
    expect(wrapper.get("#new-password").attributes("autocomplete")).toBe("new-password");
    expect(wrapper.text()).toContain("至少 12 个字符");
    await wrapper.get("#bootstrap-secret").setValue("one-time-secret");
    await wrapper.get("#administrator-name").setValue("admin");
    await wrapper.get("#new-password").setValue("correct horse battery staple");
    await wrapper.get("#new-password").trigger("keydown.enter");
    expect(client.bootstrap).toHaveBeenCalled();
    wrapper.unmount();
  });

  it("shows only implemented destinations in desktop and mobile navigation", async () => {
    const wrapper = await mountWithRouter(AppShell, "/tasks");
    expect(wrapper.find("[data-desktop-nav]").text()).toContain("任务");
    expect(wrapper.find("[data-desktop-nav]").text()).toContain("媒体");
    expect(wrapper.find("[data-desktop-nav]").text()).toContain("收件目录");
    expect(wrapper.find("[data-desktop-nav]").text()).toContain("下载任务");
    expect(wrapper.find("[data-desktop-nav]").text()).toContain("下载器连接");
    expect(wrapper.find("[data-desktop-nav]").text()).toContain("整理目标");
    expect(wrapper.find("[data-desktop-nav]").text()).toContain("来源自动化");
    expect(wrapper.find("[data-mobile-nav]").text()).toContain("自动化");
    expect(wrapper.find("[data-desktop-nav]").text()).toContain("账户与系统");
    expect(wrapper.find("[data-mobile-nav]").text()).toContain("更多");
    expect(wrapper.text()).not.toMatch(/识别|Jellyfin|即将推出/i);
    expect(wrapper.find("[data-mobile-nav]").attributes("aria-label")).toContain("移动");
    await wrapper.get("[data-account-toggle]").trigger("click");
    expect(wrapper.get("[data-account-panel]").text()).toContain("退出登录");
    wrapper.unmount();
  });

  it.each([
    [InboxListView, "/inbox-directories"],
    [RawFilesView, "/scan-tasks/018f0f10-8bc1-7a5e-8e5a-2dc913d23c87/files"],
    [MediaListView, "/media"],
    [MediaDetailView, "/media/019f0000-0000-7000-8000-000000000050"],
    [DownloaderConnectionsView, "/connections/downloaders"],
    [DownloadTaskListView, "/downloads"],
    [OrganizationTargetsView, "/organization/targets"],
    [AutomationSourcesView, "/automation/sources"],
    [ProcessingTaskDetailView, "/tasks/019f0000-0000-7000-8000-000000000081"],
  ] as const)("has no automated business-flow violations", async (component, path) => {
    const noop = vi.fn();
    const api = {
      listDeploymentRoots: vi.fn(async () => ({ items: [{ id: "incoming", label: "家庭收件区", access: "read-write" as const }], next_cursor: null })),
      listInboxDirectories: vi.fn(async () => ({ items: [], next_cursor: null })),
      listScanTaskFiles: vi.fn(async () => ({ items: [{ id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c88", relative_path: "safe/movie.mkv", size_bytes: 42, modified_at: "2026-07-18T08:00:00Z" }], next_cursor: null })),
      listScanTaskErrors: vi.fn(async () => ({ items: [], next_cursor: null })),
      listMediaItems: vi.fn(async () => ({ items: [], next_cursor: null })),
      getMediaItem: vi.fn(async () => ({
        item: { id: "019f0000-0000-7000-8000-000000000050", type: "movie", library_id: "019f0000-0000-7000-8000-000000000051", title: "沙丘", year: 2021, local_status: "partial", artwork_ref: null, updated_at: "2026-07-23T10:00:00Z" },
        metadata: [], versions: [], children: [], nfo_status: "not-requested", related_task_ids: [],
      })),
      listDownloaderConnections: vi.fn(async () => ({ items: [], next_cursor: null })),
      listDownloadTasks: vi.fn(async () => ({ items: [], next_cursor: null })),
      listOrganizationTargets: vi.fn(async () => ({ items: [], next_cursor: null })),
      listAutomationSources: vi.fn(async () => ({ items: [], next_cursor: null })),
      listAutomationEvents: vi.fn(async () => ({ items: [], next_cursor: null })),
      getIdentificationEnhancer: vi.fn(async () => ({
        kind: "ollama" as const, enabled: false, endpoint_summary: "http://127.0.0.1:11434",
        model: "qwen3:4b", timeout_ms: 3000, config_version: 1, health: "degraded" as const,
        checked_at: null, fallback_code: "automation.source-disabled" as const,
        projection_version: 1, updated_at: "2026-07-24T09:00:00Z",
      })),
      testAutomationSource: noop,
      createAutomationSource: noop,
      getAutomationSource: noop,
      updateAutomationSource: noop,
      deleteAutomationSource: noop,
      rotateAutomationWebhookSecret: noop,
      getAutomationEvent: noop,
      retryAutomationEvent: noop,
      cancelAutomationEvent: noop,
      testIdentificationEnhancer: noop,
      putIdentificationEnhancer: noop,
      getProcessingTask: vi.fn(async () => ({
        id: "019f0000-0000-7000-8000-000000000081",
        inbox_directory_id: "019f0000-0000-7000-8000-000000000084",
        file_revision_id: "019f0000-0000-7000-8000-000000000085",
        relative_path: "ready/Arrival.2016.mkv", status: "paused", stage: "planning",
        checkpoint: "planning-paused", decision_checkpoint: null, current_task_decision_id: null,
        reason: "organization.plan-paused", recovering: false, attempt_count: 1,
        next_retry_at: null, allowed_actions: ["cancel"], updated_at: "2026-07-24T09:00:00Z",
      })),
      getProcessingTaskOrganization: vi.fn(async () => ({
        task_id: "019f0000-0000-7000-8000-000000000081", state: "paused",
        plan: {
          id: "019f0000-0000-7000-8000-000000000083", version: 1,
          target_id: "019f0000-0000-7000-8000-000000000086",
          source: { root_id: "incoming", relative_path: "ready/Arrival.2016.mkv" },
          destination: { root_id: "library", relative_path: "Movies/Arrival (2016)/Arrival (2016).mkv" },
          operation: "copy", naming: "Arrival (2016)", authorization: "paused",
          risk_codes: ["organization.rule-not-matched"], created_at: "2026-07-24T09:00:00Z",
        },
        journals: [], local_result: null, allowed_actions: ["recalculate", "execute", "cancel"],
      })),
      setCsrfToken: noop,
    } as unknown as MediaFlowClient;
    const wrapper = await mountWithRouter(component, path, api);
    await flushPromises();
    if (component === InboxListView) await wrapper.get("button.primary-action").trigger("click");
    const result = await axe.run(wrapper.element, { rules: { "color-contrast": { enabled: false } } });
    expect(result.violations).toEqual([]);
    wrapper.unmount();
  });
});
