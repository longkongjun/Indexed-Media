<script setup lang="ts">
/** 管理员在当前页面签发一次性媒体只读码；离开、退出或到期即清除本地展示。 */
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useSessionStore } from "../app/session";
import { useMediaFlowClient } from "../components/injectedClient";

const client = useMediaFlowClient();
const session = useSessionStore();
const pairingCode = ref("");
const expiresAt = ref("");
const pending = ref(false);
const message = ref("");
let expiryTimer: ReturnType<typeof setTimeout> | undefined;
let generation = 0;
let disposed = false;

function clearCode(): void {
  pairingCode.value = "";
  expiresAt.value = "";
  if (expiryTimer !== undefined) clearTimeout(expiryTimer);
  expiryTimer = undefined;
}

function invalidate(): void {
  generation += 1;
  clearCode();
  pending.value = false;
  message.value = "";
}

watch(() => [session.authenticated, session.account?.id], invalidate, { flush: "sync" });
onMounted(() => { window.addEventListener("pagehide", invalidate); });

function isCurrent(requestGeneration: number): boolean {
  return !disposed && requestGeneration === generation && session.authenticated;
}

async function generate(): Promise<void> {
  if (pending.value || !session.authenticated) return;
  clearCode();
  message.value = "";
  pending.value = true;
  const requestGeneration = ++generation;
  try {
    const receipt = await client.createMediaReadOnlyDevicePairingCode();
    if (!isCurrent(requestGeneration)) return;
    const lifetime = Date.parse(receipt.expires_at) - Date.now();
    if (receipt.access_mode !== "media-read-only" || !/^[A-Z2-7]{8}$/.test(receipt.pairing_code)
      || !Number.isFinite(lifetime) || lifetime <= 0 || lifetime > 11 * 60 * 1000) {
      message.value = "配对码无效或已过期，请重新生成。";
      return;
    }
    pairingCode.value = receipt.pairing_code;
    expiresAt.value = new Date(receipt.expires_at).toLocaleString();
    expiryTimer = setTimeout(() => {
      clearCode();
      message.value = "配对码已过期，请重新生成。";
    }, lifetime);
  } catch {
    if (isCurrent(requestGeneration)) {
      message.value = "无法生成配对码，请检查连接和管理员会话后重试。";
    }
  } finally {
    if (!disposed && requestGeneration === generation) pending.value = false;
  }
}

onBeforeUnmount(() => {
  disposed = true;
  window.removeEventListener("pagehide", invalidate);
  invalidate();
});
</script>

<template>
  <section aria-labelledby="tv-pairing-heading">
    <h1 id="tv-pairing-heading">TV 配对</h1>
    <p>在 TV 上连接同一 Core，然后输入这里生成的配对码。</p>
    <p>配对后仅可浏览媒体、搜索、查看海报并打开已配置的外部播放器。任务和管理功能不可用。</p>
    <button type="button" :disabled="pending || !session.authenticated" @click="generate">
      {{ pending ? "正在生成…" : "生成 TV 只读配对码" }}
    </button>
    <div v-if="pairingCode" role="status" aria-live="polite">
      <p>TV 只读配对码：<output aria-label="TV 只读配对码">{{ pairingCode }}</output></p>
      <p>到期时间：{{ expiresAt }}。有效期内仅可使用一次。</p>
      <p>此页面只在本次显示配对码，离开页面后隐藏。仅分享给需要配对的 TV。</p>
    </div>
    <p v-if="message" role="alert">{{ message }}</p>
  </section>
</template>
