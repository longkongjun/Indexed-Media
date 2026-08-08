<script setup lang="ts">
/** 展示版本化 organization target 列表与 preflight-first 创建表单。 */
import type { TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { inject, onBeforeUnmount, onMounted } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useSessionStore } from "../app/session";
import AsyncState from "../components/AsyncState.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import { useProjectionEvents } from "../features/events/useProjectionEvents";
import OrganizationTargetForm from "../features/organization-targets/OrganizationTargetForm.vue";
import { useOrganizationTargets } from "../features/organization-targets/useOrganizationTargets";
import { taskEventSourceFactoryKey } from "../features/scan-tasks/useTaskEvents";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const client = useMediaFlowClient();
const eventSourceFactory = inject(taskEventSourceFactoryKey, undefined);
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useOrganizationTargets(client, failure);
const events = useProjectionEvents({
  refresh: () => feature.load(typeof route.query.cursor === "string" ? route.query.cursor : undefined),
  eventTypes: ["organization-target.changed"],
  matches: (event: TaskEventEnvelope) => event.type === "organization-target.changed",
  versionOf: (event) => event.type === "organization-target.changed"
    ? { key: `target:${event.payload.organization_target_id}`, version: event.payload.projection_version }
    : null,
  eventSourceFactory,
  probeClient: client,
  onUnauthorized: () => handleAuthenticatedFailure(Object.assign(new Error("expired"), { status: 401 }), session, router, route.fullPath),
});

onMounted(events.start);
onBeforeUnmount(events.stop);
</script>

<template>
  <main class="page-stack" data-organization-targets>
    <header class="page-header"><div><p class="eyebrow">安全整理配置</p><h1>整理目标</h1></div></header>
    <OrganizationTargetForm :feature="feature" mode="create" />
    <AsyncState
      :state="feature.state.value"
      :error-message="feature.errorMessage.value"
      empty-message="尚无整理目标；请先声明 read-write deployment root，再添加目标"
      offline-message="当前离线，保留最近目标投影并禁用保存"
      @retry="feature.load()"
    >
      <ul class="card-list organization-target-list">
        <li v-for="item in feature.items.value" :key="item.id" class="resource-card">
          <RouterLink :to="{ name: 'organization-target', params: { id: item.id } }">
            <strong>{{ item.display_name }}</strong>
            <span>{{ item.kind }} · {{ item.operation }} · 版本 {{ item.config_version }}</span>
            <span>自动执行：{{ item.automatic ? "已启用" : "未启用" }} · 规则 {{ item.rules.length }} 条</span>
          </RouterLink>
        </li>
      </ul>
    </AsyncState>
    <p v-if="events.diagnostic.value" class="visually-hidden">{{ events.diagnostic.value }}</p>
  </main>
</template>
