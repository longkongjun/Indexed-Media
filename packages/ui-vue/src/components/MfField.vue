<script setup lang="ts">
/** 有标签的受控原生输入。值、校验与secret生命周期归宿主；其他原生属性/事件透传input。 */
import { computed, useAttrs } from "vue";
defineOptions({ inheritAttrs: false });
const props = withDefaults(defineProps<{
  id: string; label: string; modelValue: string; type?: string; hint?: string;
  error?: string; invalid?: boolean; describedBy?: string; disabled?: boolean; readonly?: boolean;
}>(), { type: "text", hint: "", error: "", describedBy: "", invalid: false, disabled: false, readonly: false });
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const attrs = useAttrs();
const description = computed(() => [props.hint && `${props.id}-hint`, props.error && `${props.id}-error`, props.describedBy].filter(Boolean).join(" ") || undefined);
function update(event: Event): void { if (event.target instanceof HTMLInputElement) emit("update:modelValue", event.target.value); }
</script>
<template>
  <div class="mf-field">
    <label :for="id">{{ label }}</label>
    <input v-bind="attrs" :id="id" :value="modelValue" :type="type" :disabled="disabled" :readonly="readonly" :aria-invalid="Boolean(error) || invalid" :aria-describedby="description" @input="update" />
    <p v-if="hint" :id="`${id}-hint`" class="mf-field__hint">{{ hint }}</p>
    <p v-if="error" :id="`${id}-error`" class="mf-field__error">{{ error }}</p>
  </div>
</template>
<style>
.mf-field { display: grid; gap: var(--mfd-space-2); min-width: 0; font: inherit; color: var(--mfd-color-text-primary); }
.mf-field label { font-weight: var(--mfd-font-weight-semibold); }
.mf-field input { box-sizing: border-box; width: 100%; min-width: 0; min-height: var(--mfd-icon-size-touch-min); padding: .65em .85em; border: 1px solid var(--mfd-color-border-default); border-radius: var(--mfd-radius-md); background: var(--mfd-color-bg-surface); color: inherit; font: inherit; }
.mf-field input[aria-invalid="true"] { border: 2px solid var(--mfd-color-state-danger); }
.mf-field input:focus-visible { outline: 3px solid var(--mfd-color-border-focus); outline-offset: 3px; }
.mf-field input:disabled { opacity: .55; cursor: not-allowed; }
.mf-field p { margin: 0; overflow-wrap: anywhere; }
.mf-field__hint { color: var(--mfd-color-text-secondary); }
.mf-field__error { color: var(--mfd-color-state-danger); }
</style>
