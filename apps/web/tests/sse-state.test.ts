import type { MediaFlowClient, ScanTask } from "@mediaflow/api-client-ts";
import { describe, expect, it, vi } from "vitest";
import { useScanTask } from "../src/features/scan-tasks/useScanTasks";
import { useTaskEvents, type EventSourceLike } from "../src/features/scan-tasks/useTaskEvents";
import { useProjectionEvents } from "../src/features/events/useProjectionEvents";

const taskId = "018f0f10-8bc1-7a5e-8e5a-2dc913d23c87";
const otherId = "018f0f10-8bc1-7a5e-8e5a-2dc913d23c88";
const snapshot = (): ScanTask => ({ id: taskId, inbox_directory_id: otherId, status: "running", recovering: false,
  counts: { visited_directories: 1, observed_files: 2, skipped_entries: 0, errors: 0 } });
const envelope = (id: number, override: object = {}) => JSON.stringify({ id, type: "task.progress", schema_version: "1", occurred_at: "2026-07-18T08:00:00Z", task_id: taskId,
  payload: { visited_directories: 4, observed_files: 9, skipped_entries: 2, errors: 1 }, ...override });

class FakeEventSource implements EventSourceLike {
  listeners = new Map<string, Set<(event: Event | MessageEvent<string>) => void>>();
  closed = 0;
  addEventListener(type: string, listener: (event: Event | MessageEvent<string>) => void) {
    const group = this.listeners.get(type) ?? new Set(); group.add(listener); this.listeners.set(type, group);
  }
  removeEventListener(type: string, listener: (event: Event | MessageEvent<string>) => void) { this.listeners.get(type)?.delete(listener); }
  close() { this.closed += 1; }
  emit(type: string, event: Event | MessageEvent<string>) { for (const listener of this.listeners.get(type) ?? []) listener(event); }
  message(type: string, data: string) { this.emit(type, { data } as MessageEvent<string>); }
  open() { this.emit("open", new Event("open")); }
  fail() { this.emit("error", new Event("error")); }
}

