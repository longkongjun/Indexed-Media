<script setup lang="ts">
/**
 * 侧栏内嵌的 AI 通道会话摘要。
 *
 * 只展示 Core 事实的脱敏投影（`media-capability.v1` 事件流最近几条）：类型、序号与
 * 关联任务入口。组件没有写路径，也不渲染任何输入负载、凭据或宿主路径；完整活动在
 * `/capability-events` 页面查看。
 */
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import type { CapabilityActivityEvent, MediaFlowClient } from "@mediaflow/api-client-ts";
import { capabilityEventBadge } from "../components/dictionaries";
import { useMediaFlowClient } from "../components/injectedClient";

const client: MediaFlowClient = useMediaFlowClient();
const router = useRouter();
const recent = ref<CapabilityActivityEvent[]>([]);
const loading = ref(true);
const failed = ref(false);

function typeLabel(eventType: string): string {
  return capabilityEventBadge(eventType).label;
}

/** 审批等待事件（事件类型或投影状态）：升级为审批入口（approval-flow ③：会话只给深链，无审批按钮）。 */
function needsApproval(event: CapabilityActivityEvent): boolean {
  return event.event_type === "task.waiting_approval" || event.state === "needs_approval" || event.state === "waiting_approval";
}

function openTask(taskId: string): void {
  void router.push({ name: "task", params: { id: taskId } });
}

onMounted(() => {
  void client
    .listCapabilityEvents({ limit: 3 })
    .then((page) => {
      recent.value = page.items;
      loading.value = false;
    })
    .catch(() => {
      // 摘要失败不打扰管理员：折叠为不可用状态，完整页面可重试。
      failed.value = true;
      loading.value = false;
    });
});
</script>

<template>
  <section class="ai-session-panel" aria-labelledby="ai-session-heading">
    <h2 id="ai-session-heading">AI 会话</h2>
    <p class="panel-hint">AI 摘要，以任务详情页为准</p>
    <p v-if="loading" class="panel-hint" role="status">正在加载…</p>
    <p v-else-if="failed" class="panel-hint" role="status">AI 通道摘要暂时不可用</p>
    <p v-else-if="recent.length === 0" class="panel-hint">暂无 AI 通道活动</p>
    <ul v-else class="session-list" data-ai-session-list>
      <li v-for="event in recent" :key="event.sequence" :data-event-sequence="event.sequence">
        <button
          v-if="event.task_id"
          type="button"
          class="session-item"
          :class="{ 'session-item--approval': needsApproval(event) }"
          :data-approval-entry="needsApproval(event) || undefined"
          @click="openTask(event.task_id)"
        >
          <span class="session-type">{{ typeLabel(event.event_type) }}</span>
          <span v-if="needsApproval(event)" class="session-approval-link">到任务审批 →</span>
          <span class="session-meta">#{{ event.sequence }}</span>
        </button>
        <span v-else class="session-item session-item--plain">
          <span class="session-type">{{ typeLabel(event.event_type) }}</span>
          <span class="session-meta">#{{ event.sequence }}</span>
        </span>
      </li>
    </ul>
    <RouterLink to="/capability-events" class="panel-link">查看全部 AI 活动</RouterLink>
  </section>
</template>

<style scoped>
.ai-session-panel { margin-top: auto; padding: .75rem .5rem .25rem; border-top: 1px solid var(--mfd-color-bg-inverse-hover); display: grid; gap: .4rem; }
.ai-session-panel h2 { margin: 0; font-size: .8rem; font-weight: 650; letter-spacing: .02em; }
.panel-hint { margin: 0; font-size: .75rem; opacity: .8; }
.session-list { margin: 0; padding: 0; list-style: none; display: grid; gap: .25rem; }
.session-item { display: flex; width: 100%; justify-content: space-between; align-items: baseline; gap: .5rem; padding: .3rem .4rem; border: 0; border-radius: .45rem; background: transparent; color: inherit; font: inherit; font-size: .78rem; text-align: left; cursor: pointer; }
.session-item:hover, .session-item:focus-visible { background: var(--mfd-color-bg-inverse-hover); outline: none; }
.session-item--plain { cursor: default; }
.session-item--approval { box-shadow: inset 2px 0 0 var(--mfd-color-border-focus); }
.session-approval-link { flex: none; font-size: .7rem; }
.session-type { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.session-meta { flex: none; opacity: .7; font-size: .7rem; }
.panel-link { font-size: .78rem; }
</style>
