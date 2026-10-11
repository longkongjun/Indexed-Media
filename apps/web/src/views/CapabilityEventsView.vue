<script setup lang="ts">
/**
 * AI 能力通道活动的只读管理视图。
 *
 * 展示 `media-capability.v1` 事件流的脱敏投影（最新在前）：事件类型、时间、关联任务与
 * 错误码。本页没有写路径——能力调用由 AI 控制面发起，管理员在此只看事实。
 */
import { onMounted } from "vue";
import { useRoute, useRouter } from "vue-router";
import AsyncState from "../components/AsyncState.vue";
import CursorPager from "../components/CursorPager.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { capabilityEventBadge, capabilityStateBadge } from "../components/dictionaries";
import { useMediaFlowClient } from "../components/injectedClient";
import { useCapabilityEvents } from "../features/capability-activity/useCapabilityEvents";
import { useSessionStore } from "../app/session";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const activity = useCapabilityEvents(client, failure);

function typeBadge(eventType: string) {
  return capabilityEventBadge(eventType);
}

function stateBadge(state: string | null) {
  return state === null ? null : capabilityStateBadge(state);
}

onMounted(() => {
  void activity.load();
});
</script>

<template>
  <section class="capability-events" aria-labelledby="capability-events-heading">
    <h1 id="capability-events-heading">AI 能力活动</h1>
    <p class="page-help">AI 通道（media-capability.v1）事件流的脱敏只读投影；能力调用与审批由对应任务页承载。</p>
    <AsyncState :state="activity.state.value" :error-message="activity.errorMessage.value" empty-message="暂无 AI 能力活动" @retry="() => { void activity.load(); }">
        <table class="activity-table">
          <caption class="visually-hidden">AI 能力通道事件（最新在前）</caption>
          <thead>
            <tr>
              <th scope="col">序号</th>
              <th scope="col">事件</th>
              <th scope="col">时间</th>
              <th scope="col">关联任务</th>
              <th scope="col">状态</th>
              <th scope="col">错误码</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="event in activity.events.value" :key="event.sequence" :data-event-sequence="event.sequence">
              <td>{{ event.sequence }}</td>
              <td><span class="status-badge" :data-tone="typeBadge(event.event_type).tone"><span aria-hidden="true">{{ typeBadge(event.event_type).shape }}</span> {{ typeBadge(event.event_type).label }}</span></td>
              <td><time :datetime="event.occurred_at">{{ event.occurred_at }}</time></td>
              <td>
                <RouterLink v-if="event.task_id" :to="{ name: 'task', params: { id: event.task_id } }">
                  {{ event.task_id.slice(0, 8) }}…
                </RouterLink>
                <span v-else aria-label="无关联任务">—</span>
              </td>
              <td>
                <span v-if="stateBadge(event.state)" class="status-badge" :data-tone="stateBadge(event.state)?.tone"><span aria-hidden="true">{{ stateBadge(event.state)?.shape }}</span> {{ stateBadge(event.state)?.label }}</span>
                <span v-else>—</span>
              </td>
              <td>
                <code v-if="event.error_code" data-error-code>{{ event.error_code }}</code>
                <span v-else>—</span>
              </td>
            </tr>
          </tbody>
        </table>
        <CursorPager
          :next-cursor="activity.nextCursor.value"
          :shown-count="activity.events.value.length"
          @next="() => { void activity.loadMore(); }"
        />
    </AsyncState>
  </section>
</template>

<style scoped>
.capability-events { display: grid; gap: 1rem; }
.page-help { margin: 0; color: var(--mfd-color-text-secondary); }
.activity-table { width: 100%; border-collapse: collapse; background: var(--mfd-color-bg-surface); border: 1px solid var(--mfd-color-border-default); border-radius: .75rem; overflow: hidden; }
.activity-table th, .activity-table td { padding: .6rem .8rem; border-bottom: 1px solid var(--mfd-color-border-subtle); text-align: left; font-size: .9rem; }
.activity-table th { background: var(--mfd-color-bg-surface-sunken); }
.activity-table code { font-size: .8rem; }

</style>
