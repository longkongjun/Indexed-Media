const ACTIONABLE = new Set(['waiting', 'conflict', 'failed', 'partial']);

/**
 * 从兼容夹具的数据创建隔离的原型状态图。
 *
 * @param {object} data - 兼容 `structuredClone` 的夹具图。
 * @returns {object} 状态转换可独立变更的深克隆。
 * @throws {DOMException} 当 `data` 含有 `structuredClone` 不支持的值时抛出。
 */
export function createPrototypeState(data) {
  return structuredClone(data);
}

/**
 * 选择状态需要管理员操作的任务。
 *
 * @param {{ tasks: Array<{ status: string }> }} state - 包含任务记录的原型状态。
 * @returns {object[]} 含等待、冲突、失败和部分成功任务的新数组；不会克隆任务对象。
 */
export function selectActionableTasks(state) {
  return state.tasks.filter((task) => ACTIONABLE.has(task.status));
}

/**
 * 仅选择已正式确认的媒体记录，用于列表和详情页。
 *
 * @param {{ media: Array<{ formal: boolean }> }} state - 包含媒体记录的原型状态。
 * @returns {object[]} 由已确认记录组成的新数组；不会克隆记录对象。
 */
export function selectVisibleMedia(state) {
  return state.media.filter((media) => media.formal);
}

/**
 * 将一个受支持的管理员事件应用于克隆后的任务状态。
 *
 * 绝不变更输入状态。成功的转换只更新选中的任务，
 * 并可能标记明确的通用分组规则或保留已有文件结果。
 *
 * @param {{ tasks: object[] }} state - 兼容 `structuredClone` 的原型状态。
 * @param {string} taskId - 要转换的任务标识。
 * @param {string} event - 受支持事件（`confirm-identity`、`confirm-generic-grouping` 或 `retry-nfo`）。
 * @returns {object} 包含已应用转换的深克隆状态。
 * @throws {Error} 当任务未知或事件对其当前状态无效时抛出。
 * @throws {DOMException} 当状态无法进行结构化克隆时抛出。
 */
export function transitionTask(state, taskId, event) {
  const next = structuredClone(state);
  const task = next.tasks.find((item) => item.id === taskId);
  if (!task) throw new Error(`Unknown task: ${taskId}`);

  if (event === 'confirm-identity' && task.status === 'waiting') {
    task.status = 'running';
    task.stage = 'planning';
    task.nextAction = '查看整理计划';
    return next;
  }

  if (event === 'confirm-generic-grouping' && task.status === 'waiting') {
    task.status = 'running';
    task.stage = 'planning';
    task.chosenKind = 'generic';
    task.savedExplicitRule = true;
    task.nextAction = '查看通用视频整理计划';
    return next;
  }

  if (event === 'retry-nfo' && task.status === 'partial') {
    task.status = 'running';
    task.stage = 'nfo';
    task.nextAction = '查看 NFO 写入进度';
    task.localResultPreserved = true;
    return next;
  }

  throw new Error(`Invalid transition: ${task.status} -> ${event}`);
}
