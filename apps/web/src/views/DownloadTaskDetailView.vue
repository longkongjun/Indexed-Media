<script setup lang="ts">
/**
 * 展示单个下载任务的脱敏状态、进度和恢复分类。
 *
 * 页面只读，不渲染下载源、tracker、远端路径或远端诊断正文；事件只触发按 ID 重新读取。
 */
import type { TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { inject, onBeforeUnmount, onMounted } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useSessionStore } from "../app/session";
import AsyncState from "../components/AsyncState.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import { useDownloadTasks } from "../features/download-tasks/useDownloadTasks";
import { useProjectionEvents } from "../features/events/useProjectionEvents";
import { taskEventSourceFactoryKey } from "../features/scan-tasks/useTaskEvents";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const id = String(route.params.id);
const eventSourceFactory = inject(taskEventSourceFactoryKey, undefined);
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useDownloadTasks(client, {}, failure);
const events = useProjectionEvents({
  refresh: () => feature.loadDetail(id), eventTypes: ["download-task.changed"],
  matches: (event: TaskEventEnvelope) => event.type === "download-task.changed" && event.task_id === id,
  versionOf: (event) => event.type === "download-task.changed" ? { key: `download:${event.task_id}`, version: event.payload.projection_version } : null,
  eventSourceFactory, probeClient: client,
  onUnauthorized: () => failure(Object.assign(new Error("expired"), { status: 401 })),
});
onMounted(events.start);
onBeforeUnmount(events.stop);
</script>

<template>
  <main class="page-stack" data-download-task-detail><header class="page-header"><div><RouterLink :to="{ name: 'download-tasks' }">← 返回下载任务</RouterLink><h1>下载任务详情</h1></div></header><AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" offline-message="当前离线，保留最近任务详情" @retry="feature.loadDetail(id)"><article v-if="feature.selected.value" class="detail-card"><h2>{{ feature.selected.value.display_name }}</h2><dl><div><dt>连接</dt><dd>{{ feature.selected.value.connection_display_name }}</dd></div><div><dt>本地状态</dt><dd>{{ feature.selected.value.status }}</dd></div><div><dt>远端状态</dt><dd>{{ feature.selected.value.remote_status ?? '尚未关联' }}</dd></div><div><dt>进度</dt><dd>{{ (feature.selected.value.progress_basis_points / 100).toFixed(0) }}%</dd></div><div><dt>恢复分类</dt><dd>{{ feature.selected.value.failure_code ?? '无' }}</dd></div><div><dt>更新时间</dt><dd>{{ feature.selected.value.updated_at }}</dd></div></dl></article></AsyncState><p v-if="events.diagnostic.value" class="visually-hidden">{{ events.diagnostic.value }}</p></main>
</template>