describe("task SSE state", () => {
  it("loads the REST snapshot before constructing same-origin EventSource", async () => {
    const order: string[] = [];
    const api = { getScanTask: vi.fn(async () => { order.push("rest"); return snapshot(); }) } as unknown as MediaFlowClient;
    const detail = useScanTask(api, taskId);
    const source = new FakeEventSource();
    const events = useTaskEvents(detail, { eventSourceFactory: (url) => { expect(url).toBe("/api/v1/events"); order.push("sse"); return source; } });
    await events.start();
    expect(order).toEqual(["rest", "sse"]);
    expect(events.connection.value).toBe("idle");
    expect([...source.listeners.keys()]).toEqual(expect.arrayContaining(["task.progress", "task.state-changed", "stream.gap", "open", "error"]));
    source.open();
    expect(events.connection.value).toBe("connected");
  });

  it("applies absolute progress once, ignores wrong-task/out-of-order events, and keeps bounded dedup", async () => {
    const api = { getScanTask: vi.fn(async () => snapshot()) } as unknown as MediaFlowClient;
    const detail = useScanTask(api, taskId);
    const source = new FakeEventSource();
    const events = useTaskEvents(detail, { eventSourceFactory: () => source });
    await events.start();
    source.message("task.progress", envelope(12));
    source.message("task.progress", envelope(12, { payload: { visited_directories: 99, observed_files: 99, skipped_entries: 99, errors: 99 } }));
    source.message("task.progress", envelope(11));
    source.message("task.progress", envelope(13, { task_id: otherId }));
    expect(detail.task.value?.counts).toEqual({ visited_directories: 4, observed_files: 9, skipped_entries: 2, errors: 1 });
    expect(events.lastAcceptedId.value).toBe(12);
    expect(events.dedupSize.value).toBeLessThanOrEqual(1);
  });

  it("does not subscribe the scan-task projection to M3 processing events", async () => {
    const detail = useScanTask({ getScanTask: vi.fn(async () => snapshot()) } as unknown as MediaFlowClient, taskId);
    const source = new FakeEventSource();
    const events = useTaskEvents(detail, { eventSourceFactory: () => source });
    await events.start();

    source.message("processing-task.state-changed", JSON.stringify({
      id: 13,
      type: "processing-task.state-changed",
      schema_version: "1",
      occurred_at: "2026-07-18T08:00:00Z",
      task_id: taskId,
      payload: { status: "paused", stage: "identification", recovering: false, reason: "integration.unconfigured" },
    }));

    expect(source.listeners.has("processing-task.state-changed")).toBe(false);
    expect(detail.task.value).toEqual(snapshot());
    expect(events.lastAcceptedId.value).toBe(0);
  });

  it("reloads authoritative REST counts when a terminal event follows throttled progress", async () => {
    const completed = {
      ...snapshot(),
      status: "completed" as const,
      counts: { visited_directories: 51, observed_files: 1205, skipped_entries: 0, errors: 0 },
    };
    const getScanTask = vi.fn().mockResolvedValueOnce(snapshot()).mockResolvedValueOnce(completed);
    const detail = useScanTask({ getScanTask } as unknown as MediaFlowClient, taskId);
    const source = new FakeEventSource();
    const events = useTaskEvents(detail, { eventSourceFactory: () => source });
    await events.start();

    source.message("task.progress", envelope(30, {
      payload: { visited_directories: 43, observed_files: 85, skipped_entries: 0, errors: 0 },
    }));
    source.message("task.state-changed", JSON.stringify({
      id: 31,
      type: "task.state-changed",
      schema_version: "1",
      occurred_at: "2026-07-18T08:00:01Z",
      task_id: taskId,
      payload: { status: "completed", recovering: false },
    }));
    await events.refreshing.value;

    expect(getScanTask).toHaveBeenCalledTimes(2);
    expect(detail.task.value).toEqual(completed);
    expect(events.lastAcceptedId.value).toBe(31);
  });

  it("queues replayed named events during one gap refresh and applies them after the REST snapshot without replacing EventSource", async () => {
    let resolve!: (value: ScanTask) => void;
    const refreshed = { ...snapshot(), counts: { visited_directories: 4, observed_files: 9, skipped_entries: 2, errors: 1 } };
    const getScanTask = vi.fn().mockResolvedValueOnce(snapshot()).mockImplementationOnce(() => new Promise<ScanTask>((done) => { resolve = done; })).mockResolvedValueOnce(refreshed);
    const detail = useScanTask({ getScanTask } as unknown as MediaFlowClient, taskId);
    const source = new FakeEventSource();
    const factory = vi.fn(() => source);
    const refreshRelated = vi.fn(async () => undefined);
    const events = useTaskEvents(detail, { eventSourceFactory: factory, refreshRelated });
    await events.start();
    source.message("task.progress", "{/host/private");
    expect(events.diagnostic.value).toBe("事件格式无效，已忽略");
    const gap = JSON.stringify({ id: 20, type: "stream.gap", schema_version: "1", occurred_at: "2026-07-18T08:00:00Z", task_id: null, payload: { minimum_available_id: 20 } });
    source.message("stream.gap", gap);
    source.message("stream.gap", gap);
    expect(getScanTask).toHaveBeenCalledTimes(2);
    expect(refreshRelated).toHaveBeenCalledTimes(1);
    source.message("task.progress", envelope(21));
    expect(detail.task.value?.counts.observed_files).toBe(2);
    expect(events.lastAcceptedId.value).toBe(0);
    resolve(snapshot());
    await events.refreshing.value;
    expect(detail.task.value?.counts).toEqual({ visited_directories: 4, observed_files: 9, skipped_entries: 2, errors: 1 });
    expect(events.lastAcceptedId.value).toBe(21);
    expect(getScanTask).toHaveBeenCalledTimes(3);
    expect(factory).toHaveBeenCalledTimes(1);
    expect(source.closed).toBe(0);
  });

  it("never lets older queued gap events regress a newer terminal REST snapshot or Last-Event-ID", async () => {
    let resolve!: (value: ScanTask) => void;
    const persisted = { ...snapshot(), status: "completed" as const, counts: { visited_directories: 10, observed_files: 100, skipped_entries: 4, errors: 2 } };
    const getScanTask = vi.fn().mockResolvedValueOnce(snapshot()).mockImplementationOnce(() => new Promise<ScanTask>((done) => { resolve = done; })).mockResolvedValueOnce(persisted);
    const detail = useScanTask({ getScanTask } as unknown as MediaFlowClient, taskId);
    const source = new FakeEventSource();
    const events = useTaskEvents(detail, { eventSourceFactory: () => source });
    await events.start();
    source.message("stream.gap", JSON.stringify({ id: 20, type: "stream.gap", schema_version: "1", occurred_at: "2026-07-18T08:00:00Z", task_id: null, payload: { minimum_available_id: 20 } }));
    source.message("task.progress", envelope(21, { payload: { visited_directories: 9, observed_files: 90, skipped_entries: 3, errors: 1 } }));
    source.message("task.state-changed", JSON.stringify({ id: 22, type: "task.state-changed", schema_version: "1", occurred_at: "2026-07-18T08:00:01Z", task_id: taskId, payload: { status: "completed", recovering: false } }));
    resolve(persisted);
    await events.refreshing.value;
    expect(detail.task.value).toEqual(persisted);
    expect(events.lastAcceptedId.value).toBe(22);
    expect(events.dedupSize.value).toBeLessThanOrEqual(1);
    expect(getScanTask).toHaveBeenCalledTimes(3);
  });

  it("uses REST started after a buffered retry event before accepting its ID", async () => {
    let resolveFirst!: (value: ScanTask) => void;
    const terminal = { ...snapshot(), status: "partial-success" as const };
    const retried = { ...snapshot(), status: "queued" as const, counts: { visited_directories: 0, observed_files: 0, skipped_entries: 0, errors: 0 } };
    const getScanTask = vi.fn().mockResolvedValueOnce(terminal).mockImplementationOnce(() => new Promise<ScanTask>((done) => { resolveFirst = done; })).mockResolvedValueOnce(retried);
    const detail = useScanTask({ getScanTask } as unknown as MediaFlowClient, taskId);
    const source = new FakeEventSource(); const events = useTaskEvents(detail, { eventSourceFactory: () => source }); await events.start();
    source.message("stream.gap", JSON.stringify({ id: 20, type: "stream.gap", schema_version: "1", occurred_at: "2026-07-18T08:00:00Z", task_id: null, payload: { minimum_available_id: 20 } }));
    source.message("task.state-changed", JSON.stringify({ id: 21, type: "task.state-changed", schema_version: "1", occurred_at: "2026-07-18T08:00:01Z", task_id: taskId, payload: { status: "queued", recovering: false } }));
    expect(events.lastAcceptedId.value).toBe(0);
    resolveFirst(terminal); await events.refreshing.value;
    expect(getScanTask).toHaveBeenCalledTimes(3);
    expect(detail.task.value).toEqual(retried);
    expect(events.lastAcceptedId.value).toBe(21);
  });

  it("coalesces events arriving during trailing truth reads and repeats until the latest ID is covered", async () => {
    let resolveFirst!: (value: ScanTask) => void; let resolveTrailing!: (value: ScanTask) => void;
    const terminal = { ...snapshot(), status: "partial-success" as const };
    const queued = { ...snapshot(), status: "queued" as const, counts: { visited_directories: 0, observed_files: 0, skipped_entries: 0, errors: 0 } };
    const running = { ...snapshot(), status: "running" as const, counts: { visited_directories: 2, observed_files: 5, skipped_entries: 0, errors: 0 } };
    const getScanTask = vi.fn().mockResolvedValueOnce(terminal)
      .mockImplementationOnce(() => new Promise<ScanTask>((done) => { resolveFirst = done; }))
      .mockImplementationOnce(() => new Promise<ScanTask>((done) => { resolveTrailing = done; }))
      .mockResolvedValueOnce(running);
    const detail = useScanTask({ getScanTask } as unknown as MediaFlowClient, taskId);
    const source = new FakeEventSource(); const events = useTaskEvents(detail, { eventSourceFactory: () => source }); await events.start();
    source.message("stream.gap", JSON.stringify({ id: 20, type: "stream.gap", schema_version: "1", occurred_at: "2026-07-18T08:00:00Z", task_id: null, payload: { minimum_available_id: 20 } }));
    source.message("task.state-changed", JSON.stringify({ id: 21, type: "task.state-changed", schema_version: "1", occurred_at: "2026-07-18T08:00:01Z", task_id: taskId, payload: { status: "queued", recovering: false } }));
    resolveFirst(terminal); await vi.waitFor(() => expect(getScanTask).toHaveBeenCalledTimes(3));
    source.message("task.progress", envelope(22, { payload: { visited_directories: 1, observed_files: 3, skipped_entries: 0, errors: 0 } }));
    resolveTrailing(queued); await events.refreshing.value;
    expect(getScanTask).toHaveBeenCalledTimes(4);
    expect(detail.task.value).toEqual(running);
    expect(events.lastAcceptedId.value).toBe(22);
    expect(events.dedupSize.value).toBeLessThanOrEqual(1);
  });

  it("keeps REST facts during transport errors and delegates 401 cleanup", async () => {
    const source = new FakeEventSource();
    const unauthorized = vi.fn();
    const api = { getScanTask: vi.fn(async () => snapshot()), getSession: vi.fn(async () => { throw Object.assign(new Error("expired"), { status: 401 }); }) } as unknown as MediaFlowClient;
    const detail = useScanTask(api, taskId);
    const events = useTaskEvents(detail, { eventSourceFactory: () => source, onUnauthorized: unauthorized });
    await events.start();
    source.open();
    source.fail();
    await events.probePromise.value;
    expect(events.connection.value).toBe("reconnecting");
    expect(detail.task.value?.counts.observed_files).toBe(2);
    expect(unauthorized).toHaveBeenCalledTimes(1);
  });

  it("restores connected on open and ignores open/error/probe and queued refresh callbacks after stop", async () => {
    let rejectProbe!: (error: unknown) => void;
    let resolveRefresh!: (value: ScanTask) => void;
    const getScanTask = vi.fn().mockResolvedValueOnce(snapshot()).mockImplementationOnce(() => new Promise<ScanTask>((resolve) => { resolveRefresh = resolve; }));
    const getSession = vi.fn(() => new Promise<never>((_resolve, reject) => { rejectProbe = reject; }));
    const detail = useScanTask({ getScanTask, getSession } as unknown as MediaFlowClient, taskId);
    const source = new FakeEventSource(); const unauthorized = vi.fn();
    const events = useTaskEvents(detail, { eventSourceFactory: () => source, onUnauthorized: unauthorized });
    await events.start(); source.fail();
    const gap = JSON.stringify({ id: 20, type: "stream.gap", schema_version: "1", occurred_at: "2026-07-18T08:00:00Z", task_id: null, payload: { minimum_available_id: 20 } });
    source.message("stream.gap", gap); source.message("task.progress", envelope(21));
    events.stop(); source.open(); source.fail(); rejectProbe(Object.assign(new Error("expired"), { status: 401 })); resolveRefresh(snapshot());
    await Promise.allSettled([events.probePromise.value, events.refreshing.value]);
    expect(events.connection.value).toBe("closed");
    expect(detail.task.value?.counts.observed_files).toBe(2);
    expect(unauthorized).not.toHaveBeenCalled();
    expect(source.closed).toBe(1);
  });
});

