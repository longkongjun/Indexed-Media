import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vitest/config";

/**
 * 用于编译 Vue 单文件组件并运行隔离 DOM 测试的 Vite/Vitest 配置。
 *
 * 测试使用 jsdom，并在每个用例后还原监视器和模拟对象。
 */
export default defineConfig({
  plugins: [vue()],
  test: {
    environment: "jsdom",
    restoreMocks: true,
  },
});
