<script setup lang="ts">
/**
 * 展示已核对的正式 Catalog 媒体，并提供筛选、游标分页和投影刷新。
 *
 * 页面不读取候选或 ReviewCase 数据，也不构造远程图片 URL；筛选和游标来自路由查询，列表状态来自媒体 feature，
 * `catalog.media-changed` SSE 事件触发有界刷新，会话状态仅用于认证失败处理。
 */
import type { MediaItemListOptions, TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { computed, inject, onBeforeUnmount, onMounted, reactive, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useSessionStore } from "../app/session";
import AsyncState from "../components/AsyncState.vue";
import CursorPager from "../components/CursorPager.vue";
import MediaArtwork from "../components/MediaArtwork.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import { useProjectionEvents } from "../features/events/useProjectionEvents";
import { useMediaItems } from "../features/media/useMediaItems";
import { taskEventSourceFactoryKey } from "../features/scan-tasks/useTaskEvents";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const eventSourceFactory = inject(taskEventSourceFactoryKey, undefined);
const filters = reactive({ type: "", library: "", status: "", query: "" });
const mediaTypes = ["movie", "series", "generic-video"] as const;
const localStatuses = ["complete", "partial"] as const;

function stringQuery(name: string): string | undefined {
  const value = route.query[name];
  return typeof value === "string" && value.length > 0 ? value : undefined;
}
const context = computed<MediaItemListOptions>(() => ({
  type: mediaTypes.includes(stringQuery("type") as typeof mediaTypes[number]) ? stringQuery("type") as MediaItemListOptions["type"] : undefined,
  libraryId: stringQuery("library_id"),
  localStatus: localStatuses.includes(stringQuery("local_status") as typeof localStatuses[number]) ? stringQuery("local_status") as MediaItemListOptions["localStatus"] : undefined,
  query: stringQuery("q"),
  cursor: stringQuery("cursor"),
}));
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useMediaItems(client, context.value, failure);

function queryFor(next: MediaItemListOptions): Record<string, string> {
  return Object.fromEntries(Object.entries({ type: next.type, library_id: next.libraryId, local_status: next.localStatus, q: next.query, cursor: next.cursor })
    .filter((entry): entry is [string, string] => typeof entry[1] === "string" && entry[1].length > 0));
}
function syncControls(): void {
  filters.type = context.value.type ?? "";
  filters.library = context.value.libraryId ?? "";
  filters.status = context.value.localStatus ?? "";
  filters.query = context.value.query ?? "";
}
async function applyFilters(): Promise<void> {
  await router.replace({ query: queryFor({
    type: filters.type as MediaItemListOptions["type"],
    libraryId: filters.library || undefined,
    localStatus: filters.status as MediaItemListOptions["localStatus"],
    query: filters.query.trim() || undefined,
  }) });
}
async function refresh(): Promise<void> { await feature.load(context.value); }
const events = useProjectionEvents({
  refresh,
  eventTypes: ["catalog.media-changed"],
  matches: (event: TaskEventEnvelope) => event.type === "catalog.media-changed",
  versionOf: (event) => event.type === "catalog.media-changed" ? { key: `media:${event.payload.media_item_id}`, version: event.payload.projection_version } : null,
  eventSourceFactory,
  probeClient: client,
  onUnauthorized: () => failure(Object.assign(new Error("expired"), { status: 401 })),
});

syncControls();
onMounted(events.start);
onBeforeUnmount(events.stop);
watch(() => route.fullPath, () => { syncControls(); return refresh(); });
</script>

<template>
  <main class="page-stack" data-media-list>
    <header class="page-header"><div><p class="eyebrow">正式本地结果</p><h1>媒体</h1></div></header>
    <form class="filter-bar media-filter" @submit.prevent="applyFilters">
      <label>类型<select v-model="filters.type"><option value="">全部</option><option value="movie">电影</option><option value="series">剧集</option><option value="generic-video">通用视频</option></select></label>
      <label>本地状态<select v-model="filters.status"><option value="">全部</option><option value="complete">完整</option><option value="partial">部分</option></select></label>
      <label>媒体库 ID<input v-model="filters.library" maxlength="36"></label>
      <label>关键字<input v-model="filters.query" maxlength="200"></label>
      <button type="submit">应用筛选</button>
    </form>
    <AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" empty-message="尚无已整理媒体" offline-message="当前离线，保留上次正式媒体结果" @retry="refresh">
      <ul class="card-list media-card-list">
        <li v-for="item in feature.items.value" :key="item.id" class="resource-card">
          <MediaArtwork :artwork="item.artwork_ref" :title="item.title" />
          <RouterLink :to="{ name: 'media-item', params: { id: item.id }, query: queryFor(context) }"><strong>{{ item.title }}</strong><span>{{ item.type }} · {{ item.year ?? '年份未知' }} · {{ item.local_status === 'partial' ? '部分结果' : '完整' }}</span></RouterLink>
        </li>
      </ul>
      <CursorPager :has-previous-context="Boolean(context.cursor)" :next-cursor="feature.nextCursor.value" @previous="router.push({ query: queryFor({ ...context, cursor: undefined }) })" @next="router.push({ query: queryFor({ ...context, cursor: $event }) })" />
    </AsyncState>
    <p v-if="feature.state.value.kind === 'empty'" class="empty-action">处理任务完成并产生已核对本地结果后才会出现在这里。<RouterLink to="/tasks">查看任务</RouterLink></p>
    <p v-if="events.diagnostic.value" class="visually-hidden">{{ events.diagnostic.value }}</p>
  </main>
</template>
