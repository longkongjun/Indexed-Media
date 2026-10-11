<script setup lang="ts">
/**
 * 计划变更预览（plan-preview 规格 v1.0，full 变体）：审批前的完整文件影响清单。
 *
 * 这是 Web 作为"事实界面"的核心组件：批准前逐行可见"来源 → 目标 + 操作类型"；冲突行（risk_codes）
 * 阻止批准并引导修正规则。批准 = Web 人工授权（唯一授权产生方，p0-fix 后语义：capability 通道
 * 只交付不授权）；拒绝 = 不执行此计划（现有语义为取消任务，见组件 emits）。批准/拒绝均为二次确认。
 */
import type { ProcessingTaskOrganization } from "@mediaflow/api-client-ts";
import { computed, nextTick, ref } from "vue";
import {
  ORGANIZATION_JOURNAL_KIND_LABELS,
  ORGANIZATION_JOURNAL_STATUS_LABELS,
  ORGANIZATION_OPERATION_LABELS,
} from "./dictionaries";

/** 服务端权威的组织投影（含计划与已执行 journal）；canApprove/canReject 由父级传入（写可用性）。 */
const props = defineProps<{ projection: ProcessingTaskOrganization; canApprove: boolean; canReject: boolean; pending: string | null }>();
/** 批准（执行当前计划版本，产生 Web 人工授权回执）/ 拒绝（取消任务，计划作废）。 */
const emit = defineEmits<{ approve: []; reject: [] }>();

const approving = ref(false);
const rejecting = ref(false);
const confirmCancel = ref<HTMLButtonElement | null>(null);
const plan = computed(() => props.projection.plan);
const operationLabel = computed(() => (plan.value ? ORGANIZATION_OPERATION_LABELS[plan.value.operation] : ""));
const hasRisks = computed(() => (plan.value?.risk_codes.length ?? 0) > 0);
const pendingThis = computed(() => props.pending !== null);

async function openApprove(): Promise<void> {
  approving.value = true;
  rejecting.value = false;
  await nextTick();
  confirmCancel.value?.focus();
}
async function openReject(): Promise<void> {
  rejecting.value = true;
  approving.value = false;
  await nextTick();
  confirmCancel.value?.focus();
}
function closeConfirm(): void {
  approving.value = false;
  rejecting.value = false;
}
function confirmAction(): void {
  if (approving.value) {
    closeConfirm();
    emit("approve");
  } else if (rejecting.value) {
    closeConfirm();
    emit("reject");
  }
}
</script>
<template>
  <article v-if="plan" class="plan-preview" data-plan-preview tabindex="-1">
    <header class="plan-preview__header">
      <h3>整理计划 v{{ plan.version }}</h3>
      <p class="plan-preview__meta">
        <time :datetime="plan.created_at">{{ plan.created_at }}</time> · 操作 {{ operationLabel }} · 命名《{{ plan.naming }}》
      </p>
    </header>
    <p id="plan-preview-summary" class="plan-preview__summary">
      1 项文件操作：{{ operationLabel }} 1<template v-if="projection.journals.some((journal) => journal.kind === 'nfo')"> · NFO 1</template>；
      来源 {{ plan.source.root_id }}/{{ plan.source.relative_path }}，目标 {{ plan.destination.root_id }}/{{ plan.destination.relative_path }}
    </p>
    <ul class="plan-preview__list" aria-label="计划操作清单">
      <li class="plan-op-row">
        <span class="status-badge" data-tone="info" role="img" aria-label="文件操作"><span aria-hidden="true">→</span> {{ operationLabel }}</span>
        <code class="plan-op-path">{{ plan.source.root_id }}/{{ plan.source.relative_path }}</code>
        <span class="plan-op-arrow" aria-hidden="true">→</span>
        <code class="plan-op-path">{{ plan.destination.root_id }}/{{ plan.destination.relative_path }}</code>
      </li>
      <li v-for="journal in projection.journals" :key="journal.id" class="plan-op-row plan-op-row--journal">
        <span class="status-badge" data-tone="info" role="img" aria-label="已执行操作"><span aria-hidden="true">→</span> {{ ORGANIZATION_JOURNAL_KIND_LABELS[journal.kind] }}</span>
        <code v-if="journal.source" class="plan-op-path">{{ journal.source.root_id }}/{{ journal.source.relative_path }}</code>
        <span class="plan-op-arrow" aria-hidden="true">→</span>
        <code class="plan-op-path">{{ journal.destination.root_id }}/{{ journal.destination.relative_path }}</code>
        <span
          class="status-badge"
          :data-tone="journal.status === 'verified' ? 'success' : journal.status === 'manual-review' ? 'danger' : journal.status === 'executing' ? 'info' : 'neutral'"
        ><span aria-hidden="true">{{ journal.status === "verified" ? "✓" : journal.status === "manual-review" ? "✗" : "○" }}</span> {{ ORGANIZATION_JOURNAL_STATUS_LABELS[journal.status] }}</span>
      </li>
      <li v-for="risk in plan.risk_codes" :key="risk" class="plan-op-row plan-op-row--conflict">
        <span class="status-badge" data-tone="warning"><span aria-hidden="true">⚠</span> 风险提示</span>
        <code class="plan-op-path">{{ risk }}</code>
      </li>
    </ul>
    <nav class="plan-preview__actions" aria-label="计划审批操作">
      <button type="button" class="plan-preview__reject" :disabled="!canReject || pendingThis" @click="openReject">拒绝此计划</button>
      <button type="button" class="plan-preview__approve" :disabled="!canApprove || pendingThis" aria-describedby="plan-preview-summary" @click="openApprove">
        批准并执行计划 v{{ plan.version }}
      </button>
    </nav>
    <p v-if="hasRisks" class="plan-preview__conflict-note" role="status">计划带有风险提示（见清单 ⚠ 行）；执行授权仍只绑定本计划版本，如需调整可先重新计算计划。</p>
    <p class="plan-preview__auth-note">一次性授权只绑定当前计划版本，不会创建可复用规则，也不能覆盖路径、来源或文件系统冲突。</p>

    <section
      v-if="approving || rejecting"
      class="confirmation detail-card plan-preview__confirm"
      role="alertdialog"
      aria-labelledby="plan-confirm-heading"
    >
      <h3 id="plan-confirm-heading">{{ approving ? "确认批准并执行" : "确认拒绝此计划" }}</h3>
      <p v-if="approving">将执行 1 项文件操作（{{ operationLabel }}）；此操作经 journal 留痕可回滚，且只授权本计划版本一次。</p>
      <p v-else>拒绝后本计划不执行，任务将被取消（现有作废语义）；如需调整结果，可先修改规则后重新计算计划。</p>
      <div class="detail-actions">
        <button ref="confirmCancel" type="button" @click="closeConfirm">取消</button>
        <button type="button" class="plan-preview__confirm-go" @click="confirmAction">{{ approving ? `确认执行计划 v${plan.version}` : "确认拒绝并取消任务" }}</button>
      </div>
    </section>
  </article>
</template>
