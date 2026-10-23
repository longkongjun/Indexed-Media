<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { MfButton, MfMenu, MfContentCard, MfDialog, MfField, MfImage, MfRequestState, type MfRequestStateValue } from "@mediaflow/ui-vue";

const catalogue = [
  { id: "C01", name: "操作按钮", api: "MfButton · variant / type / busy / disabled → activate", batch: "B1", method: "正式公开组件", check: "Tab进入；Enter/Space只触发一次；busy保名称与焦点。" },
  { id: "C02", name: "输入字段", api: "MfField · id / label / modelValue / hint / error → update:modelValue", batch: "B2", method: "正式公开组件", check: "修改地址；触发错误；确认值保留、说明与错误正确关联。" },
  { id: "C03", name: "选择器", api: "原生 select · value / change", batch: "B2", method: "成熟原生语义", check: "使用方向键选择稳定value；长选项可读；禁用状态不变化。" },
  { id: "C04", name: "复选与开关", api: "原生 checkbox / radio · checked / change", batch: "B2", method: "成熟原生语义", check: "点击标签整行只切换一次；键盘Space切换；读出选中与名称。" },
  { id: "C05", name: "筛选分组", api: "MfButton + aria-pressed · selected由宿主持有", batch: "B1–B2", method: "公开组件组合", check: "同一条件再次激活不重置；字体放大后换行。" },
  { id: "C06", name: "状态标签", api: "span · 文本 + 独立图形 · 非交互", batch: "B3", method: "语义组合", check: "状态同时提供文字和图形；Tab不落在只读标签上。" },
  { id: "C07", name: "信息行", api: "语义列表 + 独立主/次操作", batch: "B2–B4", method: "成熟原语组合", check: "主操作与次操作各自响应；长标题撑高；不嵌套按钮。" },
  { id: "C08", name: "数据集合", api: "table / caption / th scope + 稳定行ID", batch: "B2–B4", method: "成熟原生语义", check: "窄窗仅表格容器横滚；列标题与行操作有上下文。" },
  { id: "C09", name: "导航", api: "nav + a[aria-current] · 路由由宿主持有", batch: "B1–B4", method: "成熟原生语义", check: "普通链接保持链接行为；当前页独立表达；返回状态保留。" },
  { id: "C10", name: "模态与表单", api: "MfDialog · open / busy / dirty / error → confirm / request-close", batch: "B2", method: "正式公开组件", check: "默认取消；dirty关闭要求明确决定；busy禁止关闭；失败保值；焦点返回。" },
  { id: "C11", name: "反馈", api: "role=status / alert + MfButton恢复", batch: "B1–B4", method: "成熟语义组合", check: "关键错误常驻；恢复不会静默重放真实写操作。" },
  { id: "C12", name: "请求状态", api: "MfRequestState · state / messages → retry", batch: "B1", method: "正式公开组件", check: "五态/stale分开；旧内容可读但不可操作；重试保留筛选。" },
  { id: "C13", name: "图片", api: "MfImage · src / alt / ratio / retryLabel → load / error / retry", batch: "B1", method: "正式公开组件", check: "缺图/坏地址不丢标题；重试按钮独立；装饰图不重复朗读。" },
  { id: "C14", name: "内容卡", api: "MfContentCard · title / subtitle + artwork/title/actions槽", batch: "B1", method: "正式公开组件", check: "主链接与次操作分离；长标题详情完整；海报不随暗色变暗。" },
  { id: "C15", name: "展开内容", api: "原生 details / summary", batch: "B2–B4", method: "成熟原生语义", check: "Enter/Space展开；长路径可读；收起后焦点仍有位置。" },
  { id: "C16", name: "分页与追加", api: "MfButton + cursor/exhausted由宿主持有", batch: "B2–B3", method: "公开组件组合", check: "追加不移动已有行；耗尽后保持可发现但不再触发。" },
  { id: "C17", name: "操作菜单", api: "MfMenu · open / label / items → update:open / activate", batch: "B3–B4", method: "成熟原语组合", check: "键盘展开/关闭、选择动作后焦点回触发；不伪造远端删除能力。" },
];
const selected = ref("C01");
const active = computed(() => catalogue.find(item => item.id === selected.value) ?? catalogue[0]!);
const theme = ref("light"); const large = ref(false); const disabled = ref(false); const busy = ref(false);
watch(theme, value => { if (value === "system") delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = value; }, { immediate: true });
const events = ref<string[]>([]);
function log(message: string): void { events.value = [message, ...events.value].slice(0, 8); }
const clicks = ref(0);
function activate(): void { clicks.value++; log(`activate ${clicks.value}`); }
const address = ref("https://demo.invalid"); const fieldError = ref(""); const connection = ref("library");
const enabled = ref(true); const mode = ref("local"); const filter = ref("all");
const dialogOpen = ref(false); const dirty = ref(false); const dialogBusy = ref(false); const dialogError = ref("");
const closeGuard = ref(false); const resultMode = ref("failure"); const draft = ref("演示连接");
function requestClose(reason: "clean" | "dirty"): void { if (reason === "dirty") closeGuard.value = true; else dialogOpen.value = false; }
function submitDialog(): void { dialogBusy.value = true; dialogError.value = ""; log("confirm：仅本地状态，不发网络请求"); }
function resolveDialog(): void {
  dialogBusy.value = false;
  if (resultMode.value === "failure") dialogError.value = "保存失败。草稿已保留，可以修改后重试。";
  else if (resultMode.value === "unknown") dialogError.value = "结果未知。先核对回执，不可直接重新提交。";
  else { dirty.value = false; dialogOpen.value = false; log("本地演示完成"); }
}
const requestKind = ref<"content" | "loading" | "empty" | "error" | "offline">("content"); const stale = ref(false);
const requestState = computed<MfRequestStateValue>(() => requestKind.value === "content" || requestKind.value === "empty" ? { kind: requestKind.value } : { kind: requestKind.value, stale: stale.value });
const imageMode = ref("missing");
const imageSource = computed(() => imageMode.value === "missing" ? undefined : imageMode.value === "failure" ? "/synthetic-unavailable-artwork.svg" : "data:image/svg+xml," + encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="240" height="360"><rect width="240" height="360" fill="#08744b"/><circle cx="150" cy="110" r="70" fill="#4cc38a"/><text x="24" y="310" fill="white" font-size="28">SYNTHETIC</text></svg>'));
const rows = ref(["演示条目 A", "演示条目 B"]); const exhausted = ref(false);
function more(): void { if (exhausted.value || busy.value) return; rows.value.push("演示条目 C", "演示条目 D"); exhausted.value = true; log("追加完成：原始条目与位置保持"); }
const menuOpen = ref(false);
function menuAction(id: string): void { log(`菜单动作 ${id}：只操作合成标识，无真实数据副作用`); }

</script>

<template>
  <div class="studio" :class="{ 'studio--large': large }">
    <aside class="sidebar">
      <a href="#" class="brand" @click.prevent="selected = 'C01'"><span class="brand-mark">M</span><span>MediaFlow<small>组件工作台 / Vue</small></span></a>
      <p class="section-label">FOUNDATION LIBRARY</p>
      <nav aria-label="组件目录">
        <button v-for="item in catalogue" :key="item.id" :aria-current="selected === item.id ? 'page' : undefined" @click="selected = item.id"><span>{{ item.id }}</span>{{ item.name }}</button>
      </nav>
      <p class="sidebar-note">正式库公开入口<br><code>@mediaflow/ui-vue</code><br>合成数据 · 无业务网络 · 待人工验收</p>
    </aside>
    <div class="workspace">
      <header class="topbar"><span>基础组件 / {{ active.name }}</span><div class="toolbar"><label>主题 <select v-model="theme"><option value="light">浅色</option><option value="dark">深色</option><option value="system">跟随系统</option></select></label><label><input v-model="large" type="checkbox"> 大字</label></div></header>
      <main>
        <div class="heading"><div><p class="eyebrow">{{ active.id }} · {{ active.batch }} · {{ active.method }}</p><h1>{{ active.name }}</h1><p>{{ active.check }}</p></div><span class="status">○ 待人工验收</span></div>
        <section class="example" aria-label="交互示例">
          <div class="example-toolbar"><span>交互画布</span><label><input v-model="disabled" type="checkbox"> disabled</label><label><input v-model="busy" type="checkbox"> busy</label></div>
          <div class="canvas">
            <template v-if="selected === 'C01'"><div class="row"><MfButton :busy="busy" :disabled="disabled" @activate="activate">默认操作</MfButton><MfButton variant="primary" :busy="busy" :disabled="disabled" @activate="activate">保存修改</MfButton><MfButton variant="danger" :busy="busy" :disabled="disabled" @activate="activate">危险动作示例</MfButton><MfButton variant="icon" aria-label="刷新示例" :busy="busy" :disabled="disabled" @activate="activate"><template #icon>↻</template>刷新</MfButton></div><p>有效激活 {{ clicks }} 次。所有操作仅影响本页。</p></template>
            <template v-else-if="selected === 'C02'"><MfField id="studio-address" v-model="address" label="服务器地址" hint="此地址不会发送请求" :error="fieldError" :disabled="disabled" autocomplete="url"/><div class="row"><MfButton @activate="fieldError = '无法连接。请检查地址，当前输入已保留。'">模拟校验失败</MfButton><MfButton @activate="fieldError = ''">清除错误</MfButton></div></template>
            <template v-else-if="selected === 'C03'"><label class="field">连接类型<select v-model="connection" :disabled="disabled"><option value="library">本地资源库</option><option value="remote">远端服务连接 · 这是一个较长的选项名称</option></select></label><p>稳定值：{{ connection }}</p></template>
            <template v-else-if="selected === 'C04'"><label class="check"><input v-model="enabled" type="checkbox" :disabled="disabled">启用本地提醒示例</label><fieldset><legend>数据来源</legend><label class="check"><input v-model="mode" type="radio" value="local" :disabled="disabled">本地</label><label class="check"><input v-model="mode" type="radio" value="manual" :disabled="disabled">手动选择</label></fieldset><p>当前：{{ enabled ? '启用' : '关闭' }} / {{ mode }}</p></template>
            <template v-else-if="selected === 'C05'"><div class="row" role="group" aria-label="媒体类型"><MfButton v-for="item in [{ value: 'all', name: '全部' }, { value: 'movie', name: '电影' }, { value: 'series', name: '剧集' }]" :key="item.value" :aria-pressed="filter === item.value" :variant="filter === item.value ? 'primary' : 'default'" :disabled="disabled" @activate="filter = item.value">{{ item.name }}</MfButton></div><p>筛选值 {{ filter }}</p></template>
            <template v-else-if="selected === 'C06'"><div class="row"><span class="badge">● 已完成</span><span class="badge badge--warning">△ 待确认</span><span class="badge badge--error">! 失败</span><span class="badge">○ 未知</span></div></template>
            <template v-else-if="selected === 'C07'"><ul class="item-list"><li v-for="(title, index) in ['长标题演示：一场关于遥远山谷与时间的旅行', '第二个合成条目']" :key="title"><div><strong>{{ title }}</strong><p>条目 {{ index + 1 }} · 示例辅助信息</p></div><MfButton :disabled="disabled" @activate="log(`查看 ${index + 1}`)">查看</MfButton><MfButton :disabled="disabled" @activate="log(`更多 ${index + 1}`)">更多</MfButton></li></ul></template>
            <template v-else-if="selected === 'C08'"><div class="table-scroll"><table><caption>合成连接状态</caption><thead><tr><th scope="col">名称</th><th scope="col">健康状态</th><th scope="col">最近结果</th><th scope="col">操作</th></tr></thead><tbody><tr v-for="name in ['客厅媒体库', '家庭文档目录']" :key="name"><th scope="row">{{ name }}</th><td>● 正常</td><td>本地演示</td><td><MfButton @activate="log(`查看 ${name}`)">查看{{ name }}</MfButton></td></tr></tbody></table></div></template>
            <template v-else-if="selected === 'C09'"><nav class="row" aria-label="示例页面导航"><a v-for="name in ['概览', '媒体', '任务']" :key="name" :href="`#demo-${name}`" :aria-current="mode === name ? 'page' : undefined" @click.prevent="mode = name">{{ name }}</a></nav><p>当前 {{ mode }}；仅演示导航组合，不改变产品路由。</p></template>
            <template v-else-if="selected === 'C10'"><MfButton variant="primary" @activate="dialogOpen = true">打开连接表单</MfButton><div class="row"><label>模拟结果<select v-model="resultMode"><option value="failure">失败</option><option value="success">完成</option><option value="unknown">未知</option></select></label><MfButton :disabled="!dialogBusy" @activate="resolveDialog">交付模拟回执</MfButton></div><MfDialog :open="dialogOpen" title="编辑演示连接" description="仅本地草稿，不连接真实服务。" :dirty="dirty" :busy="dialogBusy || dialogError.includes('结果未知')" :error="dialogError" @request-close="requestClose" @confirm="submitDialog"><MfField id="connection-name" v-model="draft" label="连接名称" @update:model-value="dirty = true"/><p v-if="closeGuard" role="alert">草稿未保存。<button @click="dialogOpen = false; closeGuard = false; dirty = false">放弃草稿并关闭</button><button @click="closeGuard = false">继续编辑</button></p><p v-if="dialogBusy">等待结果。请使用下方操作模拟响应。</p><div class="row"><MfButton v-if="dialogBusy" @activate="resolveDialog">交付模拟回执</MfButton><MfButton v-if="dialogError.includes('结果未知')" @activate="dialogError = ''; log('已核对回执：明确未执行，可重新审阅')">查询回执（演示未执行）</MfButton></div></MfDialog></template>
            <template v-else-if="selected === 'C11'"><div role="alert" class="notice"><strong>! 保存失败</strong><p>草稿已保留。请修正输入后主动重试。</p><MfButton @activate="log('恢复到输入字段；没有自动发送')">返回修改</MfButton></div><p role="status">普通结果通过status提供，不抢夺阅读焦点。</p></template>
            <template v-else-if="selected === 'C12'"><div class="row"><label>展示状态<select v-model="requestKind"><option v-for="kind in ['content', 'loading', 'empty', 'error', 'offline']" :key="kind">{{ kind }}</option></select></label><label><input v-model="stale" type="checkbox">保留旧内容</label></div><MfRequestState :state="requestState" @retry="requestKind = 'content'; log('本地恢复完成')"><p>旧内容或当前权威内容</p><MfButton @activate="log('内容操作')">内容操作</MfButton><template #empty><MfButton @activate="requestKind = 'content'">清除筛选</MfButton></template></MfRequestState></template>
            <template v-else-if="selected === 'C13'"><label class="field">图片来源<select v-model="imageMode"><option value="missing">缺图</option><option value="failure">加载失败</option><option value="synthetic">合成SVG</option></select></label><div class="artwork"><MfImage v-bind="imageSource ? { src: imageSource } : {}" alt="合成演示海报" retry-label="重试图片" @retry="log('重试合成图片')"><template #fallback>图片不可用</template></MfImage></div></template>
            <template v-else-if="selected === 'C14'"><div class="artwork"><MfContentCard title="穿越山谷：一个用于验证长标题可读性的合成电影名称" subtitle="电影 · 2026 · 合成内容"><template #artwork><MfImage v-bind="imageSource ? { src: imageSource } : {}" alt="" /></template><template #title><a href="#synthetic-detail" @click.prevent="log('打开合成详情，完整标题可读')">穿越山谷：一个用于验证长标题可读性的合成电影名称</a></template><template #actions><MfButton @activate="log('独立次操作')">更多信息</MfButton></template></MfContentCard></div></template>
            <template v-else-if="selected === 'C15'"><details><summary>查看长路径与附加说明</summary><p class="path">/synthetic/library/这是一条用于检查换行和完整阅读的长目录/2026/山谷旅行/第一部分.mkv</p><p>展开内容属于普通阅读区，字段不包含真实主机路径。</p></details></template>
            <template v-else-if="selected === 'C16'"><ul class="item-list"><li v-for="row in rows" :key="row">{{ row }}</li></ul><MfButton :busy="busy" :disabled="disabled" :aria-disabled="exhausted || undefined" @activate="more">{{ exhausted ? '全部加载完成' : '加载更多' }}</MfButton><p>共 {{ rows.length }} 项；演示只追加一页。</p></template>
            <template v-else-if="selected === 'C17'"><MfMenu v-model:open="menuOpen" label="条目操作" :items="[{ id: 'copy', label: '复制合成标识' }, { id: 'unsupported', label: '删除远端数据（不支持）', disabled: true }, { id: 'detail', label: '查看合成信息' }]" @activate="menuAction" /></template>
          </div>
        </section>
        <div class="reference-grid"><section class="reference"><p class="eyebrow">PUBLIC CONTRACT</p><h2>公开接口与采用方式</h2><code>{{ active.api }}</code><p>基础库不持有路由、认证、网络或业务对象。受控值与异步结果归宿主。</p></section><section class="reference"><p class="eyebrow">HUMAN REVIEW</p><h2>人工操作检查</h2><p>{{ active.check }}</p><p>分别检查浅/暗、大字、窄窗、键盘；记录版本、日期、观察与结论。工程通过不代表人工通过。</p></section></div>
        <section class="event-log"><h2>交互事件</h2><p v-if="!events.length">操作组件后显示实际事件；不记录输入值或秘密。</p><ol><li v-for="(event, index) in events" :key="`${index}-${event}`">{{ event }}</li></ol></section>
      </main>
      <footer class="page-footer">MediaFlow Foundation · Vue独立应用 · 61页设计由独立设计应用交付 · 所有数据均为合成示例</footer>
    </div>
  </div>
</template>
