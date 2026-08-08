<script setup lang="ts">
/**
 * 展示单个已核对正式 Catalog 媒体的元数据、本地版本、层级和 NFO 状态。
 *
 * 页面不导入候选或 ReviewCase 数据，也不构造远程图片 URL；媒体 ID 和返回上下文来自路由，详情状态来自媒体 feature，
 * 对应的 `catalog.media-changed` SSE 事件会刷新当前投影。
 */
import type { TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { inject, onBeforeUnmount, onMounted } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useSessionStore } from "../app/session";
import AsyncState from "../components/AsyncState.vue";
import MediaArtwork from "../components/MediaArtwork.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import { useProjectionEvents } from "../features/events/useProjectionEvents";
import { useMediaItem } from "../features/media/useMediaItem";
import { taskEventSourceFactoryKey } from "../features/scan-tasks/useTaskEvents";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const eventSourceFactory = inject(taskEventSourceFactoryKey, undefined);
const id = String(route.params.id);
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useMediaItem(client, id, failure);
const backQuery = { ...route.query };
const nfoLabels = { "not-requested": "未请求", complete: "完成", partial: "部分完成", failed: "生成失败" } as const;
const events = useProjectionEvents({
  refresh: feature.load,
  eventTypes: ["catalog.media-changed"],
  matches: (event: TaskEventEnvelope) => event.type === "catalog.media-changed" && event.payload.media_item_id === id,
  versionOf: (event) => event.type === "catalog.media-changed" ? { key: `media:${event.payload.media_item_id}`, version: event.payload.projection_version } : null,
  eventSourceFactory,
  probeClient: client,
  onUnauthorized: () => failure(Object.assign(new Error("expired"), { status: 401 })),
});
onMounted(events.start);
onBeforeUnmount(events.stop);
</script>

<template>
  <main class="page-stack" data-media-detail>
    <header class="page-header"><div><RouterLink :to="{ name: 'media', query: backQuery }">← 返回媒体</RouterLink><h1>媒体详情</h1></div></header>
    <AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" offline-message="当前离线，保留最近正式媒体详情" @retry="feature.load">
      <template v-if="feature.detail.value">
        <article class="detail-card media-detail-heading">
          <MediaArtwork :artwork="feature.detail.value.item.artwork_ref" :title="feature.detail.value.item.title" />
          <div><h2>{{ feature.detail.value.item.title }}</h2><p>{{ feature.detail.value.item.type }} · {{ feature.detail.value.item.year ?? '年份未知' }} · {{ feature.detail.value.item.local_status === 'partial' ? '部分结果' : '完整' }}</p><p>NFO：{{ nfoLabels[feature.detail.value.nfo_status] }}</p></div>
        </article>
        <section aria-labelledby="metadata-heading"><h2 id="metadata-heading">元数据</h2><dl class="metadata-list"><div v-for="field in feature.detail.value.metadata" :key="field.field"><dt>{{ field.field }}</dt><dd v-if="field.state === 'present'">{{ field.value }} <small>来源：{{ field.source_type ?? '未知' }}</small></dd><dd v-else>元数据缺失</dd></div></dl></section>
        <section aria-labelledby="versions-heading"><h2 id="versions-heading">本地版本与文件</h2><article v-for="version in feature.detail.value.versions" :key="version.id" class="resource-card media-version"><h3>{{ version.label ?? '默认版本' }}</h3><ul><li v-for="file in version.files" :key="file.id"><code>{{ file.current_relative_path }}</code><small>来源：{{ file.source_relative_path }} · {{ file.size_bytes }} B</small></li></ul></article><p v-if="feature.detail.value.versions.length === 0">尚无本地版本文件。</p></section>
        <section aria-labelledby="hierarchy-heading"><h2 id="hierarchy-heading">媒体层级</h2><ol class="media-hierarchy"><li v-for="child in feature.detail.value.children" :key="child.id"><strong>{{ child.title }}</strong><span>{{ child.type }} · 顺序 {{ child.ordinal }}</span><small v-if="child.parent_id">父级 {{ child.parent_id }}</small></li></ol><p v-if="feature.detail.value.children.length === 0">没有子级。</p></section>
        <nav v-if="feature.detail.value.related_task_ids.length" class="detail-actions" aria-label="相关任务"><RouterLink v-for="taskId in feature.detail.value.related_task_ids" :key="taskId" :to="{ name: 'task', params: { id: taskId } }">查看相关任务</RouterLink></nav>
      </template>
    </AsyncState>
    <p v-if="events.diagnostic.value" class="visually-hidden">{{ events.diagnostic.value }}</p>
  </main>
</template>
