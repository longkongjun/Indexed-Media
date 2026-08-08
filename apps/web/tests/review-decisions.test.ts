import {
  MediaFlowApiError,
  type IdentificationDetail,
  type MediaFlowClient,
  type ReviewCase,
  type ReviewDecisionRequest,
  type TaskDecisionReceipt,
} from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { nextTick, ref } from "vue";
import { describe, expect, it, vi } from "vitest";
import ReviewDecisionForm from "../src/components/ReviewDecisionForm.vue";
import { useReviewCase } from "../src/features/review-cases/useReviewCase";
import { useReviewDecision } from "../src/features/review-cases/useReviewDecision";

const caseId = "019f0000-0000-7000-8000-000000000048";
const taskId = "019f0000-0000-7000-8000-000000000043";
const reviewCase: ReviewCase = {
  id: caseId,
  task_id: taskId,
  file_revision_id: "019f0000-0000-7000-8000-000000000044",
  inbox_directory_id: "019f0000-0000-7000-8000-000000000002",
  relative_path: "incoming/Dune.2021.mkv",
  level: "ambiguous",
  reason: "identification.multiple-strong-candidates",
  title_hint: "Dune",
  version: 3,
  allowed_actions: ["select-provider-candidate", "rematch-with-hints", "select-generic-video"],
  latest_task_decision: null,
  updated_at: "2026-07-23T08:00:00Z",
};
const candidate = {
  provider: "tmdb" as const,
  media_type: "movie" as const,
  provider_id: "438631",
  title: "Dune",
  original_title: "Dune",
  year: 2021,
  locale: "zh-CN",
};
const identification = {
  review_case_id: caseId,
  task: { id: taskId, relative_path: reviewCase.relative_path },
  revision: { id: reviewCase.file_revision_id },
  decision: null,
  evidence: [{ id: "evidence-1", source: "filename", kind: "title", value: "Dune", strength: "strong", reason: "filename.title" }],
  candidates: [],
  evidence_truncated: false,
  candidates_truncated: false,
} as unknown as IdentificationDetail;
const receipt: TaskDecisionReceipt = {
  id: "019f0000-0000-7000-8000-000000000049",
  task_id: taskId,
  review_case_id: caseId,
  case_version: 3,
  kind: "select-provider-candidate",
  state: "accepted",
  created_at: "2026-07-23T09:00:00Z",
};

function api(overrides: Partial<MediaFlowClient> = {}): MediaFlowClient {
  return {
    getReviewCase: vi.fn(async () => reviewCase),
    getProcessingTaskIdentification: vi.fn(async () => identification),
    searchReviewCandidates: vi.fn(async () => ({ items: [candidate] })),
    submitReviewDecision: vi.fn(async () => receipt),
    ...overrides,
  } as MediaFlowClient;
}

const selection: ReviewDecisionRequest = {
  kind: "select-provider-candidate",
  provider: "tmdb",
  media_type: "movie",
  provider_id: candidate.provider_id,
  save_feedback: false,
};

