import { MediaFlowApiError, type BootstrapRequest, type MediaFlowClient, type LoginRequest } from "@mediaflow/api-client-ts";
import { inject, onBeforeUnmount, ref } from "vue";
import { useRouter } from "vue-router";
import { identityClientKey } from "../../app/client";
import { useSessionStore } from "../../app/session";
import { invalidateBootstrapState } from "../../app/bootstrap";

/** 由共享表单错误映射器表示的身份操作。 */
export type IdentityOperation = "setup" | "login";
/** 初始化或登录验证失败时可能获得焦点的表单控件。 */
export type IdentityErrorTarget = "bootstrap-secret" | "administrator-name" | "new-password" | "login-administrator-name" | "login-password";

interface IdentityErrorDescription {
  message: string;
  targets: IdentityErrorTarget[];
}

function describeError(error: unknown, operation: IdentityOperation): IdentityErrorDescription {
  if (error instanceof TypeError) return { message: "无法连接 MediaFlow Core，请检查网络后重试。", targets: [] };
  if (error instanceof MediaFlowApiError) {
    switch (error.body?.error.code) {
      case "bootstrap.invalid_secret": return { message: "引导密钥无效或已过期，请重新输入。", targets: ["bootstrap-secret"] };
      case "auth.invalid_credentials": return { message: "管理员名称或密码不正确。", targets: ["login-administrator-name", "login-password"] };
      case "auth.rate_limited": {
        const retryAfter = error.body.error.details?.retry_after_seconds;
        const message = typeof retryAfter === "number" && Number.isFinite(retryAfter) && retryAfter > 0
          ? `尝试次数过多，请在 ${Math.ceil(retryAfter)} 秒后重试。`
          : "尝试次数过多，请稍后再试。";
        return { message, targets: [] };
      }
      default: return {
        message: operation === "login" ? "登录服务暂时不可用，请稍后重试。" : "初始化暂时无法完成，请稍后重试。",
        targets: [],
      };
    }
  }
  return {
    message: operation === "login" ? "登录服务暂时不可用，请稍后重试。" : "初始化暂时无法完成，请稍后重试。",
    targets: [],
  };
}

/**
 * 协调初始化、登录和退出登录的 UI 状态，以及注入的 API 客户端和会话 store。
 *
 * @returns 响应式提交/错误状态，以及 `setup`、`login`、`logout` 和 `clearError` 操作。被拒绝、重复或已销毁的
 * 初始化和登录提交会解析为 `false`，而不是抛出 API 失败。
 * @throws {Error} 当前 Vue 应用未提供 MediaFlow 客户端时抛出。
 * @remarks 成功初始化会使路由器初始化缓存失效并导航到登录页。成功登录会设置 CSRF token 并消费保存的返回位置。
 * 卸载会阻止延迟响应更新状态。
 */
export function useIdentity() {
  const injectedClient = inject(identityClientKey);
  if (!injectedClient) throw new Error("MediaFlow client was not provided");
  const client: MediaFlowClient = injectedClient;
  const router = useRouter();
  const session = useSessionStore();
  const submitting = ref(false);
  const error = ref<string | null>(null);
  const errorTargets = ref<IdentityErrorTarget[]>([]);
  let disposed = false;

  onBeforeUnmount(() => { disposed = true; });

  function clearError(): void {
    error.value = null;
    errorTargets.value = [];
  }

  async function setup(body: BootstrapRequest): Promise<boolean> {
    if (submitting.value) return false;
    submitting.value = true;
    clearError();
    try {
      await client.bootstrap(body);
      if (disposed) return false;
      invalidateBootstrapState(router);
      await router.replace({ name: "login" });
      return true;
    } catch (cause) {
      if (disposed) return false;
      if (cause instanceof MediaFlowApiError && cause.body?.error.code === "bootstrap.already_completed") {
        invalidateBootstrapState(router);
        await router.replace({ name: "login" });
        return true;
      }
      const description = describeError(cause, "setup");
      error.value = description.message;
      errorTargets.value = description.targets;
      return false;
    } finally {
      if (!disposed) submitting.value = false;
    }
  }

  async function login(body: LoginRequest): Promise<boolean> {
    if (submitting.value) return false;
    submitting.value = true;
    clearError();
    try {
      const response = await client.createSession(body);
      if (disposed) return false;
      session.configureClient(client);
      session.establish(response);
      await router.replace(session.consumeReturnLocation());
      return true;
    } catch (cause) {
      if (disposed) return false;
      const description = describeError(cause, "login");
      error.value = description.message;
      errorTargets.value = description.targets;
      return false;
    } finally {
      if (!disposed) submitting.value = false;
    }
  }

  async function logout(): Promise<void> {
    await session.logout(router, client.deleteSession.bind(client));
  }

  return { client: client as MediaFlowClient, submitting, error, errorTargets, clearError, setup, login, logout };
}
