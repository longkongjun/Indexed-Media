<script setup lang="ts">
/**
 * 审批横幅（approval-banner 规格 v1.0，task 变体）：告知"有计划等待审批"并给出唯一权威审批入口。
 *
 * 由 needs_approval 语义驱动（plan.authorization === "paused"：AI 经能力通道请求交付但无 Web 人工
 * 授权回执，Core 返回 needs_approval 并停驻等待）。横幅内不出现批准/拒绝按钮——完整影响核对与
 * 审批只在 PlanPreview 全影响视图完成（dsh-collaboration 红线）；"稍后处理"仅收起本横幅。
 */
import type { ProcessingTaskOrganization } from "@mediaflow/api-client-ts";
import { Button } from "@indexed-design/ui-vue";
import { computed, ref } from "vue";
import { ORGANIZATION_OPERATION_LABELS } from "./dictionaries";

/** 等待审批的计划（服务端权威投影）；主操作目标锚点由父级页面提供。 */
const props = defineProps<{ plan: NonNullable<ProcessingTaskOrganization["plan"]> }>();
/** 主操作激活时发出（父级滚动/聚焦到 plan-preview 全影响视图）。 */
const emit = defineEmits<{ open: [planVersion: number] }>();

const dismissed = ref(false);
const operationLabel = computed(() => ORGANIZATION_OPERATION_LABELS[props.plan.operation]);
const visible = computed(() => !dismissed.value);

function openPlan(): void {
  emit("open", props.plan.version);
}
</script>
<template>
  <section v-if="visible" class="approval-banner" role="alert" data-approval-banner tabindex="-1">
    <span class="approval-banner__icon" aria-hidden="true">⚖</span>
    <div class="approval-banner__body">
      <p class="approval-banner__title">1 个整理计划等待审批</p>
      <p class="approval-banner__summary">
        《{{ plan.naming }}》· {{ operationLabel }} · {{ plan.source.root_id }}/{{ plan.source.relative_path }} → {{ plan.destination.root_id }}/{{ plan.destination.relative_path }}
      </p>
      <p class="approval-banner__version">计划版本 v{{ plan.version }}</p>
    </div>
    <div class="approval-banner__actions">
      <Button type="primary" class="approval-banner__primary" @click="openPlan">查看完整计划并审批</Button>
      <Button class="approval-banner__later" @click="dismissed = true">稍后处理</Button>
    </div>
  </section>
</template>
