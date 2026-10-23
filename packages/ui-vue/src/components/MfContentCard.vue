<script setup lang="ts">
/** 内容卡的图片/标题/副标题排版；title 槽可放宿主的原生链接，卡片本身不接管导航或添加交互节点。 */
/** 标题保留完整文本供可访问名称及浏览器 title 使用；副标题允许换行。插槽 artwork/title/actions 均由调用方提供。 */
defineProps<{ title: string; subtitle?: string }>();
/** 标题链接和次动作分别置于 title/actions，禁止在标题链接中再嵌按钮。 */
defineSlots<{ artwork?(): unknown; title?(): unknown; default?(): unknown; actions?(): unknown }>();
</script>

<template>
  <article class="mf-content-card">
    <slot name="artwork" />
    <div class="mf-content-card__body">
      <div class="mf-content-card__title" :title="title"><slot name="title">{{ title }}</slot></div>
      <p v-if="subtitle" class="mf-content-card__subtitle">{{ subtitle }}</p>
      <slot />
    </div>
    <div v-if="$slots.actions" class="mf-content-card__actions"><slot name="actions" /></div>
  </article>
</template>

<style>
.mf-content-card { display: grid; align-content: start; gap: var(--mfd-space-2); min-width: 0; font-family: var(--mfd-font-family-ui); color: var(--mfd-color-text-primary); }
.mf-content-card__body { min-width: 0; overflow-wrap: anywhere; }
.mf-content-card__title { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; overflow: hidden; font-size: var(--mfd-font-size-base); line-height: var(--mfd-font-line-height-normal); font-weight: var(--mfd-font-weight-semibold); }
.mf-content-card__title a { display: block; min-height: var(--mfd-icon-size-touch-min); color: inherit; text-decoration: none; }
.mf-content-card__title a:focus-visible { outline: var(--mfd-stroke-focus-ring) solid var(--mfd-color-border-focus); outline-offset: -3px; }
.mf-content-card__subtitle { margin: 0; font-size: var(--mfd-font-size-xs); line-height: var(--mfd-font-line-height-normal); color: var(--mfd-color-text-secondary); }
.mf-content-card__actions { display: flex; flex-wrap: wrap; gap: var(--mfd-space-2); }
</style>
