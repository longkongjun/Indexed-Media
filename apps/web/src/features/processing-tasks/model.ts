import type { ProcessingTaskListOptions } from "@mediaflow/api-client-ts";

/** ProcessingTask 中心列表查询复用的服务端筛选与游标上下文。 */
export type TaskCenterContext = ProcessingTaskListOptions;

/**
 * 可复用的异步投影展示状态。
 *
 * `stale` 表示仍可展示上一次成功数据；离线和常规失败均不应借此清空已有投影。
 */
export type TaskCenterState =
  | { kind: "loading"; stale: boolean }
  | { kind: "content" }
  | { kind: "empty" }
  | { kind: "offline"; stale: boolean }
  | { kind: "error"; stale: boolean };
