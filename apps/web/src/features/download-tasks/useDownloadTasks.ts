import type {
  CreateDownloadTaskRequest,
  DownloadTask,
  DownloadTaskListOptions,
  MediaFlowClient,
} from "@mediaflow/api-client-ts";
import { computed, reactive, ref } from "vue";
import {
  isAmbiguousWriteFailure,
  isOffline,
  safeApiError,
  type AuthenticatedFailureHandler,
  type SafeError,
} from "../../components/apiErrors";
import type { TaskCenterState } from "../processing-tasks/model";

interface PendingCreate {
  body: CreateDownloadTaskRequest;
  idempotencyKey: string;
}

/**
 * 管理下载任务列表、详情与可恢复的手工创建。
 *
 * @param client - 已认证的 MediaFlow 客户端。
 * @param initialContext - 初始服务端过滤和游标。
 * @param onFailure - 会话过期等共享失败处理器。
 * @returns 脱敏任务投影、创建表单、异步状态和读写操作。
 * @remarks 源不会进入公开响应式投影；第一次响应丢失会立即用同一请求和幂等键重放。连续不确定失败时，
 * 恢复槽只在 feature 私有闭包中保留，表单立即清空，后续显式重试仍复用原键。离线时禁止新写入。
 */
export function useDownloadTasks(
  client: MediaFlowClient,
  initialContext: DownloadTaskListOptions = {},
  onFailure?: AuthenticatedFailureHandler,
) {
  const state = ref<TaskCenterState>({ kind: "loading", stale: false });
  const items = ref<DownloadTask[]>([]);
  const selected = ref<DownloadTask | null>(null);
  const nextCursor = ref<string | null>(null);
  const errorMessage = ref("");
  const formError = ref<SafeError | null>(null);
  const context = ref<DownloadTaskListOptions>({ ...initialContext });
  const busy = ref(false);
  const form = reactive({ connectionId: "", source: "", displayName: "" });
  let pending: PendingCreate | null = null;
  let keySequence = 0;
  const canWrite = computed(() => state.value.kind !== "offline" && !busy.value);

  function nextKey(): string {
    keySequence += 1;
    return `download-${Date.now().toString(36)}-${keySequence.toString(36)}`;
  }

  async function load(next: DownloadTaskListOptions = context.value): Promise<void> {
    context.value = { ...next };
    const stale = items.value.length > 0;
    state.value = { kind: "loading", stale };
    try {
      const page = await client.listDownloadTasks(context.value);
      items.value = page.items;
      nextCursor.value = page.next_cursor;
      state.value = { kind: page.items.length > 0 ? "content" : "empty" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载下载任务，请重试").message;
    }
  }

  async function loadDetail(id: string): Promise<void> {
    const stale = selected.value !== null;
    state.value = { kind: "loading", stale };
    try {
      selected.value = await client.getDownloadTask(id);
      state.value = { kind: "content" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载下载任务详情，请重试").message;
    }
  }

  async function create(): Promise<DownloadTask | null> {
    if (!canWrite.value) throw new Error("当前离线，不能创建下载任务");
    if (!pending) {
      pending = {
        body: {
          connection_id: form.connectionId,
          source: form.source,
          display_name: form.displayName.trim(),
        },
        idempotencyKey: nextKey(),
      };
    }
    form.source = "";
    busy.value = true;
    formError.value = null;
    const request = pending;
    try {
      let created: DownloadTask;
      try {
        created = await client.createDownloadTask(request.body, request.idempotencyKey);
      } catch (error) {
        if (!isAmbiguousWriteFailure(error)) throw error;
        created = await client.createDownloadTask(request.body, request.idempotencyKey);
      }
      pending = null;
      items.value = [created, ...items.value.filter((item) => item.id !== created.id)];
      state.value = { kind: "content" };
      return created;
    } catch (error) {
      await onFailure?.(error);
      if (!isAmbiguousWriteFailure(error)) pending = null;
      formError.value = safeApiError(
        error,
        isAmbiguousWriteFailure(error)
          ? "创建结果尚未确认；重试会复用同一请求"
          : "创建下载任务失败，请检查输入",
      );
      return null;
    } finally {
      busy.value = false;
    }
  }

  function discardPending(): void {
    pending = null;
    form.source = "";
  }

  return {
    state, items, selected, nextCursor, errorMessage, formError, context, busy, form,
    canWrite, load, loadDetail, create, discardPending,
  };
}
