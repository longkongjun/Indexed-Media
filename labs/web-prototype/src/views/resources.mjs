import { hrefFor } from '../router.mjs';
import { escapeHtml, statusBadge } from '../ui.mjs';

const SETUP_STEPS = ['元数据', '收件目录', '资源库', '整理规则', '配置摘要'];
const operationLabel = { hardlink: '硬链接', move: '移动', copy: '复制' };

/**
 * 渲染五步首次运行设置引导中受限的一步。
 *
 * @param {{ sources: Array<{ path: string }> }} state - 提供示例收件路径的原型状态。
 * @param {{ params: { step?: string } }} route - 路由参数；数值步骤被限制在 1 到 5。
 * @returns {string} 仅预览配置且不会持久化或修改文件的设置 HTML。
 */
export function renderSetup(state, route) {
  const step = Math.min(5, Math.max(1, Number(route.params.step ?? 1)));
  const nav = SETUP_STEPS.map((label, index) => `<a class="setup-step${step === index + 1 ? ' is-active' : ''}" href="${hrefFor('setup', { step: index + 1 })}"><span>${index + 1}</span>${label}</a>`).join('');
  const content = [
    '验证 TMDB 元数据连接，只保存模拟健康状态。',
    `确认 ${escapeHtml(state.sources[0].path)} 位于管理员授权范围。`,
    '创建电影与剧集资源库，并按需添加通用视频整理目标；不建立动漫独立类型。',
    '选择固定文件操作、名称格式、分组归类、NFO 策略和自动执行条件。',
    '复核路径、TMDB 连接和规则摘要；此步骤不执行文件修改。',
  ][step - 1];
  return `<section class="page"><header class="page-header"><h1>首次配置</h1></header><nav class="setup-nav" aria-label="首次配置步骤">${nav}</nav><section class="card"><h2>${SETUP_STEPS[step - 1]}</h2><p>${content}</p>${step < 5 ? `<a class="button button--primary" href="${hrefFor('setup', { step: step + 1 })}">下一步</a>` : `<a class="button button--primary" href="${hrefFor('overview')}">进入运行概览</a>`}</section></section>`;
}

/**
 * 渲染已配置的收件来源、正式资源库和可复用的通用视频目标。
 *
 * @param {{ sources: object[], libraries: object[], genericTargets: object[] }} state - 原型资源配置。
 * @returns {string} 使用已转义夹具值和本地任务链接的资源库 HTML。
 */
export function renderLibraries(state) {
  const sources = state.sources.map((source) => `<article class="card"><h2>${escapeHtml(source.label)}</h2><p class="path">${escapeHtml(source.path)}</p><p>监听与对账：${escapeHtml(source.status)}</p><a class="button" href="${hrefFor('tasks', { view: 'all' })}">查看最近发现任务</a></article>`).join('');
  const libraries = state.libraries.map((library) => `<article class="card"><h2>${escapeHtml(library.label)}</h2><p class="path">${escapeHtml(library.path)}</p><dl><dt>文件操作</dt><dd>${operationLabel[library.operation]}</dd><dt>NFO</dt><dd>保留已有并补齐缺失</dd></dl><a class="button" href="${hrefFor('tasks', { library: library.id })}">相关任务</a></article>`).join('');
  const targets = state.genericTargets.map((target) => `<article class="card"><h2>通用视频整理目标：${escapeHtml(target.label)}</h2><p class="path">${escapeHtml(target.path)}</p><dl><dt>文件操作</dt><dd>${operationLabel[target.operation]}</dd><dt>名称格式</dt><dd class="path">${escapeHtml(target.naming)}</dd><dt>相似内容聚合</dt><dd>${escapeHtml(target.grouping)}</dd><dt>文件夹归类</dt><dd>${escapeHtml(target.folderClassification)}</dd><dt>NFO</dt><dd>${escapeHtml(target.nfo)}</dd></dl></article>`).join('');
  return `<section class="page"><header class="page-header"><h1>资源库</h1><a class="button button--primary" href="${hrefFor('setup', { step: 2 })}">检查配置</a></header><div class="grid grid--2">${sources}${libraries}${targets}</div></section>`;
}

/**
 * 渲染带连接测试控件的模拟 TMDB、Jellyfin 和 Core 设置。
 *
 * @param {{ health: Array<{ id: string, status: string }> }} state - 原型服务健康状态值。
 * @returns {string} 设置 HTML；控件会发出 DOM 事件，但此渲染器不尝试连接。
 */
export function renderSettings(state) {
  const item = (id, title, description, extraAction = '') => {
    const health = state.health.find((entry) => entry.id === id);
    return `<article class="card"><h2>${title}</h2><p>${description}</p>${statusBadge(health?.status ?? 'degraded')}<p><button class="button" data-action="test-connection" data-connection="${id}">测试连接</button>${extraAction}</p></article>`;
  };
  const openJellyfin = '<button class="button button--primary" data-action="external-jellyfin">打开首页</button>';
  return `<section class="page"><header class="page-header"><h1>设置</h1></header><div class="grid grid--2">${item('tmdb', 'TMDB', '元数据凭据、连接测试和健康状态。')}${item('jellyfin', 'Jellyfin', '服务地址、凭据、连接测试和健康状态；不参与资源库和整理任务。', openJellyfin)}${item('core', '系统', '管理员会话、Core、存储与诊断信息。')}</div></section>`;
}
