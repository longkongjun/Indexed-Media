import type {
  DownloaderConnection,
  DownloaderConnectionInput,
  DownloaderConnectionTestResult,
  MediaFlowClient,
} from "@mediaflow/api-client-ts";
import { computed, reactive, ref } from "vue";
import {
  isOffline,
  safeApiError,
  type AuthenticatedFailureHandler,
  type SafeError,
} from "../../components/apiErrors";
import type { TaskCenterState } from "../processing-tasks/model";

/** 下载器表单中唯一允许跨请求保留的非秘密字段，以及短生命周期凭据字段。 */
export interface DownloaderConnectionForm {
  kind: DownloaderConnectionInput["kind"];
  displayName: string;
  baseUrl: string;
  username: string;
  password: string;
  enabled: boolean;
}

/**
 * 管理下载器连接列表、候选测试和版本化写入。
 *
 * @param client - 已认证的 MediaFlow 客户端。
 * @param onFailure - 会话过期等共享失败处理器。
 * @returns 脱敏列表、详情、表单、异步状态和连接操作。
 * @remarks 用户名与密码会在每次测试或保存请求结束后立即清空；失败只保留类型、显示名、地址和启用状态。
 * 离线时保留上次成功投影，并禁止所有写入。
 */
export function useDownloaderConnections(
  client: MediaFlowClient,
  onFailure?: AuthenticatedFailureHandler,
) {
  const state = ref<TaskCenterState>({ kind: "loading", stale: false });
  const items = ref<DownloaderConnection[]>([]);
  const selected = ref<DownloaderConnection | null>(null);
  const nextCursor = ref<string | null>(null);
  const errorMessage = ref("");
  const formError = ref<SafeError | null>(null);
  const candidateResult = ref<DownloaderConnectionTestResult | null>(null);
  const busy = ref(false);
  const form = reactive<DownloaderConnectionForm>({
    kind: "qbittorrent",
    displayName: "",
    baseUrl: "",
    username: "",
    password: "",
    enabled: true,
  });
  const canWrite = computed(() => state.value.kind !== "offline" && !busy.value);

  function input(): DownloaderConnectionInput {
    return {
      kind: form.kind,
      display_name: form.displayName.trim(),
      base_url: form.baseUrl.trim(),
      username: form.username,
      password: form.password,
      enabled: form.enabled,
    };
  }

  function clearCredentials(): void {
    form.username = "";
    form.password = "";
  }

  function hydrate(connection: DownloaderConnection): void {
    form.kind = connection.kind;
    form.displayName = connection.display_name;
    form.baseUrl = connection.base_url;
    form.enabled = connection.enabled;
    clearCredentials();
  }

  async function load(cursor?: string): Promise<void> {
    const stale = items.value.length > 0;
    state.value = { kind: "loading", stale };
    try {
      const page = await client.listDownloaderConnections(cursor);
      items.value = page.items;
      nextCursor.value = page.next_cursor;
      state.value = { kind: page.items.length > 0 ? "content" : "empty" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载下载器连接，请重试").message;
    }
  }

  async function loadDetail(id: string): Promise<void> {
    const stale = selected.value !== null;
    state.value = { kind: "loading", stale };
    try {
      selected.value = await client.getDownloaderConnection(id);
      hydrate(selected.value);
      state.value = { kind: "content" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载下载器连接详情，请重试").message;
    }
  }

  async function testCandidate(): Promise<DownloaderConnectionTestResult | null> {
    if (!canWrite.value) throw new Error("当前离线，不能测试连接");
    busy.value = true;
    formError.value = null;
    candidateResult.value = null;
    try {
      candidateResult.value = await client.testDownloaderConnection(input());
      return candidateResult.value;
    } catch (error) {
      await onFailure?.(error);
      formError.value = safeApiError(error, "连接测试失败，请检查地址和凭据");
      return null;
    } finally {
      clearCredentials();
      busy.value = false;
    }
  }

  async function save(): Promise<DownloaderConnection | null> {
    if (!canWrite.value) throw new Error("当前离线，不能保存连接");
    busy.value = true;
    formError.value = null;
    try {
      const saved = await client.createDownloaderConnection(input());
      items.value = [saved, ...items.value.filter((item) => item.id !== saved.id)];
      state.value = { kind: "content" };
      return saved;
    } catch (error) {
      await onFailure?.(error);
      formError.value = safeApiError(error, "保存连接失败；若配置冲突，请刷新后重试");
      return null;
    } finally {
      clearCredentials();
      busy.value = false;
    }
  }

  async function update(): Promise<DownloaderConnection | null> {
    if (!selected.value || !canWrite.value) throw new Error("当前不能更新连接");
    busy.value = true;
    formError.value = null;
    try {
      const saved = await client.updateDownloaderConnection(
        selected.value.id,
        input(),
        selected.value.config_version,
      );
      selected.value = saved;
      hydrate(saved);
      return saved;
    } catch (error) {
      await onFailure?.(error);
      formError.value = safeApiError(error, "更新连接失败；请刷新版本后重试");
      return null;
    } finally {
      clearCredentials();
      busy.value = false;
    }
  }

  async function remove(): Promise<boolean> {
    if (!selected.value || !canWrite.value) throw new Error("当前不能删除连接");
    busy.value = true;
    formError.value = null;
    try {
      await client.deleteDownloaderConnection(selected.value.id, selected.value.config_version);
      items.value = items.value.filter((item) => item.id !== selected.value?.id);
      selected.value = null;
      return true;
    } catch (error) {
      await onFailure?.(error);
      formError.value = safeApiError(error, "删除连接失败；活动下载任务必须先结束");
      return false;
    } finally {
      busy.value = false;
    }
  }

  return {
    state, items, selected, nextCursor, errorMessage, formError, candidateResult, busy,
    form, canWrite, load, loadDetail, testCandidate, save, update, remove,
  };
}
