<script setup lang="ts">
/**
 * 展示人工审核关联识别任务的有界证据，帮助管理员判断后续决定。
 *
 * 组件只读取父级已加载的识别详情，不负责加载、筛选、修改或持久化证据。
 */
import type { IdentificationDetail } from "@mediaflow/api-client-ts";

/** 提供父级审核流程已加载的证据及其截断状态。 */
defineProps<{ detail: IdentificationDetail }>();
</script>

<template>
  <section aria-labelledby="review-evidence-heading">
    <h2 id="review-evidence-heading">识别证据</h2>
    <ul class="evidence-list">
      <li v-for="item in detail.evidence" :key="item.id">
        <strong>{{ item.value }}</strong>
        <span>{{ item.source }} · {{ item.kind }} · {{ item.strength }}</span>
        <code>{{ item.reason }}</code>
      </li>
    </ul>
    <p v-if="detail.evidence.length === 0">当前没有可展示的识别证据。</p>
    <p v-if="detail.evidence_truncated">证据较多，此处仅显示有界结果。</p>
  </section>
</template>
