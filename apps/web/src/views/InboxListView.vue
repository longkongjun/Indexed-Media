<script setup lang="ts">
/**
 * 已认证的收件目录列表及能力根相对路径创建流程。
 *
 * 路由查询状态提供页面游标。组件没有 props、发出事件或暴露的实例方法；它会在挂载或查询变化时加载，创建前执行预检，
 * 并导航到新建的目录。
 */
import { Button, Modal } from "@indexed-design/ui-vue";
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import AsyncState from "../components/AsyncState.vue";
import CursorPager from "../components/CursorPager.vue";
import ErrorSummary from "../components/ErrorSummary.vue";
import { handleAuthenticatedFailure } from "../components/apiErrors";
import { useMediaFlowClient } from "../components/injectedClient";
import { useSessionStore } from "../app/session";
import { useInboxDirectories } from "../features/inbox-directories/useInboxDirectories";

const client = useMediaFlowClient();
const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const showAdd = ref(false);
const showConfirmation = ref(false);
const failure = (error: unknown) => handleAuthenticatedFailure(error, session, router, route.fullPath);
const feature = useInboxDirectories(client, failure);
const selectedRoot = computed(() => feature.roots.value.find((root) => root.id === feature.form.rootId));

async function runPreflight(): Promise<void> { await feature.preflight(); showConfirmation.value = Boolean(feature.confirmation.value); }
// antd-vue 声明式 Modal 未实现 autoFocusButton（类型存在、仅命令式 API 消费）；C10 的"初始焦点取消、
// 关闭归触发项"由宿主落实（同 web-vue.modal 方案：focusTriggerAfterClose=false + afterClose 归还）。
// 打开时先记录触发元素再聚焦页脚取消按钮，焦点进入模态也使 ESC/Tab 圈闭立即生效。
let dialogOpener: HTMLElement | null = null;
watch(showConfirmation, async (open) => {
  if (!open || !feature.confirmation.value) return;
  dialogOpener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  await nextTick();
  document.querySelector<HTMLButtonElement>(".ant-modal-footer button")?.focus();
});
function restoreDialogFocus(): void {
  if (dialogOpener?.isConnected) dialogOpener.focus();
  dialogOpener = null;
}
async function create(): Promise<void> {
  try { const created = await feature.create(); await router.push({ name: "inbox-directory", params: { id: created.id } }); }
  catch { await nextTick(); }
}
function loadCurrent(): Promise<void> { return feature.load(typeof route.query.cursor === "string" ? route.query.cursor : undefined); }
onMounted(loadCurrent);
watch(() => route.query.cursor, loadCurrent);
</script>
<template>
  <main class="page-stack">
    <header class="page-header"><div><p class="eyebrow">扫描来源</p><h1>收件目录</h1></div><button class="primary-action" type="button" :disabled="feature.state.value.kind === 'offline'" @click="showAdd = true">添加收件目录</button></header>
    <AsyncState :state="feature.state.value" empty-message="尚未添加收件目录。添加后即可安全地启动扫描。" error-message="无法加载收件目录" offline-message="当前离线，保留上次加载的收件目录并暂停写操作" @retry="loadCurrent">
      <ul class="card-list" aria-label="收件目录列表">
        <li v-for="directory in feature.directories.value" :key="directory.id" class="resource-card">
          <RouterLink :to="{ name: 'inbox-directory', params: { id: directory.id } }"><code>{{ directory.relative_path }}</code></RouterLink>
          <span>{{ directory.health === 'available' ? '可用' : '不可用' }}</span>
        </li>
      </ul>
      <CursorPager :next-cursor="feature.nextCursor.value" @next="router.push({ query: { cursor: $event } })" />
    </AsyncState>

    <section v-if="showAdd" class="form-card" aria-labelledby="add-inbox-heading">
      <h2 id="add-inbox-heading">添加收件目录</h2>
      <ErrorSummary :message="feature.formError.value?.message" :field="feature.formError.value?.field" :focus-key="feature.formErrorOccurrence.value" />
      <form @submit.prevent="runPreflight">
        <label for="root-id">能力根</label>
        <select id="root-id" v-model="feature.form.rootId" :aria-invalid="feature.formError.value?.field === 'root-id'">
          <option value="">请选择</option><option v-for="root in feature.roots.value" :key="root.id" :value="root.id">{{ root.label }}（{{ root.access === 'read-only' ? '只读' : '读写' }}）</option>
        </select>
        <label for="relative-path">根内相对路径</label>
        <input id="relative-path" v-model="feature.form.relativePath" type="text" autocomplete="off" :aria-invalid="feature.formError.value?.field === 'relative-path'" />
        <p class="field-help">只填写能力根内的 UTF-8 相对路径，不接受宿主机绝对路径。</p>
        <button type="submit">预检目录</button>
      </form>
      <!-- 预检确认模态：antd Modal（设计库 web-vue.modal 模式）+ 自定义页脚。busy 时禁关闭/禁重复确认（C10），
           初始焦点取消、关闭归还触发项由 Modal 的 autoFocusButton/focusTriggerAfterClose 承担；MfDialog 的
           aria-describedby 描述关联在此模式下不自动接线，描述以正文首段呈现（见接入映射文档）。 -->
      <Modal
        v-if="feature.confirmation.value"
        :open="showConfirmation"
        centered
        :width="'min(38rem, calc(100vw - 2rem))'"
        :mask-closable="!feature.pendingCreate.value"
        :keyboard="!feature.pendingCreate.value"
        :closable="!feature.pendingCreate.value"
        :focus-trigger-after-close="false"
        :after-close="restoreDialogFocus"
        @cancel="showConfirmation = false"
      >
        <template #title><h2 class="inbox-confirmation-title">确认扫描范围</h2></template>
        <p>仅登记此能力根内的目录，不会自动扫描或执行文件操作。</p>
        <dl><div><dt>能力根</dt><dd>{{ feature.confirmation.value.rootLabel }}</dd></div><div><dt>相对路径</dt><dd><code>{{ feature.confirmation.value.relativePath }}</code></dd></div><div><dt>访问权限</dt><dd>{{ selectedRoot?.access === 'read-only' ? '只读' : '读写' }}</dd></div><div><dt>可读取</dt><dd>{{ feature.confirmation.value.readable ? '是' : '否' }}</dd></div><div><dt>重叠</dt><dd>{{ feature.confirmation.value.overlapsExisting ? '是' : '否' }}</dd></div><div><dt>扫描范围</dt><dd>仅此能力根内的上述相对目录</dd></div></dl>
        <p v-if="feature.formError.value?.message" role="alert">{{ feature.formError.value?.message }}</p>
        <template #footer>
          <Button :disabled="feature.pendingCreate.value" @click="showConfirmation = false">取消</Button>
          <Button type="primary" :loading="feature.pendingCreate.value" :disabled="!feature.canCreate.value" @click="create">确认并创建</Button>
        </template>
      </Modal>
    </section>
  </main>
</template>

<style scoped>
/* 模态标题用 h2 保持 heading 语义（e2e 依角色名驱动）；antd 标题容器不重置标题默认边距，这里归零。 */
.inbox-confirmation-title { margin: 0; font-size: var(--mfd-font-size-lg); line-height: var(--mfd-font-line-height-tight); }
</style>
