<script setup lang="ts">
/**
 * 渲染 C01 Action 按钮：default/primary/danger/icon 四种变体，default、hover、focus-visible、
 * pressed、disabled、busy 六种状态（见 design/ui-system 02 §2、10 §1）。
 *
 * 组件只负责表现与阻止重复激活：`disabled` 映射原生 disabled 属性；`busy` 保持可聚焦与可访问名称，
 * 设置 aria-busy 并拦截点击（preventDefault 同时阻止 submit 型的原生提交）。激活语义只通过
 * `activate` 事件暴露，原生 click 仍按 DOM 语义发生。组件不保存业务请求状态、不联网、不触发确认；
 * 危险确认由组合层决定。未声明的属性（aria-label、title、form 等）透传到原生 button。
 */
import { onMounted, ref } from "vue";
import type { MfButtonType, MfButtonVariant } from "../types";

defineOptions({ name: "MfButton" });

const props = withDefaults(
  defineProps<{
    /** 视觉变体；`icon` 以仅图标内容渲染，标签视觉隐藏但保留可访问名称。 */
    variant?: MfButtonVariant;
    /** 原生 button type；默认 `button`，避免在表单中意外提交。 */
    type?: MfButtonType;
    /** 禁用：原生 disabled，不响应指针与键盘，禁用原因由消费者在邻近位置说明。 */
    disabled?: boolean;
    /** 忙碌：显示忙碌指示、保留名称与焦点，阻止再次激活。 */
    busy?: boolean;
  }>(),
  { variant: "default", type: "button", disabled: false, busy: false },
);

/** 一次有效激活；disabled/busy 时不发出，不携带业务负载。 */
const emit = defineEmits<{ activate: [] }>();

defineSlots<{
  /** 标签内容；icon 变体下作为视觉隐藏的可访问名称。 */
  default?(): unknown;
  /** 前置图标内容；仅作装饰，始终 aria-hidden。 */
  icon?(): unknown;
}>();

const root = ref<HTMLButtonElement | null>(null);

function onClick(event: MouseEvent): void {
  if (props.busy) {
    // busy 保留焦点与名称，只拦截这次激活；disabled 由原生属性天然阻止点击。
    event.preventDefault();
    return;
  }
  emit("activate");
}

onMounted(() => {
  if (props.variant !== "icon" || !import.meta.env?.DEV) return;
  const el = root.value;
  const name =
    el?.getAttribute("aria-label") ?? el?.getAttribute("aria-labelledby") ?? el?.getAttribute("title") ?? el?.textContent ?? "";
  if (name.trim().length === 0) {
    console.warn('[MfButton] variant="icon" 需要可访问名称：提供 default 插槽文本，或透传 aria-label / aria-labelledby / title。');
  }
});
</script>

<template>
  <button
    ref="root"
    :class="['mf-button', `mf-button--${variant}`, { 'mf-button--busy': busy }]"
    :type="type"
    :disabled="disabled"
    :aria-busy="busy ? 'true' : undefined"
    @click="onClick"
  >
    <span v-if="busy" class="mf-button__spinner" aria-hidden="true"></span>
    <span v-else-if="$slots.icon" class="mf-button__icon" aria-hidden="true"><slot name="icon" /></span>
    <span v-if="$slots.default" :class="['mf-button__label', { 'mf-button__label--visually-hidden': variant === 'icon' }]">
      <slot />
    </span>
  </button>
</template>

<style>
/*
 * 尺寸与状态映射来自 design/ui-system 10 §1（C01：min44×44、radius-md、16/1.5、padding 水平16/垂直8、
 * disabled .55）与 02 §4（focus-visible 3 逻辑单位 focus 色环、hover 仅指针能力、pressed 短暂反馈）。
 * 颜色/字体/间距全部消费 --mfd-* 语义 token（子集由 src/styles/theme.css 提供）。
 */
