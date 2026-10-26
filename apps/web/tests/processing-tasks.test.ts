import type { IdentificationDetail, MediaFlowClient, ProcessingTask, ProcessingTaskOrganization, ProcessingTaskPage } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { effectScope } from "vue";
import { describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { identityClientKey } from "../src/app/client";
import App from "../src/App.vue";
import { createAppRouter } from "../src/router";
import { useProcessingTask, useProcessingTasks } from "../src/features/processing-tasks/useProcessingTasks";
import ProcessingTaskDetailView from "../src/views/ProcessingTaskDetailView.vue";
import { taskEventSourceFactoryKey, type EventSourceLike } from "../src/features/scan-tasks/useTaskEvents";
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

const noOrganizationWithTask = (id: string): ProcessingTaskOrganization => ({ ...noOrganization(), task_id: id });

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

    const selects = wrapper.findAll("select");
    const inputs = wrapper.findAll("input");
    if (selects.length < 2 || inputs.length < 2) throw new Error("task center form must expose two selects and two inputs");
    await selects[0]!.setValue("planning");
    await selects[1]!.setValue("failed");
    await inputs[0]!.setValue("inbox-b");
    await inputs[1]!.setValue("Arrival");
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

    expect(wrapper.get('ol[aria-label="处理时间线"]').text()).toContain("识别");
    expect(wrapper.text()).toContain("需要人工处理");
    expect(wrapper.text()).toContain("人工决定：planning-requested");
    expect(wrapper.get(`a[href="/review-cases/${reviewCaseId}"]`).text()).toBe("进入人工确认");
    expect(wrapper.get('nav[aria-label="处理任务操作"] button').text()).toBe("取消处理");
    expect(wrapper.text()).not.toContain("重试处理");
    expect(wrapper.get('a[href="/tasks?view=pending"]').attributes("href")).toBe("/tasks?view=pending");
    wrapper.unmount();
  });

  it("rebinds display, writes, back context and late responses when the same route switches task A→B", async () => {
    const taskAId = "018f0f10-8bc1-7a5e-8e5a-2dc913d23ca1";
    const taskBId = "018f0f10-8bc1-7a5e-8e5a-2dc913d23cb2";
    const taskA: ProcessingTask = { ...task(), id: taskAId, relative_path: "incoming/task-a-title.mkv" };
    const taskB: ProcessingTask = { ...task(), id: taskBId, relative_path: "incoming/task-b-title.mkv" };
    const cancel = vi.fn().mockResolvedValue({ ...taskB, status: "cancelled" as const });
    // A 的首个整理投影挂起，模拟切换后仍会返回的旧请求；A 的计划名用于识别迟到数据。
    const aLateOrganization: ProcessingTaskOrganization = {
      ...noOrganizationWithTask(taskAId),
      state: "paused",
      plan: {
        id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c99",
        version: 1,
        target_id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c98",
        source: { root_id: "incoming", relative_path: "incoming/task-a-title.mkv" },
        destination: { root_id: "library", relative_path: "Movies/A Late Plan.mkv" },
        operation: "copy",
        naming: "A Late Plan",
        authorization: "paused",
        risk_codes: [],
        created_at: "2026-10-26T08:00:00Z",
      },
    };
    const lateA: { resolve: null | ((value: ProcessingTaskOrganization) => void) } = { resolve: null };
    let aLateDelivered = false;
    const api = client({
      getProcessingTask: vi.fn(async (id: string) => (id === taskAId ? taskA : taskB)),
      getProcessingTaskOrganization: (id: string) => {
        if (id === taskAId && !aLateDelivered) {
          return new Promise<ProcessingTaskOrganization>((resolve) => {
            lateA.resolve = resolve;
          });
        }
        return Promise.resolve(id === taskAId ? aLateOrganization : noOrganizationWithTask(id));
      },
      cancelProcessingTask: cancel,
      listNotifications: vi.fn(async () => ({ items: [], next_cursor: null, unread_count: 0 })),
      listCapabilityEvents: vi.fn(async () => ({ items: [], next_cursor: null })),
    });
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = createAppRouter({
      pinia,
      getBootstrapStatus: async () => ({ requires_initialization: false, version: "v1" }),
      getSession: async () => ({
        account: { id: "018f0f10-8bc1-7a5e-8e5a-2dc913d23c86", administrator_name: "admin" },
        csrf_token: "a".repeat(43),
        version: "v1",
      }),
    });
    await router.push(`/tasks/${taskAId}?view=pending`);
    await router.isReady();
    const wrapper = mount(App, { global: { plugins: [pinia, router], provide: { [identityClientKey as symbol]: api } } });
    await flushPromises();
    expect(wrapper.text()).toContain("incoming/task-a-title.mkv");

    await router.push(`/tasks/${taskBId}?view=running`);
    await flushPromises();

    // B 绑定：显示、写操作与返回上下文。
    expect(wrapper.text()).toContain("incoming/task-b-title.mkv");
    expect(wrapper.text()).not.toContain("incoming/task-a-title.mkv");
    const cancelButtons = wrapper.findAll('nav[aria-label="处理任务操作"] button').filter((button) => button.text() === "取消处理");
    expect(cancelButtons).toHaveLength(1);
    await cancelButtons[0]!.trigger("click");
    await flushPromises();
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(cancel.mock.calls[0]?.[0]).toBe(taskBId);
    expect(wrapper.get('a[href="/tasks?view=running"]').attributes("href")).toBe("/tasks?view=running");

    // A 的迟到投影在 B 视图期间返回：必须被丢弃，不得污染 B 的整理区。
    lateA.resolve?.(aLateOrganization);
    aLateDelivered = true;
    await flushPromises();
    expect(wrapper.text()).not.toContain("A Late Plan");
    expect(wrapper.get("[data-organization-task]").text()).toContain("任务尚未完成身份确认");

    // 浏览器返回 A：重新以当前对象加载，写操作与返回查询重新绑定 A。
    const backToA = new Promise<void>((resolve) => {
      const remove = router.afterEach(() => { remove(); resolve(); });
    });
    router.back();
    await backToA;
    await flushPromises();
    expect(wrapper.text()).toContain("incoming/task-a-title.mkv");
    expect(wrapper.text()).not.toContain("incoming/task-b-title.mkv");
    expect(wrapper.get('a[href="/tasks?view=pending"]').attributes("href")).toBe("/tasks?view=pending");
    const cancelOnA = wrapper.findAll('nav[aria-label="处理任务操作"] button').filter((button) => button.text() === "取消处理");
    await cancelOnA[0]!.trigger("click");
    await flushPromises();
    expect(cancel).toHaveBeenCalledTimes(2);
    expect(cancel.mock.calls[1]?.[0]).toBe(taskAId);
    wrapper.unmount();
  });
});

