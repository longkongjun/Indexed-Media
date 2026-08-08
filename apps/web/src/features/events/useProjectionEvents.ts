import { acceptEventId, parseTaskEvent, type MediaFlowClient, type TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { ref } from "vue";
import { isUnauthorized } from "../../components/apiErrors";
import type { EventSourceFactory, EventSourceLike } from "../scan-tasks/useTaskEvents";

interface ProjectionVersion { key: string; version: number }

interface ProjectionEventOptions {
  refresh: () => Promise<unknown>;
  eventTypes: readonly TaskEventEnvelope["type"][];
  matches: (event: TaskEventEnvelope) => boolean;
  versionOf?: (event: TaskEventEnvelope) => ProjectionVersion | null;
  eventSourceFactory?: EventSourceFactory;
  probeClient?: Pick<MediaFlowClient, "getSession">;
  onUnauthorized?: () => unknown | Promise<unknown>;
}

function defaultFactory(url: string): EventSourceLike { return new EventSource(url) as unknown as EventSourceLike; }

/**
 * 依据严格且单调的 SSE 提示刷新一个有界 REST 投影。
 *
 * 事件只在权威刷新成功后才推进已接受 ID；流缺口、未知版本和同一投影的多次变更会合并为一次刷新，停止后不会让延迟结果写回状态。
 * `start()` 的初次权威刷新失败时会拒绝，且不会创建 EventSource 连接。
 */
export function useProjectionEvents(options: ProjectionEventOptions) {
  const connection = ref<"idle" | "connected" | "reconnecting" | "closed">("idle");
  const diagnostic = ref("");
  const lastAcceptedId = ref(0);
  const projectionVersions = ref<Record<string, number>>({});
  const refreshing = ref<Promise<void> | null>(null);
  const probePromise = ref<Promise<void> | null>(null);
  const seen = new Set<number>();
  const pendingVersions = new Map<string, number>();
  let pendingId = 0;
  let pendingGap = false;
  let pendingUnversioned = false;
  let source: EventSourceLike | null = null;
  let disposed = false;
  const namedTypes = [...new Set([...options.eventTypes, "stream.gap" as const])];

  function acceptId(id: number): boolean {
    if (!acceptEventId(seen, id)) return false;
    lastAcceptedId.value = id;
    return true;
  }

  function queue(event: TaskEventEnvelope): void {
    if (!Number.isSafeInteger(event.id) || event.id <= lastAcceptedId.value || event.id <= pendingId) return;
    pendingId = event.id;
    if (event.type === "stream.gap") {
      pendingGap = true;
      return;
    }
    const projection = options.versionOf?.(event) ?? null;
    if (!projection) {
      pendingUnversioned = true;
      return;
    }
    const accepted = projectionVersions.value[projection.key] ?? 0;
    if (projection.version <= accepted) {
      acceptId(event.id);
      pendingId = 0;
      return;
    }
    pendingVersions.set(projection.key, Math.max(pendingVersions.get(projection.key) ?? 0, projection.version));
  }

  async function refreshTruth(): Promise<void> {
    while (!disposed && pendingId > lastAcceptedId.value) {
      const coveredId = pendingId;
      const coveredVersions = new Map(pendingVersions);
      const hadGap = pendingGap;
      const hadUnversioned = pendingUnversioned;
      pendingGap = false;
      pendingUnversioned = false;
      for (const [key, version] of coveredVersions) {
        if ((pendingVersions.get(key) ?? 0) <= version) pendingVersions.delete(key);
      }
      try {
        await options.refresh();
      } catch {
        diagnostic.value = "事件状态刷新失败，将在后续事件到达时重试";
        pendingGap ||= hadGap;
        pendingUnversioned ||= hadUnversioned;
        for (const [key, version] of coveredVersions) pendingVersions.set(key, Math.max(pendingVersions.get(key) ?? 0, version));
        return;
      }
      if (disposed) return;
      const nextVersions = { ...projectionVersions.value };
      for (const [key, version] of coveredVersions) nextVersions[key] = Math.max(nextVersions[key] ?? 0, version);
      projectionVersions.value = nextVersions;
      acceptId(coveredId);
      for (const [key, version] of pendingVersions) {
        if (version <= (projectionVersions.value[key] ?? 0)) pendingVersions.delete(key);
      }
      if (!pendingGap && !pendingUnversioned && pendingVersions.size === 0 && pendingId > lastAcceptedId.value) {
        acceptId(pendingId);
      }
      if (pendingId <= lastAcceptedId.value) pendingId = 0;
    }
  }

  function ensureRefresh(): void {
    if (disposed || refreshing.value) return;
    const run = refreshTruth();
    const tracked = run.finally(() => { if (refreshing.value === tracked) refreshing.value = null; });
    refreshing.value = tracked;
  }

  function onMessage(message: MessageEvent<string>): void {
    let event: TaskEventEnvelope;
    try { event = parseTaskEvent(JSON.parse(message.data)); }
    catch { diagnostic.value = "事件格式无效，已忽略"; return; }
    if (disposed) return;
    if (event.type !== "stream.gap" && !options.matches(event)) return;
    queue(event);
    if (pendingId > lastAcceptedId.value) ensureRefresh();
  }

  function onOpen(): void { if (!disposed) connection.value = "connected"; }
  function onError(): void {
    if (disposed) return;
    connection.value = "reconnecting";
    probePromise.value = options.probeClient?.getSession()
      .then(() => undefined)
      .catch(async (error) => { if (!disposed && isUnauthorized(error)) await options.onUnauthorized?.(); })
      ?? Promise.resolve();
  }
  const messageListener = (event: Event | MessageEvent<string>) => onMessage(event as MessageEvent<string>);
  const openListener = () => onOpen();
  const errorListener = () => onError();

  async function start(): Promise<void> {
    await options.refresh();
    if (disposed) return;
    const factory = options.eventSourceFactory ?? (typeof EventSource === "undefined" ? null : defaultFactory);
    if (!factory) return;
    source = factory("/api/v1/events");
    for (const type of namedTypes) source.addEventListener(type, messageListener);
    source.addEventListener("open", openListener);
    source.addEventListener("error", errorListener);
  }

  function stop(): void {
    disposed = true;
    if (source) {
      for (const type of namedTypes) source.removeEventListener(type, messageListener);
      source.removeEventListener("open", openListener);
      source.removeEventListener("error", errorListener);
      source.close();
    }
    source = null;
    connection.value = "closed";
  }

  return { connection, diagnostic, lastAcceptedId, projectionVersions, refreshing, probePromise, start, stop };
}
