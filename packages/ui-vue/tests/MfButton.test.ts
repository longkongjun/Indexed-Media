import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import MfButton from "../src/components/MfButton.vue";

/**
 * C01 Action 按钮的 DOM 契约测试（jsdom）。
 *
 * 几何（44×44、换行、200% 文字）与 focus-visible 焦点环依赖 CSS 布局，jsdom 不计算布局，
 * 由浏览器验证单独记录；本文件只断言 DOM 结构、事件与可访问性属性。
 */
describe("MfButton 渲染 C01 外观与变体", () => {
  it.each(["default", "primary", "danger", "icon"] as const)("渲染 %s 变体的原生 button 与 class", (variant) => {
    const wrapper = mount(MfButton, { props: { variant }, slots: { default: "操作" } });
    expect(wrapper.element.tagName).toBe("BUTTON");
    expect(wrapper.classes()).toContain("mf-button");
    expect(wrapper.classes()).toContain(`mf-button--${variant}`);
  });

  it("未传 variant 时默认 default 变体", () => {
    const wrapper = mount(MfButton, { slots: { default: "保存" } });
    expect(wrapper.classes()).toContain("mf-button--default");
  });
});

describe("MfButton type 契约与表单行为", () => {
  it("默认渲染 type=button", () => {
    const wrapper = mount(MfButton, { slots: { default: "取消" } });
    expect(wrapper.attributes("type")).toBe("button");
  });

  it.each(["submit", "reset"] as const)("显式支持 type=%s", (type) => {
    const wrapper = mount(MfButton, { props: { type }, slots: { default: "提交" } });
    expect(wrapper.attributes("type")).toBe(type);
  });

  it("默认按钮在表单内点击不触发 submit", async () => {
    const form = document.createElement("form");
    document.body.appendChild(form);
    const onSubmit = vi.fn((event: Event) => event.preventDefault());
    form.addEventListener("submit", onSubmit);
    const wrapper = mount(MfButton, { attachTo: form, slots: { default: "普通操作" } });
    await wrapper.trigger("click");
    expect(onSubmit).not.toHaveBeenCalled();
    wrapper.unmount();
    form.remove();
  });

  it("type=submit 点击提交表单一次", async () => {
    const form = document.createElement("form");
    document.body.appendChild(form);
    const onSubmit = vi.fn((event: Event) => event.preventDefault());
    form.addEventListener("submit", onSubmit);
    const wrapper = mount(MfButton, { attachTo: form, props: { type: "submit" }, slots: { default: "保存" } });
    await wrapper.trigger("click");
    expect(onSubmit).toHaveBeenCalledTimes(1);
    wrapper.unmount();
    form.remove();
  });

  it("disabled 的 submit 按钮点击不提交", async () => {
    const form = document.createElement("form");
    document.body.appendChild(form);
    const onSubmit = vi.fn((event: Event) => event.preventDefault());
    form.addEventListener("submit", onSubmit);
    const wrapper = mount(MfButton, { attachTo: form, props: { type: "submit", disabled: true }, slots: { default: "保存" } });
    await wrapper.trigger("click");
    expect(onSubmit).not.toHaveBeenCalled();
    wrapper.unmount();
    form.remove();
  });

  it("busy 的 submit 按钮点击不提交", async () => {
    const form = document.createElement("form");
    document.body.appendChild(form);
    const onSubmit = vi.fn((event: Event) => event.preventDefault());
    form.addEventListener("submit", onSubmit);
    const wrapper = mount(MfButton, { attachTo: form, props: { type: "submit", busy: true }, slots: { default: "保存" } });
    await wrapper.trigger("click");
    expect(onSubmit).not.toHaveBeenCalled();
    wrapper.unmount();
    form.remove();
  });
});

describe("MfButton 激活语义", () => {
  it("点击一次恰好发出一次无参数 activate", async () => {
    const wrapper = mount(MfButton, { slots: { default: "保存" } });
    await wrapper.trigger("click");
    expect(wrapper.emitted("activate")).toEqual([[]]);
  });

  it("disabled 不激活且带原生 disabled 属性", async () => {
    const wrapper = mount(MfButton, { props: { disabled: true }, slots: { default: "保存" } });
    expect(wrapper.attributes("disabled")).toBeDefined();
    await wrapper.trigger("click");
    expect(wrapper.emitted("activate")).toBeUndefined();
  });

  it("busy 不发出 activate，busy 结束后恢复激活", async () => {
    const wrapper = mount(MfButton, { props: { busy: true }, slots: { default: "保存" } });
    await wrapper.trigger("click");
    expect(wrapper.emitted("activate")).toBeUndefined();
    await wrapper.setProps({ busy: false });
    await wrapper.trigger("click");
    expect(wrapper.emitted("activate")).toHaveLength(1);
  });
});

