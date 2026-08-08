/**
 * 使用具名的加载、空、错误、离线、冲突或部分状态面板替换普通页面内容。
 *
 * @param {string | undefined} mode - 从路由查询中选择的可选预览模式。
 * @param {string} content - 没有适用的已识别边界时返回的普通页面 HTML。
 * @returns {string} 已识别模式对应的可访问状态面板 HTML；否则原样返回 `content`。
 */
export function renderStateBoundary(mode, content) {
  if (!mode) return content;
  const states = {
    loading: '<section class="state-panel" aria-busy="true"><div class="skeleton"></div><div class="skeleton"></div><p>正在加载页面数据</p></section>',
    empty: '<section class="state-panel" aria-live="polite"><h2>暂无内容</h2><p>完成首次配置后，处理结果会显示在这里。</p><a class="button button--primary" href="#/setup?step=1">开始配置</a></section>',
    error: '<section class="state-panel state-panel--danger" aria-live="assertive"><h2>页面数据加载失败</h2><p>Core 返回错误；已有任务结果没有被清除。</p><button class="button button--primary" data-action="reload">重试</button></section>',
    offline: '<section class="state-panel state-panel--warning" aria-live="assertive"><h2>Core 当前不可达</h2><p>需要服务端确认的操作已禁用，请恢复连接后重试。</p><button class="button" data-action="reload">重新检查连接</button></section>',
    conflict: '<section class="state-panel state-panel--danger" aria-live="assertive"><h2>文件计划存在冲突</h2><p>默认不覆盖、不删除、不执行文件操作。</p><a class="button button--primary" href="#/task/task-conflict">查看来源与目标</a></section>',
    partial: '<section class="state-panel state-panel--warning" aria-live="polite"><h2>文件整理成功，NFO 写入失败</h2><p>文件结果已保留，只重试 NFO 写入。</p><a class="button button--primary" href="#/task/task-partial">查看部分成功任务</a></section>',
  };
  return states[mode] ?? content;
}
