import { defineConfig, devices } from "@playwright/test";

/** Opt-in: a real disposable Core process, SQLite database, files and Vite proxy per test. */
export default defineConfig({
  testDir: ".",
  testMatch: "real-core.spec.ts",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  reporter: [["line"], ["json", { outputFile: `${process.env.MEDIAFLOW_TEST_OUTPUT_DIR ?? "/tmp/mediaflow-real-core"}/report.json` }]],
  outputDir: `${process.env.MEDIAFLOW_TEST_OUTPUT_DIR ?? "/tmp/mediaflow-real-core"}/artifacts`,
  use: { trace: "off", screenshot: "off", video: "off" },
  projects: [
    { name: "chromium-desktop", use: { ...devices["Desktop Chrome"] } },
    { name: "chromium-mobile", use: { ...devices["Pixel 7"] } },
  ],
});
