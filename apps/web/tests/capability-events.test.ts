import type { CapabilityActivityEvent, MediaFlowClient } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import CapabilityEventsView from "../src/views/CapabilityEventsView.vue";
import { identityClientKey } from "../src/app/client";
import { useCapabilityEvents } from "../src/features/capability-activity/useCapabilityEvents";

const event = (sequence: number, overrides: Partial<CapabilityActivityEvent> = {}): CapabilityActivityEvent => ({
  sequence,
  event_id: `019f8a10-0000-7000-8000-${String(sequence).padStart(12, "0")}`,
  event_type: "capability.completed",
  occurred_at: "2026-10-10T08:00:00.000001Z",
  task_id: sequence % 2 === 0 ? "019f8a10-0000-7000-8000-000000000100" : null,
  request_id: "019f8a10-0000-7000-8000-000000000002",
  state: "succeeded",
  error_code: null,
  ...overrides,
});

function client(page: { items: CapabilityActivityEvent[]; next_cursor: string | null }, secondPage?: { items: CapabilityActivityEvent[]; next_cursor: string | null }): MediaFlowClient {
  const listCapabilityEvents = vi.fn();
  if (secondPage) listCapabilityEvents.mockResolvedValueOnce(page).mockResolvedValueOnce(secondPage);
  else listCapabilityEvents.mockResolvedValue(page);
  return { listCapabilityEvents, setCsrfToken: vi.fn() } as unknown as MediaFlowClient;
}

describe("AI capability activity", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("loads the newest-first projection and renders redacted rows", async () => {
    const api = client({ items: [event(43), event(42, { event_type: "capability.failed", error_code: "capability.request.conflict" })], next_cursor: null });
    const activity = useCapabilityEvents(api);
    await activity.load();
    expect(activity.state.value.kind).toBe("content");
    expect(activity.events.value.map((item) => item.sequence)).toEqual([43, 42]);
    expect(activity.nextCursor.value).toBeNull();
  });

  it("keeps stale content visible on load failure without throwing", async () => {
    const api = client({ items: [event(1)], next_cursor: null });
    (api.listCapabilityEvents as ReturnType<typeof vi.fn>).mockReset();
    (api.listCapabilityEvents as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new TypeError("offline"));
    const activity = useCapabilityEvents(api);
    await activity.load();
    expect(activity.state.value.kind).toBe("offline");
    expect(activity.errorMessage.value).not.toBe("");
  });

  it("appends the older page on load-more and advances the cursor", async () => {
    const api = client(
      { items: [event(5), event(4)], next_cursor: "capability-events:4" },
      { items: [event(3)], next_cursor: null },
    );
    const activity = useCapabilityEvents(api);
    await activity.load();
    expect(activity.nextCursor.value).toBe("capability-events:4");
    await activity.loadMore();
    expect(activity.events.value.map((item) => item.sequence)).toEqual([5, 4, 3]);
    expect(activity.nextCursor.value).toBeNull();
    expect(api.listCapabilityEvents).toHaveBeenLastCalledWith({ cursor: "capability-events:4" });
  });

  it("renders the read-only view with task links and error codes", async () => {
    const api = client({ items: [event(42, { event_type: "task.waiting_approval", state: "waiting_approval" })], next_cursor: null });
    const pinia = createPinia();
    setActivePinia(pinia);
    const appRouter = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: "/", component: { template: "<main />" } },
        { path: "/capability-events", component: CapabilityEventsView },
        { path: "/tasks/:id", name: "task", component: { template: "<main />" } },
        { path: "/login", name: "login", component: { template: "<main />" } },
      ],
    });
    await appRouter.push("/capability-events");
    const wrapper = mount(CapabilityEventsView, {
      global: { plugins: [pinia, appRouter], provide: { [identityClientKey as symbol]: api } },
    });
    await flushPromises();
    expect(wrapper.find(".async-state").attributes("aria-busy")).toBe("false");
    expect(wrapper.text()).toContain("等待审批");
    expect(wrapper.find("[data-event-sequence='42'] a").attributes("href")).toContain("/tasks/");
    expect(wrapper.find("h1").text()).toBe("AI 能力活动");
    wrapper.unmount();
  });
});
