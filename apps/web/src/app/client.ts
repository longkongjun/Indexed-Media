import { createMediaFlowClient, type MediaFlowClient } from "@mediaflow/api-client-ts";
import type { InjectionKey } from "vue";

/** Vue 注入键，用于应用提供且限定在应用作用域内的 MediaFlow API 客户端。 */
export const identityClientKey: InjectionKey<MediaFlowClient> = Symbol("mediaflow-client");

/**
 * 正式 Web 应用使用的默认同源 API 客户端。
 *
 * @remarks 客户端将 CSRF token 保留在内存中；已认证会话的建立会更新该 token。
 */
export const mediaFlowClient = createMediaFlowClient();
