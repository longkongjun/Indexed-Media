import type {
  IdentificationDetail,
  MediaFlowClient,
  ReviewCandidatePage,
  ReviewCandidateSearchOptions,
  ReviewCase,
} from "@mediaflow/api-client-ts";
import { ref } from "vue";
import { isOffline, safeApiError, type AuthenticatedFailureHandler } from "../../components/apiErrors";

type ReadState =
  | { kind: "loading"; stale: boolean }
  | { kind: "content" }
  | { kind: "empty" }
  | { kind: "offline"; stale: boolean }
  | { kind: "error"; stale: boolean };

type CandidateState = { kind: "idle" | "loading" | "content" | "empty" | "error" };

/**
 * 分别加载持久化的审核案例/证据投影和临时候选搜索结果。
 *
 * 候选搜索失败不会清空已加载证据或先前候选，避免短暂的远程查询异常掩盖人工审核依据。
 */
export function useReviewCase(
  client: MediaFlowClient,
  id: string,
  onFailure?: AuthenticatedFailureHandler,
) {
  const state = ref<ReadState>({ kind: "loading", stale: false });
  const evidenceState = ref<ReadState>({ kind: "loading", stale: false });
  const candidateState = ref<CandidateState>({ kind: "idle" });
  const reviewCase = ref<ReviewCase | null>(null);
  const identification = ref<IdentificationDetail | null>(null);
  const candidates = ref<ReviewCandidatePage["items"]>([]);
  const errorMessage = ref("");
  const evidenceError = ref("");
  const candidateError = ref("");

  async function loadCase(): Promise<ReviewCase | null> {
    const stale = reviewCase.value !== null;
    state.value = { kind: "loading", stale };
    try {
      reviewCase.value = await client.getReviewCase(id);
      state.value = { kind: "content" };
      errorMessage.value = "";
      return reviewCase.value;
    } catch (error) {
      await onFailure?.(error);
      state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      errorMessage.value = safeApiError(error, "无法加载人工审核，请重试").message;
      return null;
    }
  }

  async function loadIdentification(taskId: string): Promise<void> {
    const stale = identification.value !== null;
    evidenceState.value = { kind: "loading", stale };
    try {
      identification.value = await client.getProcessingTaskIdentification(taskId);
      evidenceState.value = { kind: "content" };
      evidenceError.value = "";
    } catch (error) {
      await onFailure?.(error);
      evidenceState.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale };
      evidenceError.value = safeApiError(error, "无法加载识别证据，请重试").message;
    }
  }

  async function load(): Promise<void> {
    const current = await loadCase();
    if (current) await loadIdentification(current.task_id);
  }

  async function searchCandidates(options: ReviewCandidateSearchOptions): Promise<void> {
    candidateState.value = { kind: "loading" };
    try {
      const page = await client.searchReviewCandidates(id, options);
      candidates.value = page.items;
      candidateState.value = { kind: page.items.length > 0 ? "content" : "empty" };
      candidateError.value = "";
    } catch (error) {
      await onFailure?.(error);
      candidateState.value = { kind: "error" };
      candidateError.value = safeApiError(error, "候选搜索暂时失败；已加载证据仍然保留").message;
    }
  }

  return {
    state,
    evidenceState,
    candidateState,
    reviewCase,
    identification,
    candidates,
    errorMessage,
    evidenceError,
    candidateError,
    loadCase,
    loadIdentification,
    load,
    searchCandidates,
  };
}
