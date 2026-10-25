<script setup lang="ts">
/**
 * 展示已核对的正式 Catalog 媒体，并提供筛选、游标分页和投影刷新。
 *
 * 页面不读取候选或 ReviewCase 数据，也不构造远程图片 URL；筛选和游标来自路由查询，列表状态来自媒体 feature，
 * `catalog.media-changed` SSE 事件触发有界刷新，会话状态仅用于认证失败处理。
 *
 * 设计组件来源 `@indexed-design/ui-vue`（IndexedThemeProvider 包裹全树）：Button（C01）与 RequestState（C12）；
 * 媒体网格卡（C14）是页面组合模式，不套用带边框的通用盒卡，见 docs/development/indexed-design-adoption.md。
 */
import type { MediaItemListOptions, TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { Button, RequestState } from "@indexed-design/ui-vue";
import { computed, inject, onBeforeUnmount, onMounted, reactive, watch, nextTick } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useSessionStore } from "../app/session";
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
const hasFilters = computed(() => Boolean(context.value.type || context.value.libraryId || context.value.localStatus || context.value.query));
async function clearFilters(): Promise<void> {
  await router.replace({ query: {} });
  await nextTick();
  document.querySelector<HTMLInputElement>('[data-media-list] input[maxlength="200"]')?.focus();
}
async function refresh(restoreFocus = false): Promise<void> {
  await feature.load(context.value);
  await nextTick();
  if (restoreFocus) {
    const target = feature.state.value.kind === "error" || feature.state.value.kind === "offline"
      ? '[data-media-list] [data-media-request] button' : '[data-media-list] input[maxlength="200"]';
    document.querySelector<HTMLElement>(target)?.focus();
  }
}
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
      <Button html-type="submit">应用筛选</Button>
    </form>
    <RequestState data-media-request :state="feature.state.value" :error-message="feature.errorMessage.value" :empty-message="hasFilters ? '无匹配结果，请调整或清除筛选' : '尚无已整理媒体'" offline-message="当前离线，保留上次正式媒体结果" @retry="refresh(true)">
      <template #empty><Button v-if="hasFilters" @click="clearFilters">清除筛选</Button></template>
      <ul class="card-list media-card-list">
        <li v-for="item in feature.items.value" :key="item.id">
          <article class="media-card">
            <MediaArtwork :artwork="item.artwork_ref" :title="item.title" decorative />
            <div class="media-card__body">
              <div class="media-card__title" :title="item.title"><RouterLink :to="{ name: 'media-item', params: { id: item.id }, query: queryFor(context) }">{{ item.title }}</RouterLink></div>
              <p class="media-card__subtitle">{{ item.type }} · {{ item.year ?? '年份未知' }} · {{ item.local_status === 'partial' ? '部分结果' : '完整' }}</p>
            </div>
          </article>
        </li>
      </ul>
      <CursorPager :has-previous-context="Boolean(context.cursor)" :next-cursor="feature.nextCursor.value" @previous="router.push({ query: queryFor({ ...context, cursor: undefined }) })" @next="router.push({ query: queryFor({ ...context, cursor: $event }) })" />
    </RequestState>
    <p v-if="feature.state.value.kind === 'empty' && !hasFilters" class="empty-action">处理任务完成并产生已核对本地结果后才会出现在这里。<RouterLink to="/tasks">查看任务</RouterLink></p>
    <p v-if="events.diagnostic.value" class="visually-hidden">{{ events.diagnostic.value }}</p>
  </main>
</template>

<style scoped>
/* C14 媒体网格卡的页面级组合：2:3 封面 + 两行标题 + 元信息；布局 token 暂用 --mfd-*（与 Indexed 同一 v2.0.0 基线值），见接入文档过渡策略。 */
.media-card { display: grid; align-content: start; gap: var(--mfd-space-2); min-width: 0; font-family: var(--mfd-font-family-ui); color: var(--mfd-color-text-primary); }
.media-card__body { min-width: 0; overflow-wrap: anywhere; }
.media-card__title { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; overflow: hidden; font-size: var(--mfd-font-size-base); line-height: var(--mfd-font-line-height-normal); font-weight: var(--mfd-font-weight-semibold); }
.media-card__title a { display: block; min-height: var(--mfd-icon-size-touch-min); color: inherit; text-decoration: none; }
.media-card__title a:focus-visible { outline: var(--mfd-stroke-focus-ring) solid var(--mfd-color-border-focus); outline-offset: -3px; }
.media-card__subtitle { margin: 0; font-size: var(--mfd-font-size-xs); line-height: var(--mfd-font-line-height-normal); color: var(--mfd-color-text-secondary); }
</style>
