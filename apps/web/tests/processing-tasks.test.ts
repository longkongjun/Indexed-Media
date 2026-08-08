import type { IdentificationDetail, MediaFlowClient, ProcessingTask, ProcessingTaskOrganization, ProcessingTaskPage } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { identityClientKey } from "../src/app/client";
import { useProcessingTask, useProcessingTasks } from "../src/features/processing-tasks/useProcessingTasks";
import ProcessingTaskDetailView from "../src/views/ProcessingTaskDetailView.vue";
import TaskCenterView from "../src/views/TaskCenterView.vue";

const taskId = "018f0f10-8bc1-7a5e-8e5a-2dc913d23c87";
const reviewCaseId = "018f0f10-8bc1-7a5e-8e5a-2dc913d23c89";
const task = (status: ProcessingTask["status"] = "waiting-confirmation"): ProcessingTask => ({
  id: taskId,
  inbox_directory_id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c86",
  file_revision_id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c88",
  relative_path: "incoming/Dune.2021.mkv",
  status,
  stage: "identification",
  checkpoint: status === "waiting-confirmation" ? "waiting-confirmation" : "pending",
  decision_checkpoint: null,
  current_task_decision_id: null,
  reason: status === "waiting-confirmation" ? "identification.ambiguous" : null,
  recovering: false,
  attempt_count: 1,
  next_retry_at: null,
  allowed_actions: status === "waiting-confirmation" ? ["review", "retry", "cancel"] : ["cancel"],
  updated_at: "2026-07-23T08:00:00Z",
});

const page = (): ProcessingTaskPage => ({
  items: [task()],
  next_cursor: "next-safe",
  summary: { pending: 1, running: 2, all: 3, completed: 0, snapshot_version: 7 },
});

const noOrganization = (): ProcessingTaskOrganization => ({
  task_id: taskId,
  state: "not-planned",
  plan: null,
  journals: [],
  local_result: null,
  allowed_actions: ["cancel"],
});

function client(overrides: Partial<MediaFlowClient> = {}): MediaFlowClient {
  return {
    listProcessingTasks: vi.fn(async () => page()),
    getProcessingTask: vi.fn(async () => task()),
    getProcessingTaskIdentification: vi.fn(async () => ({ review_case_id: reviewCaseId }) as unknown as IdentificationDetail),
    getProcessingTaskOrganization: vi.fn(async () => noOrganization()),
    retryProcessingTask: vi.fn(async () => task("queued")),
    cancelProcessingTask: vi.fn(async () => task("cancelled")),
    ...overrides,
  } as MediaFlowClient;
}

