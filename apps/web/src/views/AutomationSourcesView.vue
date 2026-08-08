<script setup lang="ts">
/** 自动来源、事件和本地增强器的独立加载工作台。 */
import type { AutomationEventListOptions } from "@mediaflow/api-client-ts";
import { computed, onMounted, reactive } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useSessionStore } from "../app/session";
import AsyncState from "../components/AsyncState.vue";
import CursorPager from "../components/CursorPager.vue";
import ErrorSummary from "../components/ErrorSummary.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import { useDownloaderConnections } from "../features/downloader-connections/useDownloaderConnections";
import { useInboxDirectories } from "../features/inbox-directories/useInboxDirectories";
import SecretOncePanel from "../features/source-automation/SecretOncePanel.vue";
import { useAutomationEvents } from "../features/source-automation/useAutomationEvents";
import { useAutomationSources } from "../features/source-automation/useAutomationSources";
import { useIdentificationEnhancer } from "../features/source-automation/useIdentificationEnhancer";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const sources = useAutomationSources(client, failure);
const events = useAutomationEvents(client, {}, failure);
const enhancer = useIdentificationEnhancer(client, failure);
const connections = useDownloaderConnections(client, failure);
const inboxes = useInboxDirectories(client, failure);
const eventFilters = reactive({ status: "", action: "" });
const eventContext = computed<AutomationEventListOptions>(() => ({
  sourceId: typeof route.query.source_id === "string" ? route.query.source_id : undefined,
  status: eventFilters.status as AutomationEventListOptions["status"] || undefined,
  action: eventFilters.action as AutomationEventListOptions["action"] || undefined,
}));
const workspaceOffline = computed(() => [sources.state.value, events.state.value, enhancer.state.value]
  .some((state) => state.kind === "offline"));

function sourceKind(kind: string): string {
  return { rss: "RSS / Atom", webhook: "签名 Webhook", "download-completion": "下载完成映射" }[kind] ?? kind;
}

async function applyEventFilters(): Promise<void> {
  await events.load(eventContext.value);
}

onMounted(() => {
  void sources.load();
  void events.load();
  void enhancer.load();
  void connections.load();
  void inboxes.load();
});
</script>

