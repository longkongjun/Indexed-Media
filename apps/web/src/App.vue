<script setup lang="ts">
/**
 * 应用根边界：根据初始化/会话连通性探测结果决定是否显示路由内容。
 *
 * 组件没有 props、发出事件或暴露的实例方法。它读取连通性 store，并通过 Vue Router 重试待处理路由；
 * 离线、错误和加载状态会替代路由页面内容。整树包在 IndexedThemeProvider 内，为 `@indexed-design/ui-vue`
 * 组件提供浅/暗主题容器（`.id-theme`）；mode 经 designTheme 桥跟随宿主 data-theme 三态。
 */
import { computed } from "vue";
import { IndexedThemeProvider } from "@indexed-design/ui-vue";
import { useRouter } from "vue-router";
import { useConnectivityStore } from "./app/connectivity";
import { readStyleNonce, useDesignThemeMode } from "./app/designTheme";

const router = useRouter();
const connectivity = useConnectivityStore();
const designMode = useDesignThemeMode();
const styleNonce = readStyleNonce();
const designCsp = styleNonce ? { nonce: styleNonce } : undefined;
const unavailable = computed(() => connectivity.state === "offline" || connectivity.state === "error");

async function retry(): Promise<void> {
  const destination = connectivity.pendingLocation ?? "/";
  await router.replace(destination);
}
</script>

<template>
  <IndexedThemeProvider :mode="designMode" :csp="designCsp">
    <div class="app-frame">
      <main v-if="unavailable" class="connection-state" aria-labelledby="connection-title">
        <p class="state-label">{{ connectivity.state === "offline" ? "Offline" : "Error" }}</p>
        <h1 id="connection-title">暂时无法连接 MediaFlow Core</h1>
        <p>页面尚未判定实例初始化状态。请检查 Core 和网络连接后重试。</p>
        <button type="button" @click="retry">重试连接</button>
      </main>
      <main v-else-if="connectivity.state === 'loading'" class="connection-state" aria-live="polite" aria-busy="true">
        <p class="state-label">Loading</p>
        <h1>正在连接 MediaFlow</h1>
      </main>
      <RouterView v-else />
    </div>
  </IndexedThemeProvider>
</template>

<style>
:root { font-family: var(--mfd-font-family-ui); }
body { margin: 0; }
button, input { font: inherit; }
.app-frame { min-height: 100vh; }
.connection-state { display: grid; align-content: center; justify-items: start; min-height: 100vh; box-sizing: border-box; max-width: 42rem; margin: auto; padding: 2rem; }
.state-label { color: var(--mfd-color-text-secondary); font-weight: 750; }
.connection-state button { min-height: 2.75rem; padding: .7rem 1rem; border: 0; border-radius: .5rem; background: var(--mfd-color-action-primary); color: var(--mfd-color-text-on-action); font-weight: 700; }
.connection-state button:focus-visible { outline: 3px solid var(--mfd-color-border-focus); outline-offset: 3px; }
</style>
