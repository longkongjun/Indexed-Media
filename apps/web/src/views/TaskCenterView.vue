<script setup lang="ts">
/**
 * 展示处理任务中心，支持视图切换、组合筛选、游标分页和投影摘要刷新。
 *
 * 视图、筛选与游标由路由查询驱动，任务列表和摘要来自处理任务 feature；相关 SSE 事件触发有界刷新，
 * 会话状态只用于认证失败处理，页面不直接改变 Core 的任务状态。
 */
import type { TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { computed, inject, onBeforeUnmount, onMounted, reactive, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import AsyncState from "../components/AsyncState.vue";
import CursorPager from "../components/CursorPager.vue";
import { useMediaFlowClient } from "../components/injectedClient";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useSessionStore } from "../app/session";
import { useProcessingTasks } from "../features/processing-tasks/useProcessingTasks";
import type { TaskCenterContext } from "../features/processing-tasks/model";
import { useProjectionEvents } from "../features/events/useProjectionEvents";
import { taskEventSourceFactoryKey } from "../features/scan-tasks/useTaskEvents";

const route = useRoute();
const router = useRouter();
const client = useMediaFlowClient();
const eventSourceFactory = inject(taskEventSourceFactoryKey, undefined);
const session = useSessionStore();
const views = ["pending", "running", "all", "completed"] as const;
const labels = { pending: "待处理", running: "进行中", all: "全部", completed: "已完成" } as const;
const filters = reactive({ stage: "", status: "", inbox: "", query: "" });

function stringQuery(name: string): string | undefined {
  const value = route.query[name];
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

const context = computed<TaskCenterContext>(() => ({
  view: views.includes(stringQuery("view") as typeof views[number])
    ? stringQuery("view") as typeof views[number]
    : "pending",
  stage: stringQuery("stage") as TaskCenterContext["stage"],
  status: stringQuery("status") as TaskCenterContext["status"],
  inboxDirectoryId: stringQuery("inbox_directory_id"),
  query: stringQuery("q"),
  cursor: stringQuery("cursor"),
}));
const feature = useProcessingTasks(
  client,
  context.value,
  (error) => handleAuthenticatedFailure(error, session, router, route.fullPath),
);

function syncControls(): void {
  filters.stage = context.value.stage ?? "";
  filters.status = context.value.status ?? "";
  filters.inbox = context.value.inboxDirectoryId ?? "";
  filters.query = context.value.query ?? "";
}

function queryFor(next: TaskCenterContext): Record<string, string> {
  return Object.fromEntries(Object.entries({
    view: next.view,
    stage: next.stage,
    status: next.status,
    inbox_directory_id: next.inboxDirectoryId,
    q: next.query,
    cursor: next.cursor,
  }).filter((entry): entry is [string, string] => typeof entry[1] === "string" && entry[1].length > 0));
}

async function applyFilters(): Promise<void> {
  await router.replace({ query: queryFor({
    view: context.value.view,
    stage: filters.stage as TaskCenterContext["stage"],
    status: filters.status as TaskCenterContext["status"],
    inboxDirectoryId: filters.inbox || undefined,
    query: filters.query.trim() || undefined,
  }) });
}

async function load(): Promise<void> { await feature.load(context.value); }
const events = useProjectionEvents({
  refresh: load,
  eventTypes: ["processing-task.state-changed", "processing-task.identification-decided", "task-decision.accepted"],
  matches: (event: TaskEventEnvelope) => event.type === "processing-task.state-changed" || event.type === "processing-task.identification-decided" || event.type === "task-decision.accepted",
  versionOf: (event) => event.type === "task-decision.accepted" ? { key: `case:${event.payload.case_id}`, version: event.payload.case_version } : null,
  eventSourceFactory,
  probeClient: client,
  onUnauthorized: () => handleAuthenticatedFailure(Object.assign(new Error("expired"), { status: 401 }), session, router, route.fullPath),
});
syncControls();
onMounted(events.start);
onBeforeUnmount(events.stop);
watch(() => route.fullPath, () => { syncControls(); return load(); });
</script>

<template>
  <main class="page-stack" data-task-center>
    <header class="page-header"><div><p class="eyebrow">单文件处理</p><h1>任务中心</h1></div><RouterLink to="/scan-tasks">查看扫描任务</RouterLink></header>
    <nav class="view-tabs" aria-label="任务视图">
      <RouterLink v-for="view in views" :key="view" :to="{ query: queryFor({ ...context, view, cursor: undefined }) }" :aria-current="context.view === view ? 'page' : undefined">
        {{ labels[view] }}<span v-if="feature.summary.value">{{ feature.summary.value[view] }}</span>
      </RouterLink>
    </nav>
    <form class="filter-bar task-filter" @submit.prevent="applyFilters">
      <label>阶段<select v-model="filters.stage"><option value="">全部</option><option value="identification">识别</option><option value="planning">规划</option><option value="file-operation">文件操作</option><option value="nfo">NFO</option><option value="completion">完成</option></select></label>
      <label>状态<select v-model="filters.status"><option value="">全部</option><option value="queued">等待中</option><option value="running">进行中</option><option value="waiting-confirmation">等待人工确认</option><option value="paused">已暂停</option><option value="partial-success">部分成功</option><option value="completed">已完成</option><option value="failed">失败</option><option value="cancelled">已取消</option></select></label>
      <label>收件目录 ID<input v-model="filters.inbox" maxlength="36"></label>
      <label>关键字<input v-model="filters.query" maxlength="200" placeholder="相对路径或任务 ID"></label>
      <button type="submit">应用筛选</button>
    </form>
    <AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" empty-message="当前视图没有处理任务" offline-message="当前离线，保留上次服务端结果与摘要" @retry="load">
      <ul class="card-list task-card-list">
        <li v-for="item in feature.tasks.value" :key="item.id" class="resource-card">
          <RouterLink :to="{ name: 'task', params: { id: item.id }, query: queryFor(context) }"><strong>{{ item.relative_path }}</strong></RouterLink>
          <dl class="compact-facts"><div><dt>状态</dt><dd>{{ item.status }}</dd></div><div><dt>阶段</dt><dd>{{ item.stage }}</dd></div><div><dt>原因</dt><dd>{{ item.reason ?? '—' }}</dd></div><div><dt>决定检查点</dt><dd>{{ item.decision_checkpoint ?? '—' }}</dd></div></dl>
          <p><time :datetime="item.updated_at">{{ item.updated_at }}</time></p>
          <p v-if="item.allowed_actions.length">可操作：{{ item.allowed_actions.join('、') }}</p>
        </li>
      </ul>
      <CursorPager :has-previous-context="Boolean(context.cursor)" :next-cursor="feature.nextCursor.value" @previous="router.push({ query: queryFor({ ...context, cursor: undefined }) })" @next="router.push({ query: queryFor({ ...context, cursor: $event }) })" />
    </AsyncState>
    <p v-if="events.diagnostic.value" class="visually-hidden">{{ events.diagnostic.value }}</p>
  </main>
</template>
