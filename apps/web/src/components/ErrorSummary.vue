<script setup lang="ts">
/**
 * 展示带焦点管理、可访问且可选链接到受影响字段的错误摘要。
 *
 * 组件不发出事件，也不暴露实例方法。挂载、消息变化或 `focusKey` 变化时，若存在消息且允许自动聚焦时会安排 DOM 焦点移至警报。
 */
import { nextTick, onMounted, ref, watch } from "vue";
/** 消息、可选字段目标/标题，以及让重复错误再次触发焦点的发生键。 */
const props = defineProps<{ message?: string | undefined; field?: string | undefined; heading?: string; focusKey?: string | number; suppressAutoFocus?: boolean }>();
const summary = ref<HTMLElement | null>(null);
async function focusSummary(): Promise<void> { if (props.message && !props.suppressAutoFocus) { await nextTick(); summary.value?.focus(); } }
watch([() => props.message, () => props.focusKey], focusSummary);
onMounted(focusSummary);
</script>
<template>
  <section v-if="message" ref="summary" data-error-summary class="error-summary" role="alert" tabindex="-1">
    <strong>{{ heading ?? '请检查以下问题' }}</strong>
    <p><a v-if="field" :href="`#${field}`">{{ message }}</a><span v-else>{{ message }}</span></p>
  </section>
</template>