describe("ReviewCase decisions", () => {
  it("keeps evidence when ephemeral candidate search fails", async () => {
    const client = api({ searchReviewCandidates: vi.fn(async () => { throw new TypeError("offline"); }) });
    const feature = useReviewCase(client, caseId);
    await feature.load();
    await feature.searchCandidates({ query: "Dune", mediaType: "movie", locale: "zh-CN" });
    expect(feature.reviewCase.value?.id).toBe(caseId);
    expect(feature.identification.value?.evidence[0]?.value).toBe("Dune");
    expect(feature.candidateState.value.kind).toBe("error");
    expect(feature.candidates.value).toEqual([]);
  });

  it("reuses one idempotency key after response loss and reports accepted without claiming organization", async () => {
    const submit = vi.fn().mockRejectedValueOnce(new TypeError("lost response")).mockResolvedValueOnce(receipt);
    const activeCase = ref(reviewCase);
    const decision = useReviewDecision(api({ submitReviewDecision: submit }), activeCase, ref(true), { refreshCase: vi.fn() });
    await decision.submit(selection);
    expect(submit).toHaveBeenCalledTimes(2);
    expect(submit.mock.calls[0]?.[2]).toBe(submit.mock.calls[1]?.[2]);
    expect(decision.state.value.kind).toBe("accepted");
    expect(decision.message.value).toContain("已接受");
    expect(decision.message.value).not.toContain("已整理");
  });

  it("retains the exact intent key while the result is unknown", async () => {
    const submit = vi.fn()
      .mockRejectedValueOnce(new TypeError("lost one"))
      .mockRejectedValueOnce(new TypeError("lost two"))
      .mockResolvedValueOnce(receipt);
    const decision = useReviewDecision(api({ submitReviewDecision: submit }), ref(reviewCase), ref(true), { refreshCase: vi.fn() });
    expect(await decision.submit(selection)).toBe(false);
    expect(decision.state.value.kind).toBe("unknown");
    expect(await decision.submit(selection)).toBe(true);
    expect(new Set(submit.mock.calls.map((call) => call[2])).size).toBe(1);
  });

  it("refreshes a 409 case while an external draft remains untouched", async () => {
    const conflict = new MediaFlowApiError(409, { error: { code: "request.conflict", message: "conflict", request_id: caseId, details: {} } });
    const refreshCase = vi.fn(async () => undefined);
    const draft = ref("Dune Part Two");
    const decision = useReviewDecision(
      api({ submitReviewDecision: vi.fn(async () => { throw conflict; }) }),
      ref(reviewCase), ref(true), { refreshCase },
    );
    await decision.submit({ ...selection, provider_id: "693134" });
    expect(refreshCase).toHaveBeenCalledOnce();
    expect(decision.state.value.kind).toBe("conflict");
    expect(draft.value).toBe("Dune Part Two");
  });

  it("disables every write while offline without discarding the loaded case", async () => {
    const submit = vi.fn(async () => receipt);
    const activeCase = ref(reviewCase);
    const decision = useReviewDecision(api({ submitReviewDecision: submit }), activeCase, ref(false), { refreshCase: vi.fn() });
    expect(decision.canSubmit.value).toBe(false);
    expect(await decision.submit(selection)).toBe(false);
    expect(submit).not.toHaveBeenCalled();
    expect(activeCase.value.id).toBe(caseId);
  });

  it("defaults feedback off and focuses the first invalid rematch field", async () => {
    const wrapper = mount(ReviewDecisionForm, {
      attachTo: document.body,
      props: { allowedActions: reviewCase.allowed_actions, candidates: [candidate], disabled: false, initialTitle: "" },
    });
    await wrapper.get("#review-decision-kind").setValue("rematch-with-hints");
    expect((wrapper.get("#review-save-feedback").element as HTMLInputElement).checked).toBe(false);
    await wrapper.get("form").trigger("submit");
    await nextTick();
    expect(document.activeElement).toBe(wrapper.get("#review-title").element);
    expect(wrapper.emitted("submit")).toBeUndefined();
    wrapper.unmount();
  });

  it("emits only provider identity for a selected candidate", async () => {
    const wrapper = mount(ReviewDecisionForm, {
      props: { allowedActions: ["select-provider-candidate"], candidates: [candidate], disabled: false, initialTitle: "Dune" },
    });
    await wrapper.get(`input[value="${candidate.provider_id}"]`).setValue(true);
    await wrapper.get("form").trigger("submit");
    await flushPromises();
    expect(wrapper.emitted("submit")?.[0]?.[0]).toEqual(selection);
  });

  it("labels generic video as an intent only and emits bounded local hints", async () => {
    const wrapper = mount(ReviewDecisionForm, {
      props: { allowedActions: ["select-generic-video"], candidates: [], disabled: true, initialTitle: "Camera Clip" },
    });
    expect(wrapper.text()).toContain("尚未执行规划或文件变更");
    expect(wrapper.get("button[type=submit]").attributes()).toHaveProperty("disabled");
    await wrapper.setProps({ disabled: false });
    await wrapper.get("#generic-group-hint").setValue("家庭视频");
    await wrapper.get("form").trigger("submit");
    expect(wrapper.emitted("submit")?.[0]?.[0]).toEqual({
      kind: "select-generic-video",
      display_title: "Camera Clip",
      group_hint: "家庭视频",
      save_grouping_feedback: false,
    });
  });
});
