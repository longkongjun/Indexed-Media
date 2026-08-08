import type { MediaFlowClient, ScanTask } from "@mediaflow/api-client-ts";
import { computed, ref } from "vue";
import type { AsyncState } from "../../components/asyncState";
import { isAmbiguousWriteFailure, isOffline, safeApiError, type AuthenticatedFailureHandler } from "../../components/apiErrors";

/**
 * 通过路由查询参数携带的、可恢复的扫描任务列表上下文。
 *
 * `status` 是客户端过滤条件，`cursor` 选择服务端页面，`scrollKey` 标识从详情页返回后应恢复的列表卡片。
 */
export interface TaskListContext { status?: string; cursor?: string; scrollKey?: string }

/**
 * 管理一页游标扫描任务及其可恢复的路由上下文。
 *
 * @param client - 用于列出任务的已认证 MediaFlow 传输层。
 * @param context - 初始状态、游标和滚动恢复上下文。
 * @param onFailure - 请求失败后等待的可选会话/错误钩子。
 * @returns 响应式列表状态、下一游标、安全错误消息、返回查询参数和 `load` 操作。
 * @remarks 状态过滤在内存中应用于已获取页面；失败会保留过期任务且不会抛出。
 */
export function useScanTasks(client: MediaFlowClient, context: TaskListContext = {}, onFailure?: AuthenticatedFailureHandler) {
  const state = ref<AsyncState>({ kind: "idle" });
  const tasks = ref<ScanTask[]>([]);
  const nextCursor = ref<string | null>(null);
  const errorMessage = ref("");
  const currentContext = ref<TaskListContext>({ ...context });
  const returnQuery = computed(() => ({
    ...(currentContext.value.status ? { status: currentContext.value.status } : {}),
    ...(currentContext.value.cursor ? { cursor: currentContext.value.cursor } : {}),
    ...(currentContext.value.scrollKey ? { context: currentContext.value.scrollKey } : {}),
  }));
  async function load(nextContext: TaskListContext = currentContext.value): Promise<void> {
    currentContext.value = { ...nextContext };
    const stale = tasks.value.length > 0;
    state.value = { kind: "loading", stale };
    try {
      const page = await client.listScanTasks(currentContext.value.cursor);
      tasks.value = currentContext.value.status ? page.items.filter((item) => item.status === currentContext.value.status) : page.items;
      nextCursor.value = page.next_cursor;
      state.value = { kind: tasks.value.length ? "content" : "empty" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载扫描任务，请重试").message;
    }
  }
  return { state, tasks, nextCursor, errorMessage, returnQuery, load };
}

/**
 * 管理一个扫描任务、其重试/取消可用性以及从事件流接收的快照。
 *
 * @param client - 用于详情和变更请求的已认证 MediaFlow 传输层。
 * @param id - 扫描任务 UUID。
 * @param onFailure - 请求失败后等待的可选会话/错误钩子。
 * @returns 响应式详情/操作状态，以及加载、重试、取消和应用快照操作。
 * @remarks 重试和取消会在不明确响应及其单次重放间复用同一个幂等键。API 失败会成为安全的操作错误；
 * 第二次不明确失败会保留该键，供稍后安全重试。
 */
export function useScanTask(client: MediaFlowClient, id: string, onFailure?: AuthenticatedFailureHandler) {
  const state = ref<AsyncState>({ kind: "idle" });
  const task = ref<ScanTask | null>(null);
  const errorMessage = ref("");
  const actionError = ref<{ message: string } | null>(null);
  const actionErrorOccurrence = ref(0);
  const pendingAction = ref<"retry" | "cancel" | null>(null);
  const showRetry = computed(() => Boolean(task.value && ["failed", "partial-success"].includes(task.value.status)));
  const showCancel = computed(() => Boolean(task.value && ["queued", "running"].includes(task.value.status)));
  const canRetry = computed(() => Boolean(showRetry.value && state.value.kind !== "offline" && !pendingAction.value));
  const canCancel = computed(() => Boolean(showCancel.value && state.value.kind !== "offline" && !pendingAction.value));
  const actionKeys: Record<"retry" | "cancel", string | null> = { retry: null, cancel: null };
  function setActionError(message: string): void { actionError.value = { message }; actionErrorOccurrence.value += 1; }

  async function load(): Promise<void> {
    const stale = task.value !== null;
    state.value = { kind: "loading", stale };
    try {
      task.value = await client.getScanTask(id);
      state.value = { kind: "content" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载扫描任务，请重试").message;
    }
  }

  async function act(kind: "retry" | "cancel"): Promise<void> {
    if (pendingAction.value) return;
    if ((kind === "retry" && !canRetry.value) || (kind === "cancel" && !canCancel.value)) return;
    pendingAction.value = kind;
    const key = actionKeys[kind] ?? crypto.randomUUID();
    actionKeys[kind] = key;
    const request = () => kind === "retry" ? client.retryScanTask(id, key) : client.cancelScanTask(id, key);
    const accept = (next: ScanTask) => { task.value = next; state.value = { kind: "content" }; actionKeys[kind] = null; actionError.value = null; };
    try {
      accept(await request());
    } catch (error) {
      await onFailure?.(error);
      if (isAmbiguousWriteFailure(error)) {
        try {
          accept(await request()); return;
        } catch (replayError) {
          await onFailure?.(replayError);
          if (isAmbiguousWriteFailure(replayError)) setActionError(`${kind === "retry" ? "重试" : "取消"}请求结果尚未确认；恢复连接后可安全重试`);
          else { actionKeys[kind] = null; setActionError(safeApiError(replayError, `${kind === "retry" ? "重试" : "取消"}请求失败，请重试`).message); }
        }
      } else {
        actionKeys[kind] = null;
        setActionError(safeApiError(error, `${kind === "retry" ? "重试" : "取消"}请求失败，请重试`).message);
      }
    } finally {
      pendingAction.value = null;
    }
  }

  function applySnapshot(next: ScanTask): void { task.value = next; state.value = { kind: "content" }; }
  return { client, state, task, errorMessage, actionError, actionErrorOccurrence, pendingAction, showRetry, showCancel, canRetry, canCancel, load, retry: () => act("retry"), cancel: () => act("cancel"), applySnapshot };
}

/** 由 {@link useScanTask} 返回并供 SSE 同步使用的公开响应式/操作表面。 */
export type ScanTaskFeature = ReturnType<typeof useScanTask>;
