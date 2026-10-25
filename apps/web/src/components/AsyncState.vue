<script setup lang="ts">
/**
 * 在默认结果插槽周围呈现加载、空、错误、离线和过期内容状态。
 *
 * 呈现层委托 `@indexed-design/ui-vue` 的 RequestState（C12 五态 + stale：空态经 Empty、
 * 错误/离线经通知条与重试按钮、stale 旧内容降对比并 inert）；`idle` 是实现级起始态，
 * 不属于页面状态权威值域，不进入设计容器。页面状态语义权威见 packages/ui-contract，
 * 本组件保持既有 props、`retry` 事件与 `.async-state` 类钩子；文案默认值沿用应用既有措辞。
 */
import { RequestState } from "@indexed-design/ui-vue";
import type { AsyncState } from "./asyncState";

/** 公开状态/消息输入。`state` 控制可见性；省略消息时使用安全的本地化默认值。 */
defineProps<{ state: AsyncState; emptyMessage?: string; errorMessage?: string; offlineMessage?: string }>();
/** 用户请求再次尝试加载时发出 `retry`。 */
defineEmits<{ retry: [] }>();
</script>
<template>
  <RequestState
    v-if="state.kind !== 'idle'"
    class="async-state"
    :state="state"
    :empty-message="emptyMessage ?? '暂无数据'"
    :error-message="errorMessage ?? '请求暂时失败，请重试'"
    :offline-message="offlineMessage ?? '当前离线，显示上次加载的内容'"
    @retry="$emit('retry')"
  >
    <slot />
  </RequestState>
  <section v-else class="async-state" aria-busy="false"><slot /></section>
</template>
