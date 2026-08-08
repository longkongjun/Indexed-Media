<script setup lang="ts">
/** 编辑一个 organization target；版本冲突时保留草稿并展示最新安全投影。 */
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

const route = useRoute(); const router = useRouter(); const session = useSessionStore();
const client = useMediaFlowClient(); const id = String(route.params.id);
const eventSourceFactory = inject(taskEventSourceFactoryKey, undefined);
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useOrganizationTargets(client, failure);
const events = useProjectionEvents({
  refresh: () => feature.loadDetail(id),
  eventTypes: ["organization-target.changed"],
  matches: (event: TaskEventEnvelope) => event.type === "organization-target.changed" && event.payload.organization_target_id === id,
  versionOf: (event) => event.type === "organization-target.changed"
    ? { key: `target:${id}`, version: event.payload.projection_version }
    : null,
  eventSourceFactory,
  probeClient: client,
  onUnauthorized: () => handleAuthenticatedFailure(Object.assign(new Error("expired"), { status: 401 }), session, router, route.fullPath),
});

async function remove(): Promise<void> {
  if (await feature.remove()) await router.push({ name: "organization-targets" });
}
onMounted(events.start);
onBeforeUnmount(events.stop);
</script>

<template>
  <main class="page-stack" data-organization-target-detail>
    <header class="page-header"><div><RouterLink :to="{ name: 'organization-targets' }">← 返回整理目标</RouterLink><h1>整理目标详情</h1></div></header>
    <AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" offline-message="当前离线，保留草稿与最近投影并禁用写入" @retry="feature.loadDetail(id)">
      <template v-if="feature.selected.value">
        <article class="detail-card">
          <h2>{{ feature.selected.value.display_name }}</h2>
          <dl><div><dt>能力位置</dt><dd><code class="safe-path">{{ feature.selected.value.root_id }}/{{ feature.selected.value.relative_path }}</code></dd></div><div><dt>配置版本</dt><dd>{{ feature.selected.value.config_version }}</dd></div><div><dt>固定操作</dt><dd>{{ feature.selected.value.operation }}</dd></div><div><dt>状态</dt><dd>{{ feature.selected.value.enabled ? "已启用" : "已停用" }}</dd></div></dl>
        </article>
        <OrganizationTargetForm :feature="feature" mode="update" />
        <section class="detail-card" aria-labelledby="delete-target-heading"><h2 id="delete-target-heading">删除本地目标配置</h2><p>不会删除媒体文件；活动计划仍引用时 Core 会拒绝。</p><button type="button" :disabled="!feature.canWrite.value" @click="remove">删除目标配置</button></section>
      </template>
    </AsyncState>
    <p v-if="events.diagnostic.value" class="visually-hidden">{{ events.diagnostic.value }}</p>
  </main>
</template>
