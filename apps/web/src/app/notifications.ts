import { defineStore } from "pinia";
import { ref } from "vue";

/**
 * 导航级未读通知计数（notification 规格 §3：角标计数 = 未读数，只统计需要管理员介入的类别；
 * 0 时隐藏）。AppShell 启动时加载一次，通知中心页操作后同步；失败静默（角标缺席 ≠ 数据异常）。
 */
export const useNotificationsBadgeStore = defineStore("notifications-badge", () => {
  const unreadCount = ref(0);
  const loaded = ref(false);

  function setUnread(count: number): void {
    unreadCount.value = Math.max(0, count);
    loaded.value = true;
  }

  return { unreadCount, loaded, setUnread };
});
