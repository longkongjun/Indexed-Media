import type {
  IdentificationEnhancer,
  IdentificationEnhancerConnectionTestResult,
  IdentificationEnhancerInput,
  MediaFlowClient,
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

/** 管理默认关闭的本地识别增强器，并始终明确其确定性回退边界。 */
export function useIdentificationEnhancer(
  client: MediaFlowClient,
  onFailure?: AuthenticatedFailureHandler,
) {
  const state = ref<TaskCenterState>({ kind: "loading", stale: false });
  const current = ref<IdentificationEnhancer | null>(null);
  const conflictProjection = ref<IdentificationEnhancer | null>(null);
  const testResult = ref<IdentificationEnhancerConnectionTestResult | null>(null);
  const errorMessage = ref("");
  const formError = ref<SafeError | null>(null);
  const errorKey = ref(0);
  const busy = ref(false);
  const testedCandidateKey = ref("");
  const form = reactive<IdentificationEnhancerInput>({
    enabled: false,
    base_url: "http://127.0.0.1:11434",
    model: "qwen3:4b",
    timeout_ms: 3_000,
  });
  const canWrite = computed(() => state.value.kind !== "offline" && !busy.value);
  const candidateKey = computed(() => JSON.stringify(form));
  const canSave = computed(() => canWrite.value && (
    !form.enabled
      || Boolean(testResult.value?.reachable && testResult.value.model_available
        && testedCandidateKey.value === candidateKey.value)
  ));
  const fallbackMessage = computed(() => {
    const fallback = testResult.value?.fallback_code ?? current.value?.fallback_code;
    return fallback
      ? `确定性识别仍在运行 · ${fallback}`
      : "模型只提供辅助提示；任何故障都会回退确定性识别";
  });

  function hydrate(value: IdentificationEnhancer): void {
    form.enabled = value.enabled;
    form.base_url = value.endpoint_summary;
    form.model = value.model;
    form.timeout_ms = value.timeout_ms;
  }

  async function load(): Promise<void> {
    const stale = current.value !== null;
    state.value = { kind: "loading", stale };
    try {
      current.value = await client.getIdentificationEnhancer();
      hydrate(current.value);
      state.value = { kind: "content" };
      errorMessage.value = "";
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载本地增强器，请重试").message;
    }
  }

  async function testCandidate(): Promise<IdentificationEnhancerConnectionTestResult | null> {
    if (!canWrite.value) throw new Error("当前离线，不能测试本地模型");
    busy.value = true;
    formError.value = null;
    testResult.value = null;
    testedCandidateKey.value = "";
    try {
      testResult.value = await client.testIdentificationEnhancer({ ...form });
      testedCandidateKey.value = candidateKey.value;
      return testResult.value;
    } catch (error) {
      await onFailure?.(error);
      formError.value = safeApiError(error, "本地模型测试失败，请检查地址和模型名");
      errorKey.value += 1;
      return null;
    } finally {
      busy.value = false;
    }
  }

  async function save(): Promise<IdentificationEnhancer | null> {
    if (!current.value || !canSave.value) throw new Error("启用本地模型前必须通过当前草稿测试");
    busy.value = true;
    formError.value = null;
    conflictProjection.value = null;
    try {
      current.value = await client.putIdentificationEnhancer(
        { ...form },
        current.value.config_version,
      );
      hydrate(current.value);
      return current.value;
    } catch (error) {
      await onFailure?.(error);
      if (isRequestConflict(error)) {
        try {
          conflictProjection.value = await client.getIdentificationEnhancer();
        } catch (refreshError) {
          await onFailure?.(refreshError);
        }
      }
      formError.value = safeApiError(error, "保存失败，请刷新后合并服务器版本");
      errorKey.value += 1;
      return null;
    } finally {
      busy.value = false;
    }
  }

  return {
    state, current, conflictProjection, testResult, errorMessage, formError, errorKey,
    busy, form, canWrite, canSave, fallbackMessage, load, testCandidate, save,
  };
}
