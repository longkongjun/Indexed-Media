<script setup lang="ts">
/**
 * 展示单个处理任务的持久投影，并在允许时提供重试、取消和人工确认入口。
 *
 * 任务 ID 与返回查询来自路由，详情和操作可用性来自处理任务 feature；关联 SSE 事件触发刷新，
 * 会话状态只用于认证失败处理，页面不自行推进 Core 业务流程。
 */
import type { TaskEventEnvelope } from "@mediaflow/api-client-ts";
import { computed, inject, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import ApprovalBanner from "../components/ApprovalBanner.vue";
import AsyncState from "../components/AsyncState.vue";
import ErrorSummary from "../components/ErrorSummary.vue";
import PlanPreview from "../components/PlanPreview.vue";
import TaskTimeline from "../components/TaskTimeline.vue";
import { useMediaFlowClient } from "../components/injectedClient";
import { useProcessingTask } from "../features/processing-tasks/useProcessingTask";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useSessionStore } from "../app/session";
import { processingStatusBadge } from "../components/dictionaries";
import { useProjectionEvents } from "../features/events/useProjectionEvents";
import { useOrganizationTask } from "../features/organization-task/useOrganizationTask";
import { taskEventSourceFactoryKey } from "../features/scan-tasks/useTaskEvents";

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const id = String(route.params.id);
const client = useMediaFlowClient();
const eventSourceFactory = inject(taskEventSourceFactoryKey, undefined);
const detail = useProcessingTask(
  client,
  id,
  (error) => handleAuthenticatedFailure(error, session, router, route.fullPath),
);
const organization = useOrganizationTask(
  client,
  id,
  (error) => handleAuthenticatedFailure(error, session, router, route.fullPath),
);
const rollbackConfirming = ref(false);
const rollbackHeading = ref<HTMLElement | null>(null);
const backQuery = { ...route.query };
const events = useProjectionEvents({
  refresh: () => Promise.all([detail.load(), organization.load()]),
  eventTypes: ["processing-task.state-changed", "processing-task.identification-decided", "task-decision.accepted", "organization-result.changed"],
  matches: (event: TaskEventEnvelope) => event.task_id === id && (
    event.type === "processing-task.state-changed"
    || event.type === "processing-task.identification-decided"
    || event.type === "task-decision.accepted"
    || event.type === "organization-result.changed"
  ),
  versionOf: (event) => {
    if (event.type === "task-decision.accepted") return { key: `case:${event.payload.case_id}`, version: event.payload.case_version };
    if (event.type === "organization-result.changed") return { key: `result:${event.payload.result_id}`, version: event.payload.projection_version };
    return null;
  },
  eventSourceFactory,
  probeClient: client,
  onUnauthorized: () => handleAuthenticatedFailure(Object.assign(new Error("expired"), { status: 401 }), session, router, route.fullPath),
});

const stateLabels: Record<string, string> = {
  "not-planned": "尚未规划", planned: "计划已生成", paused: "等待处理",
  "recovery-pending": "正在核对恢复", "partial-success": "部分成功",
  completed: "本地结果完成", "manual-review": "需要人工处理",
};
const nfoLabels: Record<string, string> = {
  "not-requested": "未请求", preserved: "已保留", generated: "已生成", failed: "失败",
};

const planPreviewSection = ref<HTMLElement | null>(null);
const waitingApproval = computed(() => organization.projection.value?.plan?.authorization === "paused");

/** 横幅/深链打开完整计划视图：滚动 + 聚焦（approval-flow 深链 ?plan=<version>）。 */
function openPlanPreview(): void {
  planPreviewSection.value?.scrollIntoView({ block: "start" });
  planPreviewSection.value?.querySelector<HTMLElement>("[data-plan-preview]")?.focus();
}

/** 拒绝 = 计划不执行（现有作废语义：取消任务）；批准 = Web 人工授权（唯一授权产生方）。 */
const canRejectPlan = computed(() => Boolean(detail.task.value?.allowed_actions.includes("cancel")) && detail.canCancel.value && organization.pendingAction.value === null);

onMounted(() => {
  if (typeof route.query.plan === "string") {
    void nextTick().then(openPlanPreview);
  }
});

async function openRollback(): Promise<void> {
  rollbackConfirming.value = true;
  await nextTick();
  rollbackHeading.value?.focus();
}
async function confirmRollback(): Promise<void> {
  rollbackConfirming.value = false;
  await organization.rollback();
}
onMounted(events.start);
onBeforeUnmount(events.stop);
</script>