<template>
  <main class="page-stack automation-workspace" data-automation-sources>
    <header class="page-header">
      <div><p class="eyebrow">有界入口 · 可恢复执行</p><h1>来源自动化</h1></div>
      <RouterLink class="primary-action" to="/downloads">查看下载任务</RouterLink>
    </header>

    <section class="automation-entry-grid" aria-label="创建入口">
      <button type="button" @click="sources.form.kind = 'rss'">RSS / Atom<span>测试 feed 后保存</span></button>
      <button type="button" @click="sources.form.kind = 'webhook'">签名 Webhook<span>固定动作与一次性 secret</span></button>
      <button type="button" @click="sources.form.kind = 'download-completion'">下载完成映射<span>连接到已注册收件目录</span></button>
      <a href="#identification-enhancer">本地识别增强<span>默认关闭，失败安全回退</span></a>
    </section>

    <section class="form-card" aria-labelledby="source-create-heading">
      <h2 id="source-create-heading">添加 {{ sourceKind(sources.form.kind) }}</h2>
      <ErrorSummary :message="sources.formError.value?.message" :field="sources.formError.value?.field" :focus-key="sources.errorKey.value" />
      <p v-if="sources.conflictProjection.value" class="state-notice state-notice--error" role="status">
        服务器已有配置版本 {{ sources.conflictProjection.value.config_version }}；已保留显示名等非敏感草稿并清除敏感输入，请核对后重试。
      </p>
      <form @submit.prevent="sources.save">
        <fieldset class="source-kind-fieldset"><legend>来源类型</legend>
          <label><input v-model="sources.form.kind" type="radio" value="rss">RSS / Atom</label>
          <label><input v-model="sources.form.kind" type="radio" value="webhook">签名 Webhook</label>
          <label><input v-model="sources.form.kind" type="radio" value="download-completion">下载完成映射</label>
        </fieldset>
        <label for="automation-source-name">显示名</label><input id="automation-source-name" v-model="sources.form.displayName" maxlength="120" required>
        <template v-if="sources.form.kind === 'rss'">
          <label for="automation-feed-url">Feed URL</label><input id="automation-feed-url" v-model="sources.form.feedUrl" type="url" autocomplete="off" maxlength="2048" required aria-describedby="automation-feed-help">
          <p id="automation-feed-help" class="field-help">仅用于测试/保存，不会回填到来源卡片、事件或浏览器持久状态。</p>
          <label for="automation-rss-downloader">下载器连接</label><select id="automation-rss-downloader" v-model="sources.form.downloaderConnectionId" required><option value="" disabled>请选择</option><option v-for="item in connections.items.value" :key="item.id" :value="item.id">{{ item.display_name }}</option></select>
          <label for="automation-poll-interval">轮询间隔（秒）</label><input id="automation-poll-interval" v-model.number="sources.form.pollIntervalSeconds" type="number" min="60" max="86400" required>
        </template>
        <template v-else-if="sources.form.kind === 'webhook'">
          <fieldset><legend>允许动作</legend>
            <label class="check-field"><input v-model="sources.form.allowedActions" type="checkbox" value="download.create">创建下载</label>
            <label class="check-field"><input v-model="sources.form.allowedActions" type="checkbox" value="inbox.reconcile">收件对账</label>
          </fieldset>
          <p class="field-help">Core 生成 secret；创建后只显示一次，不接受脚本或任意 action。</p>
        </template>
        <template v-else>
          <label for="automation-completion-downloader">下载器连接</label><select id="automation-completion-downloader" v-model="sources.form.downloaderConnectionId" required><option value="" disabled>请选择</option><option v-for="item in connections.items.value" :key="item.id" :value="item.id">{{ item.display_name }}</option></select>
          <label for="automation-completion-inbox">收件目录</label><select id="automation-completion-inbox" v-model="sources.form.inboxDirectoryId" required><option value="" disabled>请选择</option><option v-for="item in inboxes.directories.value" :key="item.id" :value="item.id">{{ item.relative_path }}</option></select>
          <p class="field-help">不输入或显示下载器远端保存路径。</p>
        </template>
        <label class="check-field"><input v-model="sources.form.enabled" type="checkbox">保存后启用</label>
        <div class="detail-actions">
          <button v-if="sources.form.kind === 'rss'" type="button" :disabled="!sources.canWrite.value || workspaceOffline" @click="sources.testCandidate">仅测试，不保存</button>
          <button class="primary-action" type="submit" :disabled="!sources.canSave.value || workspaceOffline">保存来源</button>
        </div>
      </form>
      <p v-if="sources.candidateResult.value" role="status" aria-live="polite">
        测试结果：{{ sources.candidateResult.value.reachable ? '可读取' : '不可读取' }} · {{ sources.candidateResult.value.detected_format ?? sources.candidateResult.value.failure_code }} · 接受 {{ sources.candidateResult.value.item_count }} / 忽略 {{ sources.candidateResult.value.ignored_item_count }}
      </p>
    </section>

    <SecretOncePanel v-if="sources.secretReceipt.value" :receipt="sources.secretReceipt.value" @copied="sources.secretCopied.value = $event" @dismiss="sources.dismissSecret()" />

    <section aria-labelledby="source-list-heading"><h2 id="source-list-heading">已配置来源</h2>
      <AsyncState :state="sources.state.value" :error-message="sources.errorMessage.value" empty-message="尚未配置自动来源" offline-message="当前离线，保留最近来源投影并禁用测试、保存、轮换与删除" @retry="sources.load()">
        <ul class="card-list automation-card-list"><li v-for="item in sources.items.value" :key="item.id" class="resource-card">
          <RouterLink :to="{ name: 'automation-source', params: { id: item.id } }"><strong>{{ item.display_name }}</strong><span>{{ sourceKind(item.kind) }} · {{ item.enabled ? '已启用' : '已停用' }} · {{ item.health }}</span><small>{{ item.endpoint_summary ?? '本地映射' }} · 配置 v{{ item.config_version }}</small></RouterLink>
          <p v-if="sources.sourceErrors[item.id]" role="alert">{{ sources.sourceErrors[item.id] }}</p>
          <button type="button" @click="sources.refreshSource(item.id)">刷新此来源</button>
        </li></ul>
        <CursorPager :has-previous-context="false" :next-cursor="sources.nextCursor.value" @next="sources.load($event)" />
      </AsyncState>
    </section>

    <section id="identification-enhancer" class="form-card" aria-labelledby="enhancer-heading">
      <h2 id="enhancer-heading">本地识别增强</h2>
      <p><strong>默认关闭。</strong>仅发送 basename、最多两个父级段和基础 parser 字段；模型只提供提示，失败会回退基础识别。</p>
      <AsyncState :state="enhancer.state.value" :error-message="enhancer.errorMessage.value" offline-message="当前离线，保留模型草稿并禁用测试与保存" @retry="enhancer.load()">
        <ErrorSummary :message="enhancer.formError.value?.message" :focus-key="enhancer.errorKey.value" />
        <p v-if="enhancer.conflictProjection.value" role="status">服务器配置版本 {{ enhancer.conflictProjection.value.config_version }}，请比较后重试。</p>
        <form @submit.prevent="enhancer.save">
          <label for="enhancer-url">Ollama 地址</label><input id="enhancer-url" v-model="enhancer.form.base_url" type="url" maxlength="255" required>
          <label for="enhancer-model">模型名</label><input id="enhancer-model" v-model="enhancer.form.model" maxlength="128" required>
          <label for="enhancer-timeout">超时（毫秒）</label><input id="enhancer-timeout" v-model.number="enhancer.form.timeout_ms" type="number" min="100" max="30000" required>
          <label class="check-field"><input v-model="enhancer.form.enabled" type="checkbox">启用本地提示</label>
          <div class="detail-actions"><button type="button" :disabled="!enhancer.canWrite.value || workspaceOffline" @click="enhancer.testCandidate">仅测试，不保存</button><button class="primary-action" type="submit" :disabled="!enhancer.canSave.value || workspaceOffline">保存模型配置</button></div>
        </form>
        <p role="status" aria-live="polite">{{ enhancer.fallbackMessage.value }}</p>
      </AsyncState>
    </section>

    <section aria-labelledby="automation-event-heading"><h2 id="automation-event-heading">自动化事件</h2>
      <form class="filter-bar" @submit.prevent="applyEventFilters"><label>状态<select v-model="eventFilters.status"><option value="">全部</option><option v-for="value in ['pending','running','retry-wait','completed','failed','cancelled']" :key="value" :value="value">{{ value }}</option></select></label><label>动作<select v-model="eventFilters.action"><option value="">全部</option><option value="create-download">创建下载</option><option value="reconcile-inbox">收件对账</option></select></label><button type="submit">应用筛选</button></form>
      <AsyncState :state="events.state.value" :error-message="events.errorMessage.value" empty-message="来源尚无事件" offline-message="当前离线，保留最近事件并禁用重试和取消" @retry="events.load(eventContext)">
        <ul class="card-list automation-card-list"><li v-for="item in events.items.value" :key="item.id" class="resource-card"><RouterLink :to="{ name: 'automation-event', params: { id: item.id } }"><strong>{{ item.source_display_name }}</strong><span>{{ item.action }} · {{ item.status }} · attempt {{ item.attempt_count }}</span><small>{{ events.resultText(item) }}<template v-if="item.failure_code"> · {{ item.failure_code }}</template></small></RouterLink></li></ul>
        <CursorPager :has-previous-context="Boolean(events.context.value.cursor)" :next-cursor="events.nextCursor.value" @next="events.load({ ...eventContext, cursor: $event })" />
      </AsyncState>
    </section>
  </main>
</template>
