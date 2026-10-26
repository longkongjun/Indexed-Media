import type { MediaFlowClient, ProcessingTaskOrganization } from "@mediaflow/api-client-ts";
import { computed, getCurrentScope, onScopeDispose, ref } from "vue";
import {
  isAmbiguousWriteFailure,
  isOffline,
  safeApiError,
  type AuthenticatedFailureHandler,
} from "../../components/apiErrors";
import type { TaskCenterState } from "../processing-tasks/model";

type OrganizationCommand = "recalculate" | "execute" | "rollback";

/**
 * 管理一个 ProcessingTask 的 organization 权威投影和三项版本绑定命令。
 *
 * @remarks 每个用户意图只生成一个幂等键。响应不明确时先 GET；只有 GET 仍显示原动作未应用时才保留键，
 * 等待用户显式重试，绝不在 SSE 缺口或重连后自动重复文件副作用。execute 绑定确认打开时的计划版本，
 * 与当前投影不一致时本地阻断并要求重新审阅，不静默批准新版本。
 */
export function useOrganizationTask(
  client: MediaFlowClient,
  id: string,
  onFailure?: AuthenticatedFailureHandler,
) {
  // 组件卸载使旧请求失去副作用归属；已发出的服务端操作不等于已撤销。
  let disposed = false;
  if (getCurrentScope()) onScopeDispose(() => { disposed = true; });
  const isActive = (): boolean => !disposed;

  const state = ref<TaskCenterState>({ kind: "loading", stale: false });
  const projection = ref<ProcessingTaskOrganization | null>(null);
  const errorMessage = ref("");
  const actionError = ref("");
  const actionErrorOccurrence = ref(0);
  const pendingAction = ref<OrganizationCommand | null>(null);
  const actionKeys: Record<OrganizationCommand, string | null> = {
    recalculate: null,
    execute: null,
    rollback: null,
  };

  const onlineForWrites = computed(() => state.value.kind !== "offline" && pendingAction.value === null);
  const canRecalculate = computed(() => allowed("recalculate"));
  const canExecute = computed(() => allowed("execute") && projection.value?.plan !== null);
  const canRollback = computed(() => allowed("rollback") && projection.value?.local_result !== null);
  const lastUpdated = computed(() =>
    projection.value?.local_result?.updated_at
      ?? projection.value?.journals.at(-1)?.updated_at
      ?? projection.value?.plan?.created_at
      ?? null,
  );

  function allowed(action: ProcessingTaskOrganization["allowed_actions"][number]): boolean {
    return Boolean(onlineForWrites.value && projection.value?.allowed_actions.includes(action));
  }

  function setActionError(message: string): void {
    if (!isActive()) return;
    actionError.value = message;
    if (message) actionErrorOccurrence.value += 1;
  }

  async function load(): Promise<void> {
    if (!isActive()) return;
    const stale = projection.value !== null;
    state.value = { kind: "loading", stale };
    try {
      const next = await client.getProcessingTaskOrganization(id);
      if (!isActive()) return;
      projection.value = next;
      state.value = { kind: "content" };
      errorMessage.value = "";
    } catch (error) {
      if (!isActive()) return;
      await onFailure?.(error);
      if (!isActive()) return;
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载整理计划与结果，请重试").message;
    }
  }

  function applied(
    kind: OrganizationCommand,
    before: ProcessingTaskOrganization,
    current: ProcessingTaskOrganization,
  ): boolean {
    if (kind === "recalculate") return (current.plan?.version ?? 0) > (before.plan?.version ?? 0);
    if (kind === "execute") {
      return current.plan?.version === before.plan?.version
        && (current.plan?.authorization === "one-time" || !current.allowed_actions.includes("execute"));
    }
    return current.local_result?.version !== before.local_result?.version
      || current.local_result?.status === "compensated"
      || current.local_result?.status === "manual-review";
  }

  async function refreshUnknown(
    kind: OrganizationCommand,
    before: ProcessingTaskOrganization,
  ): Promise<boolean> {
    if (!isActive()) return false;
    try {
      const current = await client.getProcessingTaskOrganization(id);
      if (!isActive()) return false;
      projection.value = current;
      state.value = { kind: "content" };
      errorMessage.value = "";
      return applied(kind, before, current);
    } catch (error) {
      if (!isActive()) return false;
      await onFailure?.(error);
      if (!isActive()) return false;
      state.value = isOffline(error)
        ? { kind: "offline", stale: true }
        : { kind: "error", stale: true };
      return false;
    }
  }

  async function command(kind: OrganizationCommand, confirmedPlanVersion?: number): Promise<void> {
    const before = projection.value;
    if (!isActive() || !before || pendingAction.value) return;
    if (kind === "recalculate" && !canRecalculate.value) return;
    if (kind === "execute" && !canExecute.value) return;
    if (kind === "rollback" && !canRollback.value) return;
    if (kind === "execute" && confirmedPlanVersion !== undefined && before.plan?.version !== confirmedPlanVersion) {
      // 陈旧确认：不发送任何请求，新版本必须重新核对后批准（服务端版本冲突仍是最终防线）。
      setActionError("计划版本已变化，请重新核对最新计划后再批准");
      return;
    }
    const key = actionKeys[kind] ?? crypto.randomUUID();
    actionKeys[kind] = key;
    pendingAction.value = kind;
    setActionError("");
    const request = () => {
      if (kind === "recalculate") return client.recalculateProcessingTaskOrganization(id, key);
      if (kind === "execute") {
        const plan = before.plan;
        if (!plan) throw new Error("execute requires a persisted plan");
        return client.executeProcessingTaskOrganization(id, plan.version, key);
      }
      const localResult = before.local_result;
      if (!localResult) throw new Error("rollback requires a persisted local result");
      return client.rollbackProcessingTaskOrganization(id, localResult.version, key);
    };
    try {
      const next = await request();
      if (!isActive()) return;
      projection.value = next;
      state.value = { kind: "content" };
      actionKeys[kind] = null;
    } catch (error) {
      if (!isActive()) return;
      await onFailure?.(error);
      if (!isActive()) return;
      if (isAmbiguousWriteFailure(error)) {
        if (await refreshUnknown(kind, before)) {
          actionKeys[kind] = null;
        } else {
          setActionError("操作结果尚未确认；恢复连接后可使用同一操作安全重试");
        }
      } else {
        actionKeys[kind] = null;
        setActionError(safeApiError(error, "整理操作未完成，请刷新后重试").message);
      }
    } finally {
      if (isActive()) pendingAction.value = null;
    }
  }

  return {
    state, projection, errorMessage, actionError, actionErrorOccurrence, pendingAction,
    canRecalculate, canExecute, canRollback, lastUpdated,
    load,
    recalculate: () => command("recalculate"),
    execute: (confirmedPlanVersion?: number) => command("execute", confirmedPlanVersion),
    rollback: () => command("rollback"),
  };
}