it.each(["organization", "task", "identification"] as const)("review: unloaded %s failure must not redirect the active task", async (pending) => {
  const a = "018f0f10-8bc1-7a5e-8e5a-2dc913d23ca1";
  const b = "018f0f10-8bc1-7a5e-8e5a-2dc913d23cb2";
  const late: { reject?: (reason: unknown) => void } = {};
  const defer = <T>(): Promise<T> => new Promise<T>((_, reject) => { late.reject = reject; });
  const api = client({
    getProcessingTask: (id: string) => id === a && pending === "task"
      ? defer<ProcessingTask>() : Promise.resolve({ ...task(), id }),
    getProcessingTaskIdentification: (id: string) => id === a && pending === "identification"
      ? defer<IdentificationDetail>() : Promise.resolve({ review_case_id: reviewCaseId } as IdentificationDetail),
    getProcessingTaskOrganization: (id: string) => id === a && pending === "organization"
      ? defer<ProcessingTaskOrganization>() : Promise.resolve(noOrganizationWithTask(id)),
    listNotifications: vi.fn(async () => ({ items: [], next_cursor: null, unread_count: 0 })),
    listCapabilityEvents: vi.fn(async () => ({ items: [], next_cursor: null })),
  });
  const pinia = createPinia(); setActivePinia(pinia);
  const router = createAppRouter({ pinia,
    getBootstrapStatus: async () => ({ requires_initialization: false, version: "v1" }),
    getSession: async () => ({ account: { id: a, administrator_name: "admin" }, csrf_token: "a".repeat(43), version: "v1" }),
  });
  await router.push(`/tasks/${a}`); await router.isReady();
  const wrapper = mount(App, { global: { plugins: [pinia, router], provide: { [identityClientKey as symbol]: api } } });
  try {
    await flushPromises();
    await router.push(`/tasks/${b}`); await flushPromises();
    const replace = vi.spyOn(router, "replace");
    late.reject?.(Object.assign(new Error("old request expired"), { status: 401 }));
    await flushPromises();
    expect(replace).not.toHaveBeenCalledWith({ name: "login" });
    expect(router.currentRoute.value.params.id).toBe(b);
  } finally { wrapper.unmount(); }
});

