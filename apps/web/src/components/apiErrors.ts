import { MediaFlowApiError } from "@mediaflow/api-client-ts";
import type { Router } from "vue-router";
import type { useSessionStore } from "../app/session";

/**
 * 面向用户的安全错误内容，可选择性指出需要获得焦点的字段。
 *
 * 消息包含经整理的 UI 文案，绝不暴露原始服务端异常或响应体。
 */
export interface SafeError {
  message: string;
  field?: string;
}

const messages: Record<string, SafeError> = {
  "path.invalid": { message: "请输入能力根内的有效相对路径", field: "relative-path" },
  "path.escape": { message: "路径超出能力根，请检查相对路径", field: "relative-path" },
  "path.symlink_forbidden": { message: "该路径包含不允许的符号链接", field: "relative-path" },
  "inbox.overlap": { message: "该路径与现有收件目录重叠", field: "relative-path" },
  "root.not_found": { message: "所选能力根不存在，请重新选择", field: "root-id" },
  "root.unavailable": { message: "所选能力根暂不可用，请联系部署者检查权限", field: "root-id" },
  "inbox.not_found": { message: "收件目录不存在或已移除" },
  "task.not_found": { message: "扫描任务不存在或已移除" },
  "task.invalid_state": { message: "当前任务状态不允许执行此操作" },
  "request.conflict": { message: "请求正在处理，请稍后刷新" },
  "resource.conflict": { message: "资源仍被活动任务使用，暂时不能删除" },
  "organization.root-read-only": { message: "所选能力根不可写，请选择 read-write 根", field: "root-id" },
  "organization.target-overlap": { message: "目标目录与收件目录或现有整理目标重叠", field: "relative-path" },
  "organization.target-unavailable": { message: "目标目录暂不可用，请检查部署根", field: "root-id" },
  "organization.target-exists": { message: "目标位置已有非本次操作内容，请重新计算计划" },
  "organization.source-changed": { message: "来源文件已变化，请刷新任务后重新计算" },
  "organization.hardlink-cross-device": { message: "来源与目标不在同一文件系统，不能创建硬链接" },
  "integration.unauthorized": { message: "下载器凭据已失效，请更新连接" },
  "integration.unsupported-version": { message: "下载器版本不受支持，请升级后重试" },
  "validation.failed": { message: "提交内容未通过校验，请检查字段" },
};

/**
 * 判断未知失败是否带有 HTTP 状态 401。
 *
 * @param error - 任意捕获值。
 * @returns 仅当该值是非空对象且 `status` 属性等于 401 时为 `true`。
 */
export function isUnauthorized(error: unknown): boolean {
  return typeof error === "object" && error !== null && "status" in error && error.status === 401;
}

/**
 * 将 fetch 风格的网络失败归类为离线失败。
 *
 * @param error - 任意捕获值。
 * @returns 该值是否为 `TypeError`。
 */
export function isOffline(error: unknown): boolean {
  return error instanceof TypeError;
}

/**
 * 识别虽然缺少响应但操作结果可能已经提交的写入失败。
 *
 * @param error - 任意捕获值。
 * @returns 对网络失败、HTTP 5xx API 错误或明确的 `internal.error` 响应码返回 `true`。
 */
export function isAmbiguousWriteFailure(error: unknown): boolean {
  return isOffline(error) || (error instanceof MediaFlowApiError && (error.status >= 500 || error.body?.error.code === "internal.error"));
}

/** 识别已得到确定响应的乐观版本或幂等键冲突。 */
export function isRequestConflict(error: unknown): boolean {
  return error instanceof MediaFlowApiError && error.status === 409;
}

/**
 * 将结构化 API 错误映射为经整理的安全用户文案。
 *
 * @param error - 任意捕获值。
 * @param fallback - 错误码未知或该值不是 MediaFlow API 错误时使用的消息。
 * @returns 安全的显示文案；对于已知的验证错误，还会包含相关表单字段。
 */
export function safeApiError(error: unknown, fallback = "请求暂时失败，请重试"): SafeError {
  if (error instanceof MediaFlowApiError) return messages[error.body?.error.code ?? ""] ?? { message: fallback };
  return { message: fallback };
}

/** 已认证请求失败后、功能更新其展示状态前调用的回调。 */
export type AuthenticatedFailureHandler = (error: unknown) => unknown | Promise<unknown>;

/**
 * 对已认证请求失败应用共享的会话过期恢复流程。
 *
 * @param error - 已认证操作返回的失败。
 * @param session - 将更新其安全返回位置和敏感状态的会话 store。
 * @param router - 用登录页替换当前视图的路由器。
 * @param returnLocation - 登录后要恢复的候选业务路由；不安全的值会由 store 丢弃。
 * @returns 处理 401 后返回 `true`；否则在不改变会话或导航状态时返回 `false`。
 * @remarks 处理 401 会在等待导航前清除内存中的账户和 CSRF token。
 */
export async function handleAuthenticatedFailure(
  error: unknown,
  session: ReturnType<typeof useSessionStore>,
  router: Router,
  returnLocation: string,
): Promise<boolean> {
  if (!isUnauthorized(error)) return false;
  session.saveReturnLocation(returnLocation);
  session.resetSensitiveState();
  await router.replace({ name: "login" });
  return true;
}
