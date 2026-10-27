// 审批链组件行为测试（approval-banner / plan-preview / task-timeline / AiSessionPanel 审批入口）。
import type { CapabilityActivityEvent, ProcessingTask, ProcessingTaskOrganization } from "@mediaflow/api-client-ts";
import { DOMWrapper, flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import ApprovalBanner from "../src/components/ApprovalBanner.vue";
import PlanPreview from "../src/components/PlanPreview.vue";
import TaskTimeline from "../src/components/TaskTimeline.vue";
import AiSessionPanel from "../src/app/AiSessionPanel.vue";
import type { MediaFlowClient } from "@mediaflow/api-client-ts";
import { identityClientKey } from "../src/app/client";

const documentBody = () => new DOMWrapper(document.body);

const taskId = "019f0000-0000-7000-8000-000000000110";

const plan: NonNullable<ProcessingTaskOrganization["plan"]> = {
  id: "019f0000-0000-7000-8000-000000000111",
  version: 2,
  target_id: "019f0000-0000-7000-8000-000000000112",
  source: { root_id: "incoming", relative_path: "a.mkv" },
  destination: { root_id: "library", relative_path: "Movies/a.mkv" },
  operation: "copy",
  naming: "A Film",
  authorization: "paused",
  risk_codes: ["organization.rule-not-matched"],
  created_at: "2026-10-11T10:00:00Z",
};

const projection: ProcessingTaskOrganization = {
  task_id: taskId,
  state: "paused",
  plan,
  journals: [],
  local_result: null,
  allowed_actions: ["recalculate", "execute", "retry", "cancel"],
};

const pausedTask: ProcessingTask = {
  id: taskId,
  inbox_directory_id: "019f0000-0000-7000-8000-000000000120",
  file_revision_id: "019f0000-0000-7000-8000-000000000121",
  relative_path: "incoming/a.mkv",
  status: "paused",
  stage: "planning",
  checkpoint: "planning-paused",
  decision_checkpoint: null,
  current_task_decision_id: null,
  reason: "organization.plan-paused",
  recovering: false,
  attempt_count: 1,
  next_retry_at: null,
  allowed_actions: ["cancel"],
  updated_at: "2026-10-11T10:00:00Z",
};

describe("ApprovalBanner", () => {
  it("横幅只给入口不内联审批；主操作携带计划版本", async () => {
    const wrapper = mount(ApprovalBanner, { props: { plan } });
    expect(wrapper.get("[data-approval-banner]").attributes("role")).toBe("alert");
    expect(wrapper.text()).toContain("1 个整理计划等待审批");
    expect(wrapper.text()).toContain("计划版本 v2");
    const buttons = wrapper.findAll("button").map((button) => button.text());
    expect(buttons).toContain("查看完整计划并审批");
    expect(buttons).not.toContain("批准");
    await wrapper.get("button").trigger("click");
    expect(wrapper.emitted("open")?.[0]).toEqual([2]);
    wrapper.unmount();
  });

  it("稍后处理仅收起横幅", async () => {
    const wrapper = mount(ApprovalBanner, { props: { plan } });
    await wrapper.findAll("button").find((button) => button.text() === "稍后处理")!.trigger("click");
    expect(wrapper.find("[data-approval-banner]").exists()).toBe(false);
    wrapper.unmount();
  });
});

describe("PlanPreview", () => {
  it("完整影响可见：操作类型如实 + 风险提示行 + 批准文案绑定版本", () => {
    const wrapper = mount(PlanPreview, { props: { projection, canApprove: true, canReject: true, pending: null } });
    expect(wrapper.get("[data-plan-preview]").text()).toContain("复制");
    expect(wrapper.get("[data-plan-preview]").text()).toContain("incoming/a.mkv");
    expect(wrapper.get("[data-plan-preview]").text()).toContain("library/Movies/a.mkv");
    expect(wrapper.get("[data-plan-preview]").text()).toContain("风险提示");
    expect(wrapper.get('.plan-preview__approve').text()).toContain("批准并执行计划 v2");
    expect(wrapper.get('.plan-preview__reject').text()).toContain("拒绝此计划");
    wrapper.unmount();
  });

  it("批准走二次确认且确认后 emit approve（Web 人工授权唯一路径）", async () => {
    const wrapper = mount(PlanPreview, { props: { projection, canApprove: true, canReject: true, pending: null } });
    await wrapper.get('.plan-preview__approve').trigger("click");
    expect(documentBody().find('[role="alertdialog"]').exists()).toBe(true);
    await documentBody().get('.plan-preview__confirm-go').trigger("click");
    expect(wrapper.emitted("approve")).toHaveLength(1);
    await wrapper.setProps({ pending: "execute" });
    await wrapper.setProps({ pending: null });
    expect(documentBody().find('[role="alertdialog"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("确认携带打开确认时冻结的计划版本", async () => {
    const wrapper = mount(PlanPreview, { props: { projection, canApprove: true, canReject: true, pending: null } });
    await wrapper.get('.plan-preview__approve').trigger("click");
    await documentBody().get('.plan-preview__confirm-go').trigger("click");
    expect(wrapper.emitted("approve")?.[0]).toEqual([2]);
    wrapper.unmount();
  });

  it("确认期间计划版本变化必须重新审阅：确认失效且不发 approve", async () => {
    const wrapper = mount(PlanPreview, { props: { projection, canApprove: true, canReject: true, pending: null } });
    await wrapper.get('.plan-preview__approve').trigger("click");
    expect(documentBody().find('[role="alertdialog"]').exists()).toBe(true);
    await wrapper.setProps({ projection: { ...projection, plan: { ...plan, version: 3 } } });
    expect(documentBody().find('[role="alertdialog"]').exists()).toBe(false);
    expect(wrapper.emitted("approve")).toBeUndefined();
    // 重新审阅：在新版本上重新打开确认才允许批准。
    await wrapper.get('.plan-preview__approve').trigger("click");
    await documentBody().get('.plan-preview__confirm-go').trigger("click");
    expect(wrapper.emitted("approve")?.[0]).toEqual([3]);
    wrapper.unmount();
  });

  it("写不可用或动作进行中时批准禁用", () => {
    const disabled = mount(PlanPreview, { props: { projection, canApprove: false, canReject: true, pending: null } });
    expect(disabled.get('.plan-preview__approve').attributes("disabled")).toBeDefined();
    disabled.unmount();
    const pending = mount(PlanPreview, { props: { projection, canApprove: true, canReject: true, pending: "execute" } });
    expect(pending.get('.plan-preview__approve').attributes("disabled")).toBeDefined();
    pending.unmount();
  });

  it("正式模态默认取消，Esc取消回到触发项，忙碌防重复且失败保留当前版本", async () => {
    const wrapper = mount(PlanPreview, { attachTo: document.body, props: { projection, canApprove: true, canReject: true, pending: null, actionError: "" } });
    const opener = wrapper.get<HTMLButtonElement>(".plan-preview__approve");
    opener.element.focus();
    await opener.trigger("click");
    const dialog = documentBody().get('[role="alertdialog"]');
    expect(dialog.attributes("aria-modal")).toBe("true");
    expect(dialog.get(".plan-preview__cancel").element).toBe(document.activeElement);
    await dialog.trigger("keydown", { key: "Escape", keyCode: 27 });
    expect(document.activeElement).toBe(opener.element);
    await opener.trigger("click");
    await documentBody().get(".plan-preview__confirm-go").trigger("click");
    await wrapper.setProps({ pending: "execute" });
    await documentBody().get(".plan-preview__confirm-go").trigger("click");
    await documentBody().get('[role="alertdialog"]').trigger("keydown", { key: "Escape", keyCode: 27 });
    expect(wrapper.emitted("approve")).toHaveLength(1);
    expect(documentBody().get('[role="alertdialog"]').isVisible()).toBe(true);
    await wrapper.setProps({ pending: null, actionError: "计划执行失败，请重试" });
    expect(documentBody().get('[role="alertdialog"]').text()).toContain("计划执行失败，请重试");
    expect(documentBody().get(".plan-preview__confirm-go").text()).toContain("v2");
    await documentBody().get(".plan-preview__cancel").trigger("click");
    expect(document.activeElement).toBe(opener.element);
    wrapper.unmount();
  });

  it("拒绝模态同样防重复、保留失败，取消回拒绝入口", async () => {
    const wrapper = mount(PlanPreview, { attachTo: document.body, props: { projection, canApprove: true, canReject: true, pending: null, actionError: "" } });
    const opener = wrapper.get<HTMLButtonElement>(".plan-preview__reject");
    await opener.trigger("click");
    expect(documentBody().get('[role="alertdialog"]').text()).toContain("确认拒绝此计划");
    await documentBody().get(".plan-preview__confirm-go").trigger("click");
    await wrapper.setProps({ pending: "cancel" });
    await documentBody().get(".plan-preview__confirm-go").trigger("click");
    expect(wrapper.emitted("reject")).toHaveLength(1);
    await wrapper.setProps({ pending: null, rejectError: "取消任务失败" });
    expect(documentBody().get('[role="alertdialog"]').text()).toContain("取消任务失败");
    await documentBody().get(".plan-preview__cancel").trigger("click");
    expect(document.activeElement).toBe(opener.element);
    wrapper.unmount();
  });

});

describe("TaskTimeline", () => {
  it("阶段链双编码推进：等待审批落在计划节点 ⚠", () => {
    const wrapper = mount(TaskTimeline, { props: { task: pausedTask, organization: projection } });
    const labels = wrapper.findAll(".task-timeline__label").map((label) => label.text());
    expect(labels).toEqual(["发现", "识别", "计划", "文件操作", "NFO", "完成"]);
    const planningNode = wrapper.get('[data-timeline-state="attention"]');
    expect(planningNode.text()).toContain("计划");
    expect(planningNode.text()).toContain("等待审批");
    wrapper.unmount();
  });

  it("执行后 journal 与本地结果进入对应阶段事实", async () => {
    const executed: ProcessingTaskOrganization = {
      ...projection,
      state: "partial-success",
      plan: { ...plan, authorization: "one-time" },
      journals: [{
        id: "019f0000-0000-7000-8000-000000000131",
        operation_id: "019f0000-0000-7000-8000-000000000132",
        kind: "copy",
        status: "verified",
        source: plan.source,
        destination: plan.destination,
        projection_version: 3,
        updated_at: "2026-10-11T10:05:00Z",
      }],
      local_result: {
        id: "019f0000-0000-7000-8000-000000000133",
        version: 3,
        status: "partial-success",
        nfo_status: "failed",
        catalog_media_item_id: null,
        remaining_actions: ["nfo"],
        updated_at: "2026-10-11T10:06:00Z",
      },
    };
    const task: ProcessingTask = { ...pausedTask, status: "partial-success", stage: "nfo", checkpoint: "nfo-failed" };
    const wrapper = mount(TaskTimeline, { props: { task, organization: executed } });
    // 问题节点（NFO 失败）默认展开直达原因；完成节点的未完成分组经手风琴展开。
    expect(wrapper.text()).toContain("NFO 写入失败");
    const completionToggle = wrapper
      .findAll(".task-timeline__toggle")
      .find((toggle) => toggle.find(".task-timeline__label").text() === "完成")!;
    expect(completionToggle.text()).toContain("部分成功");
    await completionToggle.trigger("click");
    expect(wrapper.text()).toContain("未完成步骤：nfo");
    wrapper.unmount();
  });
});

describe("AiSessionPanel 审批入口", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("等待审批事件升级为深链入口，面板内无审批按钮", async () => {
    const events: CapabilityActivityEvent[] = [
      {
        sequence: 41,
        event_id: "019f0000-0000-7000-8000-000000000141",
        event_type: "task.waiting_approval",
        occurred_at: "2026-10-11T10:01:00Z",
        task_id: taskId,
        request_id: "019f0000-0000-7000-8000-000000000142",
        state: "needs_approval",
        error_code: null,
      },
    ];
    const client = {
      listCapabilityEvents: vi.fn(async () => ({ items: events, next_cursor: null })),
    } as unknown as MediaFlowClient;
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: "/tasks/:id", name: "task", component: { template: "<main>task</main>" } },
        { path: "/capability-events", component: { template: "<main>events</main>" } },
      ],
    });
    const wrapper = mount(AiSessionPanel, {
      global: { plugins: [router], provide: { [identityClientKey as symbol]: client } },
    });
    await flushPromises();
    const entry = wrapper.get("[data-approval-entry]");
    expect(entry.text()).toContain("等待审批");
    expect(entry.text()).toContain("到任务审批");
    expect(wrapper.text()).toContain("AI 摘要，以任务详情页为准");
    const allButtons = wrapper.findAll("button").map((button) => button.text());
    expect(allButtons.some((text) => /批准|拒绝|确认执行/.test(text))).toBe(false);
    await entry.trigger("click");
    await flushPromises();
    expect(router.currentRoute.value.params.id).toBe(taskId);
    wrapper.unmount();
  });
});
