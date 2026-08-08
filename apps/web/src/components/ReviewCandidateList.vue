<script setup lang="ts">
/**
 * 渲染人工审核可选的临时候选，并作为受控单选列表上报选择结果。
 *
 * 组件不搜索、提交或持久化候选；候选、当前选择和禁用状态均由父级审核流程提供。
 */
import type { ReviewCandidatePage } from "@mediaflow/api-client-ts";

/** 提供当前搜索得到的候选、父级维护的选中项，以及审核不可操作时的禁用边界。 */
defineProps<{ candidates: ReviewCandidatePage["items"]; modelValue: string | null; disabled?: boolean }>();
/** 用户切换候选时向父级同步候选 ID，不在组件内提交审核决定。 */
defineEmits<{ "update:modelValue": [value: string] }>();
</script>

<template>
  <fieldset class="candidate-list" :disabled="disabled" aria-describedby="candidate-help">
    <legend>选择候选</legend>
    <p id="candidate-help">这里只提交服务商、媒体类型和候选 ID；候选摘要不会持久化到决定中。</p>
    <label v-for="(item, index) in candidates" :key="`${item.provider}:${item.media_type}:${item.provider_id}`">
      <input
        :id="`review-candidate-${index}`"
        name="review-candidate"
        type="radio"
        :value="item.provider_id"
        :checked="modelValue === item.provider_id"
        @change="$emit('update:modelValue', item.provider_id)"
      >
      <span><strong>{{ item.title }}</strong> · {{ item.media_type }} · {{ item.year ?? '年份未知' }}</span>
    </label>
    <p v-if="candidates.length === 0">请先搜索候选。</p>
  </fieldset>
</template>
