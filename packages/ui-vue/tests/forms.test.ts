import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MfDialog, MfField } from "../src";

afterEach(() => { document.body.innerHTML = ""; });

describe("受控表单", () => {
  it("输入只发一次更新，关联标签提示与错误；父级更新清错且保留值", async () => {
    const field = mount(MfField, { props: { id: "address", label: "服务器地址", modelValue: "https://demo.test", hint: "仅演示", error: "连接失败", autocomplete: "url" } });
    expect(field.get("label").attributes("for")).toBe("address");
    expect(field.get("input").attributes("aria-describedby")).toBe("address-hint address-error");
    expect(field.get("input").attributes("aria-invalid")).toBe("true");
    await field.get("input").setValue("https://fixed.test");
    expect(field.emitted("update:modelValue")).toEqual([["https://fixed.test"]]);
    await field.setProps({ modelValue: "https://fixed.test", error: "" });
    expect(field.get("input").element.value).toBe("https://fixed.test");
    expect(field.find("#address-error").exists()).toBe(false);
  });
  it("宿主可提供额外错误说明，安全属性和原生键盘事件落在输入", async () => {
    const keydown = vi.fn();
    const field = mount(MfField, { props: { id: "password", label: "密码", modelValue: "", type: "password", describedBy: "summary", invalid: true }, attrs: { autocomplete: "current-password", required: true, onKeydown: keydown } });
    expect(field.get("input").attributes("type")).toBe("password");
    expect(field.get("input").attributes("aria-describedby")).toBe("summary");
    expect(field.get("input").attributes("autocomplete")).toBe("current-password");
    await field.get("input").trigger("keydown", { key: "Enter" });
    expect(keydown).toHaveBeenCalledTimes(1);
  });
});

describe("受控模态", () => {
  it("Tab循环忽略type=hidden和CSS隐藏祖先中的控件", async () => {
    const wrapper = mount(MfDialog, { attachTo: document.body, props: { open: false, title: "隐藏字段" }, slots: { default: '<input type="hidden" value="internal"><div style="display:none"><button>隐藏操作</button></div><input style="visibility:hidden" />' } });
    await wrapper.setProps({ open: true }); await nextTick();
    (wrapper.get('[data-action="confirm"]').element as HTMLElement).focus();
    await wrapper.get("dialog").trigger("keydown", { key: "Tab" });
    expect(document.activeElement).toBe(wrapper.get('[data-action="cancel"]').element);
    await wrapper.get("dialog").trigger("keydown", { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(wrapper.get('[data-action="confirm"]').element);
    wrapper.unmount();
  });

  it("busy拦确认与关闭；dirty关闭由宿主决定，失败不丢草稿", async () => {
    const wrapper = mount(MfDialog, { props: { open: true, title: "连接设置", busy: true, dirty: true }, slots: { default: '<input value="draft" />' } });
    await wrapper.get('[data-action="cancel"]').trigger("click");
    await wrapper.get('[data-action="confirm"]').trigger("click");
    expect(wrapper.emitted("request-close")).toBeUndefined();
    expect(wrapper.emitted("confirm")).toBeUndefined();
    await wrapper.setProps({ busy: false, error: "保存失败" });
    await wrapper.get('[data-action="cancel"]').trigger("click");
    expect(wrapper.emitted("request-close")).toEqual([["dirty"]]);
    expect(wrapper.get("input").element.value).toBe("draft");
    expect(wrapper.get('[role="alert"]').text()).toContain("保存失败");
    expect(wrapper.get("dialog").attributes("open")).toBeDefined();
  });
  it("打开默认焦点在取消，关闭后归还触发者，Esc不能绕过dirty", async () => {
    const trigger = document.createElement("button"); document.body.append(trigger); trigger.focus();
    const wrapper = mount(MfDialog, { attachTo: document.body, props: { open: false, title: "确认", dirty: true } });
    await wrapper.setProps({ open: true }); await nextTick();
    expect(document.activeElement).toBe(wrapper.get('[data-action="cancel"]').element);
    await wrapper.get("dialog").trigger("cancel");
    expect(wrapper.emitted("request-close")).toEqual([["dirty"]]);
    await wrapper.setProps({ open: false }); await nextTick();
    expect(document.activeElement).toBe(trigger);
    wrapper.unmount();
  });
});
