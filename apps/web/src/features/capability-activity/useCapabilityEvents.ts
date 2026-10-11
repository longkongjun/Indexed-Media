import type { CapabilityActivityEvent, MediaFlowClient } from "@mediaflow/api-client-ts";
import { ref } from "vue";
import { isOffline, safeApiError, type AuthenticatedFailureHandler } from "../../components/apiErrors";

/**
 * 管理 AI 能力通道活动的最新在前只读列表。
 *
 * @param client - 已认证的管理员 MediaFlow 传输层。
 * @param onFailure - 请求失败后等待的可选会话/错误钩子。
 * @returns 响应式事件、游标、错误消息、异步状态与追加加载操作。
 * @remarks 只读投影，无写路径；追加加载失败保留已加载事件。
 */
export function useCapabilityEvents(client: MediaFlowClient, onFailure?: AuthenticatedFailureHandler) {
  const state = ref<{ kind: "loading" | "content" | "empty" | "error" | "offline"; stale: boolean }>({
    kind: "loading",
    stale: false,
  });
  const events = ref<CapabilityActivityEvent[]>([]);
  const nextCursor = ref<string | null>(null);
  const errorMessage = ref("");
  let loadEpoch = 0;

  async function load(): Promise<void> {
    // epoch 守卫：旧响应（成功或失败）不得覆盖更新一轮的状态。
    const epoch = ++loadEpoch;
    state.value = { kind: "loading", stale: events.value.length > 0 };
    try {
      const page = await client.listCapabilityEvents();
      if (epoch !== loadEpoch) return;
      events.value = page.items;
      nextCursor.value = page.next_cursor;
      state.value = { kind: page.items.length ? "content" : "empty", stale: false };
      errorMessage.value = "";
    } catch (error) {
      if (epoch !== loadEpoch) return;
      await onFailure?.(error);
      state.value = { kind: isOffline(error) ? "offline" : "error", stale: events.value.length > 0 };
      errorMessage.value = safeApiError(error, "无法加载 AI 活动，请重试").message;
    }
  }

  async function loadMore(): Promise<void> {
    if (!nextCursor.value) return;
    const epoch = ++loadEpoch;
    const cursor = nextCursor.value;
    try {
      const page = await client.listCapabilityEvents({ cursor });
      if (epoch !== loadEpoch) return;
      events.value = [...events.value, ...page.items];
      nextCursor.value = page.next_cursor;
      errorMessage.value = "";
    } catch (error) {
      if (epoch !== loadEpoch) return;
      await onFailure?.(error);
      errorMessage.value = safeApiError(error, "无法加载更早的 AI 活动，请重试").message;
    }
  }

  return { state, events, nextCursor, errorMessage, load, loadMore };
}
