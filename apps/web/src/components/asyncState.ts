/**
 * 异步内容的有限展示状态。
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
