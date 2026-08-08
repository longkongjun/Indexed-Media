import type { MediaFlowClient, MediaItemListOptions, MediaItemPage } from "@mediaflow/api-client-ts";
import { ref } from "vue";
import { isOffline, safeApiError, type AuthenticatedFailureHandler } from "../../components/apiErrors";
import type { TaskCenterState } from "../processing-tasks/model";

/**
 * 管理正式 Catalog 的有界列表投影、服务端筛选和游标。
 *
 * @param client - 用于查询正式媒体目录的已认证 MediaFlow 传输层。
 * @param initialContext - 初次加载使用的服务端筛选和游标。
 * @param onFailure - 请求失败后等待的可选会话/错误钩子。
 * @returns 响应式条目、游标、查询上下文、错误消息和异步状态，以及重新加载操作。
 * @remarks 每次加载会复制调用方查询，避免其后续修改影响在途请求；失败会保留已加载目录并标记为过期。
 */
export function useMediaItems(
  client: MediaFlowClient,
  initialContext: MediaItemListOptions = {},
  onFailure?: AuthenticatedFailureHandler,
) {
  const state = ref<TaskCenterState>({ kind: "loading", stale: false });
  const items = ref<MediaItemPage["items"]>([]);
  const nextCursor = ref<string | null>(null);
  const errorMessage = ref("");
  const context = ref<MediaItemListOptions>({ ...initialContext });

  async function load(next: MediaItemListOptions = context.value): Promise<void> {
    context.value = { ...next };
    const stale = items.value.length > 0;
    state.value = { kind: "loading", stale };
    try {
      const page = await client.listMediaItems(context.value);
      items.value = page.items;
      nextCursor.value = page.next_cursor;
      state.value = { kind: page.items.length > 0 ? "content" : "empty" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载正式媒体目录，请重试").message;
    }
  }

  return { state, items, nextCursor, errorMessage, context, load };
}
