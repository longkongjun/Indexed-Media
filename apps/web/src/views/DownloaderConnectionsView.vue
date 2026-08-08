<script setup lang="ts">
/**
 * 展示脱敏下载器连接、候选测试和新连接表单。
 *
 * 页面只渲染 Core 返回的公开健康/能力投影；用户名与密码在任一请求后清空，候选测试不会保存配置。
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
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useDownloaderConnections(client, failure);

onMounted(() => feature.load(typeof route.query.cursor === "string" ? route.query.cursor : undefined));
</script>

<template>
  <main class="page-stack" data-downloader-connections>
    <header class="page-header"><div><p class="eyebrow">内置协议连接</p><h1>下载器连接</h1></div><RouterLink class="primary-action" to="/downloads">查看下载任务</RouterLink></header>
    <section class="form-card" aria-labelledby="connection-create-heading">
      <h2 id="connection-create-heading">添加连接</h2>
      <ErrorSummary :message="feature.formError.value?.message" :field="feature.formError.value?.field" />
      <form @submit.prevent="feature.save">
        <label for="downloader-kind">类型</label><select id="downloader-kind" v-model="feature.form.kind"><option value="qbittorrent">qBittorrent</option><option value="transmission">Transmission</option></select>
        <label for="downloader-name">显示名</label><input id="downloader-name" v-model="feature.form.displayName" maxlength="120" required>
        <label for="downloader-url">基地址</label><input id="downloader-url" v-model="feature.form.baseUrl" type="url" maxlength="2048" required>
        <label for="downloader-username">用户名</label><input id="downloader-username" v-model="feature.form.username" autocomplete="off" maxlength="256">
        <label for="downloader-password">密码</label><input id="downloader-password" v-model="feature.form.password" type="password" autocomplete="new-password" maxlength="4096">
        <label class="check-field"><input v-model="feature.form.enabled" type="checkbox">允许后台任务使用</label>
        <div class="detail-actions"><button type="button" :disabled="!feature.canWrite.value" @click="feature.testCandidate">仅测试，不保存</button><button class="primary-action" type="submit" :disabled="!feature.canWrite.value">保存连接</button></div>
      </form>
      <p v-if="feature.candidateResult.value" role="status">测试结果：{{ feature.candidateResult.value.reachable ? '可连接' : '不可连接' }} · {{ feature.candidateResult.value.health }}</p>
    </section>
    <AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" empty-message="尚未配置下载器" offline-message="当前离线，保留最近连接投影并禁用写入" @retry="feature.load()">
      <ul class="card-list">
        <li v-for="item in feature.items.value" :key="item.id" class="resource-card">
          <RouterLink :to="{ name: 'downloader-connection', params: { id: item.id } }"><strong>{{ item.display_name }}</strong><span>{{ item.kind }} · {{ item.health }} · {{ item.enabled ? '已启用' : '已停用' }}</span></RouterLink>
        </li>
      </ul>
    </AsyncState>
  </main>
</template>