it("does not replay an uncertain task write or report its error after scope disposal", async () => {
  const pending: { reject?: (error: unknown) => void } = {};
  const cancel = vi.fn(() => new Promise<ProcessingTask>((_, reject) => { pending.reject = reject; }));
  const onFailure = vi.fn();
  const api = client({ cancelProcessingTask: cancel });
  const scope = effectScope();
  const feature = scope.run(() => useProcessingTask(api, taskId, onFailure))!;
  await feature.load();
  const action = feature.cancel();
  expect(cancel).toHaveBeenCalledTimes(1);
  scope.stop();
  pending.reject?.(new TypeError("response lost"));
  await action;
  expect(onFailure).not.toHaveBeenCalled();
  await feature.cancel();
  expect(cancel).toHaveBeenCalledTimes(1);
});


it.each(["task", "organization", "identification"] as const)("recovers a final SSE event after the real detail page catches a %s failure", async (failedProjection) => {
  const getTask = vi.fn().mockResolvedValue(task());
  const getOrganization = vi.fn().mockResolvedValue(noOrganization());
  const getIdentification = vi.fn().mockResolvedValue({ review_case_id: reviewCaseId });
  const api = client({ getProcessingTask: getTask, getProcessingTaskOrganization: getOrganization, getProcessingTaskIdentification: getIdentification });
  const listeners = new Map<string, (event: Event | MessageEvent<string>) => void>();
  const source: EventSourceLike = {
    addEventListener: (type, listener) => { listeners.set(type, listener); },
    removeEventListener: (type) => { listeners.delete(type); },
    close: vi.fn(),
  };
  const pinia = createPinia(); setActivePinia(pinia);
  const router = createRouter({ history: createMemoryHistory(), routes: [
    { path: "/tasks/:id", name: "task", component: ProcessingTaskDetailView },
    { path: "/tasks", name: "tasks", component: { template: "<main>tasks</main>" } },
    { path: "/review-cases/:id", name: "review-case", component: { template: "<main>review</main>" } },
  ] });
  await router.push(`/tasks/${taskId}`);
  const wrapper = mount(ProcessingTaskDetailView, { global: { plugins: [pinia, router], provide: {
    [identityClientKey as symbol]: api,
    [taskEventSourceFactoryKey as symbol]: () => source,
  } } });
  try {
    await flushPromises();
    const failed = failedProjection === "task" ? getTask : failedProjection === "organization" ? getOrganization : getIdentification;
    failed.mockRejectedValueOnce(new TypeError("offline"));
    const event = JSON.stringify({ id: 10, type: "processing-task.state-changed", schema_version: "1", occurred_at: "2026-07-23T10:00:00Z", task_id: taskId, payload: { status: "paused", stage: "identification", recovering: false, reason: "integration.unconfigured" } });
    listeners.get("processing-task.state-changed")?.({ data: event } as MessageEvent<string>);
    await flushPromises();
    expect(wrapper.text()).toContain("事件状态刷新失败");
    expect(getTask).toHaveBeenCalledTimes(2);
    listeners.get("open")?.(new Event("open")); await flushPromises();
    expect(getTask).toHaveBeenCalledTimes(3);
    expect(getOrganization).toHaveBeenCalledTimes(3);
    expect(wrapper.text()).not.toContain("事件状态刷新失败");
    expect(wrapper.text()).toContain(task().relative_path);
    listeners.get("processing-task.state-changed")?.({ data: event } as MessageEvent<string>);
    await flushPromises();
    expect(getTask).toHaveBeenCalledTimes(3);
  } finally { wrapper.unmount(); }
});
