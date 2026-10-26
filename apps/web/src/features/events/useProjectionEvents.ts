import { acceptEventId, parseTaskEvent, type MediaFlowClient, type TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { ref } from "vue";
import { isUnauthorized } from "../../components/apiErrors";
import type { EventSourceFactory, EventSourceLike } from "../scan-tasks/useTaskEvents";

interface ProjectionVersion { key: string; version: number }

interface ProjectionEventOptions {
  refresh: () => Promise<unknown>;
  isFresh: () => boolean;
  eventTypes: readonly TaskEventEnvelope["type"][];
  matches: (event: TaskEventEnvelope) => boolean;
  versionOf?: (event: TaskEventEnvelope) => ProjectionVersion | null;
  eventSourceFactory?: EventSourceFactory | undefined;
  probeClient?: Pick<MediaFlowClient, "getSession">;
  onUnauthorized?: () => unknown;
}

function defaultFactory(url: string): EventSourceLike { return new EventSource(url); }

/**
 * 依据严格且单调的 SSE 提示刷新一个有界 REST 投影。
 *
 * 事件只在权威刷新返回且 isFresh 确认必要投影可用后才推进已接受 ID；流缺口、未知版本和同一投影的多次变更会合并为一次刷新，重连会重试未确认事件，停止后不会让延迟结果写回状态。
 * `start()` 先执行初次权威刷新再创建 EventSource；刷新抛出的异常会传递给调用方。
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
  const pending = { id: 0, gap: false, unversioned: false };
  let source: EventSourceLike | null = null;
  const lifecycle = { disposed: false };
  const namedTypes = [...new Set([...options.eventTypes, "stream.gap" as const])];

  function acceptId(id: number): boolean {
    if (!acceptEventId(seen, id)) return false;
    lastAcceptedId.value = id;
    return true;
  }

  function queue(event: TaskEventEnvelope): void {
    if (!Number.isSafeInteger(event.id) || event.id <= lastAcceptedId.value || event.id <= pending.id) return;
    const hadPending = pending.id > lastAcceptedId.value;
    pending.id = event.id;
    if (event.type === "stream.gap") {
      pending.gap = true;
      return;
    }
    const projection = options.versionOf?.(event) ?? null;
    if (!projection) {
      pending.unversioned = true;
      return;
    }
    const accepted = projectionVersions.value[projection.key] ?? 0;
    if (projection.version <= accepted) {
      if (!hadPending && options.isFresh()) {
        acceptId(event.id);
        pending.id = 0;
      }
      return;
    }
    pendingVersions.set(projection.key, Math.max(pendingVersions.get(projection.key) ?? 0, projection.version));
  }

  async function refreshTruth(): Promise<void> {
    while (!lifecycle.disposed && pending.id > lastAcceptedId.value) {
      const coveredId = pending.id;
      const coveredVersions = new Map(pendingVersions);
      const hadGap = pending.gap;
      const hadUnversioned = pending.unversioned;
      pending.gap = false;
      pending.unversioned = false;
      for (const [key, version] of coveredVersions) {
        if ((pendingVersions.get(key) ?? 0) <= version) pendingVersions.delete(key);
      }
      try {
        await options.refresh();
        if (!options.isFresh()) throw new Error("projection refresh incomplete");
      } catch {
        // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- await 期间 stop() 可使当前实例失效
        if (lifecycle.disposed) return;
        diagnostic.value = "事件状态刷新失败，将在重连或后续事件到达时重试";
        pending.gap ||= hadGap;
        pending.unversioned ||= hadUnversioned;
        for (const [key, version] of coveredVersions) pendingVersions.set(key, Math.max(pendingVersions.get(key) ?? 0, version));
        return;
      }
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- await 期间 stop() 可翻转 disposed，类型层不可见
      if (lifecycle.disposed) return;
      diagnostic.value = "";
      const nextVersions = { ...projectionVersions.value };
      for (const [key, version] of coveredVersions) nextVersions[key] = Math.max(nextVersions[key] ?? 0, version);
      projectionVersions.value = nextVersions;
      acceptId(coveredId);
      for (const [key, version] of pendingVersions) {
        if (version <= (projectionVersions.value[key] ?? 0)) pendingVersions.delete(key);
      }
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- await 期间并发事件回调可置位 gap/unversioned，类型层不可见
      if (!pending.gap && !pending.unversioned && pendingVersions.size === 0 && pending.id > lastAcceptedId.value) {
        acceptId(pending.id);
      }
      if (pending.id <= lastAcceptedId.value) pending.id = 0;
    }
  }

  function ensureRefresh(): void {
    if (lifecycle.disposed || refreshing.value || pending.id <= lastAcceptedId.value) return;
    const run = refreshTruth();
    const tracked = run.finally(() => { if (refreshing.value === tracked) refreshing.value = null; });
    refreshing.value = tracked;
  }

  function onMessage(message: MessageEvent<string>): void {
    if (lifecycle.disposed) return;
    let event: TaskEventEnvelope;
    try { event = parseTaskEvent(JSON.parse(message.data)); }
    catch { diagnostic.value = "事件格式无效，已忽略"; return; }
    if (event.type !== "stream.gap" && !options.matches(event)) return;
    queue(event);
    if (pending.id > lastAcceptedId.value) ensureRefresh();
  }

  function onOpen(): void {
    if (lifecycle.disposed) return;
    connection.value = "connected";
    // 重连可能先于在途刷新失败；等它结束后再检查尚未确认的事件。
    if (refreshing.value) void refreshing.value.then(ensureRefresh);
    else ensureRefresh();
  }
  function onError(): void {
    if (lifecycle.disposed) return;
    connection.value = "reconnecting";
    probePromise.value = options.probeClient?.getSession()
      .then(() => undefined)
      .catch(async (error: unknown) => { if (!lifecycle.disposed && isUnauthorized(error)) await options.onUnauthorized?.(); })
      ?? Promise.resolve();
  }
  const messageListener = (event: Event | MessageEvent<string>) => { onMessage(event as MessageEvent<string>); };
  const openListener = () => { onOpen(); };
  const errorListener = () => { onError(); };

  async function start(): Promise<void> {
    if (lifecycle.disposed) return;
    await options.refresh();
    // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- await 期间 stop() 可使当前实例失效
    if (lifecycle.disposed) return;
    const factory = options.eventSourceFactory ?? (typeof EventSource === "undefined" ? null : defaultFactory);
    if (!factory) return;
    source = factory("/api/v1/events");
    for (const type of namedTypes) source.addEventListener(type, messageListener);
    source.addEventListener("open", openListener);
    source.addEventListener("error", errorListener);
  }

  function stop(): void {
    lifecycle.disposed = true;
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