describe("projection-aware SSE refresh", () => {
  it("refreshes only matching media projections and deduplicates event/version regressions", async () => {
    const refresh = vi.fn(async () => undefined);
    const source = new FakeEventSource();
    const events = useProjectionEvents({
      refresh,
      eventTypes: ["catalog.media-changed"],
      matches: (event) => event.type === "catalog.media-changed" && event.payload.media_item_id === taskId,
      versionOf: (event) => event.type === "catalog.media-changed" ? { key: `media:${event.payload.media_item_id}`, version: event.payload.projection_version } : null,
      eventSourceFactory: () => source,
    });
    await events.start();
    const media = (id: number, version: number) => JSON.stringify({ id, type: "catalog.media-changed", schema_version: "1", occurred_at: "2026-07-23T10:00:00Z", task_id: null, payload: { media_item_id: taskId, projection_version: version, change: "updated" } });
    source.message("catalog.media-changed", media(10, 7));
    await events.refreshing.value;
    source.message("catalog.media-changed", media(11, 7));
    source.message("catalog.media-changed", media(9, 8));
    source.message("integration.health-changed", JSON.stringify({ id: 12, type: "integration.health-changed", schema_version: "1", occurred_at: "2026-07-23T10:00:00Z", task_id: null, payload: { kind: "tmdb", health: "healthy", failure_code: null } }));
    expect(refresh).toHaveBeenCalledTimes(2);
    expect(events.lastAcceptedId.value).toBe(11);
    expect(events.projectionVersions.value[`media:${taskId}`]).toBe(7);
  });

  it("uses a stream gap for one bounded truth refresh without any command surface", async () => {
    const refresh = vi.fn(async () => undefined);
    const source = new FakeEventSource();
    const events = useProjectionEvents({ refresh, eventTypes: ["catalog.media-changed"], matches: () => false, eventSourceFactory: () => source });
    await events.start();
    source.message("stream.gap", JSON.stringify({ id: 20, type: "stream.gap", schema_version: "1", occurred_at: "2026-07-23T10:00:00Z", task_id: null, payload: { minimum_available_id: 20 } }));
    await events.refreshing.value;
    expect(refresh).toHaveBeenCalledTimes(2);
    expect(events.lastAcceptedId.value).toBe(20);
  });
});
