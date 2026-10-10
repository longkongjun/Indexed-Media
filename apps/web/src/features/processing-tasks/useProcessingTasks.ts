import type {
  MediaFlowClient,
  ProcessingTask,
  ProcessingTaskPage,
} from "@mediaflow/api-client-ts";
import { ref } from "vue";
import { isOffline, safeApiError, type AuthenticatedFailureHandler } from "../../components/apiErrors";
import type { TaskCenterContext, TaskCenterState } from "./model";

/**
 * 管理 ProcessingTask 中心的有界列表投影、汇总信息和分页上下文。
 *
 * @param client - 用于查询处理任务的已认证 MediaFlow 传输层。
 * @param initialContext - 初次加载使用的筛选、游标和返回定位上下文。
 * @param onFailure - 请求失败后等待的可选会话/错误钩子。
 * @returns 响应式任务、汇总、游标、查询上下文、错误消息和异步状态，以及重新加载操作。
 * @remarks 每次加载复制调用方上下文；失败会保留已加载任务，并把网络 `TypeError` 表示为离线。
 */
export function useProcessingTasks(
  client: MediaFlowClient,
  initialContext: TaskCenterContext = {},
  onFailure?: AuthenticatedFailureHandler,
) {
  const state = ref<TaskCenterState>({ kind: "loading", stale: false });
  const tasks = ref<ProcessingTask[]>([]);
  const summary = ref<ProcessingTaskPage["summary"] | null>(null);
  const nextCursor = ref<string | null>(null);
  const errorMessage = ref("");
  const context = ref<TaskCenterContext>({ ...initialContext });
  let loadEpoch = 0;

  async function load(next: TaskCenterContext = context.value): Promise<void> {
    // epoch 守卫：连续触发多个 load 时，只有最新请求的结果可以写入状态；
    // 乱序晚到的旧响应（成功或失败）一律丢弃，避免旧数据覆盖新筛选上下文。
    const epoch = ++loadEpoch;
    context.value = { ...next };
    const stale = tasks.value.length > 0;
    state.value = { kind: "loading", stale };
    try {
      const page = await client.listProcessingTasks(next);
      if (epoch !== loadEpoch) return;
      tasks.value = page.items;
      summary.value = page.summary;
      nextCursor.value = page.next_cursor;
      state.value = { kind: page.items.length > 0 ? "content" : "empty" };
      errorMessage.value = "";
    } catch (error) {
      if (epoch !== loadEpoch) return;
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载处理任务，请重试").message;
    }
  }

  return { state, tasks, summary, nextCursor, errorMessage, context, load };
}

export { useProcessingTask } from "./useProcessingTask";
