/** MfButton 的视觉变体；`icon` 为仅图标的方形按钮，必须提供可访问名称。 */
export type MfButtonVariant = "default" | "primary" | "danger" | "icon";

/** 请求容器的展示值域；idle 表示宿主尚未发起请求，stale 表示保留但禁止操作的旧内容。 */
export type MfRequestStateValue =
  | { kind: "idle" | "content" | "empty" }
  | { kind: "loading" | "error" | "offline"; stale?: boolean };

/** 原生 button type；默认 `button` 避免在表单中意外提交，表单提交用途显式传 `submit`。 */
export type MfButtonType = "button" | "submit" | "reset";
