<script setup lang="ts">
/**
 * 在默认结果插槽周围呈现加载、空、错误、离线和过期内容状态。
 *
 * 对齐设计系统 state-container 规格 v1.0（页面级四态 + stale 组合；conflict/partial/success
 * 随审批链 UI 落地）：形状符双编码、唯一主操作（重试）、stale 旧内容降低对比并禁交互（inert）、
 * 加载指示 <300ms 不闪（静默等待）。页面状态语义权威见 packages/ui-contract。
 *
 * 输入决定异步状态和可选的面向用户消息。任一恢复按钮被激活时，组件会无负载发出 `retry`，且没有暴露的实例方法
 * 或其他副作用。
 */
import { onUnmounted, ref, watch } from "vue";
import type { AsyncState } from "./asyncState";

/** 加载指示延迟（state-container 规格 §3：<300ms 不闪 Loading）。 */
const LOADING_DELAY_MS = 300;

/** 公开状态/消息输入。`state` 控制可见性；省略消息时使用安全的本地化默认值。 */
const props = defineProps<{ state: AsyncState; emptyMessage?: string; errorMessage?: string; offlineMessage?: string }>();
/** 用户请求再次尝试加载时发出 `retry`。 */
defineEmits<{ retry: [] }>();

const showLoading = ref(false);
let loadingTimer: number | undefined;

watch(
  () => props.state,
  (state) => {
    window.clearTimeout(loadingTimer);
    if (state.kind === "loading") {
      loadingTimer = window.setTimeout(() => {
        showLoading.value = true;
      }, LOADING_DELAY_MS);
    } else {
      showLoading.value = false;
    }
  },
  { immediate: true },
);

onUnmounted(() => {
  window.clearTimeout(loadingTimer);
});

/** stale 旧内容是否以降对比 + 禁交互呈现（loading 仅在指示出现后降对比，避免快刷新闪烁）。 */
const staleDimmed = (): boolean => {
  const state = props.state;
  if (state.kind === "error" || state.kind === "offline") return Boolean(state.stale);
  if (state.kind === "loading") return Boolean(state.stale) && showLoading.value;
  return false;
};
</script>
<template>
  <section class="async-state" :aria-busy="state.kind === 'loading'">
    <div v-if="state.kind === 'loading' && showLoading && !state.stale" class="state-notice state-notice--loading" role="status">
      <span class="state-shape" aria-hidden="true">◌</span>
      <strong>正在加载</strong>
    </div>
    <div v-else-if="state.kind === 'loading' && showLoading && state.stale" class="state-notice state-notice--loading state-notice--inline" role="status">
      <span class="state-shape" aria-hidden="true">◌</span>
      <strong>正在刷新</strong>
    </div>
    <template v-if="state.kind === 'offline'">
      <div class="state-notice state-notice--offline" role="status">
        <span class="state-shape" aria-hidden="true">⚡</span>
        <strong>离线</strong>
        <span>{{ offlineMessage ?? '当前离线，显示上次加载的内容' }}</span>
        <button type="button" @click="$emit('retry')">重试</button>
      </div>
    </template>
    <template v-else-if="state.kind === 'error'">
      <div class="state-notice state-notice--error" role="alert">
        <span class="state-shape" aria-hidden="true">⛒</span>
        <strong>加载失败</strong>
        <span>{{ errorMessage ?? '请求暂时失败，请重试' }}</span>
        <button type="button" class="state-notice__primary" @click="$emit('retry')">重试</button>
      </div>
    </template>
    <p v-else-if="state.kind === 'empty'" class="empty-state">
      <span class="state-shape" aria-hidden="true">▢</span>
      {{ emptyMessage ?? '暂无数据' }}
    </p>
    <div v-if="state.kind === 'content' || ('stale' in state && Boolean(state.stale))" class="async-state__content" :class="{ 'async-state__content--stale': staleDimmed() }" :inert="staleDimmed() || undefined">
      <slot />
    </div>
  </section>
</template>
