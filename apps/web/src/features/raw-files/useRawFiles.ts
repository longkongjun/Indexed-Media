import type { DiscoveredFilePage, MediaFlowClient, ScanErrorPage } from "@mediaflow/api-client-ts";
import { ref } from "vue";
import type { AsyncState } from "../../components/asyncState";
import { isOffline, safeApiError, type AuthenticatedFailureHandler } from "../../components/apiErrors";

/**
 * 管理一个扫描任务中独立分页的已发现文件和扫描错误投影。
 *
 * @param client - 用于获取两个投影的已认证 MediaFlow 传输层。
 * @param taskId - 请求其持久化结果的扫描任务 UUID。
 * @param options - 加载操作未收到显式游标时使用的初始游标。
 * @param onFailure - 请求失败后等待的可选会话/错误钩子。
 * @returns 响应式结果、游标、消息和异步状态 ref，以及 `load` 和 `loadErrors` 操作。
 * @remarks 加载只替换所请求的投影；失败会保留过期条目，并将网络 `TypeError` 分类为离线，而不是向组件抛出。
 */
export function useRawFiles(client: MediaFlowClient, taskId: string, options: { cursor?: string | null; errorCursor?: string | null } = {}, onFailure?: AuthenticatedFailureHandler) {
  const state = ref<AsyncState>({ kind: "idle" });
  const errorState = ref<AsyncState>({ kind: "idle" });
  const files = ref<DiscoveredFilePage["items"]>([]);
  const errors = ref<ScanErrorPage["items"]>([]);
  const nextCursor = ref<string | null>(null);
  const nextErrorCursor = ref<string | null>(null);
  const errorMessage = ref("");
  const pageLabel = ref("当前结果");

  async function load(cursor = options.cursor ?? undefined): Promise<void> {
    const stale = files.value.length > 0;
    state.value = { kind: "loading", stale };
    try {
      const page = await client.listScanTaskFiles(taskId, cursor || undefined);
      files.value = page.items;
      nextCursor.value = page.next_cursor;
      state.value = { kind: files.value.length ? "content" : "empty" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载原始文件，请重试").message;
    }
  }
  async function loadErrors(cursor = options.errorCursor ?? undefined): Promise<void> {
    const stale = errors.value.length > 0;
    errorState.value = { kind: "loading", stale };
    try {
      const page = await client.listScanTaskErrors(taskId, cursor || undefined);
      errors.value = page.items;
      nextErrorCursor.value = page.next_cursor;
      errorState.value = { kind: errors.value.length ? "content" : "empty" };
    } catch (error) {
      await onFailure?.(error);
      errorState.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
    }
  }
  return { state, errorState, files, errors, nextCursor, nextErrorCursor, errorMessage, pageLabel, load, loadErrors };
}
