<script setup lang="ts">
/**
 * 已认证收件目录详情页，提供感知健康状态且幂等的扫描创建。
 *
 * 路由参数提供目录 ID。组件没有 props、发出事件或暴露的实例方法；挂载时加载目录，写操作使用一个可安全重放的键，
 * 成功后导航到任务详情。
 */
import type { InboxDirectory } from "@mediaflow/api-client-ts";
import { onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import AsyncState from "../components/AsyncState.vue";
import ErrorSummary from "../components/ErrorSummary.vue";
import { handleAuthenticatedFailure, isAmbiguousWriteFailure, isOffline, safeApiError } from "../components/apiErrors";
import type { AsyncState as AsyncStateValue } from "../components/asyncState";
import { useMediaFlowClient } from "../components/injectedClient";
import { useSessionStore } from "../app/session";

const client = useMediaFlowClient();
const route = useRoute(); const router = useRouter(); const session = useSessionStore();
const id = String(route.params.id); const state = ref<AsyncStateValue>({ kind: "idle" }); const directory = ref<InboxDirectory | null>(null);
const errorMessage = ref(""); const actionError = ref(""); const actionErrorOccurrence = ref(0); const pending = ref(false); let startKey: string | null = null;
async function load(): Promise<void> { const stale = Boolean(directory.value); state.value = { kind: "loading", stale }; try { directory.value = await client.getInboxDirectory(id); state.value = { kind: "content" }; } catch (error) { if (await handleAuthenticatedFailure(error, session, router, route.fullPath)) return; state.value = isOffline(error) ? { kind: "offline", stale } : { kind: "error", stale }; errorMessage.value = safeApiError(error, "无法加载收件目录").message; } }
function setActionError(message: string): void { actionError.value = message; actionErrorOccurrence.value += 1; }
async function startScan(): Promise<void> {
  if (pending.value || state.value.kind === "offline" || directory.value?.health !== "available") return;
  pending.value = true; const key = startKey ?? crypto.randomUUID(); startKey = key;
  try { const task = await client.createScanTask(id, key); startKey = null; actionError.value = ""; await router.push({ name: "scan-task", params: { id: task.id } }); }
  catch (error) {
    if (await handleAuthenticatedFailure(error, session, router, route.fullPath)) { startKey = null; return; }
    if (isAmbiguousWriteFailure(error)) {
      try {
        const task = await client.createScanTask(id, key); startKey = null; actionError.value = ""; await router.push({ name: "scan-task", params: { id: task.id } }); return;
      } catch (replayError) {
        if (await handleAuthenticatedFailure(replayError, session, router, route.fullPath)) { startKey = null; return; }
        if (isAmbiguousWriteFailure(replayError)) setActionError("启动扫描的请求结果尚未确认；恢复连接后可使用同一操作安全重试");
        else { startKey = null; setActionError(safeApiError(replayError, "无法启动扫描，请重试").message); }
      }
    } else { startKey = null; setActionError(safeApiError(error, "无法启动扫描，请重试").message); }
  } finally { pending.value = false; }
}
onMounted(load);
</script>
<template><main class="page-stack"><header class="page-header"><div><RouterLink to="/inbox-directories">← 返回收件目录</RouterLink><h1>收件目录详情</h1></div></header><ErrorSummary v-if="actionError" data-action-error :message="actionError" :focus-key="actionErrorOccurrence" heading="扫描操作未完成" /><AsyncState :state="state" :error-message="errorMessage" offline-message="当前离线，显示上次检查结果并暂停扫描" @retry="load"><article v-if="directory" class="detail-card"><dl><div><dt>相对路径</dt><dd><code>{{ directory.relative_path }}</code></dd></div><div><dt>健康</dt><dd>{{ directory.health === 'available' ? '可用' : '不可用' }}</dd></div><div><dt>检查时间</dt><dd><time :datetime="directory.last_checked_at">{{ directory.last_checked_at }}</time></dd></div></dl><button data-start-scan class="primary-action" type="button" :disabled="pending || state.kind === 'offline' || directory.health !== 'available'" @click="startScan">{{ pending ? '正在启动…' : '开始扫描' }}</button></article></AsyncState></main></template>
