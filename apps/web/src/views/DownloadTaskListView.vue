<script setup lang="ts">
/**
 * 展示 MediaFlow 自有下载任务，并提供安全手工创建、筛选与游标分页。
 *
 * magnet/私有 torrent URL 只存在于密码式输入和 feature 的短期恢复槽，不会进入任务卡片、错误正文或路由查询。
 */
import type { DownloadTaskListOptions, TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { computed, inject, onBeforeUnmount, onMounted, reactive, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useSessionStore } from "../app/session";
import AsyncState from "../components/AsyncState.vue";
import CursorPager from "../components/CursorPager.vue";
import ErrorSummary from "../components/ErrorSummary.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import { useDownloaderConnections } from "../features/downloader-connections/useDownloaderConnections";
import { useDownloadTasks } from "../features/download-tasks/useDownloadTasks";
import { useProjectionEvents } from "../features/events/useProjectionEvents";
import { taskEventSourceFactoryKey } from "../features/scan-tasks/useTaskEvents";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const eventSourceFactory = inject(taskEventSourceFactoryKey, undefined);
const filters = reactive({ connectionId: "", status: "", query: "" });
const statuses = ["queued", "submitting", "monitoring", "retry-wait", "completed", "failed"] as const;
const queryString = (name: string) => typeof route.query[name] === "string" && route.query[name] ? String(route.query[name]) : undefined;
const context = computed<DownloadTaskListOptions>(() => ({
  connectionId: queryString("connection_id"),
  status: statuses.includes(queryString("status") as typeof statuses[number]) ? queryString("status") as DownloadTaskListOptions["status"] : undefined,
  query: queryString("q"), cursor: queryString("cursor"),
}));
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useDownloadTasks(client, context.value, failure);
const connections = useDownloaderConnections(client, failure);

function queryFor(next: DownloadTaskListOptions): Record<string, string> {
  return Object.fromEntries(Object.entries({ connection_id: next.connectionId, status: next.status, q: next.query, cursor: next.cursor })
    .filter((entry): entry is [string, string] => typeof entry[1] === "string" && entry[1].length > 0));
}
function syncControls(): void { filters.connectionId = context.value.connectionId ?? ""; filters.status = context.value.status ?? ""; filters.query = context.value.query ?? ""; }
async function applyFilters(): Promise<void> { await router.replace({ query: queryFor({ connectionId: filters.connectionId || undefined, status: filters.status as DownloadTaskListOptions["status"], query: filters.query.trim() || undefined }) }); }
async function refresh(): Promise<void> { await feature.load(context.value); }
const events = useProjectionEvents({
  refresh, eventTypes: ["download-task.changed"],
  matches: (event: TaskEventEnvelope) => event.type === "download-task.changed",
  versionOf: (event) => event.type === "download-task.changed" ? { key: `download:${event.task_id}`, version: event.payload.projection_version } : null,
  eventSourceFactory, probeClient: client,
  onUnauthorized: () => failure(Object.assign(new Error("expired"), { status: 401 })),
});

syncControls();
onMounted(() => { void connections.load(); events.start(); });
onBeforeUnmount(events.stop);
watch(() => route.fullPath, () => { syncControls(); return refresh(); }, { immediate: true });
</script>

<template>
  <main class="page-stack" data-download-task-list>
    <header class="page-header"><div><p class="eyebrow">只管理 MediaFlow 创建的任务</p><h1>下载任务</h1></div><RouterLink class="primary-action" to="/connections/downloaders">管理连接</RouterLink></header>
    <section class="form-card" aria-labelledby="download-create-heading"><h2 id="download-create-heading">手工添加下载</h2><ErrorSummary :message="feature.formError.value?.message" /><form @submit.prevent="feature.create"><label for="download-connection">下载器连接</label><select id="download-connection" v-model="feature.form.connectionId" required><option value="" disabled>请选择</option><option v-for="item in connections.items.value" :key="item.id" :value="item.id" :disabled="!item.enabled">{{ item.display_name }}</option></select><label for="download-name">显示名</label><input id="download-name" v-model="feature.form.displayName" maxlength="512" required><label for="download-source">Magnet 或 HTTPS torrent URL</label><input id="download-source" v-model="feature.form.source" type="password" autocomplete="off" maxlength="8192" required><p class="field-help">源会加密保存，提交后立即从表单清除且不会在页面回显。</p><button class="primary-action" type="submit" :disabled="!feature.canWrite.value">创建下载任务</button></form></section>
    <form class="filter-bar task-filter" @submit.prevent="applyFilters"><label>连接<select v-model="filters.connectionId"><option value="">全部</option><option v-for="item in connections.items.value" :key="item.id" :value="item.id">{{ item.display_name }}</option></select></label><label>状态<select v-model="filters.status"><option value="">全部</option><option v-for="status in statuses" :key="status" :value="status">{{ status }}</option></select></label><label>关键字<input v-model="filters.query" maxlength="120"></label><button type="submit">应用筛选</button></form>
    <AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" empty-message="尚无下载任务" offline-message="当前离线，保留最近任务投影并禁用写入" @retry="refresh"><ul class="card-list task-card-list"><li v-for="item in feature.items.value" :key="item.id" class="resource-card"><RouterLink :to="{ name: 'download-task', params: { id: item.id } }"><strong>{{ item.display_name }}</strong><span>{{ item.connection_display_name }} · {{ item.status }} · {{ (item.progress_basis_points / 100).toFixed(0) }}%</span></RouterLink></li></ul><CursorPager :has-previous-context="Boolean(context.cursor)" :next-cursor="feature.nextCursor.value" @previous="router.push({ query: queryFor({ ...context, cursor: undefined }) })" @next="router.push({ query: queryFor({ ...context, cursor: $event }) })" /></AsyncState>
    <p v-if="events.diagnostic.value" class="visually-hidden">{{ events.diagnostic.value }}</p>
  </main>
</template>
