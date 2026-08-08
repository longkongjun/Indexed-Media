<script setup lang="ts">
/**
 * 已认证扫描任务列表，支持状态筛选、游标分页和滚动上下文恢复。
 *
 * 路由查询是公开输入。组件没有 props、发出事件或暴露的实例方法；筛选/分页操作会替换或推入路由状态，
 * 其变化将重新加载列表并可能恢复卡片可见性。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import AsyncState from "../components/AsyncState.vue"; import CursorPager from "../components/CursorPager.vue"; import ScanStatusBadge from "../components/ScanStatusBadge.vue";
import { useMediaFlowClient } from "../components/injectedClient"; import { useSessionStore } from "../app/session"; import { handleAuthenticatedFailure } from "../components/apiErrors"; import { useScanTasks } from "../features/scan-tasks/useScanTasks";
const client = useMediaFlowClient(); const route = useRoute(); const router = useRouter(); const session = useSessionStore(); const status = ref(typeof route.query.status === "string" ? route.query.status : "");
const context = computed(() => ({ status: status.value || undefined, cursor: typeof route.query.cursor === "string" ? route.query.cursor : undefined, scrollKey: typeof route.query.context === "string" ? route.query.context : undefined }));
const feature = useScanTasks(client, context.value, (error) => handleAuthenticatedFailure(error, session, router, route.fullPath));
async function loadCurrent(): Promise<void> { await feature.load(context.value); if (context.value.scrollKey) document.querySelector(`[data-context="${CSS.escape(context.value.scrollKey)}"]`)?.scrollIntoView(); }
async function applyFilter(): Promise<void> { await router.replace({ query: status.value ? { status: status.value } : {} }); }
onMounted(loadCurrent);
watch(() => [route.query.status, route.query.cursor, route.query.context], () => { status.value = typeof route.query.status === "string" ? route.query.status : ""; return loadCurrent(); });
</script>
<template><main class="page-stack"><header class="page-header"><div><p class="eyebrow">持久任务</p><h1>扫描任务</h1></div><RouterLink :to="{ name: 'tasks' }">返回任务中心</RouterLink></header><form class="filter-bar" @submit.prevent="applyFilter"><label for="task-status">状态筛选</label><select id="task-status" v-model="status"><option value="">全部</option><option value="queued">等待中</option><option value="running">扫描中</option><option value="partial-success">部分成功</option><option value="completed">已完成</option><option value="failed">失败</option><option value="cancelled">已取消</option></select><button type="submit">应用</button></form><AsyncState :state="feature.state.value" :error-message="feature.errorMessage.value" empty-message="当前筛选没有扫描任务" offline-message="当前离线，显示上次加载的任务；内容可能已过期" @retry="loadCurrent"><ul class="card-list"><li v-for="item in feature.tasks.value" :key="item.id" :data-context="`task-${item.id}`" class="resource-card"><RouterLink :to="{ name: 'scan-task', params: { id: item.id }, query: { ...feature.returnQuery.value, context: `task-${item.id}` } }"><ScanStatusBadge :status="item.status" :recovering="item.recovering" /><span>{{ item.counts.observed_files }} 个文件 · {{ item.counts.errors }} 个错误</span></RouterLink></li></ul><CursorPager :has-previous-context="Boolean(route.query.cursor)" :next-cursor="feature.nextCursor.value" @previous="router.push({ query: status ? { status } : {} })" @next="router.push({ query: { ...(status ? { status } : {}), cursor: $event } })" /></AsyncState></main></template>
