import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { h } from "vue";
import { MfContentCard, MfImage, MfRequestState } from "../src";

afterEach(() => vi.useRealTimers());

describe("MfImage", () => {
  it("缺图保留可访问名称和比例，不创建空 src 请求", () => {
    const image = mount(MfImage, { props: { alt: "很长的媒体标题" }, slots: { fallback: "暂无封面" } });
    expect(image.find("img").exists()).toBe(false);
    expect(image.get('[role="img"]').attributes("aria-label")).toBe("很长的媒体标题");
    expect(image.classes()).toContain("mf-image--poster");
    expect(image.text()).toContain("暂无封面");
  });
  it("加载失败后保留说明，重试恢复原 URL；换源重置失败", async () => {
    const image = mount(MfImage, { props: { src: "/one.png", alt: "封面", retryLabel: "重试封面" }, slots: { fallback: "封面不可用" } });
    await image.get("img").trigger("error");
    expect(image.find("img").exists()).toBe(false);
    await image.get("button").trigger("click");
    expect(image.get("img").attributes("src")).toBe("/one.png");
    expect(image.emitted("retry")).toHaveLength(1);
    await image.get("img").trigger("error");
    await image.setProps({ src: "/two.png" });
    expect(image.get("img").attributes("src")).toBe("/two.png");
    await image.get("img").trigger("load");
    expect(image.emitted("load")).toHaveLength(1);
    expect(image.emitted("error")).toHaveLength(2);
  });
  it("装饰性缺图不增加读屏噪声；无源不提供无效重试", () => {
    const image = mount(MfImage, { props: { alt: "", retryLabel: "重试" } });
    expect(image.find('[role="img"]').exists()).toBe(false);
    expect(image.find("button").exists()).toBe(false);
  });
  it("图片重试保持按钮焦点，成功交给图片，防止操作节点消失后落回body", async () => {
    const image = mount(MfImage, { attachTo: document.body, props: { src: "/image.png", alt: "封面", retryLabel: "重试" } });
    await image.get("img").trigger("error");
    (image.get("button").element as HTMLElement).focus();
    await image.get("button").trigger("click");
    expect(document.activeElement).toBe(image.get("button").element);
    await image.get("button").trigger("click");
    expect(image.emitted("retry")).toHaveLength(1);
    await image.get("img").trigger("load");
    expect(document.activeElement).toBe(image.get("img").element);
    expect(image.find("button").exists()).toBe(false);
    image.unmount();
  });
});

describe("MfContentCard", () => {
  it("保留完整标题，并由调用方提供原生链接，卡片不添加额外交互节点", () => {
    const title = "长标题".repeat(25);
    const card = mount(MfContentCard, { props: { title, subtitle: "电影 · 2026" }, slots: {
      title: () => h("a", { href: "/detail" }, title), artwork: () => h("span", "缺图"),
    } });
    expect(card.get("a").text()).toBe(title);
    expect(card.findAll("a, button, [tabindex]")).toHaveLength(1);
    expect(card.text()).toContain("电影 · 2026");
  });
});

describe("MfRequestState", () => {
  it("慢加载延迟300ms提示，快请求不闪；卸载取消定时器", async () => {
    vi.useFakeTimers();
    const state = mount(MfRequestState, { props: { state: { kind: "loading" } } });
    expect(state.attributes("aria-busy")).toBe("true");
    expect(state.find('[role="status"]').exists()).toBe(false);
    await vi.advanceTimersByTimeAsync(299);
    await state.setProps({ state: { kind: "content" } });
    await vi.advanceTimersByTimeAsync(1);
    expect(state.find('[role="status"]').exists()).toBe(false);
    await state.setProps({ state: { kind: "loading" } });
    await vi.advanceTimersByTimeAsync(300);
    expect(state.get('[role="status"]').text()).toContain("正在加载");
    state.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
  it.each(["error", "offline"] as const)("%s 显示旧内容但禁止旧操作，恢复只发事件", async (kind) => {
    const state = mount(MfRequestState, { props: { state: { kind, stale: true }, errorMessage: "服务不可用", offlineMessage: "网络断开" }, slots: { default: '<a href="/old">旧结果</a>' } });
    expect(state.get(".mf-request-state__content").attributes("inert")).toBeDefined();
    expect(state.get("a").text()).toBe("旧结果");
    expect(state.get(kind === "error" ? '[role="alert"]' : '[role="status"]').text()).toContain(kind === "error" ? "服务不可用" : "网络断开");
    await state.get("button").trigger("click");
    expect(state.emitted("retry")).toEqual([[]]);
    await state.setProps({ state: { kind: "content" } });
    expect(state.get(".mf-request-state__content").attributes("inert")).toBeUndefined();
  });
  it("空结果支持宿主提供清除筛选，未加载内容不冒充结果", () => {
    const state = mount(MfRequestState, { props: { state: { kind: "empty" }, emptyMessage: "无匹配结果" }, slots: { default: "旧数据", empty: '<button>清除筛选</button>' } });
    expect(state.text()).toContain("无匹配结果");
    expect(state.text()).toContain("清除筛选");
    expect(state.text()).not.toContain("旧数据");
  });
});