describe("MfButton busy 语义", () => {
  it("busy 设置 aria-busy、保留标签、渲染隐藏的忙碌指示、不用原生 disabled", () => {
    const wrapper = mount(MfButton, { props: { busy: true }, slots: { default: "保存" } });
    expect(wrapper.attributes("aria-busy")).toBe("true");
    expect(wrapper.attributes("disabled")).toBeUndefined();
    expect(wrapper.text()).toContain("保存");
    const spinner = wrapper.find(".mf-button__spinner");
    expect(spinner.exists()).toBe(true);
    expect(spinner.attributes("aria-hidden")).toBe("true");
  });

  it("busy 替换图标位且图标位不参与可访问树", () => {
    const wrapper = mount(MfButton, {
      props: { busy: true },
      slots: { default: "保存", icon: "<svg aria-hidden='true'></svg>" },
    });
    expect(wrapper.find(".mf-button__icon").exists()).toBe(false);
    expect(wrapper.find(".mf-button__spinner").exists()).toBe(true);
  });

  it("busy 保持焦点在按钮上", async () => {
    const wrapper = mount(MfButton, { attachTo: document.body, slots: { default: "保存" } });
    (wrapper.element as HTMLButtonElement).focus();
    expect(document.activeElement).toBe(wrapper.element);
    await wrapper.setProps({ busy: true });
    expect(wrapper.attributes("disabled")).toBeUndefined();
    expect(document.activeElement).toBe(wrapper.element);
    wrapper.unmount();
  });

  it("busy 与 disabled 同时为真时以原生 disabled 为准", () => {
    const wrapper = mount(MfButton, { props: { busy: true, disabled: true }, slots: { default: "保存" } });
    expect(wrapper.attributes("disabled")).toBeDefined();
    expect(wrapper.classes()).toContain("mf-button--busy");
  });
});

describe("MfButton 内容与可访问名称", () => {
  it("标签来自 default 插槽，图标来自 icon 插槽且被 aria-hidden 包裹", () => {
    const wrapper = mount(MfButton, {
      slots: { default: "下载", icon: "<svg></svg>" },
    });
    expect(wrapper.find(".mf-button__label").text()).toBe("下载");
    const icon = wrapper.find(".mf-button__icon");
    expect(icon.exists()).toBe(true);
    expect(icon.attributes("aria-hidden")).toBe("true");
  });

  it("未声明属性透传到原生 button", () => {
    const wrapper = mount(MfButton, {
      attrs: { "aria-label": "删除所选", title: "删除所选", form: "bulk-form", "data-testid": "delete" },
      slots: { icon: "<svg></svg>" },
    });
    expect(wrapper.attributes("aria-label")).toBe("删除所选");
    expect(wrapper.attributes("title")).toBe("删除所选");
    expect(wrapper.attributes("form")).toBe("bulk-form");
    expect(wrapper.attributes("data-testid")).toBe("delete");
  });

  it("icon 变体标签视觉隐藏但保留在 DOM", () => {
    const wrapper = mount(MfButton, {
      props: { variant: "icon" },
      slots: { default: "删除", icon: "<svg></svg>" },
    });
    const label = wrapper.find(".mf-button__label");
    expect(label.text()).toBe("删除");
    expect(label.classes()).toContain("mf-button__label--visually-hidden");
  });

  it("icon 变体缺少可访问名称时开发模式警告一次", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    mount(MfButton, { props: { variant: "icon" }, slots: { icon: "<svg></svg>" } });
    expect(warn).toHaveBeenCalledTimes(1);
    expect(String(warn.mock.calls[0]?.[0])).toContain("MfButton");
  });

  it("icon 变体有 aria-label 时不警告", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    mount(MfButton, {
      props: { variant: "icon" },
      attrs: { "aria-label": "删除" },
      slots: { icon: "<svg></svg>" },
    });
    expect(warn).not.toHaveBeenCalled();
  });

  it("非 icon 变体缺名称不触发图标警告", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    mount(MfButton, { slots: { default: "保存" } });
    expect(warn).not.toHaveBeenCalled();
  });
});

describe("MfButton 原生键盘与焦点", () => {
  beforeEach(() => { document.body.innerHTML = ""; });
  afterEach(() => { document.body.innerHTML = ""; });

  it("根为原生 button 且不设置 tabindex", () => {
    const wrapper = mount(MfButton, { slots: { default: "保存" } });
    expect(wrapper.element.tagName).toBe("BUTTON");
    expect(wrapper.attributes("tabindex")).toBeUndefined();
  });

  it("原生 focus 可聚焦", () => {
    const wrapper = mount(MfButton, { attachTo: document.body, slots: { default: "保存" } });
    (wrapper.element as HTMLButtonElement).focus();
    expect(document.activeElement).toBe(wrapper.element);
    wrapper.unmount();
  });
});
