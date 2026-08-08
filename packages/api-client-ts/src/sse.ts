import type { TaskEventEnvelope } from "./client.js";
import { parseTaskEvent } from "./events.js";

const knownEventTypes = new Set([
  "task.progress",
  "task.state-changed",
  "processing-task.state-changed",
  "processing-task.identification-decided",
  "inbox.discovery-health-changed",
  "integration.health-changed",
  "stream.gap",
]);

/** 浏览器或 HTTP 解析器解码字段后的传输层 SSE 帧。 */
export interface SseFrame {
  /** SSE `id` 字段中的正 JavaScript 安全整数。 */
  id: number;
  /** 稳定的 SSE `event` 名称。 */
  type: string;
  /** SSE `data` 字段解析得到的 JSON 值。 */
  data: unknown;
}

/** 有状态的重放游标和当前传输层所支持事件子集的接收边界。 */
export interface MediaFlowSseState {
  /** 最近单调接收的传输 ID，包含已忽略的未来事件名称。 */
  readonly lastEventId: number;
  /**
   * 接收一帧已解码数据；返回已知有效信封，忽略重放或未知名称。
   *
   * @throws {TypeError} `frame.id` 不是正 JavaScript 安全整数、已知事件无法解析，或传输字段与事件信封不一致时抛出。
   * @remarks 上述失败均不推进 `lastEventId`；未知事件名称会推进游标，重复或乱序 ID 则保持游标不变。
   */
  accept(frame: SseFrame): TaskEventEnvelope | undefined;
}

/**
 * 创建 SSE 重连循环使用的事件子集传输状态。
 *
 * @param options - 可选的已持久化重放游标。
 * @returns 接收当前传输层支持的事件子集并保存单调游标的状态对象。
 * @throws {RangeError} 初始 `lastEventId` 不是非负 JavaScript 安全整数时抛出。
 * @remarks `task-decision.accepted`、`review-case.updated` 和 `catalog.media-changed` 虽能由通用解析器识别，
 * 当前仍按未知传输事件忽略并推进 `lastEventId`；其他未知名称采用相同策略，避免旧客户端在未来 schema 成员上无限重连。
 * 接收帧的失败契约由 `MediaFlowSseState.accept` 定义。
 */
export function createSseState(options: { lastEventId?: number } = {}): MediaFlowSseState {
  let lastEventId = options.lastEventId ?? 0;
  if (!Number.isSafeInteger(lastEventId) || lastEventId < 0) {
    throw new RangeError("lastEventId must be a nonnegative safe integer");
  }

  return {
    get lastEventId() {
      return lastEventId;
    },
    accept(frame) {
      if (!Number.isSafeInteger(frame.id) || frame.id < 1) {
        throw new TypeError("SSE event id must be a positive safe integer");
      }
      if (frame.id <= lastEventId) return undefined;
      if (!knownEventTypes.has(frame.type)) {
        lastEventId = frame.id;
        return undefined;
      }
      const event = parseTaskEvent(frame.data);
      if (event.id !== frame.id || event.type !== frame.type) {
        throw new TypeError("SSE transport fields do not match the event envelope");
      }
      lastEventId = frame.id;
      return event;
    },
  };
}
