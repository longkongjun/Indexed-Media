import type { MediaFlowClient, SessionResponse } from "@mediaflow/api-client-ts";
import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { Router } from "vue-router";

type CsrfClient = Pick<MediaFlowClient, "setCsrfToken">;

const businessPath = /^\/(?:tasks(?:\/|$)|review-cases(?:\/|$)|media(?:\/|$)|scan-tasks(?:\/|$)|inbox-directories(?:\/|$)|downloads(?:\/|$)|connections\/downloaders(?:\/|$)|organization\/targets(?:\/|$)|automation\/(?:sources|events)(?:\/|$))/;

/**
 * 仅接受适用于登录后导航的精确同源业务路由。
 *
 * @param value - 待验证的原始路由字符串。
 * @returns 保持不变（含查询参数）的路由；外部、格式错误、含片段、控制字符、反斜杠、设置页、登录页或其他非业务位置
 * 则返回 `null`。
 */
export function safeReturnLocation(value: string): string | null {
  if (!value.startsWith("/") || value.startsWith("//") || value.includes("\\") || /[\u0000-\u001f]/.test(value)) return null;
  if (value.includes("#")) return null;
  try {
    const parsed = new URL(value, "https://mediaflow.invalid");
    const normalized = `${parsed.pathname}${parsed.search}`;
    return parsed.origin === "https://mediaflow.invalid" && normalized === value && businessPath.test(parsed.pathname)
      ? normalized
      : null;
  } catch {
    return null;
  }
}

/**
 * 持有已认证账户投影、内存中的 CSRF token 以及安全的登录后目标位置。
 *
 * @returns 可绑定客户端、建立或清除会话、消费返回位置并退出登录的 Pinia store。建立或重置操作会更新所绑定客户端的
 * CSRF token。即使 Core 的退出请求失败，退出登录也始终清除本地敏感状态并以登录页替换当前路由。
 */
export const useSessionStore = defineStore("session", () => {
  const account = ref<SessionResponse["account"] | null>(null);
  const csrfToken = ref<string | null>(null);
  const returnLocation = ref<string | null>(null);
  const signedOut = ref(false);
  let client: CsrfClient | null = null;

  const authenticated = computed(() => account.value !== null && csrfToken.value !== null);

  function configureClient(nextClient: CsrfClient): void {
    client = nextClient;
    client.setCsrfToken(csrfToken.value);
  }

  function establish(response: SessionResponse): void {
    account.value = response.account;
    csrfToken.value = response.csrf_token;
    signedOut.value = false;
    client?.setCsrfToken(response.csrf_token);
  }

  function resetSensitiveState(): void {
    account.value = null;
    csrfToken.value = null;
    client?.setCsrfToken(null);
  }

  function saveReturnLocation(value: string): void {
    returnLocation.value = safeReturnLocation(value);
  }

  function consumeReturnLocation(): string {
    const destination = returnLocation.value ?? "/inbox-directories";
    returnLocation.value = null;
    return destination;
  }

  async function logout(router: Router, deleteSession?: MediaFlowClient["deleteSession"]): Promise<void> {
    try {
      await deleteSession?.();
    } catch {
      // 即使 Core 无法确认退出登录，也必须执行本地清理。
    } finally {
      resetSensitiveState();
      returnLocation.value = null;
      signedOut.value = true;
      await router.replace({ name: "login" });
    }
  }

  return {
    account,
    csrfToken,
    returnLocation,
    signedOut,
    authenticated,
    configureClient,
    establish,
    resetSensitiveState,
    saveReturnLocation,
    consumeReturnLocation,
    logout,
  };
});
