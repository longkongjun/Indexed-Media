<script setup lang="ts">
/** 请求展示容器：五态、旧内容和恢复操作。请求/认证/筛选由宿主负责，300ms 内完成的加载不闪提示。 */
import { computed, onUnmounted, ref, watch } from "vue";
import type { MfRequestStateValue } from "../types";
import MfButton from "./MfButton.vue";

/** 普通展示状态和可本地化消息；默认槽放结果，empty 槽放清筛选等宿主操作。stale 旧内容可读但禁止交互。 */
const props = withDefaults(defineProps<{
  state: MfRequestStateValue;
  loadingMessage?: string; refreshingMessage?: string; emptyMessage?: string;
  errorMessage?: string; offlineMessage?: string; retryLabel?: string;
}>(), {
  loadingMessage: "正在加载", refreshingMessage: "正在刷新", emptyMessage: "暂无数据",
  errorMessage: "请求暂时失败，请重试", offlineMessage: "当前离线，请检查网络", retryLabel: "重试",
});
/** 用户请求恢复时无负载通知，不执行网络请求或修改宿主状态。 */
defineEmits<{ retry: [] }>();
/** 默认槽放内容，空状态操作槽不携带业务数据或派生请求。 */
defineSlots<{ default?(): unknown; empty?(): unknown }>();
const showLoading = ref(false);
let timer: ReturnType<typeof setTimeout> | undefined;
watch(() => props.state.kind, (kind) => {
  clearTimeout(timer);
  showLoading.value = false;
  if (kind === "loading") timer = setTimeout(() => { showLoading.value = true; }, 300);
}, { immediate: true });
onUnmounted(() => clearTimeout(timer));
const stale = computed(() => "stale" in props.state && Boolean(props.state.stale));
const notice = computed(() => {
  if (props.state.kind === "loading" && showLoading.value) return stale.value ? props.refreshingMessage : props.loadingMessage;
  if (props.state.kind === "error") return props.errorMessage;
  if (props.state.kind === "offline") return props.offlineMessage;
  if (props.state.kind === "empty") return props.emptyMessage;
  return "";
});
</script>

<template>
  <section class="mf-request-state" :aria-busy="state.kind === 'loading'">
    <div v-if="notice" class="mf-request-state__notice" :class="`mf-request-state__notice--${state.kind}`" :role="state.kind === 'error' ? 'alert' : 'status'">
      <span aria-hidden="true">{{ state.kind === 'error' ? '⛒' : state.kind === 'offline' ? '⚡' : state.kind === 'empty' ? '▢' : '◌' }}</span>
      <span>{{ notice }}</span>
      <MfButton v-if="state.kind === 'error' || state.kind === 'offline'" :variant="state.kind === 'error' ? 'primary' : 'default'" @activate="$emit('retry')">{{ retryLabel }}</MfButton>
      <slot v-if="state.kind === 'empty'" name="empty" />
    </div>
    <div v-if="state.kind === 'content' || stale" class="mf-request-state__content" :inert="stale || undefined"><slot /></div>
  </section>
</template>

<style>
.mf-request-state { min-width: 0; color: var(--mfd-color-text-primary); font-family: var(--mfd-font-family-ui); }
.mf-request-state__notice { display: flex; flex-wrap: wrap; align-items: center; gap: var(--mfd-space-2); padding: var(--mfd-space-8); margin-bottom: var(--mfd-space-4); border: 1px solid var(--mfd-color-border-default); border-radius: var(--mfd-radius-lg); background: var(--mfd-color-bg-surface); overflow-wrap: anywhere; }
.mf-request-state__notice--error { border-color: var(--mfd-color-state-danger); background: var(--mfd-color-state-danger-bg); }
.mf-request-state__notice--offline { border-color: var(--mfd-color-state-warning-border); }
@media (max-width: 48rem) { .mf-request-state__notice { padding: var(--mfd-space-4); } }
</style>
