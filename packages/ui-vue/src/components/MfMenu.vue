<script setup lang="ts">
/** 受控操作菜单。普通item值由宿主持有，不绑定路由、业务实体或远端动作。 */
import { nextTick, onBeforeUnmount, onMounted, ref, useId, watch } from "vue";
const props = defineProps<{ open: boolean; label: string; items: readonly { id: string; label: string; disabled?: boolean }[] }>();
const emit = defineEmits<{ "update:open": [open: boolean]; activate: [id: string] }>();
const root = ref<HTMLElement | null>(null); const trigger = ref<HTMLButtonElement | null>(null); const id = useId();
let focusLast = false;
function items(): HTMLButtonElement[] { return [...(root.value?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [])]; }
watch(() => props.open, async (open) => { if (open) { await nextTick(); const all=items(); (focusLast ? all.at(-1) : all[0])?.focus(); } }, { immediate: true });
function close(restore=true): void { emit("update:open", false); if (restore) trigger.value?.focus(); }
function openMenu(last=false): void { focusLast=last; emit("update:open",true); }
function activate(index: number): void { const item=props.items[index]; if (!item || item.disabled) return; emit("activate",item.id); close(); }
function keydown(event: KeyboardEvent): void {
 const all=items(); const current=all.indexOf(document.activeElement as HTMLButtonElement);
 if (event.key === "Escape") { event.preventDefault(); close(); }
 else if (event.key === "Tab") close();
 else if (["ArrowDown","ArrowUp","Home","End"].includes(event.key)) {
  event.preventDefault(); let index=0;
  if(event.key === "End") index=all.length-1;
  else if(event.key === "ArrowDown") index=(current+1)%all.length;
  else if(event.key === "ArrowUp") index=(current-1+all.length)%all.length;
  all[index]?.focus();
 }
}
function outside(event: PointerEvent): void { if(props.open && event.target instanceof Node && !root.value?.contains(event.target)) close(false); }
onMounted(()=>document.addEventListener("pointerdown",outside));
onBeforeUnmount(()=>document.removeEventListener("pointerdown",outside));
</script>
<template>
 <div ref="root" class="mf-menu">
  <button ref="trigger" type="button" aria-haspopup="menu" :aria-expanded="open" :aria-controls="`${id}-menu`" @click="open ? close() : openMenu()" @keydown.down.prevent="openMenu()" @keydown.up.prevent="openMenu(true)">{{ label }} <span aria-hidden="true">⌄</span></button>
  <div v-if="open" :id="`${id}-menu`" role="menu" :aria-label="label" @keydown="keydown">
   <button v-for="(item,index) in props.items" :key="item.id" type="button" role="menuitem" tabindex="-1" :aria-disabled="item.disabled || undefined" @click="activate(index)">{{ item.label }}</button>
  </div>
 </div>
</template>
<style>
.mf-menu { position: relative; width: fit-content; max-width: 100%; color: var(--mfd-color-text-primary); font-family: var(--mfd-font-family-ui); }
.mf-menu button { font: inherit; color: inherit; background: var(--mfd-color-bg-surface); min-height: 44px; padding: .65rem 1rem; text-align: left; border: 1px solid var(--mfd-color-border-default); border-radius: var(--mfd-radius-md); overflow-wrap: anywhere; cursor: pointer; }
.mf-menu button:focus-visible { outline: 3px solid var(--mfd-color-border-focus); outline-offset: 2px; }
.mf-menu [role="menu"] { position: absolute; z-index: 5; min-width: min(15rem, calc(100vw - 4rem)); max-width: calc(100vw - 4rem); padding: .4rem; border: 1px solid var(--mfd-color-border-strong); border-radius: var(--mfd-radius-md); background: var(--mfd-color-bg-surface); }
.mf-menu [role="menuitem"] { display: block; width: 100%; border: 0; }
.mf-menu [aria-disabled="true"] { opacity: .55; cursor: not-allowed; }
.mf-menu [role="menuitem"]:hover { background: var(--mfd-color-bg-hover); }
</style>
