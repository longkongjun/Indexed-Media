<script setup lang="ts">
/**
 * 通知中心：应用内通知的最新在前列表与已读标记。
 *
 * 只读事实 + 显式已读动作；出站 Webhook 的配置与健康在同一页面的独立区块维护，
 * 其失败绝不影响应用内投递。
 */
import { onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import AsyncState from "../components/AsyncState.vue";
import CursorPager from "../components/CursorPager.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import { useNotifications } from "../features/notifications/useNotifications";
import { useSessionStore } from "../app/session";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const notifications = useNotifications(client, failure);
const unreadOnly = ref(false);

const KIND_LABELS: Record<string, string> = {
  "review.waiting": "待人工审核",
  "organization.paused": "整理已暂停",
  "organization.rolled-back": "整理已回滚",
  "system.notice": "系统通知",
};

function kindLabel(kind: string): string {
  return KIND_LABELS[kind] ?? kind;
}

function openTask(taskId: string): void {
  void router.push({ name: "task", params: { id: taskId } });
}

async function reload(): Promise<void> {
  await notifications.load({ unreadOnly: unreadOnly.value });
}

async function markRead(notificationId?: string): Promise<void> {
  await notifications.markRead(notificationId);
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
      <button type="button" :disabled="notifications.unreadCount.value === 0" @click="() => { void markRead(); }">
        全部标记已读
      </button>
    </div>
    <AsyncState :state="notifications.state.value" :error-message="notifications.errorMessage.value" empty-message="暂无通知" @retry="reload">
      <ul class="notification-list">
        <li
          v-for="item in notifications.items.value"
          :key="item.id"
          :data-notification-kind="item.kind"
          :class="{ unread: item.read_at === null }"
        >
          <div class="notification-main">
            <span class="kind">{{ kindLabel(item.kind) }}</span>
            <span class="title">{{ item.title }}</span>
            <p class="body">{{ item.body }}</p>
            <time :datetime="item.created_at">{{ item.created_at }}</time>
          </div>
          <div class="notification-actions">
            <button v-if="item.task_id" type="button" class="link-button" @click="openTask(item.task_id)">查看任务</button>
            <button v-if="item.read_at === null" type="button" @click="() => { void markRead(item.id); }">标记已读</button>
          </div>
        </li>
      </ul>
      <CursorPager :next-cursor="notifications.nextCursor.value" @next="() => { void notifications.loadMore(); }" />
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
.notification-list li.unread { border-color: var(--mfd-color-action-primary); box-shadow: inset 3px 0 0 var(--mfd-color-action-primary); }
.kind { display: inline-block; margin-right: .5rem; padding: .1rem .5rem; border-radius: .4rem; background: var(--mfd-color-bg-surface-sunken); font-size: .78rem; }
.title { font-weight: 650; }
.body { margin: .3rem 0; color: var(--mfd-color-text-secondary); font-size: .9rem; }
time { color: var(--mfd-color-text-muted); font-size: .78rem; }
.notification-actions { display: flex; flex-direction: column; gap: .4rem; align-items: flex-end; }
.notification-actions button { padding: .3rem .7rem; border: 1px solid var(--mfd-color-border-default); border-radius: .45rem; background: var(--mfd-color-action-secondary-bg); font-size: .82rem; }
.link-button { color: var(--mfd-color-text-link); }
</style>
