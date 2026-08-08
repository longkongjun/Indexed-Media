import { mount } from "@vue/test-utils";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import AsyncState from "../src/components/AsyncState.vue";
import CursorPager from "../src/components/CursorPager.vue";
import ScanStatusBadge from "../src/components/ScanStatusBadge.vue";
import ErrorSummary from "../src/components/ErrorSummary.vue";

const layoutCss = readFileSync(resolve("src/styles/layout.css"), "utf8");
const responsiveCss = readFileSync(resolve("src/styles/responsive.css"), "utf8");

describe("responsive and accessible business primitives", () => {
  it("keeps stale content visible in Offline and never maps it to Empty", () => {
    const wrapper = mount(AsyncState, { props: { state: { kind: "offline", stale: true }, offlineMessage: "已离线，显示上次结果" }, slots: { default: "原始文件 safe/movie.mkv" } });
    expect(wrapper.text()).toContain("Offline");
    expect(wrapper.text()).toContain("已离线，显示上次结果");
    expect(wrapper.text()).toContain("safe/movie.mkv");
    expect(wrapper.text()).not.toContain("暂无数据");
  });

  it("uses text plus a non-color-only status mark and avoids fake progress", () => {
    const wrapper = mount(ScanStatusBadge, { props: { status: "partial-success", recovering: true } });
    expect(wrapper.text()).toContain("部分成功");
    expect(wrapper.text()).toContain("正在恢复");
    expect(wrapper.get("[data-status-shape]").attributes("aria-hidden")).toBe("true");
    expect(wrapper.text()).not.toMatch(/%|预计|ETA/i);
  });

  it("offers opaque cursor actions as context navigation rather than page numbers", async () => {
    const wrapper = mount(CursorPager, { props: { hasPreviousContext: true, nextCursor: "opaque-token" } });
    expect(wrapper.text()).toContain("返回上一组结果");
    expect(wrapper.text()).toContain("继续查看更多");
    expect(wrapper.text()).not.toContain("opaque-token");
    await wrapper.get("[data-next-cursor]").trigger("click");
    expect(wrapper.emitted("next")?.[0]).toEqual(["opaque-token"]);
  });

  it("provides separate wide table and mobile semantic list structures without future navigation", () => {
    document.body.innerHTML = `<main><table data-wide-results><caption>原始文件</caption></table><ul data-mobile-results><li>safe/movie.mkv</li></ul><nav>任务 媒体 收件目录 整理 更多</nav></main>`;
    expect(document.querySelector("table[data-wide-results] caption")?.textContent).toBe("原始文件");
    expect(document.querySelector("ul[data-mobile-results] li")?.textContent).toContain("safe/movie.mkv");
    expect(document.body.textContent).not.toMatch(/识别|Jellyfin|即将推出/);
  });

  it("refocuses a repeated identical error when its occurrence token changes", async () => {
    const wrapper = mount(ErrorSummary, { attachTo: document.body, props: { message: "相同错误", focusKey: 1 } });
    await wrapper.vm.$nextTick(); expect(document.activeElement).toBe(wrapper.get("[data-error-summary]").element);
    const elsewhere = document.createElement("button"); document.body.append(elsewhere); elsewhere.focus();
    await wrapper.setProps({ focusKey: 2 }); await wrapper.vm.$nextTick();
    expect(document.activeElement).toBe(wrapper.get("[data-error-summary]").element);
    wrapper.unmount();
  });

  it("keeps the 390px task center bounded while tabs remain horizontally reachable", () => {
    expect(layoutCss).toContain(".view-tabs");
    expect(layoutCss).toContain("overflow-x: auto");
    expect(layoutCss).toContain(".task-card-list .resource-card");
    expect(responsiveCss).toContain("[data-task-center]");
    expect(responsiveCss).toContain("max-width: 100%");
    expect(responsiveCss).toMatch(/\.task-filter, \.compact-facts \{ grid-template-columns: 1fr; \}/);
  });

  it("wraps long capability paths and keeps organization layouts bounded on mobile", () => {
    expect(layoutCss).toContain(".safe-path");
    expect(layoutCss).toMatch(/\.safe-path[^}]*overflow-wrap:\s*anywhere/s);
    expect(layoutCss).toContain(".organization-task-grid");
    expect(responsiveCss).toContain(".organization-target-form");
    expect(responsiveCss).toContain(".organization-task-grid");
  });

  it("stacks source automation cards and bounds secret blocks at 390px", () => {
    expect(layoutCss).toContain(".automation-entry-grid");
    expect(layoutCss).toContain(".secret-once pre");
    expect(layoutCss).toMatch(/\.secret-once pre[^}]*overflow-wrap:\s*anywhere/s);
    expect(responsiveCss).toContain("[data-automation-sources]");
    expect(responsiveCss).toMatch(/\.automation-entry-grid \{ grid-template-columns: 1fr; \}/);
  });
});
