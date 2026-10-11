import type { JellyfinIntegration, MediaFlowClient } from "@mediaflow/api-client-ts";
import { ref } from "vue";

/**
 * 管理 Jellyfin 独立连接的脱敏健康投影。
 *
 * @param client - 已认证的管理员 MediaFlow 传输层。
 * @returns 响应式投影、刷新动作与加载/失败状态。
 * @remarks 只读展示与显式重测；Jellyfin 未配置或不可用绝不影响其他数据。
 */
export function useJellyfin(client: MediaFlowClient) {
  const integration = ref<JellyfinIntegration | null>(null);
  const loading = ref(false);
  const failed = ref(false);

  async function load(): Promise<void> {
    loading.value = true;
    try {
      integration.value = await client.getJellyfinIntegration();
      failed.value = false;
    } catch {
      failed.value = true;
    } finally {
      loading.value = false;
    }
  }

  async function refreshHealth(): Promise<void> {
    loading.value = true;
    try {
      integration.value = await client.refreshJellyfinHealth();
      failed.value = false;
    } catch {
      failed.value = true;
    } finally {
      loading.value = false;
    }
  }

  return { integration, loading, failed, load, refreshHealth };
}
