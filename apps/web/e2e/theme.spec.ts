import { expect, test } from "@playwright/test";
import { m2Fixtures } from "../../../packages/test-fixtures/src/index.ts";

/**
 * 主题三态与皮肤切换 e2e（theme-switching 模式 + skins 二维）：
 * 三态切换 + 记忆持久 + 刷新防闪 + 皮肤属性；并在真实页面上按计算样式复核
 * 双主题正文对比度 ≥ 4.5:1（ui-design 风险 2 的遗留动作）。
 */

const { session } = m2Fixtures;

function luminance(rgb: string): number {
  const match = rgb.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
  if (!match) throw new Error(`unexpected color: ${rgb}`);
  const channels = [Number(match[1]), Number(match[2]), Number(match[3])].map((value) => {
    const scaled = value / 255;
    return scaled <= 0.03928 ? scaled / 12.92 : ((scaled + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * channels[0]! + 0.7152 * channels[1]! + 0.0722 * channels[2]!;
}

function contrast(foreground: string, background: string): number {
  const a = luminance(foreground);
  const b = luminance(background);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

test("three-state theme and skin switching persist and survive reloads", async ({ page }) => {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === "/api/v1/capability-events") return json({ items: [], next_cursor: null });
    if (path === "/api/v1/notifications") return json({ items: [], next_cursor: null, unread_count: 0 });
    if (path === "/api/v1/processing-tasks") return json({ items: [], next_cursor: null });
    return route.fulfill({ status: 404, body: "" });
  });

  await page.goto("/tasks");
  await expect(page.getByRole("heading", { name: "任务中心" })).toBeVisible();

  // 默认跟随系统：无 data-theme 属性（prefers-color-scheme 驱动）。
  await expect(page.locator("html")).not.toHaveAttribute("data-theme");

  // 窄屏入口是底部导航"更多"，宽屏是侧栏"账户与系统"。
  const accountButtonName = (page.viewportSize()?.width ?? 0) < 768 ? "更多" : "账户与系统";
  await page.getByRole("button", { name: accountButtonName }).click();
  const themeGroup = page.getByRole("radiogroup", { name: "主题" });

  // 手动暗色：属性 + 记忆。
  await themeGroup.getByLabel("暗色").check();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  expect(await page.evaluate(() => localStorage.getItem("mfd-theme"))).toBe("dark");

  // 刷新防闪：属性在首帧前恢复。
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.getByRole("heading", { name: "任务中心" })).toBeVisible();

  // 手动浅色 → 跟随系统（属性移除）。
  await page.getByRole("button", { name: accountButtonName }).click();
  await themeGroup.getByLabel("浅色").check();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await themeGroup.getByLabel("跟随系统").check();
  await expect(page.locator("html")).not.toHaveAttribute("data-theme");
  expect(await page.evaluate(() => localStorage.getItem("mfd-theme"))).toBe("system");

  // 皮肤二维：非基线写 data-skin，基线移除。
  const skinGroup = page.getByRole("radiogroup", { name: "皮肤" });
  await skinGroup.getByLabel("极光").check();
  await expect(page.locator("html")).toHaveAttribute("data-skin", "aurora");
  await skinGroup.getByLabel("常青").check();
  await expect(page.locator("html")).not.toHaveAttribute("data-skin");
});

test("real pages keep AA text contrast in both themes", async ({ page }) => {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path === "/api/v1/system/bootstrap-status") return json({ requires_initialization: false, version: "v1" });
    if (path === "/api/v1/session") return json(session);
    if (path === "/api/v1/capability-events") return json({ items: [], next_cursor: null });
    if (path === "/api/v1/notifications") return json({ items: [], next_cursor: null, unread_count: 0 });
    if (path === "/api/v1/processing-tasks") return json({ items: [], next_cursor: null });
    return route.fulfill({ status: 404, body: "" });
  });

  for (const theme of ["light", "dark"] as const) {
    await page.goto("/tasks");
    await page.evaluate((choice) => {
      localStorage.setItem("mfd-theme", choice);
    }, theme);
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
    await expect(page.getByRole("heading", { name: "任务中心" })).toBeVisible();

    // 真实页面计算样式：标题文字对其实际生效背景（向上取最近非透明底）的对比度。
    const pairs = await page.evaluate(() => {
      const isTransparent = (color: string) => color === "transparent" || /rgba\(.+,\s*0\)/.test(color);
      const effectiveBackground = (element: Element): string => {
        for (let node: Element | null = element; node; node = node.parentElement) {
          const color = getComputedStyle(node).backgroundColor;
          if (!isTransparent(color)) return color;
        }
        return "rgb(255, 255, 255)";
      };
      const sample = (selector: string) => {
        const element = document.querySelector(selector);
        if (!element) return null;
        return { color: getComputedStyle(element).color, background: effectiveBackground(element) };
      };
      return {
        heading: sample("h1"),
        help: sample(".page-help, .eyebrow"),
      };
    });
    if (!pairs.heading || !pairs.help) throw new Error("theme contrast samples missing");
    expect(contrast(pairs.heading.color, pairs.heading.background), "heading on effective background").toBeGreaterThanOrEqual(4.5);
    expect(contrast(pairs.help.color, pairs.help.background), "secondary text on effective background").toBeGreaterThanOrEqual(4.5);
  }
});