.mf-button {
  box-sizing: border-box;
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(--mfd-space-2);
  min-inline-size: var(--mfd-icon-size-touch-min);
  min-block-size: var(--mfd-icon-size-touch-min);
  padding: var(--mfd-space-2) var(--mfd-space-4);
  border: var(--mfd-stroke-hair) solid var(--mfd-color-border-strong);
  border-radius: var(--mfd-radius-md);
  background: var(--mfd-color-action-secondary-bg);
  color: var(--mfd-color-text-primary);
  font-family: var(--mfd-font-family-ui);
  font-size: var(--mfd-font-size-base);
  font-weight: var(--mfd-font-weight-semibold);
  line-height: var(--mfd-font-line-height-normal);
  text-align: center;
  white-space: normal;
  overflow-wrap: anywhere;
  cursor: pointer;
  user-select: none;
  -webkit-tap-highlight-color: transparent;
  transition:
    background-color var(--mfd-motion-duration-fast) var(--mfd-motion-easing-standard),
    border-color var(--mfd-motion-duration-fast) var(--mfd-motion-easing-standard),
    color var(--mfd-motion-duration-fast) var(--mfd-motion-easing-standard),
    opacity var(--mfd-motion-duration-fast) var(--mfd-motion-easing-standard);
}

.mf-button--primary {
  border-color: transparent;
  background: var(--mfd-color-action-primary);
  color: var(--mfd-color-text-on-action);
}

.mf-button--danger {
  border-color: transparent;
  background: var(--mfd-color-action-danger);
  color: var(--mfd-color-text-on-danger);
}

.mf-button--icon {
  padding: 0;
}

/* hover 仅在指针设备上出现（02 §2），半透明 bg-hover 以叠层保留底色。 */
@media (hover: hover) {
  .mf-button:not(:disabled):not(.mf-button--busy):hover {
    background-image: linear-gradient(var(--mfd-color-bg-hover), var(--mfd-color-bg-hover));
  }

  .mf-button--primary:not(:disabled):not(.mf-button--busy):hover {
    background: var(--mfd-color-action-primary-hover);
    background-image: none;
  }

  .mf-button--danger:not(:disabled):not(.mf-button--busy):hover {
    background: var(--mfd-color-action-danger-hover);
    background-image: none;
  }
}

/* pressed 短暂反馈：primary 用 active 色阶，danger 无 active token 与 hover 相同，普通型边框加深。 */
.mf-button:not(:disabled):not(.mf-button--busy):active {
  box-shadow: inset 0 0 0 var(--mfd-stroke-hair) var(--mfd-color-border-strong);
}

.mf-button--primary:not(:disabled):not(.mf-button--busy):active {
  background: var(--mfd-color-action-primary-active);
  background-image: none;
  box-shadow: none;
}

.mf-button--danger:not(:disabled):not(.mf-button--busy):active {
  background: var(--mfd-color-action-danger-hover);
  background-image: none;
  box-shadow: none;
}

.mf-button:focus-visible {
  outline: var(--mfd-stroke-focus-ring) solid var(--mfd-color-border-focus);
  outline-offset: var(--mfd-stroke-focus-ring);
}

.mf-button:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.mf-button--busy {
  cursor: progress;
}

.mf-button__icon,
.mf-button__spinner {
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  inline-size: var(--mfd-icon-size-md);
  block-size: var(--mfd-icon-size-md);
}

.mf-button__icon > :where(svg, img) {
  inline-size: 100%;
  block-size: 100%;
}

.mf-button__spinner {
  border: var(--mfd-stroke-strong) solid currentColor;
  border-inline-end-color: transparent;
  border-radius: 50%;
  animation: mf-button-spin calc(var(--mfd-motion-duration-slow) * 2) linear infinite;
}

@keyframes mf-button-spin {
  to {
    transform: rotate(1turn);
  }
}

.mf-button__label--visually-hidden {
  position: absolute;
  inline-size: 1px;
  block-size: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
  border: 0;
}

@media (prefers-reduced-motion: reduce) {
  .mf-button {
    transition: none;
  }

  .mf-button__spinner {
    animation: none;
  }
}
</style>
