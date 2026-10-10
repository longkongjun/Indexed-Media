import type {
  AutomationEvent,
  AutomationEventListOptions,
  MediaFlowClient,
} from "@mediaflow/api-client-ts";
import { computed, ref } from "vue";
import {
  isOffline,
  safeApiError,
  type AuthenticatedFailureHandler,
  type SafeError,
} from "../../components/apiErrors";
import type { TaskCenterState } from "../processing-tasks/model";

/** 管理脱敏 automation event 列表、详情和服务端授权的恢复动作。 */
export function useAutomationEvents(
  client: MediaFlowClient,
  initialContext: AutomationEventListOptions = {},
  onFailure?: AuthenticatedFailureHandler,
) {
  const state = ref<TaskCenterState>({ kind: "loading", stale: false });
  const items = ref<AutomationEvent[]>([]);
  const selected = ref<AutomationEvent | null>(null);
  let loadEpoch = 0;
  const context = ref<AutomationEventListOptions>({ ...initialContext });
  const nextCursor = ref<string | null>(null);
  const errorMessage = ref("");
  const actionError = ref<SafeError | null>(null);
  const busy = ref(false);
  let keySequence = 0;
  const canWrite = computed(() => state.value.kind !== "offline" && !busy.value);

  function replaceItem(value: AutomationEvent): void {
    items.value = [value, ...items.value.filter((item) => item.id !== value.id)];
    if (selected.value?.id === value.id) selected.value = value;
  }

  async function load(next: AutomationEventListOptions = context.value): Promise<void> {
    // epoch 守卫：连续触发多个 load 时，只有最新请求的结果可以写入状态；
    // 乱序晚到的旧响应（成功或失败）一律丢弃，避免旧数据覆盖新筛选上下文。
    const epoch = ++loadEpoch;
    context.value = { ...next };
    const stale = items.value.length > 0;
    state.value = { kind: "loading", stale };
    try {
      const page = await client.listAutomationEvents(next);
      if (epoch !== loadEpoch) return;
      items.value = page.items;
      nextCursor.value = page.next_cursor;
      state.value = { kind: page.items.length > 0 ? "content" : "empty" };
      errorMessage.value = "";
    } catch (error) {
      if (epoch !== loadEpoch) return;
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载自动化事件，请重试").message;
    }
  }

  async function loadDetail(id: string): Promise<void> {
    const stale = selected.value !== null;
    state.value = { kind: "loading", stale };
    try {
      selected.value = await client.getAutomationEvent(id);
      state.value = { kind: "content" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载事件详情，请重试").message;
    }
  }

  const canRetry = (value: AutomationEvent) => canWrite.value && value.allowed_actions.includes("retry");
  const canCancel = (value: AutomationEvent) => canWrite.value && value.allowed_actions.includes("cancel");

  function nextKey(action: string): string {
    keySequence += 1;
    return `automation-${action}-${Date.now().toString(36)}-${keySequence.toString(36)}`;
  }

  async function retry(value: AutomationEvent): Promise<AutomationEvent | null> {
    if (!canRetry(value)) return null;
    busy.value = true;
    actionError.value = null;
    try {
      const updated = await client.retryAutomationEvent(value.id, nextKey("retry"));
      replaceItem(updated);
      return updated;
    } catch (error) {
      await onFailure?.(error);
      actionError.value = safeApiError(error, "事件重试失败，请刷新真值后重试");
      return null;
    } finally {
      busy.value = false;
    }
  }

  async function cancel(value: AutomationEvent): Promise<AutomationEvent | null> {
    if (!canCancel(value)) return null;
    busy.value = true;
    actionError.value = null;
    try {
      const updated = await client.cancelAutomationEvent(value.id, nextKey("cancel"));
      replaceItem(updated);
      return updated;
    } catch (error) {
      await onFailure?.(error);
      actionError.value = safeApiError(error, "事件取消失败，请刷新真值后重试");
      return null;
    } finally {
      busy.value = false;
    }
  }

  function resultText(value: AutomationEvent): string {
    if (value.action === "reconcile-inbox" && value.status === "completed") {
      return value.result_count === 0
        ? "对账完成，未发现新文件"
        : `对账完成，发现 ${String(value.result_count ?? 0)} 个新文件`;
    }
    if (value.downstream_kind === "download-task" && value.downstream_id) {
      return "下载任务已创建并关联原事件";
    }
    return value.status === "completed" ? "动作已完成" : "等待动作结果";
  }

  return {
    state, items, selected, context, nextCursor, errorMessage, actionError,
    busy, canWrite, load, loadDetail, canRetry, canCancel, retry, cancel, resultText,
  };
}
