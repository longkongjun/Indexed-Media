import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";
import App from "../src/App.vue";
afterEach(() => { document.body.innerHTML = ""; });
function openComponent(wrapper: ReturnType<typeof mount>, id: string): Promise<void> {
  const button = wrapper.findAll('nav[aria-label="组件目录"] button').find(button => button.text().startsWith(id));
  if (!button) throw new Error(`组件 ${id} 不存在`);
  return button.trigger("click");
}
describe("公开库组件工作台", () => {
  it("17组目录可访问，busy操作不重复，表单错误保留输入", async () => {
    const studio = mount(App, { attachTo: document.body });
    expect(studio.findAll('nav[aria-label="组件目录"] button')).toHaveLength(17);
    await studio.get(".canvas .mf-button").trigger("click");
    expect(studio.text()).toContain("有效激活 1 次");
    await studio.findAll('.example-toolbar input')[1]!.setValue(true);
    await studio.get(".canvas .mf-button").trigger("click");
    expect(studio.text()).toContain("有效激活 1 次");
    await openComponent(studio, "C02");
    await studio.get("#studio-address").setValue("https://retained.test");
    await studio.get(".canvas .mf-button").trigger("click");
    expect(studio.get<HTMLInputElement>("#studio-address").element.value).toBe("https://retained.test");
    expect(studio.get("#studio-address-error").text()).toContain("当前输入已保留");
    studio.unmount();
  });
  it("模态修改→dirty关闭→保留→失败，不静默丢值或重发", async () => {
    const studio = mount(App, { attachTo: document.body });
    await openComponent(studio, "C10");
    await studio.get(".canvas > .mf-button").trigger("click");
    await studio.get("#connection-name").setValue("保留我的草稿");
    await studio.get('[data-action="cancel"]').trigger("click");
    expect(studio.text()).toContain("草稿未保存");
    const keep = studio.findAll("dialog button").find(button => button.text() === "继续编辑");
    await keep!.trigger("click");
    await studio.get('[data-action="confirm"]').trigger("click");
    const deliver = studio.findAll("dialog button").find(button => button.text() === "交付模拟回执");
    await deliver!.trigger("click");
    expect(studio.get('[role="alert"]').text()).toContain("保存失败");
    expect(studio.get<HTMLInputElement>("#connection-name").element.value).toBe("保留我的草稿");
    studio.unmount();
  });
  it("请求旧内容禁止操作，恢复到content，分页追加只执行一次", async () => {
    const studio = mount(App);
    await openComponent(studio, "C12");
    await studio.get(".canvas select").setValue("offline");
    await studio.get('.canvas input[type="checkbox"]').setValue(true);
    expect(studio.get(".mf-request-state__content").attributes("inert")).toBeDefined();
    await studio.get(".mf-request-state__notice .mf-button").trigger("click");
    expect(studio.find(".mf-request-state__content").attributes("inert")).toBeUndefined();
    await openComponent(studio, "C16");
    await studio.get(".canvas > .mf-button").trigger("click");
    await studio.get(".canvas > .mf-button").trigger("click");
    expect(studio.findAll(".item-list li")).toHaveLength(4);
    studio.unmount();
  });
});
