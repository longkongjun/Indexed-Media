<script setup lang="ts">
/**
 * 提供人工识别确认流程：查看审核上下文与证据、搜索临时候选并提交允许的决定。
 *
 * 审核 ID 来自路由，审核、证据和候选状态来自 review feature，在线可写性由连通性 store 与加载状态共同决定；
 * SSE 事件会刷新审核投影。页面不直接执行文件规划或文件变更，决定提交交由 review decision feature 处理。
 */
import type { TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { computed, inject, onBeforeUnmount, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useConnectivityStore } from "../app/connectivity";
import { useSessionStore } from "../app/session";
import AsyncState from "../components/AsyncState.vue";
import ReviewDecisionForm from "../components/ReviewDecisionForm.vue";
import ReviewEvidenceList from "../components/ReviewEvidenceList.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import { useReviewCase } from "../features/review-cases/useReviewCase";
import { useReviewDecision } from "../features/review-cases/useReviewDecision";
import { useProjectionEvents } from "../features/events/useProjectionEvents";
import { taskEventSourceFactoryKey } from "../features/scan-tasks/useTaskEvents";

const route = useRoute();
const router = useRouter();
const client = useMediaFlowClient();
const eventSourceFactory = inject(taskEventSourceFactoryKey, undefined);
const session = useSessionStore();
const connectivity = useConnectivityStore();
const id = String(route.params.id);
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useReviewCase(client, id, failure);
const online = computed(() => connectivity.state === "online" && feature.state.value.kind !== "offline");
const decision = useReviewDecision(client, feature.reviewCase, online, {
  refreshCase: feature.loadCase,
  onFailure: failure,
});
const query = ref("");
const mediaType = ref<"movie" | "tv">("movie");
const locale = ref("zh-CN");
const searchError = ref("");

async function load(): Promise<void> {
  await feature.load();
  query.value ||= feature.reviewCase.value?.title_hint ?? "";
}

async function search(): Promise<void> {
  const normalized = query.value.trim();
  if (!normalized) {
    searchError.value = "请输入候选搜索词";
    return;
  }
  searchError.value = "";
  await feature.searchCandidates({ query: normalized, mediaType: mediaType.value, locale: locale.value, limit: 20 });
}

const events = useProjectionEvents({
  refresh: load,
  eventTypes: ["review-case.updated", "task-decision.accepted"],
  matches: (event: TaskEventEnvelope) => (event.type === "review-case.updated" || event.type === "task-decision.accepted") && event.payload.case_id === id,
  versionOf: (event) => event.type === "review-case.updated" || event.type === "task-decision.accepted" ? { key: `case:${event.payload.case_id}`, version: event.payload.case_version } : null,
  eventSourceFactory,
  probeClient: client,
  onUnauthorized: () => failure(Object.assign(new Error("expired"), { status: 401 })),
});
onMounted(events.start);
onBeforeUnmount(events.stop);
</script>

<template>
  <main class="page-stack review-case-view">
    <header class="page-header">
      <div><RouterLink :to="{ name: 'tasks' }">← 返回任务中心</RouterLink><h1>人工识别确认</h1></div>
      <RouterLink v-if="feature.reviewCase.value" :to="{ name: 'task', params: { id: feature.reviewCase.value.task_id } }">查看处理任务</RouterLink>
    </header>
    <AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" offline-message="当前离线，保留已加载审核内容并暂停写操作" @retry="load">
      <template v-if="feature.reviewCase.value">
        <article class="detail-card review-summary">
          <h2><code>{{ feature.reviewCase.value.relative_path }}</code></h2>
          <dl class="compact-facts"><div><dt>级别</dt><dd>{{ feature.reviewCase.value.level }}</dd></div><div><dt>原因</dt><dd>{{ feature.reviewCase.value.reason }}</dd></div><div><dt>版本</dt><dd>{{ feature.reviewCase.value.version }}</dd></div><div><dt>最近更新</dt><dd><time :datetime="feature.reviewCase.value.updated_at">{{ feature.reviewCase.value.updated_at }}</time></dd></div></dl>
          <p v-if="feature.reviewCase.value.latest_task_decision">最近决定：{{ feature.reviewCase.value.latest_task_decision.kind }} · {{ feature.reviewCase.value.latest_task_decision.state }}</p>
        </article>

        <AsyncState :state="feature.evidenceState.value" :error-message="feature.evidenceError.value" offline-message="当前离线，显示已加载证据" @retry="feature.loadIdentification(feature.reviewCase.value.task_id)">
          <ReviewEvidenceList v-if="feature.identification.value" :detail="feature.identification.value" />
        </AsyncState>

        <section v-if="feature.reviewCase.value.allowed_actions.includes('select-provider-candidate')" class="form-card" aria-labelledby="candidate-search-heading">
          <h2 id="candidate-search-heading">搜索临时候选</h2>
          <form class="candidate-search" @submit.prevent="search">
            <label for="candidate-query">搜索词</label><input id="candidate-query" v-model="query" maxlength="200">
            <label for="candidate-media-type">媒体类型</label><select id="candidate-media-type" v-model="mediaType"><option value="movie">电影</option><option value="tv">剧集</option></select>
            <label for="candidate-locale">语言</label><select id="candidate-locale" v-model="locale"><option value="zh-CN">简体中文</option><option value="en-US">English</option></select>
            <button type="submit" :disabled="!online || feature.candidateState.value.kind === 'loading'">搜索候选</button>
          </form>
          <p v-if="searchError" role="alert">{{ searchError }}</p>
          <p v-if="feature.candidateState.value.kind === 'loading'" role="status">正在搜索候选</p>
          <p v-else-if="feature.candidateState.value.kind === 'empty'">没有找到候选，可改用重新匹配或通用视频。</p>
          <p v-else-if="feature.candidateState.value.kind === 'error'" role="alert">{{ feature.candidateError.value }}</p>
        </section>

        <p v-if="decision.message.value" :class="{ 'state-notice': true, 'state-notice--error': decision.state.value.kind === 'error' || decision.state.value.kind === 'conflict' }" role="status">{{ decision.message.value }}</p>
        <ReviewDecisionForm
          :allowed-actions="feature.reviewCase.value.allowed_actions"
          :candidates="feature.candidates.value"
          :disabled="!decision.canSubmit.value"
          :initial-title="feature.reviewCase.value.title_hint"
          @submit="decision.submit"
        />
      </template>
    </AsyncState>
    <p v-if="events.diagnostic.value" class="visually-hidden">{{ events.diagnostic.value }}</p>
  </main>
</template>