<template>
  <main class="page-stack">
    <header class="page-header"><div><RouterLink :to="{ name: 'tasks', query: backQuery }">← 返回任务中心</RouterLink><h1>处理任务详情</h1></div></header>
    <ApprovalBanner
      v-if="organization.projection.value?.plan && waitingApproval"
      :plan="organization.projection.value.plan"
      @open="openPlanPreview"
    />
    <ErrorSummary v-if="detail.actionError.value" :message="detail.actionError.value" heading="任务操作未完成" />
    <AsyncState :state="detail.state.value" :error-message="detail.errorMessage.value" offline-message="当前离线，保留最近投影并暂停写操作" @retry="detail.load">
      <article v-if="detail.task.value" class="detail-card">
        <h2><code>{{ detail.task.value.relative_path }}</code></h2>
        <dl class="count-grid"><div><dt>状态</dt><dd><span class="status-badge" :data-tone="processingStatusBadge(detail.task.value.status).tone"><span aria-hidden="true">{{ processingStatusBadge(detail.task.value.status).shape }}</span> {{ processingStatusBadge(detail.task.value.status).label }}</span></dd></div><div><dt>阶段</dt><dd>{{ detail.task.value.stage }}</dd></div><div><dt>检查点</dt><dd>{{ detail.task.value.checkpoint }}</dd></div><div><dt>尝试次数</dt><dd>{{ detail.task.value.attempt_count }}</dd></div></dl>
        <TaskTimeline :task="detail.task.value" :organization="organization.projection.value" />
        <p v-if="detail.task.value.reason">原因：{{ detail.task.value.reason }}</p>
        <nav class="detail-actions" aria-label="处理任务操作">
          <RouterLink v-if="detail.canReview.value && detail.reviewCaseId.value" :to="{ name: 'review-case', params: { id: detail.reviewCaseId.value } }">进入人工确认</RouterLink>
          <span v-else-if="detail.task.value.allowed_actions.includes('review')">需要人工确认</span>
          <button v-if="detail.task.value.allowed_actions.includes('retry')" type="button" :disabled="!detail.canRetry.value" @click="detail.retry">重试处理</button>
          <button v-if="detail.task.value.allowed_actions.includes('cancel')" type="button" :disabled="!detail.canCancel.value" @click="detail.cancel">取消处理</button>
        </nav>
        <p v-if="detail.reviewLookupError.value" role="alert">{{ detail.reviewLookupError.value }}</p>
      </article>
    </AsyncState>
    <section class="organization-task-section" data-organization-task aria-labelledby="organization-task-heading">
      <h2 id="organization-task-heading">安全整理</h2>
      <ErrorSummary
        v-if="organization.actionError.value"
        :message="organization.actionError.value"
        :focus-key="organization.actionErrorOccurrence.value"
        heading="整理操作结果"
      />
      <AsyncState
        :state="organization.state.value"
        :error-message="organization.errorMessage.value"
        empty-message="当前任务尚无整理投影"
        offline-message="当前离线，保留最近计划与 journal，所有整理写操作已禁用"
        @retry="organization.load"
      >
        <div v-if="organization.projection.value" class="organization-task-grid">
          <article class="detail-card">
            <header class="detail-heading"><div><p class="eyebrow">服务端状态</p><h3>{{ stateLabels[organization.projection.value.state] }}</h3></div><time v-if="organization.lastUpdated.value" :datetime="organization.lastUpdated.value">{{ organization.lastUpdated.value }}</time></header>
            <p v-if="organization.projection.value.state === 'not-planned'">任务尚未完成身份确认或 worker 尚未生成计划。</p>
          </article>

          <div ref="planPreviewSection" class="plan-preview-section">
            <PlanPreview
              v-if="organization.projection.value.plan"
              :projection="organization.projection.value"
              :can-approve="organization.canExecute.value"
              :can-reject="canRejectPlan"
              :pending="organization.pendingAction.value"
              @approve="organization.execute"
              @reject="detail.cancel"
            />
          </div>

          <article class="detail-card">
            <h3>Journal 与实际结果</h3>
            <p v-if="organization.projection.value.journals.length === 0">尚无文件副作用 journal。</p>
            <ol v-else class="organization-journal" aria-label="整理 journal">
              <li v-for="journal in organization.projection.value.journals" :key="journal.id">
                <strong>{{ journal.kind }} · {{ journal.status }}</strong>
                <code class="safe-path">{{ journal.destination.root_id }}/{{ journal.destination.relative_path }}</code>
                <small>operation {{ journal.operation_id }} · 版本 {{ journal.projection_version }}</small>
              </li>
            </ol>
            <template v-if="organization.projection.value.local_result">
              <p>本地结果：{{ organization.projection.value.local_result.status }}</p>
              <p>NFO：{{ nfoLabels[organization.projection.value.local_result.nfo_status] }}</p>
              <p v-if="organization.projection.value.local_result.remaining_actions.length">未完成：{{ organization.projection.value.local_result.remaining_actions.join("、") }}；重试只继续这些步骤，不重复 verified 文件操作。</p>
              <RouterLink v-if="organization.projection.value.local_result.catalog_media_item_id" :to="{ name: 'media-item', params: { id: organization.projection.value.local_result.catalog_media_item_id } }">查看正式媒体</RouterLink>
              <p v-else>Catalog 尚未建立；文件结果不等同于完整完成。</p>
            </template>
          </article>

          <nav class="detail-actions" aria-label="安全整理操作">
            <button v-if="organization.projection.value.allowed_actions.includes('recalculate')" data-organization-action="recalculate" type="button" :disabled="!organization.canRecalculate.value" @click="organization.recalculate">重新计算计划</button>
            <button v-if="organization.projection.value.allowed_actions.includes('rollback')" data-organization-action="rollback" type="button" :disabled="!organization.canRollback.value" @click="openRollback">回滚本次操作</button>
          </nav>

          <section v-if="rollbackConfirming" class="confirmation detail-card" role="alertdialog" aria-labelledby="rollback-confirmation-heading">
            <h3 id="rollback-confirmation-heading" ref="rollbackHeading" tabindex="-1">确认安全回滚</h3>
            <p>只补偿仍与 verified journal 一致的本次产物；外部变化会转为 manual-review，不会强制覆盖或删除未知数据。</p>
            <div class="detail-actions"><button type="button" @click="rollbackConfirming = false">取消</button><button type="button" @click="confirmRollback">确认回滚当前结果版本</button></div>
          </section>
        </div>
      </AsyncState>
    </section>
    <p v-if="events.diagnostic.value" class="visually-hidden">{{ events.diagnostic.value }}</p>
  </main>
</template>
