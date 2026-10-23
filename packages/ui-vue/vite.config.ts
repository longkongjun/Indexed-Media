import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vitest/config";

/**
 * packages/ui-vue 的构建与测试配置。
 *
 * 构建产出 dist/index.js（Vue 外部化、不内联运行时）与 dist/style.css（theme.css 先于组件样式抽取）；
 * 测试使用 jsdom 运行隔离 DOM 用例，样式与几何验证不依赖本配置的 CSS 处理。
 */
export default defineConfig({
  plugins: [vue()],
  build: {
    lib: {
      entry: "src/index.ts",
      formats: ["es"],
      fileName: "index",
      cssFileName: "style",
    },
    rollupOptions: {
      external: ["vue"],
    },
  },
  test: {
    environment: "jsdom",
    restoreMocks: true,
  },
});
