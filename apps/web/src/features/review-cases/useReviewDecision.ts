import {
  type MediaFlowClient,
  type ReviewCase,
  type ReviewDecisionRequest,
  type TaskDecisionReceipt,
} from "@mediaflow/api-client-ts";
import { computed, ref, type Ref } from "vue";
import {
  isAmbiguousWriteFailure,
  isRequestConflict,
  safeApiError,
  type AuthenticatedFailureHandler,
} from "../../components/apiErrors";

type DecisionState =
  | { kind: "idle" }
  | { kind: "submitting" }
  | { kind: "unknown" }
  | { kind: "accepted" }
  | { kind: "conflict" }
  | { kind: "error" };

interface ReviewDecisionOptions {
  refreshCase: () => Promise<unknown>;
  onFailure?: AuthenticatedFailureHandler;
}

/**
 * 提交一次不可变的人工决定意图，并以确定性的传输重放确认结果。
 *
 * 首次提交会冻结决定体和幂等键；结果不明确时只接受同一决定体的重放，并拒绝任何修改后的决定，直到成功或确定性失败解除冻结。
 */
export function useReviewDecision(
  client: MediaFlowClient,
  reviewCase: Ref<ReviewCase | null>,
  online: Readonly<Ref<boolean>>,
  options: ReviewDecisionOptions,
) {
  const state = ref<DecisionState>({ kind: "idle" });
  const receipt = ref<TaskDecisionReceipt | null>(null);
  const message = ref("");
  let idempotencyKey: string | null = null;
  let pendingBody: ReviewDecisionRequest | null = null;
  let pendingDigest: string | null = null;
  const canSubmit = computed(() => Boolean(
    online.value && reviewCase.value && state.value.kind !== "submitting" && state.value.kind !== "accepted",
  ));

  function clearIntent(): void {
    idempotencyKey = null;
    pendingBody = null;
    pendingDigest = null;
  }

  function accept(next: TaskDecisionReceipt): true {
    receipt.value = next;
    state.value = { kind: "accepted" };
    message.value = "人工决定已接受，正在等待后台应用；尚未执行规划或文件变更。";
    clearIntent();
    return true;
  }

  async function handleDefinitive(error: unknown): Promise<false> {
    await options.onFailure?.(error);
    if (isRequestConflict(error)) {
      clearIntent();
      await options.refreshCase();
      state.value = { kind: "conflict" };
      message.value = "审核内容已变化，已刷新最新版本；你的草稿仍保留，请核对后重新提交。";
      return false;
    }
    clearIntent();
    state.value = { kind: "error" };
    message.value = safeApiError(error, "人工决定提交失败，请检查后重试").message;
    return false;
  }

  async function submit(body: ReviewDecisionRequest): Promise<boolean> {
    const current = reviewCase.value;
    if (!online.value || !current || state.value.kind === "submitting" || state.value.kind === "accepted") return false;
    const digest = JSON.stringify(body);
    if (state.value.kind === "unknown" && pendingDigest !== digest) {
      message.value = "上一次提交结果尚未确认；请先重试原决定，再修改草稿。";
      return false;
    }
    if (!idempotencyKey) {
      idempotencyKey = crypto.randomUUID();
      pendingBody = structuredClone(body);
      pendingDigest = digest;
    }
    state.value = { kind: "submitting" };
    message.value = "";
    const request = () => client.submitReviewDecision(current.id, current.version, idempotencyKey!, pendingBody!);
    try {
      return accept(await request());
    } catch (error) {
      if (!isAmbiguousWriteFailure(error)) return handleDefinitive(error);
      await options.onFailure?.(error);
      try {
        return accept(await request());
      } catch (replayError) {
        if (!isAmbiguousWriteFailure(replayError)) return handleDefinitive(replayError);
        await options.onFailure?.(replayError);
        state.value = { kind: "unknown" };
        message.value = "提交结果尚未确认；恢复连接后可用同一请求安全重试。";
        return false;
      }
    }
  }

  return { state, receipt, message, canSubmit, submit };
}
