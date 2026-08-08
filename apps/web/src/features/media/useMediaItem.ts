import type { MediaFlowClient, MediaItemDetail } from "@mediaflow/api-client-ts";
import { ref } from "vue";
import { isOffline, safeApiError, type AuthenticatedFailureHandler } from "../../components/apiErrors";
import type { TaskCenterState } from "../processing-tasks/model";

/**
 * 加载一条正式 Catalog 媒体记录，并保留上一次成功详情以支持失败后的过期展示。
 *
 * @param client - 用于读取正式媒体详情的已认证 MediaFlow 传输层。
 * @param id - 正式 MediaItem UUID。
 * @param onFailure - 请求失败后等待的可选会话/错误钩子。
 * @returns 响应式详情、错误消息和异步状态，以及重新加载操作。
 * @remarks 加载失败不会清空已有详情；网络 `TypeError` 会显示为离线状态而不会向组件抛出。
 */
export function useMediaItem(client: MediaFlowClient, id: string, onFailure?: AuthenticatedFailureHandler) {
  const state = ref<TaskCenterState>({ kind: "loading", stale: false });
  const detail = ref<MediaItemDetail | null>(null);
  const errorMessage = ref("");

  async function load(): Promise<void> {
    const stale = detail.value !== null;
    state.value = { kind: "loading", stale };
    try {
      detail.value = await client.getMediaItem(id);
      state.value = { kind: "content" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载媒体详情，请重试").message;
    }
  }

  return { state, detail, errorMessage, load };
}
