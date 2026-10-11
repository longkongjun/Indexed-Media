// UI Contract 漂移防线：页面状态权威（packages/ui-contract 定本）与 web 实现锚点双向对齐。
// 方向一（编译期）：KINDS 键集经 satisfies 与 TaskCenterState["kind"] 严格双射（vue-tsc 拦截）。
// 方向二（运行时）：权威 JSON 状态集、stale 载体、展示态映射与下方断言一致（vitest 拦截）。
import { describe, expect, it } from "vitest";
import displayStateMapping from "../../../packages/ui-contract/contract/display-state-mapping.json";
import pageStatesDocument from "../../../packages/ui-contract/contract/page-states.json";
import type { TaskCenterState } from "../src/features/processing-tasks/model";

// satisfies 要求键集恰为 TaskCenterState["kind"] 全集（多一个/少一个/改名都编译失败）；
// 值 0 只为占位，语义在键。
const KINDS = {
  loading: 0,
  content: 0,
  empty: 0,
  offline: 0,
  error: 0,
} as const satisfies Record<TaskCenterState["kind"], 0>;

const STALE_CARRIER_KINDS = new Set(["loading", "offline", "error"]);

describe("ui-contract 页面状态权威对齐", () => {
  it("权威五态与 TaskCenterState kind 集一致", () => {
    const authorityIds = pageStatesDocument.states.map((state) => state.id).sort();
    expect(authorityIds).toEqual([...Object.keys(KINDS)].sort());
  });

  it("权威 stale 载体仅限 loading/offline/error", () => {
    for (const state of pageStatesDocument.states) {
      expect(state.stale, `states[${state.id}].stale`).toBe(STALE_CARRIER_KINDS.has(state.id));
    }
  });

  it("权威实现锚点覆盖 web 与 kmp-sdk", () => {
    const platforms = pageStatesDocument.implementations.map((entry) => entry.platform);
    expect(platforms).toContain("web");
    expect(platforms).toContain("kmp-sdk");
  });

  it("展示态映射：四个页面态直映射容器，content 无容器，三个操作态叠加", () => {
    const pageDriven = displayStateMapping.display_states.filter((state) => state.driven_by === "page-state");
    const operationDriven = displayStateMapping.display_states.filter((state) => state.driven_by === "operation");
    expect(pageDriven.map((state) => state.id).sort()).toEqual(["empty", "error", "loading", "offline"]);
    expect(pageDriven.flatMap((state) => state.page_states ?? []).sort()).toEqual(["empty", "error", "loading", "offline"]);
    expect(operationDriven.map((state) => state.id).sort()).toEqual(["conflict", "partial", "success"]);
    for (const state of operationDriven) {
      expect(state.page_states).toBeNull();
      expect(state.operation_semantics?.length ?? 0).toBeGreaterThan(0);
    }
  });
});
