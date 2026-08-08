<script setup lang="ts">
/** 一次性展示 Webhook secret；复制结果和离开确认均由文本与键盘表达。 */
import type { WebhookSecretReceipt } from "@mediaflow/api-client-ts";
import { ref } from "vue";

defineProps<{ receipt: WebhookSecretReceipt }>();
const emit = defineEmits<{ copied: [value: boolean]; dismiss: [] }>();
const confirmed = ref(false);
const copyStatus = ref("");

async function copy(secret: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(secret);
    copyStatus.value = "Secret 已复制，请确认已安全保存";
    emit("copied", true);
  } catch {
    copyStatus.value = "无法访问剪贴板，请手动选择并复制";
    emit("copied", false);
  }
}
</script>

<template>
  <section class="secret-once" role="alertdialog" aria-labelledby="secret-once-heading" aria-describedby="secret-once-help">
    <h2 id="secret-once-heading">Webhook secret 仅显示这一次</h2>
    <p id="secret-once-help">离开后无法再次读取；丢失时只能轮换，旧 secret 会立即失效。</p>
    <pre data-secret-once tabindex="0">{{ receipt.secret }}</pre>
    <div class="detail-actions">
      <button type="button" @click="copy(receipt.secret)">复制 secret</button>
      <label class="check-field"><input v-model="confirmed" type="checkbox" @change="$emit('copied', confirmed)">我已安全保存</label>
      <button class="primary-action" type="button" :disabled="!confirmed" @click="$emit('dismiss')">完成并清除</button>
    </div>
    <p aria-live="polite">{{ copyStatus }}</p>
  </section>
</template>
