<script setup lang="ts">
/** 一个 automation event 的脱敏详情和服务端授权恢复动作。 */
import { computed, onMounted } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useSessionStore } from "../app/session";
import AsyncState from "../components/AsyncState.vue";
import ErrorSummary from "../components/ErrorSummary.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import { useAutomationEvents } from "../features/source-automation/useAutomationEvents";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const id = computed(() => String(route.params.id));
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useAutomationEvents(client, {}, failure);
onMounted(() => feature.loadDetail(id.value));
</script>

<template>
  <main class="page-stack" data-automation-event-detail>
    <header class="page-header"><div><p class="eyebrow">可恢复事件</p><h1>{{ feature.selected.value?.source_display_name ?? '自动化事件' }}</h1></div><RouterLink to="/automation/sources">返回工作台</RouterLink></header>
    <AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" offline-message="当前离线，保留事件真值并禁用重试和取消" @retry="feature.loadDetail(id)">
      <ErrorSummary :message="feature.actionError.value?.message" />
      <section v-if="feature.selected.value" class="detail-card"><h2>事件真值</h2><dl>
        <div><dt>动作</dt><dd>{{ feature.selected.value.action }}</dd></div><div><dt>状态</dt><dd>{{ feature.selected.value.status }}</dd></div><div><dt>尝试</dt><dd>{{ feature.selected.value.attempt_count }}</dd></div><div><dt>结果</dt><dd>{{ feature.resultText(feature.selected.value) }}</dd></div><div v-if="feature.selected.value.failure_code"><dt>稳定错误</dt><dd class="safe-value">{{ feature.selected.value.failure_code }}</dd></div><div v-if="feature.selected.value.downstream_id"><dt>下游关联</dt><dd>{{ feature.selected.value.downstream_kind }} · {{ feature.selected.value.downstream_id }}</dd></div>
      </dl><p v-if="feature.selected.value.downstream_id" class="field-help">下游事实已经提交，取消不会删除下载任务、处理任务或文件。</p><div class="detail-actions"><button type="button" :disabled="!feature.canRetry(feature.selected.value)" @click="feature.retry(feature.selected.value)">重试原事件</button><button type="button" :disabled="!feature.canCancel(feature.selected.value)" @click="feature.cancel(feature.selected.value)">取消未提交动作</button></div></section>
    </AsyncState>
  </main>
</template>
