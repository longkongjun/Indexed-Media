import { defineConfig, devices } from "@playwright/test";

const outputRoot = process.env.MEDIAFLOW_TEST_OUTPUT_DIR ?? "/tmp/mediaflow-playwright";
/**
 * 使用专用本地 Vite 服务器串行执行桌面端和移动端 M2 浏览器流程的 Playwright 配置。
 *
 * 产物写入 `MEDIAFLOW_TEST_OUTPUT_DIR`（或 `/tmp/mediaflow-playwright`）下方，仅在失败时保留追踪记录。
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  retries: 0,
  reporter: [["line"]],
  outputDir: `${outputRoot}/artifacts`,
  use: { baseURL: "http://127.0.0.1:4198", trace: "retain-on-failure" },
  projects: [
    { name: "chromium-desktop", use: { ...devices["Desktop Chrome"] } },
    { name: "chromium-mobile", use: { ...devices["Pixel 7"] } },
  ],
  webServer: { command: "./node_modules/.bin/vite --host 127.0.0.1 --port 4198 --strictPort", cwd: import.meta.dirname, url: "http://127.0.0.1:4198", reuseExistingServer: false, timeout: 30_000 },
});
