<script setup lang="ts">
/**
 * 在默认结果插槽周围呈现加载、空、错误、离线和过期内容状态。
 *
 * 输入决定异步状态和可选的面向用户消息。任一恢复按钮被激活时，组件会无负载发出 `retry`，且没有暴露的实例方法
 * 或其他副作用。
 */
import type { AsyncState } from "./asyncState";
/**
 * 公开状态/消息输入。`state` 控制可见性；省略消息时使用安全的本地化默认值。
 */
defineProps<{ state: AsyncState; emptyMessage?: string; errorMessage?: string; offlineMessage?: string }>();
/** 用户请求再次尝试加载时发出 `retry`。 */
defineEmits<{ retry: [] }>();
</script>
<template>
  <section class="async-state" :aria-busy="state.kind === 'loading'">
    <p v-if="state.kind === 'loading' && !state.stale" role="status">正在加载…</p>
    <template v-if="state.kind === 'offline'">
      <div class="state-notice state-notice--offline" role="status"><strong>Offline</strong><span>{{ offlineMessage ?? '当前离线，显示上次加载的内容' }}</span><button type="button" @click="$emit('retry')">重试</button></div>
    </template>
    <template v-else-if="state.kind === 'error'">
      <div class="state-notice state-notice--error" role="alert"><strong>加载失败</strong><span>{{ errorMessage ?? '请求暂时失败，请重试' }}</span><button type="button" @click="$emit('retry')">重试</button></div>
    </template>
    <p v-else-if="state.kind === 'empty'" class="empty-state">{{ emptyMessage ?? '暂无数据' }}</p>
    <slot v-if="state.kind === 'content' || ('stale' in state && Boolean(state.stale))" />
  </section>
</template>
