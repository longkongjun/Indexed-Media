import type { MediaFlowClient, ProcessingTaskOrganization } from "@mediaflow/api-client-ts";
import { computed, ref } from "vue";
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
 * 等待用户显式重试，绝不在 SSE 缺口或重连后自动重复文件副作用。
 */
export function useOrganizationTask(
  client: MediaFlowClient,
  id: string,
  onFailure?: AuthenticatedFailureHandler,
) {
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
    actionError.value = message;
    if (message) actionErrorOccurrence.value += 1;
  }

  async function load(): Promise<void> {
    const stale = projection.value !== null;
    state.value = { kind: "loading", stale };
    try {
      projection.value = await client.getProcessingTaskOrganization(id);
      state.value = { kind: "content" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
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
    try {
      const current = await client.getProcessingTaskOrganization(id);
      projection.value = current;
      state.value = { kind: "content" };
      errorMessage.value = "";
      return applied(kind, before, current);
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error)
        ? { kind: "offline", stale: true }
        : { kind: "error", stale: true };
      return false;
    }
  }

  async function command(kind: OrganizationCommand): Promise<void> {
    const before = projection.value;
    if (!before || pendingAction.value) return;
    if (kind === "recalculate" && !canRecalculate.value) return;
    if (kind === "execute" && !canExecute.value) return;
    if (kind === "rollback" && !canRollback.value) return;
    const key = actionKeys[kind] ?? crypto.randomUUID();
    actionKeys[kind] = key;
    pendingAction.value = kind;
    setActionError("");
    const request = () => {
      if (kind === "recalculate") return client.recalculateProcessingTaskOrganization(id, key);
      if (kind === "execute") return client.executeProcessingTaskOrganization(id, before.plan!.version, key);
      return client.rollbackProcessingTaskOrganization(id, before.local_result!.version, key);
    };
    try {
      projection.value = await request();
      state.value = { kind: "content" };
      actionKeys[kind] = null;
    } catch (error) {
      await onFailure?.(error);
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
      pendingAction.value = null;
    }
  }

  return {
    state, projection, errorMessage, actionError, actionErrorOccurrence, pendingAction,
    canRecalculate, canExecute, canRollback, lastUpdated,
    load,
    recalculate: () => command("recalculate"),
    execute: () => command("execute"),
    rollback: () => command("rollback"),
  };
}
