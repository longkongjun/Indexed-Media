import type { MediaFlowClient, ProcessingTask, ProcessingTaskOrganization } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { identityClientKey } from "../src/app/client";
import { useOrganizationTask } from "../src/features/organization-task/useOrganizationTask";
import ProcessingTaskDetailView from "../src/views/ProcessingTaskDetailView.vue";

const taskId = "019f0000-0000-7000-8000-000000000081";
const resultId = "019f0000-0000-7000-8000-000000000082";
const planId = "019f0000-0000-7000-8000-000000000083";
const task: ProcessingTask = {
  id: taskId,
  inbox_directory_id: "019f0000-0000-7000-8000-000000000084",
  file_revision_id: "019f0000-0000-7000-8000-000000000085",
  relative_path: "ready/Arrival.2016.mkv",
  status: "paused",
  stage: "planning",
  checkpoint: "planning-paused",
  decision_checkpoint: null,
  current_task_decision_id: null,
  reason: "organization.plan-paused",
  recovering: false,
  attempt_count: 2,
  next_retry_at: null,
  allowed_actions: ["retry", "cancel"],
  updated_at: "2026-07-24T09:00:00Z",
};

function organization(overrides: Partial<ProcessingTaskOrganization> = {}): ProcessingTaskOrganization {
  return {
    task_id: taskId,
    state: "paused",
    plan: {
      id: planId,
      version: 3,
      target_id: "019f0000-0000-7000-8000-000000000086",
      source: { root_id: "incoming", relative_path: "ready/Arrival.2016.mkv" },
      destination: { root_id: "library", relative_path: "Movies/Arrival (2016)/Arrival (2016).mkv" },
      operation: "copy",
      naming: "Arrival (2016)",
      authorization: "paused",
      risk_codes: ["organization.rule-not-matched"],
      created_at: "2026-07-24T09:00:00Z",
    },
    journals: [],
    local_result: null,
    allowed_actions: ["recalculate", "execute", "retry", "cancel"],
    ...overrides,
  };
}

function client(overrides: Partial<MediaFlowClient> = {}): MediaFlowClient {
  return {
    getProcessingTask: vi.fn(async () => task),
    getProcessingTaskOrganization: vi.fn(async () => organization()),
    recalculateProcessingTaskOrganization: vi.fn(async () => organization({
      plan: { ...organization().plan!, version: 4 },
    })),
    executeProcessingTaskOrganization: vi.fn(async () => organization({
      plan: { ...organization().plan!, authorization: "one-time" },
      allowed_actions: ["recalculate", "cancel"],
    })),
    rollbackProcessingTaskOrganization: vi.fn(async () => organization({
      state: "completed",
      local_result: {
        id: resultId, version: 6, status: "compensated", nfo_status: "failed",
        catalog_media_item_id: null, remaining_actions: [], updated_at: "2026-07-24T09:04:00Z",
      },
      allowed_actions: [],
    })),
    retryProcessingTask: vi.fn(async () => ({ ...task, status: "queued" as const })),
    cancelProcessingTask: vi.fn(async () => ({ ...task, status: "cancelled" as const })),
    setCsrfToken: vi.fn(),
    ...overrides,
  } as unknown as MediaFlowClient;
}

