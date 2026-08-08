import { describe, expect, it } from "vitest";
import type { components } from "../src/generated/mediaflow.js";
import { createSseState } from "../src/sse.js";

type M3ContractSurface = [
  components["schemas"]["TmdbIntegration"],
  components["schemas"]["DiscoveryPolicy"],
  components["schemas"]["ProcessingTask"],
  components["schemas"]["IdentificationDetail"],
  components["schemas"]["ReviewCase"],
];

describe("M3 identification contract", () => {
  it("exports the five generated read-model schemas", () => {
    const contractOnly: M3ContractSurface | undefined = undefined;
    expect(contractOnly).toBeUndefined();
  });

  it("ignores unknown events without losing the replay cursor", () => {
    const state = createSseState({ lastEventId: 41 });

    expect(state.accept({ id: 42, type: "future.event", data: {} })).toBeUndefined();
    expect(state.lastEventId).toBe(42);
  });

  it("accepts a known M3 event and rejects a replayed id", () => {
    const state = createSseState();
    const data = {
      id: 43,
      type: "processing-task.state-changed",
      schema_version: "1",
      occurred_at: "2026-07-23T08:00:00Z",
      task_id: "019f0000-0000-7000-8000-000000000043",
      payload: {
        status: "waiting-confirmation",
        stage: "identification",
        recovering: false,
        reason: "identification.ambiguous",
      },
    } as const;

    expect(state.accept({ id: data.id, type: data.type, data })).toEqual(data);
    expect(state.accept({ id: data.id, type: data.type, data })).toBeUndefined();
    expect(state.lastEventId).toBe(data.id);
  });

  it("rejects malformed known events without advancing the replay cursor", () => {
    const state = createSseState({ lastEventId: 50 });
    const data = {
      id: 51,
      type: "processing-task.identification-decided",
      schema_version: "1",
      occurred_at: "2026-07-23T08:00:01Z",
      task_id: "019f0000-0000-7000-8000-000000000043",
      payload: {
        decision_id: "019f0000-0000-7000-8000-000000000047",
        level: "ambiguous",
        reason: "raw.upstream.message",
      },
    };

    expect(() => state.accept({ id: data.id, type: data.type, data })).toThrow(TypeError);
    expect(state.lastEventId).toBe(50);
  });
});
