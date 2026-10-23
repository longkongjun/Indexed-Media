/** MfButton 的视觉变体；`icon` 为仅图标的方形按钮，必须提供可访问名称。 */
export type MfButtonVariant = "default" | "primary" | "danger" | "icon";

/** 原生 button type；默认 `button` 避免在表单中意外提交，表单提交用途显式传 `submit`。 */
export type MfButtonType = "button" | "submit" | "reset";
