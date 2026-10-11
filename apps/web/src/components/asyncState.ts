/**
 * 异步内容的有限展示状态。
 *
 * 语义权威：`packages/ui-contract`（contract/page-states.json 五态 + stale）。本类型在权威五态之外
 * 增加 `idle`（请求尚未发起的实现级起始态，不属于页面状态权威值域）。
 *
 * `stale` 仅在加载期间或错误/离线切换后有意义，并允许调用方保持已加载内容可见；`content` 和 `empty`
 * 表示已完成的请求。
 */
export type AsyncState =
  | { kind: "idle" }
  | { kind: "loading"; stale?: boolean }
  | { kind: "content" }
  | { kind: "empty" }
  | { kind: "error"; stale?: boolean }
  | { kind: "offline"; stale?: boolean };
