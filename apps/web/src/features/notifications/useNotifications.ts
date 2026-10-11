import type { MediaFlowClient, Notification } from "@mediaflow/api-client-ts";
import { ref } from "vue";
import { isOffline, safeApiError, type AuthenticatedFailureHandler } from "../../components/apiErrors";

/**
 * 管理应用内通知的最新在前列表与已读状态。
 *
 * @param client - 已认证的管理员 MediaFlow 传输层。
 * @param onFailure - 请求失败后等待的可选会话/错误钩子。
 * @returns 响应式通知、未读计数、游标与加载/已读操作。
 * @remarks 已读标记幂等；加载失败保留已加载内容。
 */
export function useNotifications(client: MediaFlowClient, onFailure?: AuthenticatedFailureHandler) {
  const state = ref<{ kind: "loading" | "content" | "empty" | "error" | "offline"; stale: boolean }>({
    kind: "loading",
    stale: false,
  });
  const items = ref<Notification[]>([]);
  const nextCursor = ref<string | null>(null);
  const unreadCount = ref(0);
  const errorMessage = ref("");
  let loadEpoch = 0;

  async function load(options: { unreadOnly?: boolean } = {}): Promise<void> {
    const epoch = ++loadEpoch;
    state.value = { kind: "loading", stale: items.value.length > 0 };
    try {
      const page = await client.listNotifications({ unreadOnly: options.unreadOnly });
      if (epoch !== loadEpoch) return;
      items.value = page.items;
      nextCursor.value = page.next_cursor;
      unreadCount.value = page.unread_count;
      state.value = { kind: page.items.length ? "content" : "empty", stale: false };
      errorMessage.value = "";
    } catch (error) {
      if (epoch !== loadEpoch) return;
      await onFailure?.(error);
      state.value = { kind: isOffline(error) ? "offline" : "error", stale: items.value.length > 0 };
      errorMessage.value = safeApiError(error, "无法加载通知，请重试").message;
    }
  }

  async function loadMore(): Promise<void> {
    if (!nextCursor.value) return;
    const epoch = ++loadEpoch;
    try {
      const page = await client.listNotifications({ cursor: nextCursor.value });
      if (epoch !== loadEpoch) return;
      items.value = [...items.value, ...page.items];
      nextCursor.value = page.next_cursor;
      errorMessage.value = "";
    } catch (error) {
      if (epoch !== loadEpoch) return;
      await onFailure?.(error);
      errorMessage.value = safeApiError(error, "无法加载更早的通知，请重试").message;
    }
  }

  /**
   * 标记已读（幂等）。
   *
   * @param notificationId - 省略时标记全部已读。
   * @returns 成功返回 true；失败返回 false（错误经 `errorMessage` 透出，已加载内容不清空）。
   */
  async function markRead(notificationId?: string): Promise<boolean> {
    try {
      await client.markNotificationsRead(notificationId);
      if (notificationId) {
        unreadCount.value = Math.max(0, unreadCount.value - 1);
      } else {
        unreadCount.value = 0;
      }
      return true;
    } catch (error) {
      await onFailure?.(error);
      errorMessage.value = safeApiError(error, "无法标记已读，请重试").message;
      return false;
    }
  }

  return { state, items, nextCursor, unreadCount, errorMessage, load, loadMore, markRead };
}
