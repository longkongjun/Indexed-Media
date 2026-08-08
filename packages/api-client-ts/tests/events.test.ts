import { describe, expect, it } from "vitest";
import { acceptEventId, parseTaskEvent } from "../src/events.js";

describe("task events", () => {
  it("parses a progress event and rejects duplicate ids", () => {
    const event = parseTaskEvent({
      id: 1,
      type: "task.progress",
      schema_version: "1",
      occurred_at: "2026-07-17T00:00:00Z",
      task_id: "019f0000-0000-7000-8000-000000000001",
      payload: { visited_directories: 1, observed_files: 2, skipped_entries: 0, errors: 0 },
    });

    expect(event.type).toBe("task.progress");
    const seen = new Set<number>();
    expect(acceptEventId(seen, event.id)).toBe(true);
    expect(acceptEventId(seen, event.id)).toBe(false);
  });

  it("rejects duplicate and out-of-order ids with constant memory", () => {
    const seen = new Set<number>();
    for (let id = 1; id <= 10_000; id += 1) {
      expect(acceptEventId(seen, id)).toBe(true);
      expect(seen.size).toBe(1);
    }
    expect(acceptEventId(seen, 10_000)).toBe(false);
    expect(acceptEventId(seen, 9_999)).toBe(false);
    expect(acceptEventId(seen, 10_001)).toBe(true);
    expect([...seen]).toEqual([10_001]);
  });

  it("rejects extra fields, invalid formats, invalid payloads, and non-null stream gap tasks", () => {
    const validGap = { id: 2, type: "stream.gap", schema_version: "1", occurred_at: "2026-07-17T00:00:00Z", task_id: null, payload: { minimum_available_id: 1 } };
    expect(parseTaskEvent(validGap)).toEqual(validGap);
    for (const invalid of [
      { ...validGap, bootstrap_secret: "leak" },
      { ...validGap, occurred_at: "not-a-date" },
      { ...validGap, occurred_at: "2026-02-30T00:00:00Z" },
      { ...validGap, task_id: "019f0000-0000-7000-8000-000000000001" },
      { ...validGap, payload: { minimum_available_id: 0 } },
      { ...validGap, id: Number.MAX_SAFE_INTEGER + 1 },
      { ...validGap, payload: { minimum_available_id: Number.MAX_SAFE_INTEGER + 1 } },
      { ...validGap, payload: { minimum_available_id: 1, absolute_path: "/mnt/private" } },
      { ...validGap, type: "task.progress", task_id: "not-a-uuid", payload: { visited_directories: 1 } },
      { ...validGap, type: "task.progress", task_id: null, payload: { visited_directories: 1, observed_files: 2, skipped_entries: 0, errors: 0 } },
      { ...validGap, type: "task.state-changed", task_id: null, payload: { status: "running", recovering: false } },
    ]) {
      expect(() => parseTaskEvent(invalid)).toThrow(TypeError);
    }
  });

  it("accepts the exact maximum safe public event id", () => {
    const event = {
      id: Number.MAX_SAFE_INTEGER,
      type: "stream.gap",
      schema_version: "1",
      occurred_at: "2026-07-17T00:00:00Z",
      task_id: null,
      payload: { minimum_available_id: Number.MAX_SAFE_INTEGER },
    };
    expect(parseTaskEvent(event)).toEqual(event);
  });

  it("parses a minimal downloader task change without accepting secret fields", () => {
    const event = {
      id: 46,
      type: "download-task.changed",
      schema_version: "1",
      occurred_at: "2026-07-24T05:00:00Z",
      task_id: "019f0000-0000-7000-8000-000000000062",
      payload: {
        projection_version: 3,
        status: "monitoring",
        remote_status: "downloading",
        progress_basis_points: 5000,
        failure_code: null,
      },
    };

    expect(parseTaskEvent(event)).toEqual(event);
    expect(() => parseTaskEvent({
      ...event,
      payload: { ...event.payload, source: "magnet:?xt=urn:btih:secret" },
    })).toThrow(TypeError);
  });

  it("parses minimal organization changes without accepting host paths or NFO", () => {
    const targetChanged = {
      id: 47,
      type: "organization-target.changed",
      schema_version: "1",
      occurred_at: "2026-07-24T06:00:00Z",
      task_id: null,
      payload: {
        organization_target_id: "019f0000-0000-7000-8000-000000000071",
        projection_version: 3,
        change: "updated",
      },
    };
    const resultChanged = {
      id: 48,
      type: "organization-result.changed",
      schema_version: "1",
      occurred_at: "2026-07-24T06:00:01Z",
      task_id: "019f0000-0000-7000-8000-000000000043",
      payload: {
        result_id: "019f0000-0000-7000-8000-000000000072",
        projection_version: 2,
        status: "partial-success",
      },
    };

    expect(parseTaskEvent(targetChanged)).toEqual(targetChanged);
    expect(parseTaskEvent(resultChanged)).toEqual(resultChanged);
    for (const event of [targetChanged, resultChanged]) {
      expect(() => parseTaskEvent({
        ...event,
        payload: { ...event.payload, absolute_path: "/mnt/private", nfo_xml: "<movie/>" },
      })).toThrow(TypeError);
    }
  });

  it("parses an organization processing transition with the closed shared reason", () => {
    const event = {
      id: 49,
      type: "processing-task.state-changed",
      schema_version: "1",
      occurred_at: "2026-07-24T06:00:02Z",
      task_id: "019f0000-0000-7000-8000-000000000043",
      payload: {
        status: "partial-success",
        stage: "nfo",
        recovering: false,
        reason: "organization.nfo-failed",
      },
    };
    expect(parseTaskEvent(event)).toEqual(event);
  });

  it("parses minimal source automation and enhancer changes without sensitive payloads", () => {
    const events = [
      {
        id: 50, type: "automation-source.changed", schema_version: "1", occurred_at: "2026-07-24T07:00:00Z", task_id: null,
        payload: { automation_source_id: "019f0000-0000-7000-8000-000000000081", projection_version: 2, change: "updated" },
      },
      {
        id: 51, type: "automation-event.changed", schema_version: "1", occurred_at: "2026-07-24T07:00:01Z", task_id: null,
        payload: { automation_event_id: "019f0000-0000-7000-8000-000000000082", projection_version: 3, status: "retry-wait", action: "create-download", failure_code: "integration.unavailable" },
      },
      {
        id: 52, type: "identification-enhancer.changed", schema_version: "1", occurred_at: "2026-07-24T07:00:02Z", task_id: null,
        payload: { projection_version: 4, enabled: true, health: "degraded", fallback_code: "provider.timeout" },
      },
    ];
    for (const event of events) {
      expect(parseTaskEvent(event)).toEqual(event);
      for (const leak of [
        { feed_url: "https://secret.invalid/token" }, { signature: "sha256=secret" },
        { absolute_path: "/mnt/private" }, { model_output: "raw" },
      ]) {
        expect(() => parseTaskEvent({ ...event, payload: { ...event.payload, ...leak } })).toThrow(TypeError);
      }
    }
  });
});
