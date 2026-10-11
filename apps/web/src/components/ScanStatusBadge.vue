<script setup lang="ts">
/**
 * 渲染本地化且通过形状强化的扫描任务状态和可选恢复指示器。
 *
 * 词表、色调与形状符来自共享字典（dictionaries.ts，status-badge 规格 v1.0）；
 * 组件仅负责展示：不发出事件、不暴露实例方法，也不修改任务状态。
 */
import type { ScanTask } from "@mediaflow/api-client-ts";
import { computed } from "vue";
import { SCAN_STATUS_BADGES } from "./dictionaries";
/** 必填的持久任务状态，以及表示 Core 正在恢复任务的可选标记。 */
const props = defineProps<{ status: ScanTask["status"]; recovering?: boolean }>();
const entry = computed(() => SCAN_STATUS_BADGES[props.status]);
</script>
<template>
  <span class="status-badge" :data-status="props.status" :data-tone="entry.tone">
    <span data-status-shape aria-hidden="true">{{ entry.shape }}</span>
    {{ entry.label }}<span v-if="recovering"> · 正在恢复</span>
  </span>
</template>
