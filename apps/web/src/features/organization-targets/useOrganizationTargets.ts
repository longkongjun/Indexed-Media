import type {
  DeploymentRootList,
  MediaFlowClient,
  OrganizationTarget,
  OrganizationTargetInput,
  OrganizationTargetPreflight,
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

type OrganizationRule = OrganizationTargetInput["rules"][number];

/** 整理目标表单只承载契约允许的能力根内配置。 */
export interface OrganizationTargetForm {
  kind: OrganizationTargetInput["kind"];
  displayName: string;
  rootId: string;
  relativePath: string;
  operation: OrganizationTargetInput["operation"];
  namingPattern: OrganizationTargetInput["naming_pattern"];
  nfoPolicy: OrganizationTargetInput["nfo_policy"];
  automatic: boolean;
  enabled: boolean;
  rules: OrganizationRule[];
}

function initialForm(): OrganizationTargetForm {
  return {
    kind: "movie",
    displayName: "",
    rootId: "",
    relativePath: "",
    operation: "copy",
    namingPattern: "movie",
    nfoPolicy: "preserve-only",
    automatic: false,
    enabled: true,
    rules: [],
  };
}

/**
 * 管理版本化 organization target 列表、无副作用 preflight 和冲突恢复。
 *
 * @remarks 保存前必须有与当前根/相对路径完全匹配的成功 preflight。409 时只刷新最新服务端投影，
 * 不覆盖本地非敏感草稿；所有公开状态只包含 root ID 和相对路径。
 */
export function useOrganizationTargets(
  client: MediaFlowClient,
  onFailure?: AuthenticatedFailureHandler,
) {
  const state = ref<TaskCenterState>({ kind: "loading", stale: false });
  const items = ref<OrganizationTarget[]>([]);
  const roots = ref<DeploymentRootList["items"]>([]);
  const selected = ref<OrganizationTarget | null>(null);
  const conflictLatest = ref<OrganizationTarget | null>(null);
  const nextCursor = ref<string | null>(null);
  const preflightResult = ref<OrganizationTargetPreflight | null>(null);
  const errorMessage = ref("");
  const formError = ref<SafeError | null>(null);
  const errorOccurrence = ref(0);
  const busy = ref(false);
  const form = reactive<OrganizationTargetForm>(initialForm());
  const acceptedPreflight = ref<string | null>(null);

  const writableRoots = computed(() => roots.value.filter((root) => root.access === "read-write"));
  const canWrite = computed(() => state.value.kind !== "offline" && !busy.value);
  const preflightMatches = computed(() => acceptedPreflight.value === candidateSignature());

  function candidateSignature(): string {
    return `${form.rootId}\u0000${form.relativePath}`;
  }

  function setFormError(error: SafeError | null): void {
    formError.value = error;
    if (error) errorOccurrence.value += 1;
  }

  function input(): OrganizationTargetInput {
    return {
      kind: form.kind,
      display_name: form.displayName.trim(),
      root_id: form.rootId,
      relative_path: form.relativePath.trim(),
      operation: form.operation,
      naming_pattern: form.namingPattern,
      nfo_policy: form.nfoPolicy,
      automatic: form.automatic,
      enabled: form.enabled,
      rules: form.rules.map((rule) => ({
        media_kind: rule.media_kind,
        inbox_directory_id: rule.inbox_directory_id?.trim() || null,
        explicit_tag: rule.explicit_tag?.trim() || null,
        enabled: rule.enabled,
      })),
    };
  }

  function hydrate(target: OrganizationTarget): void {
    Object.assign(form, {
      kind: target.kind,
      displayName: target.display_name,
      rootId: target.root_id,
      relativePath: target.relative_path,
      operation: target.operation,
      namingPattern: target.naming_pattern,
      nfoPolicy: target.nfo_policy,
      automatic: target.automatic,
      enabled: target.enabled,
    });
    form.rules.splice(0, form.rules.length, ...target.rules.map((rule) => ({ ...rule })));
    acceptedPreflight.value = null;
    preflightResult.value = null;
  }

  async function load(cursor?: string): Promise<void> {
    const stale = items.value.length > 0;
    state.value = { kind: "loading", stale };
    try {
      const [rootPage, targetPage] = await Promise.all([
        client.listDeploymentRoots(),
        client.listOrganizationTargets(cursor),
      ]);
      roots.value = rootPage.items;
      items.value = targetPage.items;
      nextCursor.value = targetPage.next_cursor;
      state.value = { kind: targetPage.items.length > 0 ? "content" : "empty" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载整理目标，请重试").message;
    }
  }

  async function loadDetail(id: string): Promise<void> {
    const stale = selected.value !== null;
    state.value = { kind: "loading", stale };
    try {
      const [rootPage, target] = await Promise.all([
        client.listDeploymentRoots(),
        client.getOrganizationTarget(id),
      ]);
      roots.value = rootPage.items;
      selected.value = target;
      conflictLatest.value = null;
      hydrate(target);
      state.value = { kind: "content" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载整理目标详情，请重试").message;
    }
  }

  async function preflight(): Promise<OrganizationTargetPreflight | null> {
    if (!canWrite.value) return null;
    acceptedPreflight.value = null;
    preflightResult.value = null;
    setFormError(null);
    if (!form.rootId) {
      setFormError({ message: "请选择可写能力根", field: "root-id" });
      return null;
    }
    if (!form.relativePath.trim()) {
      setFormError({ message: "请输入能力根内的相对目录", field: "relative-path" });
      return null;
    }
    busy.value = true;
    try {
      const result = await client.preflightOrganizationTarget({
        root_id: form.rootId,
        relative_path: form.relativePath.trim(),
      });
      if (result.failure_code || !result.writable || result.overlaps_existing) {
        const rootFailure = result.failure_code === "organization.root-read-only" || !result.writable;
        setFormError({
          message: rootFailure
            ? "所选能力根不可写，请选择 read-write 根"
            : result.failure_code === "organization.target-unavailable"
              ? "目标目录暂不可用，请检查部署根"
              : "目标目录与收件目录或现有整理目标重叠",
          field: rootFailure ? "root-id" : "relative-path",
        });
        return null;
      }
      form.relativePath = result.relative_path;
      preflightResult.value = result;
      acceptedPreflight.value = candidateSignature();
      return result;
    } catch (error) {
      await onFailure?.(error);
      setFormError(safeApiError(error, "目标检查失败，请核对能力根和相对路径"));
      return null;
    } finally {
      busy.value = false;
    }
  }

  function requirePreflight(): boolean {
    if (preflightMatches.value) return true;
    setFormError({ message: "保存前请先检查当前能力根和相对目录", field: "relative-path" });
    return false;
  }

  async function save(): Promise<OrganizationTarget | null> {
    if (!canWrite.value || !requirePreflight()) return null;
    busy.value = true;
    setFormError(null);
    try {
      const saved = await client.createOrganizationTarget(input());
      items.value = [saved, ...items.value.filter((item) => item.id !== saved.id)];
      selected.value = saved;
      hydrate(saved);
      state.value = { kind: "content" };
      return saved;
    } catch (error) {
      await onFailure?.(error);
      setFormError(safeApiError(error, "保存整理目标失败，请检查字段"));
      return null;
    } finally {
      busy.value = false;
    }
  }

  async function update(): Promise<OrganizationTarget | null> {
    if (!selected.value || !canWrite.value || !requirePreflight()) return null;
    const current = selected.value;
    busy.value = true;
    setFormError(null);
    conflictLatest.value = null;
    try {
      const saved = await client.updateOrganizationTarget(current.id, input(), current.config_version);
      selected.value = saved;
      hydrate(saved);
      return saved;
    } catch (error) {
      await onFailure?.(error);
      if (isRequestConflict(error)) {
        try {
          conflictLatest.value = await client.getOrganizationTarget(current.id);
          setFormError({
            message: `配置已由其他页面更新到版本 ${conflictLatest.value.config_version}；草稿已保留，请重新检查后保存`,
            field: "organization-conflict",
          });
        } catch (refreshError) {
          await onFailure?.(refreshError);
          setFormError({ message: "配置版本已变化；草稿已保留，刷新最新投影后重试" });
        }
      } else {
        setFormError(safeApiError(error, "更新整理目标失败，请检查字段"));
      }
      return null;
    } finally {
      busy.value = false;
    }
  }

  async function remove(): Promise<boolean> {
    if (!selected.value || !canWrite.value) return false;
    busy.value = true;
    setFormError(null);
    try {
      await client.deleteOrganizationTarget(selected.value.id, selected.value.config_version);
      items.value = items.value.filter((item) => item.id !== selected.value?.id);
      selected.value = null;
      return true;
    } catch (error) {
      await onFailure?.(error);
      setFormError(safeApiError(error, "删除失败；活动计划必须先结束"));
      return false;
    } finally {
      busy.value = false;
    }
  }

  function addRule(): void {
    form.rules.push({
      media_kind: form.kind,
      inbox_directory_id: null,
      explicit_tag: null,
      enabled: true,
    });
  }

  function removeRule(index: number): void {
    form.rules.splice(index, 1);
  }

  return {
    state, items, roots, writableRoots, selected, conflictLatest, nextCursor,
    preflightResult, errorMessage, formError, errorOccurrence, busy, form,
    canWrite, preflightMatches, load, loadDetail, preflight, save, update, remove,
    addRule, removeRule,
  };
}