describe("organization task recovery", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("refreshes before replaying an uncertain execution and reuses one user-intent key", async () => {
    const execute = vi.fn()
      .mockRejectedValueOnce(new TypeError("response lost"))
      .mockResolvedValueOnce(organization({
        plan: { ...organization().plan!, authorization: "one-time" },
        allowed_actions: ["recalculate", "cancel"],
      }));
    const get = vi.fn(async () => organization());
    const feature = useOrganizationTask(client({
      getProcessingTaskOrganization: get,
      executeProcessingTaskOrganization: execute,
    }), taskId);
    await feature.load();
    await feature.execute();
    expect(get).toHaveBeenCalledTimes(2);
    expect(execute).toHaveBeenCalledTimes(1);
    expect(feature.actionError.value).toContain("同一操作");
    await feature.execute();
    expect(execute).toHaveBeenCalledTimes(2);
    expect(execute.mock.calls[0]?.[2]).toBe(execute.mock.calls[1]?.[2]);
    expect(feature.projection.value?.plan?.authorization).toBe("one-time");
  });

  it("keeps partial and manual-review facts offline while disabling every write", async () => {
    const partial = organization({
      state: "partial-success",
      journals: [{
        id: "019f0000-0000-7000-8000-000000000087", operation_id: "019f0000-0000-7000-8000-000000000088",
        kind: "copy", status: "verified",
        source: { root_id: "incoming", relative_path: "ready/Arrival.2016.mkv" },
        destination: { root_id: "library", relative_path: "Movies/Arrival (2016)/Arrival (2016).mkv" },
        projection_version: 4, updated_at: "2026-07-24T09:02:00Z",
      }],
      local_result: {
        id: resultId, version: 5, status: "partial-success", nfo_status: "failed",
        catalog_media_item_id: null, remaining_actions: ["nfo"], updated_at: "2026-07-24T09:03:00Z",
      },
      allowed_actions: ["retry", "rollback"],
    });
    const get = vi.fn().mockResolvedValueOnce(partial).mockRejectedValueOnce(new TypeError("offline"));
    const feature = useOrganizationTask(client({ getProcessingTaskOrganization: get }), taskId);
    await feature.load();
    await feature.load();
    expect(feature.projection.value).toEqual(partial);
    expect(feature.state.value).toEqual({ kind: "offline", stale: true });
    expect(feature.canRollback.value).toBe(false);
    expect(feature.canExecute.value).toBe(false);
  });

  it("gets current rollback truth after response loss and never creates a second automatic command", async () => {
    const partial = organization({
      state: "partial-success",
      local_result: {
        id: resultId, version: 5, status: "partial-success", nfo_status: "failed",
        catalog_media_item_id: null, remaining_actions: ["nfo"], updated_at: "2026-07-24T09:03:00Z",
      },
      allowed_actions: ["retry", "rollback"],
    });
    const compensated = organization({
      state: "completed",
      local_result: { ...partial.local_result!, version: 6, status: "compensated", remaining_actions: [] },
      allowed_actions: [],
    });
    const rollback = vi.fn().mockRejectedValueOnce(new TypeError("lost"));
    const get = vi.fn().mockResolvedValueOnce(partial).mockResolvedValueOnce(compensated);
    const feature = useOrganizationTask(client({
      getProcessingTaskOrganization: get,
      rollbackProcessingTaskOrganization: rollback,
    }), taskId);
    await feature.load();
    await feature.rollback();
    expect(rollback).toHaveBeenCalledTimes(1);
    expect(get).toHaveBeenCalledTimes(2);
    expect(feature.projection.value?.local_result?.status).toBe("compensated");
    expect(feature.actionError.value).toBe("");
  });

  it("renders plan, journal and result actions strictly from the organization projection", async () => {
    const withoutExecute = organization({
      state: "partial-success",
      journals: [{
        id: "019f0000-0000-7000-8000-000000000087", operation_id: "019f0000-0000-7000-8000-000000000088",
        kind: "copy", status: "verified", source: { root_id: "incoming", relative_path: "ready/Arrival.2016.mkv" },
        destination: { root_id: "library", relative_path: `Movies/${"long-segment/".repeat(24)}Arrival.mkv` },
        projection_version: 4, updated_at: "2026-07-24T09:02:00Z",
      }],
      local_result: {
        id: resultId, version: 5, status: "partial-success", nfo_status: "failed",
        catalog_media_item_id: null, remaining_actions: ["nfo"], updated_at: "2026-07-24T09:03:00Z",
      },
      allowed_actions: ["retry", "rollback"],
    });
    const api = client({ getProcessingTaskOrganization: vi.fn(async () => withoutExecute) });
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: "/tasks", name: "tasks", component: { template: "<main>tasks</main>" } },
        { path: "/tasks/:id", name: "task", component: ProcessingTaskDetailView },
        { path: "/review-cases/:id", name: "review-case", component: { template: "<main>review</main>" } },
        { path: "/media/:id", name: "media-item", component: { template: "<main>media</main>" } },
        { path: "/login", name: "login", component: { template: "<main>login</main>" } },
      ],
    });
    await router.push(`/tasks/${taskId}`);
    const wrapper = mount(ProcessingTaskDetailView, {
      global: { plugins: [pinia, router], provide: { [identityClientKey as symbol]: api } },
    });
    await flushPromises();
    expect(wrapper.get("[data-organization-task]").text()).toContain("部分成功");
    expect(wrapper.get("[data-organization-task]").text()).toContain("NFO：失败");
    expect(wrapper.get("[data-organization-task]").text()).toContain("verified");
    expect(wrapper.find('[data-organization-action="execute"]').exists()).toBe(false);
    expect(wrapper.get('[data-organization-action="rollback"]').text()).toContain("回滚");
    expect(wrapper.text()).not.toMatch(/container_path|\/private\/|NFO_MUST_NOT_RENDER/);
    wrapper.unmount();
  });
});
