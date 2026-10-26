/**
 * 针对已初始化的隔离Core同源部署检查CSP、主题与预检模态；不创建目录/文件业务记录，不采图或trace。
 * 输入MEDIAFLOW_DEPLOYED_URL、MEDIAFLOW_CSP_TEST_ADMIN、MEDIAFLOW_CSP_TEST_PASSWORD；仅用于明确授权的测试实例。
 */
import assert from "node:assert/strict";
import { chromium, expect } from "@playwright/test";

const url = process.env.MEDIAFLOW_DEPLOYED_URL;
const administrator = process.env.MEDIAFLOW_CSP_TEST_ADMIN;
const password = process.env.MEDIAFLOW_CSP_TEST_PASSWORD;
assert(url && administrator && password, "需要显式测试URL及一次性测试管理员凭据");
const browser = await chromium.launch({ headless: true });
const results = [];
async function check(name, action) {
  await action();
  results.push(name);
  console.log(`PASS ${name}`);
}
try {
  for (const viewport of [{ width: 1440, height: 1000 }, { width: 390, height: 844 }]) {
    const context = await browser.newContext({ baseURL: url, viewport });
    await context.addInitScript(() => localStorage.setItem("mfd-theme", "dark"));
    const page = await context.newPage();
    page.setDefaultTimeout(10_000);
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.addInitScript(() => {
      window.cspViolations = [];
      document.addEventListener("securitypolicyviolation", event => {
        window.cspViolations.push(event.effectiveDirective);
      });
    });
    await check(`${viewport.width}:文档nonce与安全头`, async () => {
      const response = await page.goto("/login");
      assert(response);
      const nonce = await page.locator('meta[name="csp-nonce"]').getAttribute("content");
      assert.match(nonce, /^[A-Za-z0-9_-]{43}$/);
      const policy = response.headers()["content-security-policy"];
      assert.equal(policy, `default-src 'self'; connect-src 'self'; frame-ancestors 'none'; style-src 'self' 'nonce-${nonce}'`);
      assert.equal(response.headers()["cache-control"], "no-store");
      await expect(page.locator('.id-theme[data-theme="dark"]')).toBeVisible();
      await expect.poll(() => page.locator(".ant-input").first().evaluate(el => getComputedStyle(el).backgroundColor)).not.toBe("rgba(0, 0, 0, 0)");
    });
    await page.locator("#login-administrator-name").fill(administrator);
    await page.locator("#login-password").fill(password);
    await page.getByRole("button", { name: "登录", exact: true }).click();
    await page.waitForURL("**/inbox-directories");
    await page.goto("/inbox-directories");
    await page.getByRole("button", { name: "添加收件目录", exact: true }).click();
    await page.getByLabel("能力根", { exact: true }).selectOption("incoming");
    await page.getByLabel("根内相对路径", { exact: true }).fill("camera");
    const trigger = page.getByRole("button", { name: "预检目录", exact: true });
    const modalBackgrounds = [];
    for (const mode of ["light", "dark"]) {
      await check(`${viewport.width}:${mode}主题与模态焦点`, async () => {
        await page.getByRole("button", { name: viewport.width < 768 ? "更多" : "账户与系统", exact: true }).click();
        await page.getByRole("radio", { name: mode === "light" ? "浅色" : "暗色", exact: true }).check();
        await page.getByRole("button", { name: viewport.width < 768 ? "更多" : "账户与系统", exact: true }).click();
        await expect(page.locator(`.id-theme[data-theme="${mode}"]`)).toBeVisible();
        await trigger.click();
        const dialog = page.getByRole("dialog");
        await expect(dialog).toBeVisible();
        await expect(dialog.getByRole("button", { name: /^取\s*消$/ })).toBeFocused();
        await expect.poll(() => page.locator(".ant-modal-content").evaluate(el => {
          const rect = el.getBoundingClientRect();
          return rect.width > 200 && rect.width <= innerWidth && rect.x >= 0;
        })).toBe(true);
        const geometry = await page.locator(".ant-modal-content").evaluate(el => ({
          background: getComputedStyle(el).backgroundColor,
          position: getComputedStyle(el.closest(".ant-modal-wrap")).position,
          width: el.getBoundingClientRect().width,
          x: el.getBoundingClientRect().x,
        }));
        assert.notEqual(geometry.background, "rgba(0, 0, 0, 0)");
        modalBackgrounds.push(geometry.background);
        assert.equal(geometry.position, "fixed");
        assert(geometry.width > 200 && geometry.width <= viewport.width && geometry.x >= 0);
        for (let index = 0; index < 8; index++) {
          await page.keyboard.press("Tab");
          assert(await dialog.evaluate(el => el.contains(document.activeElement)));
        }
        await page.keyboard.press("Shift+Tab");
        assert(await dialog.evaluate(el => el.contains(document.activeElement)));
        await page.keyboard.press("Escape");
        await expect(dialog).not.toBeVisible();
        await expect(trigger).toBeFocused();
      });
    }
    assert.notEqual(modalBackgrounds[0], modalBackgrounds[1], "浅暗模态必须实际换色");
    await check(`${viewport.width}:授权样式无CSP违例`, async () => {
      assert.deepEqual(await page.evaluate(() => window.cspViolations), []);
      assert.deepEqual(errors, []);
      const styles = await page.locator("style[data-css-hash]").evaluateAll(els => els.map(el => el.nonce));
      assert(styles.length > 0);
      const nonce = await page.locator('meta[name="csp-nonce"]').getAttribute("content");
      assert(styles.every(value => value === nonce));
    });
    await check(`${viewport.width}:非法样式和脚本仍被阻断`, async () => {
      const blocked = await page.evaluate(() => {
        const probe = document.createElement("div"); probe.id = "csp-probe"; document.body.append(probe);
        for (const nonce of [null, "wrong-nonce"]) {
          const style = document.createElement("style");
          if (nonce) style.nonce = nonce;
          style.textContent = "#csp-probe { z-index: 937; }"; document.head.append(style);
        }
        probe.setAttribute("style", "z-index: 939");
        for (const nonce of [null, document.querySelector('meta[name="csp-nonce"]').content]) {
          const script = document.createElement("script");
          if (nonce) script.nonce = nonce;
          script.textContent = "window.cspUnexpectedExecution = true"; document.head.append(script);
        }
        return { zIndex: getComputedStyle(probe).zIndex, executed: window.cspUnexpectedExecution === true };
      });
      assert.notEqual(blocked.zIndex, "937"); assert.notEqual(blocked.zIndex, "939"); assert.equal(blocked.executed, false);
      await expect.poll(() => page.evaluate(() => window.cspViolations.length)).toBeGreaterThanOrEqual(5);
    });
    await check(`${viewport.width}:刷新nonce轮换与主题记忆`, async () => {
      const old = await page.locator('meta[name="csp-nonce"]').getAttribute("content");
      await page.reload();
      await expect(page.locator('.id-theme[data-theme="dark"]')).toBeVisible();
      assert.notEqual(await page.locator('meta[name="csp-nonce"]').getAttribute("content"), old);
    });
    await context.close();
  }
  console.log(JSON.stringify({ passed: results.length, checks: results }));
} finally {
  await browser.close();
}
