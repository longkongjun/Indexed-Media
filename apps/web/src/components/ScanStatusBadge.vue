<script setup lang="ts">
/**
 * 渲染本地化且通过形状强化的扫描任务状态和可选恢复指示器。
 *
 * 组件仅负责展示：不发出事件、不暴露实例方法，也不修改任务状态。
 */
import type { ScanTask } from "@mediaflow/api-client-ts";
/** 必填的持久任务状态，以及表示 Core 正在恢复任务的可选标记。 */
const props = defineProps<{ status: ScanTask["status"]; recovering?: boolean }>();
const labels: Record<ScanTask["status"], string> = { queued: "等待中", running: "扫描中", "partial-success": "部分成功", completed: "已完成", failed: "失败", cancelled: "已取消" };
</script>
<template><span class="status-badge" :data-status="props.status"><span data-status-shape aria-hidden="true">◆</span> {{ labels[props.status] }}<span v-if="recovering"> · 正在恢复</span></span></template>
