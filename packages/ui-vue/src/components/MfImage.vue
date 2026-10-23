<script setup lang="ts">
/** 通用图片及缺图容器；URL 由宿主提供，不解释媒体引用。失败保留尺寸/名称，重试重新挂载原生 img。 */
import { ref, watch } from "vue";
import MfButton from "./MfButton.vue";

/** alt 必填，装饰图传空串；ratio 是海报/横幅比例，fallback 槽提供缺图文案。retryLabel 非空才显示失败恢复操作。 */
const props = withDefaults(defineProps<{ src?: string; alt: string; ratio?: "poster" | "landscape"; retryLabel?: string }>(), { ratio: "poster" });
/** 原生加载结果无负载通知宿主；retry 表示用户主动重试，不修改传入 URL。 */
const emit = defineEmits<{ load: []; error: []; retry: [] }>();
/** 缺图说明由宿主提供，命名由 alt 统一承担，避免重复朗读。 */
defineSlots<{ fallback?(): unknown }>();
const failed = ref(false);
const retrying = ref(false);
const root = ref<HTMLElement | null>(null);
watch(() => props.src, () => { failed.value = false; retrying.value = false; });
function fail(): void { failed.value = true; retrying.value = false; emit("error"); }
function retry(): void { failed.value = false; retrying.value = true; emit("retry"); }
function loaded(event: Event): void {
  // 重试按钮在加载期间保焦点；成功移除按钮前，将键盘焦点交给有名称的原生图片。
  if (retrying.value && root.value?.contains(document.activeElement)) (event.currentTarget as HTMLImageElement).focus();
  retrying.value = false;
  emit("load");
}
</script>

<template>
  <div ref="root" class="mf-image" :class="`mf-image--${ratio}`">
    <img v-if="src && !failed" class="mf-image__asset" :src="src" :alt="alt" tabindex="-1" loading="lazy" decoding="async" @load="loaded" @error="fail">
    <template v-else>
      <div class="mf-image__fallback" :role="alt ? 'img' : undefined" :aria-label="alt || undefined" :aria-hidden="!alt || undefined">
        <span aria-hidden="true">▧</span><span aria-hidden="true"><slot name="fallback" /></span>
      </div>
    </template>
    <MfButton v-if="src && (failed || retrying) && retryLabel" class="mf-image__retry" :busy="retrying" @activate="retry">{{ retryLabel }}</MfButton>
  </div>
</template>

<style>
.mf-image { position: relative; display: grid; min-width: 0; overflow: hidden; border-radius: var(--mfd-radius-lg); background: var(--mfd-color-media-placeholder); color: var(--mfd-color-text-secondary); font-family: var(--mfd-font-family-ui); }
.mf-image--poster { aspect-ratio: 2 / 3; }
.mf-image--landscape { aspect-ratio: 16 / 9; }
.mf-image__asset { width: 100%; height: 100%; min-height: 0; object-fit: cover; }
.mf-image__fallback { display: grid; align-content: center; justify-items: center; gap: var(--mfd-space-2); padding: var(--mfd-space-4); overflow-wrap: anywhere; text-align: center; }
.mf-image__retry { position: absolute; bottom: var(--mfd-space-2); justify-self: center; max-width: 100%; }
.mf-image__asset:focus-visible { outline: var(--mfd-stroke-focus-ring) solid var(--mfd-color-border-focus); outline-offset: -3px; }
</style>