describe("ProcessingTask center", () => {
  it("sends every route filter to Core and never client-filters a returned page", async () => {
    const api = client();
    const feature = useProcessingTasks(api, {
      view: "pending",
      stage: "identification",
      status: "waiting-confirmation",
      inboxDirectoryId: "inbox-a",
      query: "%_Dune",
      cursor: "cursor-a",
    });
    await feature.load();
    expect(api.listProcessingTasks).toHaveBeenCalledWith({
      view: "pending",
      stage: "identification",
      status: "waiting-confirmation",
      inboxDirectoryId: "inbox-a",
      query: "%_Dune",
      cursor: "cursor-a",
    });
    expect(feature.tasks.value).toEqual(page().items);
    expect(feature.summary.value).toEqual(page().summary);
    expect(feature.nextCursor.value).toBe("next-safe");
  });

  it("preserves stale server content and summary while offline", async () => {
    const api = client();
    const feature = useProcessingTasks(api, { view: "all" });
    await feature.load();
    (api.listProcessingTasks as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new TypeError("offline"));
    await feature.load({ view: "running" });
    expect(feature.state.value).toEqual({ kind: "offline", stale: true });
    expect(feature.tasks.value).toHaveLength(1);
    expect(feature.summary.value?.all).toBe(3);
  });

  it("uses only server allowed actions and keeps one idempotency key across response-loss replay", async () => {
    const retry = vi.fn().mockRejectedValueOnce(new TypeError("lost")).mockResolvedValueOnce(task("queued"));
    const detail = useProcessingTask(client({ retryProcessingTask: retry }), taskId);
    await detail.load();
    expect(detail.canReview.value).toBe(true);
    expect(detail.canRetry.value).toBe(true);
    expect(detail.canCancel.value).toBe(true);
    await detail.retry();
    expect(retry).toHaveBeenCalledTimes(2);
    expect(retry.mock.calls[0]?.[1]).toBe(retry.mock.calls[1]?.[1]);
    expect(detail.task.value?.status).toBe("queued");
  });

  it("renders four server-summary views and round-trips every filter through the route", async () => {
    const api = client();
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: "/tasks", name: "tasks", component: TaskCenterView },
        { path: "/tasks/:id", name: "task", component: ProcessingTaskDetailView },
        { path: "/review-cases/:id", name: "review-case", component: { template: "<main>review</main>" } },
        { path: "/scan-tasks", name: "scan-tasks", component: { template: "<main>scan</main>" } },
        { path: "/login", name: "login", component: { template: "<main>login</main>" } },
      ],
    });
    await router.push(`/tasks?view=running&stage=identification&status=paused&inbox_directory_id=${task().inbox_directory_id}&q=Dune&cursor=cursor-a`);
    const wrapper = mount(TaskCenterView, {
      global: { plugins: [pinia, router], provide: { [identityClientKey as symbol]: api } },
    });
    await flushPromises();

    expect(wrapper.get('nav[aria-label="任务视图"]').findAll("a")).toHaveLength(4);
    expect(wrapper.get('a[aria-current="page"]').text()).toContain("进行中");
    expect(wrapper.get('nav[aria-label="任务视图"]').text()).toMatch(/待处理1.*进行中2.*全部3.*已完成0/);
    expect(api.listProcessingTasks).toHaveBeenLastCalledWith({
      view: "running", stage: "identification", status: "paused",
      inboxDirectoryId: task().inbox_directory_id, query: "Dune", cursor: "cursor-a",
    });
    expect(wrapper.get(`a[href^="/tasks/${taskId}"]`).attributes("href")).toContain("cursor=cursor-a");

    await wrapper.findAll("select")[0]!.setValue("planning");
    await wrapper.findAll("select")[1]!.setValue("failed");
    await wrapper.findAll("input")[0]!.setValue("inbox-b");
    await wrapper.findAll("input")[1]!.setValue("Arrival");
    await wrapper.get("form").trigger("submit");
    await flushPromises();
    expect(router.currentRoute.value.query).toEqual({
      view: "running", stage: "planning", status: "failed",
      inbox_directory_id: "inbox-b", q: "Arrival",
    });
    expect(api.listProcessingTasks).toHaveBeenLastCalledWith({
      view: "running", stage: "planning", status: "failed",
      inboxDirectoryId: "inbox-b", query: "Arrival", cursor: undefined,
    });
    wrapper.unmount();
  });

  it("renders a durable detail timeline and exposes only server-allowed actions", async () => {
    const detailTask: ProcessingTask = {
      ...task(), allowed_actions: ["review", "cancel"], decision_checkpoint: "planning-requested",
    };
    const api = client({ getProcessingTask: vi.fn(async () => detailTask) });
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: "/tasks", name: "tasks", component: { template: "<main>tasks</main>" } },
        { path: "/tasks/:id", name: "task", component: ProcessingTaskDetailView },
        { path: "/review-cases/:id", name: "review-case", component: { template: "<main>review</main>" } },
        { path: "/login", name: "login", component: { template: "<main>login</main>" } },
      ],
    });
    await router.push(`/tasks/${taskId}?view=pending`);
    const wrapper = mount(ProcessingTaskDetailView, {
      global: { plugins: [pinia, router], provide: { [identityClientKey as symbol]: api } },
    });
    await flushPromises();

    expect(wrapper.get('ol[aria-label="处理时间线"]').text()).toContain("持久检查点：waiting-confirmation");
    expect(wrapper.text()).toContain("人工决定：planning-requested");
    expect(wrapper.get(`a[href="/review-cases/${reviewCaseId}"]`).text()).toBe("进入人工确认");
    expect(wrapper.get("button").text()).toBe("取消处理");
    expect(wrapper.text()).not.toContain("重试处理");
    expect(wrapper.get('a[href="/tasks?view=pending"]').attributes("href")).toBe("/tasks?view=pending");
    wrapper.unmount();
  });
});
