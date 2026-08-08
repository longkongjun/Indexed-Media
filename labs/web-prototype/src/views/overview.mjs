import { hrefFor } from '../router.mjs';
import { selectActionableTasks } from '../state.mjs';
import { statusBadge } from '../ui.mjs';

/**
 * 渲染本地处理概览、待处理计数、阶段和服务健康状态。
 *
 * @param {{ tasks: object[], health: object[] }} state - 用于计数和健康状态行的原型状态。
 * @returns {string} 含本地导航操作的概览 HTML；不执行网络请求或状态变更。
 */
export function renderOverview(state) {
  const actionable = selectActionableTasks(state);
  const stages = [
    { label: '发现', value: 'discovery' },
    { label: '识别', value: 'identification' },
    { label: '计划', value: 'planning' },
    { label: '文件操作', value: 'organization' },
    { label: 'NFO', value: 'nfo' },
    { label: '完成', value: 'complete' },
  ];
  const stageHtml = stages.map(({ label, value }) => {
    const count = state.tasks.filter((task) => task.stage === value).length;
    return `<a class="stage" href="${hrefFor('tasks', { view: 'all', stage: value })}"><strong>${label}</strong><span>${count}</span></a>`;
  }).join('<span aria-hidden="true">→</span>');
  const health = state.health.map((item) => {
    const action = item.id === 'jellyfin' ? '<button class="button" data-action="external-jellyfin">打开 Jellyfin 首页</button>' : '';
    return `<li class="list-item"><span>${item.label}</span><span>${statusBadge(item.status)} ${action}</span></li>`;
  }).join('');
  return `<section class="page"><header class="page-header"><h1>概览</h1></header><div class="card"><h2>处理阶段</h2><div class="stage-row">${stageHtml}</div></div><div class="grid grid--2"><section class="card"><h2>需要处理</h2><p><strong>${actionable.length}</strong> 项需要管理员介入</p><a class="button button--primary" href="${hrefFor('tasks', { view: 'actionable' })}">查看待处理任务</a></section><section class="card"><h2>近期结果</h2><p>本地成功 8 · 部分成功 1</p><a class="button" href="${hrefFor('media')}">查看媒体结果</a></section></div><section class="card"><h2>服务状态</h2><ul class="list health-list">${health}</ul></section></section>`;
}
