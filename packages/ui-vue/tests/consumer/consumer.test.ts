import { existsSync, readFileSync } from "node:fs";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

/**
 * 最小无业务消费者：只按 package.json 的公开 exports 解析构建产物入口并消费，
 * 不从 src 内部路径导入、不复制源码、不依赖 apps/web 或任何 MediaFlow 业务包。
 * 运行前必须先执行 `pnpm build`（由 `pnpm test:consumer` 或 `pnpm verify` 保证）。
 */
const pkgRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const manifest = JSON.parse(readFileSync(resolve(pkgRoot, "package.json"), "utf8")) as {
  exports: { ".": { types: string; default: string }; "./style.css": string };
};

const mainExport = manifest.exports["."];
const styleExport = manifest.exports["./style.css"];
if (mainExport === undefined || styleExport === undefined) throw new Error("package.json exports 缺少公开入口");
const jsEntry = resolve(pkgRoot, mainExport.default);
const styleEntry = resolve(pkgRoot, styleExport);
const typesEntry = resolve(pkgRoot, mainExport.types);

it("公开入口指向存在的构建产物", () => {
  for (const entry of [jsEntry, styleEntry, typesEntry]) {
    if (!existsSync(entry)) throw new Error(`缺少构建产物 ${entry}；先运行 pnpm build`);
  }
});

describe("构建产物公开入口消费", () => {
  it("从 dist 导入 MfButton 并完成激活交互", async () => {
    const ui = await import(pathToFileURL(jsEntry).href);
    expect(typeof ui.MfButton).toBe("object");

    const wrapper = mount(ui.MfButton, { props: { variant: "primary" }, slots: { default: "保存" } });
    expect(wrapper.element.tagName).toBe("BUTTON");
    expect(wrapper.classes()).toContain("mf-button--primary");
    expect(wrapper.attributes("type")).toBe("button");
    await wrapper.trigger("click");
    expect(wrapper.emitted("activate")).toHaveLength(1);
    wrapper.unmount();
  });

  it("产物中的 disabled/busy 语义仍然成立", async () => {
    const ui = await import(pathToFileURL(jsEntry).href);
    const busy = mount(ui.MfButton, { props: { busy: true }, slots: { default: "保存" } });
    await busy.trigger("click");
    expect(busy.emitted("activate")).toBeUndefined();
    busy.unmount();

    const disabled = mount(ui.MfButton, { props: { disabled: true }, slots: { default: "保存" } });
    expect(disabled.attributes("disabled")).toBeDefined();
    disabled.unmount();
  });

  it("style.css 提供 .mf-button 规则、主题变量且不选中宿主 button", () => {
    const css = readFileSync(styleEntry, "utf8");
    expect(css).toContain(".mf-button");
    expect(css).toContain("--mfd-color-action-primary:");
    // 主题块必须先于组件规则，保证宿主可覆盖顺序稳定。
    expect(css.indexOf(":root")).toBeLessThan(css.indexOf(".mf-button"));
    const withoutComments = css.replace(/\/\*[\s\S]*?\*\//g, "");
    for (const match of withoutComments.matchAll(/([^{}]+)\{/g)) {
      const raw = match[1];
      if (raw === undefined) continue;
      const selector = raw.trim();
      if (selector.startsWith("@")) continue;
      const functional = selector.replace(/::?[a-z-]+\([^)]*\)/g, "");
      for (const part of functional.split(",")) {
        const trimmed = part.trim();
        if (trimmed.length === 0) continue;
        expect(
          /^\.mf-(button|image|content-card|request-state|field|dialog|menu)(?:[ _:.\[#>-]|$)/.test(trimmed) ||
            trimmed.startsWith(":root") ||
            trimmed.startsWith("[data-theme") ||
            trimmed.startsWith("[data-skin") ||
            trimmed === "to",
          `选择器超出作用域: ${trimmed}`,
        ).toBe(true);
      }
    }
  });

  it("JS 产物外部化 Vue 且类型声明导出 MfButton", () => {
    const js = readFileSync(jsEntry, "utf8");
    expect(js).toMatch(/from\s*["']vue["']/);
    const dts = readFileSync(typesEntry, "utf8");
    expect(dts).toContain("MfButton");
  });
});


it("公开产物中的 B1 图片/卡片/请求状态支持缺图、原生链接及恢复事件", async () => {
  const ui = await import(pathToFileURL(jsEntry).href);
  for (const name of ["MfImage", "MfContentCard", "MfRequestState"]) {
    expect(ui[name]).toBeDefined();
    expect(readFileSync(typesEntry, "utf8")).toContain(name);
    expect(readFileSync(styleEntry, "utf8")).toContain(({ MfImage: ".mf-image", MfContentCard: ".mf-content-card", MfRequestState: ".mf-request-state" } as Record<string, string>)[name]);
  }
  const image = mount(ui.MfImage, { props: { src: "/broken.png", alt: "封面", retryLabel: "重试图片" } });
  await image.get("img").trigger("error");
  await image.get("button").trigger("click");
  expect(image.get("img").attributes("src")).toBe("/broken.png");
  const card = mount(ui.MfContentCard, { props: { title: "完整标题" }, slots: { title: '<a href="/one">完整标题</a>' } });
  expect(card.get("a").attributes("href")).toBe("/one");
  const state = mount(ui.MfRequestState, { props: { state: { kind: "error", stale: true }, errorMessage: "失败" }, slots: { default: "旧结果" } });
  expect(state.get(".mf-request-state__content").attributes("inert")).toBeDefined();
  await state.get("button").trigger("click");
  expect(state.emitted("retry")).toEqual([[]]);
  image.unmount(); card.unmount(); state.unmount();
});
