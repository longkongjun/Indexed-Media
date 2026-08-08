import { hrefFor } from './router.mjs';

/**
 * 定义五个稳定的主导航目的地及其文本安全的原型图标。
 *
 * @type {Array<{ page: string, label: string, icon: string }>}
 */
export const NAV_ITEMS = [
  { page: 'overview', label: '概览', icon: '◉' },
  { page: 'tasks', label: '任务', icon: '任' },
  { page: 'media', label: '媒体', icon: '媒' },
  { page: 'libraries', label: '资源库', icon: '库' },
  { page: 'settings', label: '设置', icon: '设' },
];

/**
 * 转义一个值，以便插入 HTML 文本或带引号属性上下文。
 *
 * @param {unknown} value - 要字符串化并转义的值。
 * @returns {string} 已编码的文本，其中包含的和号、尖括号、引号和撇号均被处理。
 */
export function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (char) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
  })[char]);
}

/**
 * 为已知或回退状态值渲染本地化的文本状态徽章。
 *
 * @param {string} status - 同时用于 CSS 修饰符和可见标签查询的状态键。
 * @returns {string} 已转义的徽章 HTML，包含图标和文本，避免含义依赖颜色。
 */
export function statusBadge(status) {
  const labels = {
    healthy: '正常', degraded: '需关注', waiting: '待确认', running: '运行中',
    conflict: '冲突', failed: '失败', partial: '部分成功', success: '成功',
  };
  return `<span class="status status--${escapeHtml(status)}">● ${escapeHtml(labels[status] ?? status)}</span>`;
}

/**
 * 将页面 HTML 包装在响应式桌面端/移动端应用外壳中。
 *
 * @param {{ route: { page: string, params: Record<string, string> }, body: string, actionableCount: number }} input - 当前路由、可信渲染器输出和任务徽章计数。
 * @returns {string} 含活动导航、系统状态和给定页面主体的外壳 HTML。
 */
export function renderShell({ route, body, actionableCount }) {
  const nav = NAV_ITEMS.map((item) => {
    const active = route.page === item.page || (route.page === 'task' && item.page === 'tasks') || (route.page === 'media-detail' && item.page === 'media');
    const badge = item.page === 'tasks' && actionableCount ? `<span class="nav-badge">${actionableCount}</span>` : '';
    return `<a class="nav-link${active ? ' is-active' : ''}" href="${hrefFor(item.page)}"><span aria-hidden="true">${item.icon}</span><span>${item.label}</span>${badge}</a>`;
  }).join('');
  const mobileNav = NAV_ITEMS.slice(0, 3).map((item) => {
    const active = route.page === item.page || (route.page === 'task' && item.page === 'tasks') || (route.page === 'media-detail' && item.page === 'media');
    const badge = item.page === 'tasks' && actionableCount ? `<span class="nav-badge">${actionableCount}</span>` : '';
    return `<a class="nav-link${active ? ' is-active' : ''}" href="${hrefFor(item.page)}"><span aria-hidden="true">${item.icon}</span><span>${item.label}</span>${badge}</a>`;
  }).join('') + `<a class="nav-link${['more', 'libraries', 'settings'].includes(route.page) ? ' is-active' : ''}" href="${hrefFor('more')}"><span aria-hidden="true">•••</span><span>更多</span></a>`;
  const systemState = route.params.state === 'offline' ? 'Core 离线 ◐' : '系统正常 ●';

  return `<div class="app-shell"><aside class="sidebar"><strong class="brand">MediaFlow</strong><nav aria-label="主导航">${nav}</nav></aside><header class="topbar"><strong>MediaFlow</strong><span class="system-state">${systemState}</span></header><main id="main-content" tabindex="-1">${body}</main><nav class="bottom-nav" aria-label="移动导航">${mobileNav}</nav></div>`;
}
