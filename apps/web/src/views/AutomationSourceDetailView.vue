<script setup lang="ts">
/** 单个来源的安全详情、版本化替换、轮换和删除入口。 */
import { computed, onMounted } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useSessionStore } from "../app/session";
import AsyncState from "../components/AsyncState.vue";
import ErrorSummary from "../components/ErrorSummary.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import SecretOncePanel from "../features/source-automation/SecretOncePanel.vue";
import { useAutomationSources } from "../features/source-automation/useAutomationSources";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const id = computed(() => String(route.params.id));
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useAutomationSources(client, failure);

async function remove(): Promise<void> {
  if (await feature.remove()) await router.push({ name: "automation-sources" });
}

onMounted(() => feature.loadDetail(id.value));
</script>

<template>
  <main class="page-stack" data-automation-source-detail>
    <header class="page-header"><div><p class="eyebrow">脱敏来源详情</p><h1>{{ feature.selected.value?.display_name ?? '自动来源' }}</h1></div><RouterLink to="/automation/sources">返回工作台</RouterLink></header>
    <AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" offline-message="当前离线，保留来源投影并禁用更新、轮换和删除" @retry="feature.loadDetail(id)">
      <section v-if="feature.selected.value" class="detail-card"><h2>安全投影</h2><dl>
        <div><dt>类型</dt><dd>{{ feature.selected.value.kind }}</dd></div><div><dt>状态</dt><dd>{{ feature.selected.value.enabled ? '已启用' : '已停用' }} · {{ feature.selected.value.health }}</dd></div><div><dt>Endpoint 摘要</dt><dd class="safe-value">{{ feature.selected.value.endpoint_summary ?? '本地映射' }}</dd></div><div><dt>配置版本</dt><dd>{{ feature.selected.value.config_version }}</dd></div><div v-if="feature.selected.value.secret_fingerprint"><dt>Secret 指纹</dt><dd>{{ feature.selected.value.secret_fingerprint }}</dd></div>
      </dl><RouterLink :to="{ name: 'automation-sources', query: { source_id: id } }">查看此来源事件</RouterLink></section>
      <section v-if="feature.selected.value" class="form-card" aria-labelledby="source-update-heading"><h2 id="source-update-heading">更新来源</h2><ErrorSummary :message="feature.formError.value?.message" :focus-key="feature.errorKey.value" /><p v-if="feature.conflictProjection.value" role="status">服务器配置已到 v{{ feature.conflictProjection.value.config_version }}；敏感输入已清除。</p><form @submit.prevent="feature.update">
        <label for="source-detail-name">显示名</label><input id="source-detail-name" v-model="feature.form.displayName" maxlength="120" required>
        <label v-if="feature.form.kind === 'rss'" for="source-detail-feed">重新输入 Feed URL</label><input v-if="feature.form.kind === 'rss'" id="source-detail-feed" v-model="feature.form.feedUrl" type="url" autocomplete="off" required>
        <label class="check-field"><input v-model="feature.form.enabled" type="checkbox">启用</label>
        <button v-if="feature.form.kind === 'rss'" type="button" :disabled="!feature.canWrite.value" @click="feature.testCandidate">仅测试，不保存</button>
        <p v-if="feature.candidateResult.value" role="status">测试结果：{{ feature.candidateResult.value.reachable ? '可连接' : '不可连接' }} · {{ feature.candidateResult.value.health }}</p>
        <button class="primary-action" type="submit" :disabled="!feature.canSave.value">保存更新</button>
      </form><div class="detail-actions"><button v-if="feature.selected.value.kind === 'webhook'" type="button" :disabled="!feature.canWrite.value" @click="feature.rotateSecret">轮换 Webhook secret</button><button type="button" :disabled="!feature.canWrite.value || feature.selected.value.enabled" @click="remove">删除已停用来源</button></div></section>
      <SecretOncePanel v-if="feature.secretReceipt.value" :receipt="feature.secretReceipt.value" @copied="feature.secretCopied.value = $event" @dismiss="feature.dismissSecret()" />
    </AsyncState>
  </main>
</template>
