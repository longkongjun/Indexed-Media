import { mount } from "@vue/test-utils";
import { h, nextTick } from "vue";
import { Button, IndexedThemeProvider, Modal } from "@indexed-design/ui-vue";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { afterEach, expect, it } from "vitest";
import { readStyleNonce } from "../src/app/designTheme";

afterEach(() => document.querySelector('meta[name="csp-nonce"]')?.remove());

it("只读取Core提供的文档nonce，占位符/缺省不授权", () => {
  expect(readStyleNonce()).toBeUndefined();
  const meta = document.createElement("meta");
  meta.name = "csp-nonce";
  meta.content = "__MEDIAFLOW_STYLE_NONCE__";
  document.head.append(meta);
  expect(readStyleNonce()).toBeUndefined();
  meta.content = "a".repeat(43);
  expect(readStyleNonce()).toBe(meta.content);
});

it("首屏主题脚本为同源外部文件，保留暗色/皮肤及存储失败恢复", () => {
  const html = readFileSync("index.html", "utf8");
  expect(html).toContain('<script src="/theme-init.js"></script>');
  expect(html).not.toMatch(/<script(?![^>]*src=)[^>]*>\s*\S/);
  const source = readFileSync("public/theme-init.js", "utf8");
  const dataset: Record<string, string> = {};
  runInNewContext(source, { document: { documentElement: { dataset } }, localStorage: {
    getItem: (key: string) => key === "mfd-theme" ? "dark" : "sepia",
  } });
  expect(dataset).toEqual({ theme: "dark", skin: "sepia" });
  expect(() => { runInNewContext(source, { document: { documentElement: { dataset: {} } }, localStorage: {
    getItem: () => { throw new Error("storage unavailable"); },
  } }); }).not.toThrow();
});

it("正式包的主题/模态/动画及图标样式在插入前均带文档nonce", async () => {
  const meta = document.createElement("meta");
  meta.name = "csp-nonce";
  meta.content = "b".repeat(43);
  document.head.append(meta);
  const before = new Set(document.querySelectorAll("style"));
  const wrapper = mount(IndexedThemeProvider, {
    attachTo: document.body,
    attrs: { csp: { nonce: meta.content } },
    slots: { default: () => h(Modal, { open: true, title: "CSP回归", footer: null }, {
      default: () => h(Button, { type: "primary" }, () => "确定"),
    }) },
  });
  try {
    for (const mode of ["light", "dark"] as const) {
      await wrapper.setProps({ mode });
      await nextTick();
      const styles = [...document.querySelectorAll("style")].filter(el => !before.has(el) && el.textContent);
      expect(styles.length).toBeGreaterThan(0);
      expect(styles.every(el => el.nonce === meta.content)).toBe(true);
    }
  } finally {
    wrapper.unmount();
    for (const style of document.querySelectorAll("style")) if (!before.has(style)) style.remove();
  }
});
