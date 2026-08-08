<script setup lang="ts">
/**
 * 展示并版本化更新一个脱敏下载器连接。
 *
 * 页面不回填任何凭据；更新必须重新输入凭据，删除只移除本地配置且会尊重活动任务冲突。
 */
import { onMounted } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useSessionStore } from "../app/session";
import AsyncState from "../components/AsyncState.vue";
import ErrorSummary from "../components/ErrorSummary.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import { useDownloaderConnections } from "../features/downloader-connections/useDownloaderConnections";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const id = String(route.params.id);
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useDownloaderConnections(client, failure);

async function remove(): Promise<void> {
  if (await feature.remove()) await router.push({ name: "downloader-connections" });
}
onMounted(() => feature.loadDetail(id));
</script>

<template>
  <main class="page-stack" data-downloader-connection-detail>
    <header class="page-header"><div><RouterLink :to="{ name: 'downloader-connections' }">← 返回下载器连接</RouterLink><h1>连接详情</h1></div></header>
    <AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" offline-message="当前离线，保留最近连接投影并禁用写入" @retry="feature.loadDetail(id)">
      <template v-if="feature.selected.value">
        <article class="detail-card"><h2>{{ feature.selected.value.display_name }}</h2><dl><div><dt>类型</dt><dd>{{ feature.selected.value.kind }}</dd></div><div><dt>地址</dt><dd>{{ feature.selected.value.base_url }}</dd></div><div><dt>健康</dt><dd>{{ feature.selected.value.health }}</dd></div><div><dt>配置版本</dt><dd>{{ feature.selected.value.config_version }}</dd></div></dl></article>
        <section class="form-card" aria-labelledby="connection-update-heading"><h2 id="connection-update-heading">更新连接</h2><p class="field-help">安全原因：用户名和密码不会回填，更新时请重新输入。</p><ErrorSummary :message="feature.formError.value?.message" />
          <form @submit.prevent="feature.update"><label for="detail-kind">类型</label><select id="detail-kind" v-model="feature.form.kind"><option value="qbittorrent">qBittorrent</option><option value="transmission">Transmission</option></select><label for="detail-name">显示名</label><input id="detail-name" v-model="feature.form.displayName" required><label for="detail-url">基地址</label><input id="detail-url" v-model="feature.form.baseUrl" type="url" required><label for="detail-user">用户名</label><input id="detail-user" v-model="feature.form.username" autocomplete="off"><label for="detail-password">密码</label><input id="detail-password" v-model="feature.form.password" type="password" autocomplete="new-password"><label class="check-field"><input v-model="feature.form.enabled" type="checkbox">允许后台任务使用</label><div class="detail-actions"><button class="primary-action" type="submit" :disabled="!feature.canWrite.value">保存更新</button><button type="button" :disabled="!feature.canWrite.value" @click="remove">删除本地连接</button></div></form>
        </section>
      </template>
    </AsyncState>
  </main>
</template>
