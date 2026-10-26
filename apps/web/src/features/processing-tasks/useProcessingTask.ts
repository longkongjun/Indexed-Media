import type { MediaFlowClient, ProcessingTask } from "@mediaflow/api-client-ts";
import { computed, getCurrentScope, onScopeDispose, ref } from "vue";
import {
  isAmbiguousWriteFailure,
  isOffline,
  safeApiError,
  type AuthenticatedFailureHandler,
} from "../../components/apiErrors";
import type { TaskCenterState } from "./model";

/**
 * 加载单文件 ProcessingTask，并协调审核入口及可重放的重试、取消操作。
 *
 * @param client - 用于读取任务和提交任务操作的已认证 MediaFlow 传输层。
 * @param id - ProcessingTask UUID。
 * @param onFailure - 请求失败后等待的可选会话/错误钩子。
 * @returns 响应式任务、审核入口、错误和操作能力，以及加载、重试和取消操作。
 * @remarks 结果不明确的写入会使用同一个幂等键重放一次；连续不明确时保留该键，以便恢复连接后安全重试。
 */
export function useProcessingTask(
  client: MediaFlowClient,
  id: string,
  onFailure?: AuthenticatedFailureHandler,
) {
  // 组件卸载使旧请求失去副作用归属；已发出的服务端操作不等于已撤销。
  let disposed = false;
  if (getCurrentScope()) onScopeDispose(() => { disposed = true; });
  const isActive = (): boolean => !disposed;

  const state = ref<TaskCenterState>({ kind: "loading", stale: false });
  const task = ref<ProcessingTask | null>(null);
  const errorMessage = ref("");
  const actionError = ref("");
  const reviewCaseId = ref<string | null>(null);
  const reviewLookupError = ref("");
  const pendingAction = ref<"retry" | "cancel" | null>(null);
  const actionKeys: Record<"retry" | "cancel", string | null> = { retry: null, cancel: null };
  const onlineForWrites = computed(() => state.value.kind !== "offline" && pendingAction.value === null);
  const canReview = computed(() => Boolean(task.value?.allowed_actions.includes("review") && onlineForWrites.value));
  const canRetry = computed(() => Boolean(task.value?.allowed_actions.includes("retry") && onlineForWrites.value));
  const canCancel = computed(() => Boolean(task.value?.allowed_actions.includes("cancel") && onlineForWrites.value));

  async function load(): Promise<void> {
    if (!isActive()) return;
    const stale = task.value !== null;
    state.value = { kind: "loading", stale };
    try {
      const next = await client.getProcessingTask(id);
      if (!isActive()) return;
      task.value = next;
      state.value = { kind: "content" };
      errorMessage.value = "";
      reviewCaseId.value = null;
      reviewLookupError.value = "";
      if (task.value.allowed_actions.includes("review")) {
        try {
          const identification = await client.getProcessingTaskIdentification(id);
          if (!isActive()) return;
          reviewCaseId.value = identification.review_case_id;
        } catch (error) {
          if (!isActive()) return;
          await onFailure?.(error);
          if (!isActive()) return;
          reviewLookupError.value = safeApiError(error, "无法定位当前审核案例，请刷新后重试").message;
        }
      }
    } catch (error) {
      if (!isActive()) return;
      await onFailure?.(error);
      if (!isActive()) return;
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载处理任务，请重试").message;
    }
  }

  async function act(kind: "retry" | "cancel"): Promise<void> {
    if (!isActive() || pendingAction.value || (kind === "retry" ? !canRetry.value : !canCancel.value)) return;
    pendingAction.value = kind;
    const key = actionKeys[kind] ?? crypto.randomUUID();
    actionKeys[kind] = key;
    const request = () => kind === "retry"
      ? client.retryProcessingTask(id, key)
      : client.cancelProcessingTask(id, key);
    const accept = (next: ProcessingTask) => {
      if (!isActive()) return;
      task.value = next;
      state.value = { kind: "content" };
      actionKeys[kind] = null;
      actionError.value = "";
    };
    try {
      accept(await request());
    } catch (error) {
      if (!isActive()) return;
      await onFailure?.(error);
      if (!isActive()) return;
      if (isAmbiguousWriteFailure(error)) {
        try {
          accept(await request());
        } catch (replayError) {
          if (!isActive()) return;
          await onFailure?.(replayError);
          if (!isActive()) return;
          if (isAmbiguousWriteFailure(replayError)) {
            actionError.value = "操作结果尚未确认；恢复连接后可使用同一请求安全重试";
          } else {
            actionKeys[kind] = null;
            actionError.value = safeApiError(replayError, "任务操作失败，请重试").message;
          }
        }
      } else {
        actionKeys[kind] = null;
        actionError.value = safeApiError(error, "任务操作失败，请重试").message;
      }
    } finally {
      if (isActive()) pendingAction.value = null;
    }
  }

  return {
    state,
    task,
    errorMessage,
    actionError,
    reviewCaseId,
    reviewLookupError,
    pendingAction,
    canReview,
    canRetry,
    canCancel,
    load,
    retry: () => act("retry"),
    cancel: () => act("cancel"),
  };
}
