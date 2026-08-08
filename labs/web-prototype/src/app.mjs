import { prototypeData } from './fixtures.mjs';
import { hrefFor, parseHash } from './router.mjs';
import { createPrototypeState, selectActionableTasks, transitionTask } from './state.mjs';
import { renderShell } from './ui.mjs';
import { renderMedia, renderMediaDetail } from './views/media.mjs';
import { renderOverview } from './views/overview.mjs';
import { renderLibraries, renderSettings, renderSetup } from './views/resources.mjs';
import { renderStateBoundary } from './views/states.mjs';
import { renderTaskDetail, renderTasks } from './views/tasks.mjs';

let state = createPrototypeState(prototypeData);

function renderCurrentPage(route) {
  if (route.page === 'overview') return renderOverview(state);
  if (route.page === 'tasks') return renderTasks(state, route);
  if (route.page === 'task') return renderTaskDetail(state, route.id);
  if (route.page === 'media') return renderMedia(state, route);
  if (route.page === 'media-detail') return renderMediaDetail(state, route.id);
  if (route.page === 'setup') return renderSetup(state, route);
  if (route.page === 'libraries') return renderLibraries(state);
  if (route.page === 'settings') return renderSettings(state);
  if (route.page === 'more') return '<section class="page"><header class="page-header"><h1>更多</h1></header><div class="grid"><a class="card" href="#/libraries"><strong>资源库</strong><p>收件目录、资源库、规则和整理目标</p></a><a class="card" href="#/settings"><strong>设置</strong><p>TMDB、Jellyfin 和系统状态</p></a></div></section>';
  return '<section class="page"><header class="page-header"><h1>页面不存在</h1></header><a class="button" href="#/overview">返回概览</a></section>';
}

/**
 * 将当前 location hash 选定的路由渲染到原型应用外壳中。
 *
 * 读取浏览器位置和模块本地的原型状态，然后替换 `#app` 的内容；
 * 渲染和 DOM 查询失败会向上传播。
 *
 * @returns {void}
 */
export function renderApp() {
  const route = parseHash(window.location.hash);
  const body = renderStateBoundary(route.params.state, renderCurrentPage(route));
  document.querySelector('#app').innerHTML = renderShell({
    route,
    body,
    actionableCount: selectActionableTasks(state).length,
  });
}

window.addEventListener('hashchange', renderApp);
renderApp();

document.addEventListener('click', (event) => {
  const reload = event.target.closest('[data-action="reload"]');
  if (reload) {
    event.preventDefault();
    window.location.hash = '#/overview';
    return;
  }

  const external = event.target.closest('[data-action="external-jellyfin"]');
  if (external) {
    event.preventDefault();
    window.alert('原型仅演示跳转边界；不会连接或打开真实 Jellyfin。');
    return;
  }

  const connection = event.target.closest('[data-action="test-connection"]');
  if (connection) {
    event.preventDefault();
    window.alert(`原型连接测试：${connection.dataset.connection} 返回模拟健康结果。`);
    return;
  }

  const button = event.target.closest('[data-event][data-task-id]');
  if (!button) return;
  state = transitionTask(state, button.dataset.taskId, button.dataset.event);
  renderApp();
});

document.addEventListener('change', (event) => {
  const filter = event.target.closest('[data-filter]');
  if (!filter) return;
  const route = parseHash(window.location.hash);
  const params = { ...route.params };
  if (filter.value) params[filter.dataset.filter] = filter.value;
  else delete params[filter.dataset.filter];
  window.location.hash = hrefFor('tasks', params);
});
