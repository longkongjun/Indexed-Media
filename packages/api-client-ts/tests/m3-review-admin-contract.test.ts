import { describe, expect, it, vi } from "vitest";
import { createMediaFlowClient } from "../src/client.js";
import { parseTaskEvent } from "../src/events.js";

describe("M3 review and admin client contract", () => {
  it("exposes the review, task-center, and media methods", () => {
    const client = createMediaFlowClient({ fetch: vi.fn<typeof globalThis.fetch>() });

    expect(client).toEqual(expect.objectContaining({
      searchReviewCandidates: expect.any(Function),
      submitReviewDecision: expect.any(Function),
      listMediaItems: expect.any(Function),
      getMediaItem: expect.any(Function),
    }));
  });

  it("maps bounded filters and mutation guards to the contract paths", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation(
      async () => new Response(JSON.stringify({}), { status: 200, headers: { "content-type": "application/json" } }),
    );
    const client = createMediaFlowClient({ fetch }) as ReturnType<typeof createMediaFlowClient> & Record<string, (...args: never[]) => Promise<unknown>>;
    client.setCsrfToken("csrf-value");
    const taskId = "019f0000-0000-7000-8000-000000000043";
    const caseId = "019f0000-0000-7000-8000-000000000048";
    const mediaId = "019f0000-0000-7000-8000-000000000050";

    expect(client.searchReviewCandidates).toBeTypeOf("function");
    expect(client.submitReviewDecision).toBeTypeOf("function");
    expect(client.listMediaItems).toBeTypeOf("function");
    expect(client.getMediaItem).toBeTypeOf("function");

    await client.listProcessingTasks({ view: "pending", stage: "identification", query: "Dune", cursor: "task-cursor" } as never);
    await client.searchReviewCandidates(caseId as never, { query: "Dune", mediaType: "movie", locale: "zh-CN", limit: 20 } as never);
    await client.submitReviewDecision(caseId as never, 3 as never, "decision-key" as never, {
      kind: "select-provider-candidate",
      provider: "tmdb",
      media_type: "movie",
      provider_id: "438631",
      save_feedback: false,
    } as never);
    await client.listMediaItems({ type: "movie", query: "Dune", cursor: "media-cursor" } as never);
    await client.getMediaItem(mediaId as never);

    expect(fetch.mock.calls.map(([path]) => path)).toEqual([
      "/api/v1/processing-tasks?cursor=task-cursor&view=pending&stage=identification&q=Dune",
      `/api/v1/review-cases/${caseId}/candidates?q=Dune&media_type=movie&locale=zh-CN&limit=20`,
      `/api/v1/review-cases/${caseId}/decisions`,
      "/api/v1/media-items?cursor=media-cursor&type=movie&q=Dune",
      `/api/v1/media-items/${mediaId}`,
    ]);
    const mutation = fetch.mock.calls[2]?.[1] as RequestInit;
    expect(mutation.method).toBe("POST");
    expect(mutation.headers).toEqual(expect.objectContaining({
      "X-CSRF-Token": "csrf-value",
      "If-Match": "3",
      "Idempotency-Key": "decision-key",
      "Content-Type": "application/json",
    }));
    expect(JSON.parse(String(mutation.body))).toEqual(expect.objectContaining({ provider_id: "438631" }));
  });

  it("parses a minimal decision event and rejects leaked candidate fields", () => {
    const event = {
      id: 45,
      type: "task-decision.accepted",
      schema_version: "1",
      occurred_at: "2026-07-23T09:00:00Z",
      task_id: "019f0000-0000-7000-8000-000000000043",
      payload: {
        case_id: "019f0000-0000-7000-8000-000000000048",
        decision_id: "019f0000-0000-7000-8000-000000000049",
        kind: "select-provider-candidate",
        case_version: 2,
      },
    };

    expect(() => parseTaskEvent(event)).not.toThrow();
    expect(() => parseTaskEvent({ ...event, payload: { ...event.payload, title: "must-not-leak" } })).toThrow(TypeError);
  });
});
