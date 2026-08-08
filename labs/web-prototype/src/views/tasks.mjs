import { hrefFor } from '../router.mjs';
import { selectActionableTasks } from '../state.mjs';
import { escapeHtml, statusBadge } from '../ui.mjs';

const views = [
  ['actionable', '待处理'], ['running', '运行中'], ['all', '全部'], ['completed', '已完成'],
];

function tasksForView(state, route) {
  const view = route.params.view ?? 'actionable';
  let tasks = view === 'actionable'
    ? selectActionableTasks(state)
    : view === 'running'
      ? state.tasks.filter((task) => task.status === 'running')
      : view === 'completed'
        ? state.tasks.filter((task) => ['success', 'partial'].includes(task.status))
        : state.tasks;
  if (route.params.stage) tasks = tasks.filter((task) => task.stage === route.params.stage);
  if (route.params.library) tasks = tasks.filter((task) => task.libraryId === route.params.library);
  return tasks;
}

/**
 * 渲染具有稳定视图及可选阶段、资源库筛选器的任务中心。
 *
 * @param {{ tasks: object[], libraries: object[] }} state - 包含任务和资源库记录的原型状态。
 * @param {{ params: { view?: string, stage?: string, library?: string } }} route - 当前任务筛选查询。
 * @returns {string} 筛选控件和任务列表 HTML；渲染器不会变更状态。
 */
export function renderTasks(state, route) {
  const view = route.params.view ?? 'actionable';
  const tabs = views.map(([value, label]) => `<a class="button${view === value ? ' button--primary' : ''}" href="${hrefFor('tasks', { view: value })}">${label}</a>`).join('');
  const selected = (actual, expected) => actual === expected ? ' selected' : '';
  const stages = [['', '全部阶段'], ['discovery', '发现'], ['identification', '识别'], ['planning', '计划'], ['organization', '文件操作'], ['nfo', 'NFO'], ['complete', '完成']];
  const stageOptions = stages.map(([value, label]) => `<option value="${value}"${selected(route.params.stage ?? '', value)}>${label}</option>`).join('');
  const libraryOptions = [['', '全部资源库'], ...state.libraries.map((library) => [library.id, library.label])].map(([value, label]) => `<option value="${value}"${selected(route.params.library ?? '', value)}>${escapeHtml(label)}</option>`).join('');
  const items = tasksForView(state, route).map((task) => `<li class="list-item"><a href="${hrefFor('task', { id: task.id, view })}"><strong>${escapeHtml(task.title)}</strong></a><div>${statusBadge(task.status)} · ${escapeHtml(task.stage)}</div><small>${escapeHtml(task.nextAction)}</small></li>`).join('');
  return `<section class="page"><header class="page-header"><h1>任务中心</h1></header><nav class="nav-tabs" aria-label="任务视图">${tabs}</nav><div class="filters"><label>阶段<select data-filter="stage">${stageOptions}</select></label><label>资源库<select data-filter="library">${libraryOptions}</select></label></div><ul class="list">${items}</ul></section>`;
}

/**
 * 渲染任务证据及当前状态允许的恢复或后续操作。
 *
 * @param {{ tasks: object[], genericTargets: object[] }} state - 包含任务和通用目标记录的原型状态。
 * @param {string} taskId - 由详情路由选定的标识。
 * @returns {string} 任务详情 HTML；无匹配任务时返回不抛错的未找到卡片。
 */
export function renderTaskDetail(state, taskId) {
  const task = state.tasks.find((item) => item.id === taskId);
  if (!task) return '<section class="card"><h1>任务不存在</h1><a class="button" href="#/tasks">返回任务中心</a></section>';
  const followUpHref = task.status === 'success'
    ? hrefFor('media')
    : task.status === 'running'
      ? hrefFor('tasks', { view: 'running' })
      : hrefFor('libraries');
  const action = task.status === 'waiting'
    ? `<button class="button button--primary" data-task-id="${task.id}" data-event="confirm-identity">选择候选并重新计算计划</button>`
    : task.status === 'partial'
      ? `<button class="button button--primary" data-task-id="${task.id}" data-event="retry-nfo">只重试 NFO 写入</button>`
      : `<a class="button" href="${followUpHref}">${escapeHtml(task.nextAction)}</a>`;
  const preserved = task.localResultPreserved ? '<p class="notice">本地结果已保留，不会重复执行文件整理。</p>' : '<p class="notice">尚未执行文件修改。</p>';
  const target = state.genericTargets[0];
  const genericChoice = task.status === 'waiting'
    ? `<section class="card"><h2>通用视频整理目标</h2><p><strong>${escapeHtml(target.label)}</strong></p><p class="path">目标根目录：${escapeHtml(target.path)}</p><p class="path">名称格式：${escapeHtml(target.naming)}</p><p>相似内容聚合依据：${escapeHtml(target.grouping)}</p><p>文件夹归类：${escapeHtml(target.folderClassification)}</p><p class="notice">当前存在分组歧义，确认前不会修改文件或写入 NFO。</p><button class="button" data-task-id="${task.id}" data-event="confirm-generic-grouping">确认分组并保存明确规则</button></section>`
    : '';
  return `<section class="page"><header class="page-header"><h1>${escapeHtml(task.title)}</h1>${statusBadge(task.status)}</header><div class="grid grid--2"><section class="card"><h2>阶段</h2><ol class="timeline"><li>发现</li><li>识别或确认</li><li>计划</li><li>文件操作</li><li>NFO</li><li>完成</li></ol></section><section class="card"><h2>当前结果</h2>${preserved}<p>当前阶段：${escapeHtml(task.stage)}</p>${action}</section></div>${genericChoice}<section class="card"><h2>依据与影响</h2><p class="path">来源：/nas/inbox/example.mkv</p><p class="path">目标：/nas/media/example/example.mkv</p><p>操作：按资源库固定规则执行；越权、覆盖或前置条件失败时暂停。</p></section></section>`;
}
