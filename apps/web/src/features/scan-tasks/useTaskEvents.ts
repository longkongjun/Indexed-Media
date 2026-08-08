import { acceptEventId, parseTaskEvent, type MediaFlowClient, type TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { ref, type InjectionKey } from "vue";
import { isUnauthorized } from "../../components/apiErrors";
import type { ScanTaskFeature } from "./useScanTasks";

/**
 * 扫描任务同步所需的最小 EventSource 契约；测试会在需要时提供它。
 *
 * 监听器接收浏览器事件；`close` 必须停止后续投递并释放底层连接。
 */
export interface EventSourceLike {
  addEventListener(type: string, listener: (event: Event | MessageEvent<string>) => void): void;
  removeEventListener(type: string, listener: (event: Event | MessageEvent<string>) => void): void;
  close(): void;
}
/** 为提供的同源 URL 创建兼容 EventSource 的连接。 */
export type EventSourceFactory = (url: string) => EventSourceLike;
/** Vue 注入键，用于在应用或测试中覆盖任务事件连接的创建方式。 */
export const taskEventSourceFactoryKey: InjectionKey<EventSourceFactory> = Symbol("mediaflow-event-source-factory");
interface Options {
  eventSourceFactory?: EventSourceFactory;
  refreshRelated?: () => Promise<void>;
  onUnauthorized?: () => unknown | Promise<unknown>;
}
type ScanTaskEvent = Extract<TaskEventEnvelope, { type: "task.progress" | "task.state-changed" }>;

function defaultFactory(url: string): EventSourceLike { return new EventSource(url) as unknown as EventSourceLike; }

/**
 * 将扫描任务功能与版本一 SSE 任务事件同步。
 *
 * @param detail - 接收已接受快照和权威重新加载结果的任务详情表面。
 * @param options - 可选的 EventSource 工厂、关联数据刷新函数及会话过期回调。
 * @returns 响应式连接/诊断/去重状态，以及面向生命周期的 `start` 和 `stop` 操作。
 * @remarks `start` 会在打开 `/api/v1/events` 前加载任务。重复或乱序 ID 会被忽略；流缺口会缓冲最新 ID，
 * 在接受后续事件前重新加载权威状态。`stop` 会移除监听器、关闭来源，并防止延迟刷新结果修改功能状态。
 */
export function useTaskEvents(detail: ScanTaskFeature, options: Options = {}) {
  const connection = ref<"idle" | "connected" | "reconnecting" | "closed">("idle");
  const diagnostic = ref("");
  const lastAcceptedId = ref(0);
  const dedupSize = ref(0);
  const refreshing = ref<Promise<void> | null>(null);
  const probePromise = ref<Promise<void> | null>(null);
  const seen = new Set<number>();
  let source: EventSourceLike | null = null;
  let disposed = false;
  let pendingTruthId = 0;
  const namedTypes = ["task.progress", "task.state-changed", "stream.gap"] as const;
  const terminalStatuses = new Set(["completed", "partial-success", "failed", "cancelled"]);

  function acceptId(id: number): boolean {
    if (!acceptEventId(seen, id)) return false;
    lastAcceptedId.value = id;
    dedupSize.value = seen.size;
    return true;
  }

  function applyEvent(event: ScanTaskEvent): void {
    if (disposed || !detail.task.value || !acceptEventId(seen, event.id)) return;
    lastAcceptedId.value = event.id;
    dedupSize.value = seen.size;
    if (event.type === "task.progress") detail.applySnapshot({ ...detail.task.value, counts: { ...event.payload } });
    else detail.applySnapshot({ ...detail.task.value, status: event.payload.status, recovering: event.payload.recovering });
  }

  function bufferTruthId(id: number): boolean {
    if (!Number.isSafeInteger(id) || id <= lastAcceptedId.value || id <= pendingTruthId) return false;
    pendingTruthId = id;
    return true;
  }

  async function refreshAfterGap(): Promise<void> {
    while (!disposed && pendingTruthId > lastAcceptedId.value) {
      const coveredId = pendingTruthId;
      try { await Promise.all([detail.load(), options.refreshRelated?.()]); }
      catch { diagnostic.value = "事件状态刷新失败，将在后续事件到达时重试"; return; }
      if (disposed) { pendingTruthId = 0; return; }
      if (detail.state.value.kind !== "content") { diagnostic.value = "事件状态尚未确认，将在后续事件到达时重试"; return; }
      acceptId(coveredId);
      if (pendingTruthId === coveredId) pendingTruthId = 0;
    }
  }

  function ensureTruthRefresh(): void {
    if (disposed || refreshing.value) return;
    const run = refreshAfterGap();
    const tracked = run.finally(() => { if (refreshing.value === tracked) refreshing.value = null; });
    refreshing.value = tracked;
  }

  function onMessage(message: MessageEvent<string>): void {
    let event;
    try { event = parseTaskEvent(JSON.parse(message.data)); }
    catch { diagnostic.value = "事件格式无效，已忽略"; return; }
    if (disposed || (event.type !== "stream.gap" && event.task_id !== detail.task.value?.id)) return;
    if (event.type === "stream.gap") {
      bufferTruthId(event.id);
      ensureTruthRefresh();
      return;
    }
    if (event.type !== "task.progress" && event.type !== "task.state-changed") return;
    if (event.type === "task.state-changed" && terminalStatuses.has(event.payload.status)) {
      bufferTruthId(event.id);
      ensureTruthRefresh();
      return;
    }
    if (refreshing.value || pendingTruthId > lastAcceptedId.value) {
      bufferTruthId(event.id);
      ensureTruthRefresh();
      return;
    }
    applyEvent(event);
  }

  function onError(): void {
    if (disposed) return;
    connection.value = "reconnecting";
    const candidate = detail as ScanTaskFeature & { client?: MediaFlowClient };
    const api = candidate.client;
    probePromise.value = api?.getSession
      ? api.getSession().then(() => undefined).catch(async (error) => {
        if (!disposed && isUnauthorized(error)) await options.onUnauthorized?.();
      })
      : Promise.resolve();
  }

  function onOpen(): void { if (!disposed) connection.value = "connected"; }
  const namedMessage = (event: Event | MessageEvent<string>) => onMessage(event as MessageEvent<string>);
  const openListener = () => onOpen();
  const errorListener = () => onError();

  async function start(): Promise<void> {
    await detail.load();
    if (disposed || !detail.task.value) return;
    source = (options.eventSourceFactory ?? defaultFactory)("/api/v1/events");
    for (const type of namedTypes) source.addEventListener(type, namedMessage);
    source.addEventListener("open", openListener);
    source.addEventListener("error", errorListener);
  }

  function stop(): void {
    disposed = true;
    pendingTruthId = 0;
    if (source) {
      for (const type of namedTypes) source.removeEventListener(type, namedMessage);
      source.removeEventListener("open", openListener);
      source.removeEventListener("error", errorListener);
      source.close();
    }
    source = null;
    connection.value = "closed";
  }
  return { connection, diagnostic, lastAcceptedId, dedupSize, refreshing, probePromise, start, stop };
}
