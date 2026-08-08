import type {
  AutomationSource,
  AutomationSourceConnectionTestResult,
  AutomationSourceInput,
  MediaFlowClient,
  WebhookSecretReceipt,
} from "@mediaflow/api-client-ts";
import { computed, reactive, ref } from "vue";
import {
  isOffline,
  isRequestConflict,
  safeApiError,
  type AuthenticatedFailureHandler,
  type SafeError,
} from "../../components/apiErrors";
import type { TaskCenterState } from "../processing-tasks/model";

export interface AutomationSourceForm {
  kind: AutomationSource["kind"];
  displayName: string;
  enabled: boolean;
  feedUrl: string;
  downloaderConnectionId: string;
  inboxDirectoryId: string;
  pollIntervalSeconds: number;
  allowedActions: ("download.create" | "inbox.reconcile")[];
}

/** 管理脱敏自动来源、候选测试、版本冲突和一次性 Webhook secret。 */
export function useAutomationSources(
  client: MediaFlowClient,
  onFailure?: AuthenticatedFailureHandler,
) {
  const state = ref<TaskCenterState>({ kind: "loading", stale: false });
  const items = ref<AutomationSource[]>([]);
  const selected = ref<AutomationSource | null>(null);
  const nextCursor = ref<string | null>(null);
  const errorMessage = ref("");
  const formError = ref<SafeError | null>(null);
  const errorKey = ref(0);
  const candidateResult = ref<AutomationSourceConnectionTestResult | null>(null);
  const conflictProjection = ref<AutomationSource | null>(null);
  const secretReceipt = ref<WebhookSecretReceipt | null>(null);
  const secretCopied = ref(false);
  const sourceErrors = reactive<Record<string, string>>({});
  const busy = ref(false);
  const testedCandidateKey = ref("");
  let keySequence = 0;
  const form = reactive<AutomationSourceForm>({
    kind: "rss",
    displayName: "",
    enabled: true,
    feedUrl: "",
    downloaderConnectionId: "",
    inboxDirectoryId: "",
    pollIntervalSeconds: 900,
    allowedActions: ["download.create"],
  });
  const canWrite = computed(() => state.value.kind !== "offline" && !busy.value);
  const candidateKey = computed(() => JSON.stringify(input()));
  const canSave = computed(() => canWrite.value && (
    form.kind !== "rss"
      || Boolean(candidateResult.value?.reachable && testedCandidateKey.value === candidateKey.value)
  ));
  const canLeaveSecret = computed(() => secretReceipt.value !== null && secretCopied.value);

  function input(): AutomationSourceInput {
    const common = { display_name: form.displayName.trim(), enabled: form.enabled };
    switch (form.kind) {
      case "rss":
        return {
          ...common,
          kind: "rss",
          feed_url: form.feedUrl.trim(),
          downloader_connection_id: form.downloaderConnectionId,
          poll_interval_seconds: form.pollIntervalSeconds,
        };
      case "webhook":
        return { ...common, kind: "webhook", allowed_actions: [...form.allowedActions] };
      case "download-completion":
        return {
          ...common,
          kind: "download-completion",
          downloader_connection_id: form.downloaderConnectionId,
          inbox_directory_id: form.inboxDirectoryId,
        };
    }
  }

  function clearSensitive(): void {
    form.feedUrl = "";
  }

  function hydrate(value: AutomationSource): void {
    form.kind = value.kind;
    form.displayName = value.display_name;
    form.enabled = value.enabled;
    form.downloaderConnectionId = value.downloader_connection_id ?? "";
    form.inboxDirectoryId = value.inbox_directory_id ?? "";
    form.pollIntervalSeconds = value.poll_interval_seconds ?? 900;
    form.allowedActions = [...value.allowed_actions];
    clearSensitive();
  }

  function replaceItem(value: AutomationSource): void {
    items.value = [value, ...items.value.filter((item) => item.id !== value.id)];
    state.value = { kind: "content" };
  }

  async function load(cursor?: string): Promise<void> {
    const stale = items.value.length > 0;
    state.value = { kind: "loading", stale };
    try {
      const page = await client.listAutomationSources(cursor);
      items.value = page.items;
      nextCursor.value = page.next_cursor;
      state.value = { kind: page.items.length > 0 ? "content" : "empty" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载自动来源，请重试").message;
    }
  }

  async function loadDetail(id: string): Promise<void> {
    const stale = selected.value !== null;
    state.value = { kind: "loading", stale };
    try {
      selected.value = await client.getAutomationSource(id);
      hydrate(selected.value);
      state.value = { kind: "content" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载来源详情，请重试").message;
    }
  }

  async function refreshSource(id: string): Promise<void> {
    try {
      const fresh = await client.getAutomationSource(id);
      replaceItem(fresh);
      delete sourceErrors[id];
    } catch (error) {
      await onFailure?.(error);
      sourceErrors[id] = safeApiError(error, "此来源刷新失败，保留上次投影").message;
      if (isOffline(error)) state.value = { kind: "offline", stale: true };
    }
  }

  async function testCandidate(): Promise<AutomationSourceConnectionTestResult | null> {
    if (!canWrite.value) throw new Error("当前离线，不能测试来源");
    busy.value = true;
    formError.value = null;
    candidateResult.value = null;
    testedCandidateKey.value = "";
    try {
      candidateResult.value = await client.testAutomationSource(input());
      testedCandidateKey.value = candidateKey.value;
      return candidateResult.value;
    } catch (error) {
      await onFailure?.(error);
      formError.value = safeApiError(error, "来源测试失败，请检查字段");
      errorKey.value += 1;
      return null;
    } finally {
      busy.value = false;
    }
  }

  async function save(): Promise<AutomationSource | null> {
    if (!canSave.value) throw new Error("当前来源必须先通过测试才能保存");
    busy.value = true;
    formError.value = null;
    dismissSecret(true);
    try {
      const result = await client.createAutomationSource(input());
      const saved = "secret" in result ? result.source : result;
      if ("secret" in result) secretReceipt.value = result;
      replaceItem(saved);
      clearSensitive();
      return saved;
    } catch (error) {
      await onFailure?.(error);
      formError.value = safeApiError(error, "保存来源失败，请检查字段");
      errorKey.value += 1;
      clearSensitive();
      return null;
    } finally {
      busy.value = false;
    }
  }

  async function update(): Promise<AutomationSource | null> {
    if (!selected.value || !canSave.value) throw new Error("当前来源必须先通过测试才能更新");
    busy.value = true;
    formError.value = null;
    conflictProjection.value = null;
    const id = selected.value.id;
    try {
      const saved = await client.updateAutomationSource(
        id,
        input(),
        selected.value.config_version,
      );
      selected.value = saved;
      replaceItem(saved);
      hydrate(saved);
      return saved;
    } catch (error) {
      await onFailure?.(error);
      if (isRequestConflict(error)) {
        clearSensitive();
        try {
          conflictProjection.value = await client.getAutomationSource(id);
        } catch (refreshError) {
          await onFailure?.(refreshError);
        }
      }
      formError.value = safeApiError(error, "更新来源失败，请刷新后合并");
      errorKey.value += 1;
      return null;
    } finally {
      busy.value = false;
    }
  }

  async function rotateSecret(): Promise<WebhookSecretReceipt | null> {
    if (!selected.value || selected.value.kind !== "webhook" || !canWrite.value) {
      throw new Error("当前不能轮换 secret");
    }
    busy.value = true;
    formError.value = null;
    dismissSecret(true);
    keySequence += 1;
    try {
      const receipt = await client.rotateAutomationWebhookSecret(
        selected.value.id,
        selected.value.config_version,
        `webhook-rotate-${Date.now().toString(36)}-${keySequence.toString(36)}`,
      );
      secretReceipt.value = receipt;
      selected.value = receipt.source;
      replaceItem(receipt.source);
      return receipt;
    } catch (error) {
      await onFailure?.(error);
      formError.value = safeApiError(error, "轮换 Webhook secret 失败");
      errorKey.value += 1;
      return null;
    } finally {
      busy.value = false;
    }
  }

  async function remove(): Promise<boolean> {
    if (!selected.value || !canWrite.value) throw new Error("当前不能删除来源");
    busy.value = true;
    formError.value = null;
    try {
      await client.deleteAutomationSource(selected.value.id, selected.value.config_version);
      items.value = items.value.filter((item) => item.id !== selected.value?.id);
      selected.value = null;
      return true;
    } catch (error) {
      await onFailure?.(error);
      formError.value = safeApiError(error, "删除失败；请先禁用并处理未终结事件");
      errorKey.value += 1;
      return false;
    } finally {
      busy.value = false;
    }
  }

  function dismissSecret(force = false): void {
    if (!force && !secretCopied.value) return;
    secretReceipt.value = null;
    secretCopied.value = false;
  }

  return {
    state, items, selected, nextCursor, errorMessage, formError, errorKey,
    candidateResult, conflictProjection, secretReceipt, secretCopied, sourceErrors,
    busy, form, canWrite, canSave, canLeaveSecret, load, loadDetail, refreshSource,
    testCandidate, save, update, rotateSecret, remove, dismissSecret,
  };
}
