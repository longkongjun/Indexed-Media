// 主题三态与皮肤控制器的行为测试（theme-switching 模式 §1-4 + skins 二维）。
// 当前 jsdom 环境不提供 localStorage（Node 实验性实现未启用），用内存 stub 驱动存储交互；
// 模块本身对存储不可用已有 try/catch 容错（隐私模式同路径）。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  applySkinChoice,
  applyThemeChoice,
  initTheme,
  readSkinChoice,
  readThemeChoice,
} from "../src/app/theme";

const root = () => document.documentElement;

function stubLocalStorage(): void {
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => void store.set(key, value),
    removeItem: (key: string) => void store.delete(key),
    clear: () => store.clear(),
  });
}

beforeEach(() => {
  stubLocalStorage();
  delete root().dataset.theme;
  delete root().dataset.skin;
  root().classList.remove("theme-transition");
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("主题三态", () => {
  it("手动浅色/暗色写 data-theme 并持久化", () => {
    applyThemeChoice("dark");
    expect(root().dataset.theme).toBe("dark");
    expect(localStorage.getItem("mfd-theme")).toBe("dark");
    applyThemeChoice("light");
    expect(root().dataset.theme).toBe("light");
    expect(localStorage.getItem("mfd-theme")).toBe("light");
  });

  it("跟随系统删除 data-theme（交给 prefers-color-scheme）并持久化 system", () => {
    applyThemeChoice("dark");
    applyThemeChoice("system");
    expect(root().dataset.theme).toBeUndefined();
    expect(localStorage.getItem("mfd-theme")).toBe("system");
  });

  it("非法存储值归一为跟随系统", () => {
    localStorage.setItem("mfd-theme", "neon");
    expect(readThemeChoice()).toBe("system");
  });

  it("切换打开过渡窗口并在结束后关闭", () => {
    vi.useFakeTimers();
    applyThemeChoice("dark");
    expect(root().classList.contains("theme-transition")).toBe(true);
    vi.advanceTimersByTime(500);
    expect(root().classList.contains("theme-transition")).toBe(false);
  });
});

describe("皮肤维度", () => {
  it("非基线皮肤写 data-skin，基线删除属性", () => {
    applySkinChoice("aurora");
    expect(root().dataset.skin).toBe("aurora");
    expect(localStorage.getItem("mfd-skin")).toBe("aurora");
    applySkinChoice("evergreen");
    expect(root().dataset.skin).toBeUndefined();
    expect(localStorage.getItem("mfd-skin")).toBe("evergreen");
  });

  it("非法存储值归一为常青基线", () => {
    localStorage.setItem("mfd-skin", "vaporwave");
    expect(readSkinChoice()).toBe("evergreen");
  });
});

describe("启动对齐", () => {
  it("initTheme 归一并回写非法存储值", () => {
    localStorage.setItem("mfd-theme", "dark");
    localStorage.setItem("mfd-skin", "neon");
    initTheme();
    expect(localStorage.getItem("mfd-theme")).toBe("dark");
    expect(localStorage.getItem("mfd-skin")).toBe("evergreen");
  });
});
