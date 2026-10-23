import { nextTick } from "vue";
import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";
import { MfMenu } from "../src";
afterEach(()=>{document.body.innerHTML="";});
describe("受控操作菜单",()=>{
 it("打开首项、方向环绕、Home/End、禁用不激活、Esc归触发者",async()=>{
  const menu=mount(MfMenu,{attachTo:document.body,props:{open:false,label:"条目操作",items:[{id:"copy",label:"复制"},{id:"disabled",label:"不可用",disabled:true},{id:"detail",label:"详情"}]}});
  await menu.get('button').trigger('click'); expect(menu.emitted('update:open')).toEqual([[true]]);
  await menu.setProps({open:true}); await nextTick();
  const all=menu.findAll('[role="menuitem"]'); expect(document.activeElement).toBe(all[0]!.element);
  await menu.get('[role="menu"]').trigger('keydown',{key:'End'}); expect(document.activeElement).toBe(all[2]!.element);
  await menu.get('[role="menu"]').trigger('keydown',{key:'ArrowDown'}); expect(document.activeElement).toBe(all[0]!.element);
  await menu.get('[role="menu"]').trigger('keydown',{key:'ArrowDown'}); await all[1]!.trigger('click'); expect(menu.emitted('activate')).toBeUndefined();
  await menu.get('[role="menu"]').trigger('keydown',{key:'Escape'}); expect(menu.emitted('update:open')!.at(-1)).toEqual([false]); expect(document.activeElement).toBe(menu.get('button').element);
  await all[0]!.trigger('click'); expect(menu.emitted('activate')).toEqual([['copy']]); menu.unmount();
 });
});
