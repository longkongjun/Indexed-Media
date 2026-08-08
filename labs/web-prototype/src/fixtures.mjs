/**
 * 提供用于初始化离线原型的外观可变夹具图。
 *
 * 调用方应在状态转换前克隆此值，以避免测试和页面渲染共享变更；
 * 路径、健康状态和任务结果均为示例性本地数据。
 *
 * @type {{ health: object[], tasks: object[], media: object[], sources: object[], libraries: object[], genericTargets: object[] }}
 */
export const prototypeData = {
  health: [
    { id: 'core', label: 'Core', status: 'healthy' },
    { id: 'inbox', label: '收件目录', status: 'healthy' },
    { id: 'tmdb', label: 'TMDB', status: 'healthy' },
    { id: 'jellyfin', label: 'Jellyfin', status: 'degraded' },
  ],
  tasks: [
    { id: 'task-auto', title: '电影自动整理', status: 'success', stage: 'complete', libraryId: 'movies', nextAction: '查看媒体', localResultPreserved: true },
    { id: 'task-review', title: '低置信度识别', status: 'waiting', stage: 'identification', libraryId: 'series', nextAction: '选择候选或通用视频目标', localResultPreserved: false },
    { id: 'task-conflict', title: '目标文件已存在', status: 'conflict', stage: 'planning', libraryId: 'movies', nextAction: '修正规则', localResultPreserved: false },
    { id: 'task-failed', title: '目标路径越过授权目录', status: 'failed', stage: 'safety-check', libraryId: 'movies', nextAction: '查看越权路径', localResultPreserved: false },
    { id: 'task-partial', title: 'NFO 写入失败', status: 'partial', stage: 'nfo', libraryId: 'movies', nextAction: '只重试 NFO 写入', localResultPreserved: true },
    { id: 'task-recovery', title: '中断后恢复原任务', status: 'running', stage: 'organization', libraryId: 'series', nextAction: '查看恢复进度', localResultPreserved: true },
  ],
  media: [
    { id: 'media-movie', title: '银翼杀手 2049', kind: 'movie', formal: true, result: 'success', taskId: 'task-auto' },
    { id: 'media-series', title: '绝命毒师', kind: 'series', formal: true, result: 'success', taskId: 'task-recovery' },
    { id: 'media-generic', title: '家庭课程录像', kind: 'generic', formal: true, result: 'success', taskId: 'task-review' },
    { id: 'media-partial', title: '沙丘 2', kind: 'movie', formal: true, result: 'partial', taskId: 'task-partial' },
    { id: 'media-unconfirmed', title: '未知文件', kind: 'movie', formal: false, result: 'waiting', taskId: 'task-review' },
  ],
  sources: [{ id: 'source-inbox', label: '专用收件目录', path: '/nas/inbox', status: 'watching' }],
  libraries: [
    { id: 'movies', label: '电影资源库', path: '/nas/media/movies', operation: 'hardlink', nfo: 'preserve-and-fill' },
    { id: 'series', label: '剧集资源库', path: '/nas/media/series', operation: 'move', nfo: 'preserve-and-fill' },
  ],
  genericTargets: [
    {
      id: 'generic-courses',
      label: '课程与家庭录像',
      path: '/nas/media/generic',
      operation: 'move',
      naming: '{group}/{normalizedTitle} - {sequence}',
      grouping: '来源目录 + 规范化名称 + 连续编号',
      folderClassification: '按已确认分组建立一级文件夹',
      nfo: '简单本地标题与分组信息',
    },
  ],
};

/**
 * 将每个 MVP 验收标识映射到展示该标识的原型流程。
 *
 * 此列表由范围检查和测试使用，没有运行时副作用。
 *
 * @type {Array<{ id: string, flows: string[] }>}
 */
export const scenarioCoverage = [
  { id: 'MVP-AC-001', flows: ['P2', 'P7'] },
  { id: 'MVP-AC-002', flows: ['P2', 'P7'] },
  { id: 'MVP-AC-003', flows: ['P3'] },
  { id: 'MVP-AC-004', flows: ['P3', 'P7'] },
  { id: 'MVP-AC-005', flows: ['P4'] },
  { id: 'MVP-AC-006', flows: ['P4'] },
  { id: 'MVP-AC-007', flows: ['P4', 'P7'] },
  { id: 'MVP-AC-008', flows: ['P2', 'P5'] },
  { id: 'MVP-AC-009', flows: ['P6'] },
  { id: 'MVP-AC-010', flows: ['P4'] },
];
