<script setup lang="ts">
/**
 * 应用根边界：根据初始化/会话连通性探测结果决定是否显示路由内容。
 *
 * 组件没有 props、发出事件或暴露的实例方法。它读取连通性 store，并通过 Vue Router 重试待处理路由；
 * 离线、错误和加载状态会替代路由页面内容。
 */
import { computed } from "vue";
import { useRouter } from "vue-router";
import { useConnectivityStore } from "./app/connectivity";

const router = useRouter();
const connectivity = useConnectivityStore();
const unavailable = computed(() => connectivity.state === "offline" || connectivity.state === "error");

async function retry(): Promise<void> {
  const destination = connectivity.pendingLocation ?? "/";
  await router.replace(destination);
}
</script>

<template>
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
</template>

<style>
:root { font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color-scheme: light; }
body { margin: 0; }
button, input { font: inherit; }
.app-frame { min-height: 100vh; }
.connection-state { display: grid; align-content: center; justify-items: start; min-height: 100vh; box-sizing: border-box; max-width: 42rem; margin: auto; padding: 2rem; }
.state-label { color: #28684d; font-weight: 750; }
.connection-state button { min-height: 2.75rem; padding: .7rem 1rem; border: 0; border-radius: .5rem; background: #176b49; color: white; font-weight: 700; }
.connection-state button:focus-visible { outline: 3px solid #0b7a50; outline-offset: 3px; }
</style>
