import { defineStore } from "pinia";
import { ref } from "vue";

/**
 * 由应用外壳暴露的连通性状态。
 *
 * `loading` 表示正在进行初始化或会话探测，`offline` 专用于 fetch 层失败，`error` 表示其他失败。
 */
export type ConnectivityState = "online" | "loading" | "offline" | "error";

/**
 * 跟踪连通性探测和导航失败后应重试的路由。
 *
 * @returns Pinia store；其 `begin`、`connected` 和 `failed` 操作会更新可见连接状态；`failed` 将 `TypeError`
 * 归类为离线，并保留请求的位置以便重试。
 */
export const useConnectivityStore = defineStore("connectivity", () => {
  const state = ref<ConnectivityState>("loading");
  const pendingLocation = ref<string | null>(null);

  function begin(location: string): void {
    state.value = "loading";
    pendingLocation.value = location;
  }

  function connected(): void {
    state.value = "online";
    pendingLocation.value = null;
  }

  function failed(error: unknown, location: string): void {
    state.value = error instanceof TypeError ? "offline" : "error";
    pendingLocation.value = location;
  }

  return { state, pendingLocation, begin, connected, failed };
});
