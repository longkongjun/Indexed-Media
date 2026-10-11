<script setup lang="ts">
/**
 * 通知中心：应用内通知的最新在前列表与已读标记。
 *
 * 只读事实 + 显式已读动作；出站 Webhook 的配置与健康在同一页面的独立区块维护，
 * 其失败绝不影响应用内投递。
 */
import { nextTick, onMounted, ref, watchEffect } from "vue";
import { useRoute, useRouter } from "vue-router";
import AsyncState from "../components/AsyncState.vue";
import CursorPager from "../components/CursorPager.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { notificationKindLabel } from "../components/dictionaries";
import { useMediaFlowClient } from "../components/injectedClient";
import { useNotifications } from "../features/notifications/useNotifications";
import { useNotificationsBadgeStore } from "../app/notifications";
import { useSessionStore } from "../app/session";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const notifications = useNotifications(client, failure);
const notificationsBadge = useNotificationsBadgeStore();
const unreadOnly = ref(false);

// 导航角标同步：仅在加载完成（content/empty）后回写，避免加载中的 0 抹掉外壳已取得的计数。
watchEffect(() => {
  const kind = notifications.state.value.kind;
  if (kind === "content" || kind === "empty") {
    notificationsBadge.setUnread(notifications.unreadCount.value);
  }
});

function kindLabel(kind: string): string {
  return notificationKindLabel(kind);
}

/** 跳转即读（notification 规格 §3）：查看任务前先标记该条已读，失败不阻塞导航。 */
function openTask(taskId: string, notificationId: string, unread: boolean): void {
  if (unread) void notifications.markRead(notificationId);
  void router.push({ name: "task", params: { id: taskId } });
}

async function reload(): Promise<void> {
  await notifications.load({ unreadOnly: unreadOnly.value });
}

/** 逐条已读：成功后本地更新（不重载列表），并把焦点留在该行（可访问性：不丢焦点）。 */
async function markRead(notificationId: string, event?: Event): Promise<void> {
  const row = event ? ((event.currentTarget as HTMLElement).closest("li") as HTMLElement | null) : null;
  if (!(await notifications.markRead(notificationId))) return;
  const item = notifications.items.value.find((entry) => entry.id === notificationId);
  if (item && item.read_at === null) item.read_at = new Date().toISOString();
  await nextTick();
  row?.focus();
}

/** 全部已读后重载列表与计数。 */
async function markAllRead(): Promise<void> {
  if (!(await notifications.markRead())) return;
  await reload();
}

onMounted(() => {
  void reload();
});
</script>

<template>
  <section class="notifications-page" aria-labelledby="notifications-heading">
    <h1 id="notifications-heading">通知中心</h1>
    <p class="page-help">应用内通知是唯一必达通道；出站 Webhook 只是附加投递，其故障不影响本地通知。</p>
    <div class="toolbar">
      <label class="check-field">
        <input v-model="unreadOnly" type="checkbox" @change="() => { void reload(); }" />只看未读
      </label>
      <button type="button" :disabled="notifications.unreadCount.value === 0" @click="() => { void markAllRead(); }">
        全部标记已读
      </button>
    </div>
    <AsyncState :state="notifications.state.value" :error-message="notifications.errorMessage.value" empty-message="暂无通知。需要管理员关注的事件会出现在这里。" @retry="reload">
      <ul class="notification-list">
        <li
          v-for="item in notifications.items.value"
          :key="item.id"
          :data-notification-kind="item.kind"
          :class="{ unread: item.read_at === null }"
          tabindex="-1"
        >
          <div class="notification-main">
            <span class="kind">{{ kindLabel(item.kind) }}</span>
            <span v-if="item.read_at === null" class="visually-hidden">未读</span>
            <span class="title">{{ item.title }}</span>
            <p class="body">{{ item.body }}</p>
            <time :datetime="item.created_at">{{ item.created_at }}</time>
          </div>
          <div class="notification-actions">
            <button v-if="item.task_id" type="button" class="link-button" @click="openTask(item.task_id, item.id, item.read_at === null)">查看任务</button>
            <button v-if="item.read_at === null" type="button" @click="markRead(item.id, $event)">标记已读</button>
          </div>
        </li>
      </ul>
      <CursorPager :next-cursor="notifications.nextCursor.value" :shown-count="notifications.items.value.length" @next="() => { void notifications.loadMore(); }" />
    </AsyncState>
  </section>
</template>

<style scoped>
.notifications-page { display: grid; gap: 1rem; }
.page-help { margin: 0; color: var(--mfd-color-text-secondary); }
.toolbar { display: flex; gap: 1rem; align-items: center; }
.toolbar button { padding: .45rem .8rem; border: 1px solid var(--mfd-color-border-default); border-radius: .5rem; background: var(--mfd-color-action-secondary-bg); color: var(--mfd-color-text-primary); }
.toolbar button:disabled { opacity: .5; }
.notification-list { margin: 0; padding: 0; list-style: none; display: grid; gap: .6rem; }
.notification-list li { display: flex; justify-content: space-between; gap: 1rem; padding: .8rem 1rem; border: 1px solid var(--mfd-color-border-default); border-radius: var(--mfd-radius-lg); background: var(--mfd-color-bg-surface); }
.notification-list li.unread { border-color: var(--mfd-color-border-strong); box-shadow: inset 3px 0 0 var(--mfd-color-border-strong); }
.kind { display: inline-block; margin-right: .5rem; padding: .1rem .5rem; border-radius: var(--mfd-radius-full); background: var(--mfd-color-bg-selected); color: var(--mfd-color-text-secondary); font-size: var(--mfd-font-size-xs); }
.title { font-weight: 400; color: var(--mfd-color-text-secondary); }
.notification-list li.unread .title { font-weight: 650; color: var(--mfd-color-text-primary); }
.body { margin: .3rem 0; color: var(--mfd-color-text-secondary); font-size: .9rem; }
time { color: var(--mfd-color-text-muted); font-size: .78rem; }
.notification-actions { display: flex; flex-direction: column; gap: .4rem; align-items: flex-end; }
.notification-actions button { padding: .3rem .7rem; border: 1px solid var(--mfd-color-border-default); border-radius: .45rem; background: var(--mfd-color-action-secondary-bg); font-size: .82rem; }
.link-button { color: var(--mfd-color-text-link); }
</style>
