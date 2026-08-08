import type { DeploymentRootList, InboxDirectory, InboxDirectoryPreflight, MediaFlowClient } from "@mediaflow/api-client-ts";
import { computed, reactive, ref, watch } from "vue";
import type { AsyncState } from "../../components/asyncState";
import { isOffline, safeApiError, type AuthenticatedFailureHandler, type SafeError } from "../../components/apiErrors";

type DeploymentRoot = DeploymentRootList["items"][number];
interface Confirmation extends InboxDirectoryPreflight {
  rootLabel: string;
  access: DeploymentRoot["access"];
  relativePath: string;
  overlapsExisting: boolean;
}

/**
 * 验证客户端侧能力根相对路径的形态。
 *
 * @param value - 用户输入的路径；验证时忽略两端空白。
 * @returns 空路径、绝对路径、反斜杠或父级遍历路径对应的安全用户验证消息；其他情况返回 `null`。
 * @remarks 路径包含关系、符号链接、可读性和重叠检查仍以 Core 为准。
 */
export function validateRelativePath(value: string): string | null {
  const path = value.trim();
  if (!path) return "请输入能力根内的相对路径";
  if (path.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(path)) return "只能输入能力根内的相对路径";
  if (path.includes("\\") || path.split("/").some((segment) => segment === "..")) return "相对路径不能包含越级或反斜杠";
  return null;
}

/**
 * 管理部署根发现、收件目录分页、预检确认和创建操作。
 *
 * @param client - 用于读写的已认证 MediaFlow 传输层。
 * @param onFailure - 请求失败后等待的可选会话/错误钩子。
 * @returns 响应式列表/表单状态及 `load`、`preflight` 和 `create` 操作。根或去除两端空白后的路径发生变化时，
 * 预检结果会失效。
 * @throws {Error} 离线、没有当前成功的预检，或发生原始 API 错误时，`create` 会拒绝。
 * @remarks 创建成功后会将返回的目录置于本地列表开头；失败会更新可聚焦的表单错误状态。
 */
export function useInboxDirectories(client: MediaFlowClient, onFailure?: AuthenticatedFailureHandler) {
  const state = ref<AsyncState>({ kind: "idle" });
  const roots = ref<DeploymentRoot[]>([]);
  const directories = ref<InboxDirectory[]>([]);
  const nextCursor = ref<string | null>(null);
  const form = reactive({ rootId: "", relativePath: "" });
  const verified = ref<InboxDirectoryPreflight | null>(null);
  const verifiedKey = ref("");
  const formError = ref<SafeError | null>(null);
  const formErrorOccurrence = ref(0);
  const pendingCreate = ref(false);

  const currentKey = computed(() => `${form.rootId}\u0000${form.relativePath.trim()}`);
  watch(currentKey, (key) => { if (key !== verifiedKey.value) verified.value = null; });
  const confirmation = computed<Confirmation | null>(() => {
    const result = verified.value;
    const root = roots.value.find((item) => item.id === result?.root_id);
    return result && root && verifiedKey.value === currentKey.value ? {
      ...result,
      rootLabel: root.label,
      access: root.access,
      relativePath: result.relative_path,
      overlapsExisting: result.overlaps_existing,
    } : null;
  });
  const canCreate = computed(() => Boolean(state.value.kind !== "offline" && confirmation.value?.readable && !confirmation.value.overlapsExisting && !pendingCreate.value));
  function setFormError(error: SafeError): void { formError.value = error; formErrorOccurrence.value += 1; }

  async function load(cursor?: string): Promise<void> {
    const stale = directories.value.length > 0;
    state.value = { kind: "loading", stale };
    try {
      const [rootPage, directoryPage] = await Promise.all([client.listDeploymentRoots(), client.listInboxDirectories(cursor)]);
      roots.value = rootPage.items;
      directories.value = directoryPage.items;
      nextCursor.value = directoryPage.next_cursor;
      state.value = { kind: directories.value.length ? "content" : "empty" };
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
    }
  }

  async function preflight(): Promise<void> {
    formError.value = null;
    const validation = validateRelativePath(form.relativePath);
    if (!form.rootId) { setFormError({ message: "请选择能力根", field: "root-id" }); return; }
    if (validation) { setFormError({ message: validation, field: "relative-path" }); return; }
    const request = { root_id: form.rootId, relative_path: form.relativePath.trim() };
    try {
      const result = await client.preflightInboxDirectory(request);
      verified.value = result;
      verifiedKey.value = `${request.root_id}\u0000${request.relative_path}`;
      if (!result.readable) setFormError({ message: "该目录当前不可读，请检查权限", field: "relative-path" });
      else if (result.overlaps_existing) setFormError({ message: "该路径与现有收件目录重叠", field: "relative-path" });
    } catch (error) {
      await onFailure?.(error);
      verified.value = null;
      setFormError(safeApiError(error, "无法完成目录预检，请稍后重试"));
    }
  }

  async function create(): Promise<InboxDirectory> {
    if (state.value.kind === "offline") throw new Error("当前离线，无法创建收件目录");
    if (!canCreate.value || !confirmation.value) throw new Error("需要重新预检后才能创建");
    pendingCreate.value = true;
    formError.value = null;
    try {
      const created = await client.createInboxDirectory({ root_id: form.rootId, relative_path: form.relativePath.trim() });
      directories.value = [created, ...directories.value.filter((item) => item.id !== created.id)];
      state.value = { kind: "content" };
      return created;
    } catch (error) {
      await onFailure?.(error);
      setFormError(safeApiError(error, "创建收件目录失败，请重试"));
      throw error;
    } finally {
      pendingCreate.value = false;
    }
  }

  return { state, roots, directories, nextCursor, form, formError, formErrorOccurrence, pendingCreate, confirmation, canCreate, load, preflight, create };
}
