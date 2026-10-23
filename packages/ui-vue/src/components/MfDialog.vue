<script setup lang="ts">
/** 受控原生模态。关闭仅发请求，由宿主处理dirty确认、未知结果和业务校验；默认安全焦点。 */
import { nextTick, onBeforeUnmount, ref, useId, watch } from "vue";
import MfButton from "./MfButton.vue";
const props = withDefaults(defineProps<{ open: boolean; title: string; description?: string; busy?: boolean; confirmDisabled?: boolean; dirty?: boolean; error?: string; confirmLabel?: string; cancelLabel?: string }>(), { description: "", busy: false, confirmDisabled: false, dirty: false, error: "", confirmLabel: "确认", cancelLabel: "取消" });
const emit = defineEmits<{ confirm: []; "request-close": [reason: "clean" | "dirty"] }>();
defineSlots<{ default?(): unknown }>();
const id = useId();
const dialog = ref<HTMLDialogElement | null>(null);
let opener: HTMLElement | null = null;
function closeRequest(): void { if (!props.busy) emit("request-close", props.dirty ? "dirty" : "clean"); }
function confirm(): void { if (!props.busy && !props.confirmDisabled) emit("confirm"); }
function restoreFocus(): void { if (opener?.isConnected) opener.focus(); }
watch(() => props.open, async (open) => {
  if (open) {
    opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    await nextTick();
    const element = dialog.value;
    if (!element) return;
    if (typeof element.showModal === "function") { if (!element.open) element.showModal(); }
    else element.setAttribute("open", "");
    element.querySelector<HTMLElement>('[data-action="cancel"]')?.focus();
  } else {
    if (dialog.value?.open && typeof dialog.value.close === "function") dialog.value.close();
    else dialog.value?.removeAttribute("open");
    restoreFocus();
  }
}, { immediate: true });
onBeforeUnmount(restoreFocus);
function keydown(event: KeyboardEvent): void {
  if (event.key !== "Tab" || !dialog.value) return;
  const nodes = [...dialog.value.querySelectorAll<HTMLElement>('button, input, select, textarea, a[href], summary, [contenteditable="true"], [tabindex="0"]')].filter(node => {
    if (node.matches(':disabled, input[type="hidden"], [tabindex="-1"]') || node.closest('[hidden], [inert]')) return false;
    // 隐藏控件及其隐藏祖先不进入可见焦点序列；不依赖布局矩形，
    // 使浏览器和DOM测试器使用同一套规则。
    for (let ancestor: HTMLElement | null = node; ancestor; ancestor = ancestor.parentElement) {
      const style = getComputedStyle(ancestor);
      if (style.display === "none" || style.visibility === "hidden" || style.visibility === "collapse" || style.contentVisibility === "hidden") return false;
    }
    return true;
  });
  const first = nodes[0]; const last = nodes.at(-1);
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
}
</script>
<template>
  <dialog ref="dialog" class="mf-dialog" :aria-labelledby="`${id}-title`" :aria-describedby="description ? `${id}-description` : undefined" :aria-busy="busy" @cancel.prevent="closeRequest" @keydown="keydown">
    <h2 :id="`${id}-title`">{{ title }}</h2>
    <p v-if="description" :id="`${id}-description`">{{ description }}</p>
    <div class="mf-dialog__content"><slot /></div>
    <p v-if="error" role="alert" class="mf-dialog__error">{{ error }}</p>
    <footer>
      <MfButton data-action="cancel" :busy="busy" @activate="closeRequest">{{ cancelLabel }}</MfButton>
      <MfButton data-action="confirm" variant="primary" :busy="busy" :disabled="confirmDisabled" @activate="confirm">{{ confirmLabel }}</MfButton>
    </footer>
  </dialog>
</template>
<style>
.mf-dialog { box-sizing: border-box; width: min(36rem, calc(100% - 2rem)); max-height: calc(100dvh - 2rem); overflow: auto; padding: 1.5rem; border: 1px solid var(--mfd-color-border-default); border-radius: var(--mfd-radius-lg); background: var(--mfd-color-bg-surface); color: var(--mfd-color-text-primary); font-family: var(--mfd-font-family-ui); line-height: 1.5; }
.mf-dialog::backdrop { background: rgb(0 0 0 / .5); }
.mf-dialog h2 { margin: 0 0 1rem; font-size: 1.375rem; overflow-wrap: anywhere; }
.mf-dialog__content { min-width: 0; }
.mf-dialog__error { color: var(--mfd-color-state-danger); }
.mf-dialog footer { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: .75rem; margin-top: 1.5rem; }
</style>
