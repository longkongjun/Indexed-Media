import { hrefFor } from '../router.mjs';
import { selectVisibleMedia } from '../state.mjs';
import { escapeHtml, statusBadge } from '../ui.mjs';

const kindLabel = { movie: '电影', series: '剧集', generic: '通用视频' };

/**
 * 渲染带可选类型筛选器的已确认本地媒体网格。
 *
 * @param {object} state - 包含媒体记录的原型状态。
 * @param {{ params: { kind?: string } }} route - 路由参数；缺少 `kind` 表示所有类型。
 * @returns {string} 媒体页面 HTML；忽略未确认记录且不启动播放。
 */
export function renderMedia(state, route) {
  const kind = route.params.kind ?? 'all';
  const items = selectVisibleMedia(state).filter((media) => kind === 'all' || media.kind === kind);
  const tabs = [['all', '全部'], ['movie', '电影'], ['series', '剧集'], ['generic', '通用视频']].map(([value, label]) => `<a class="button${kind === value ? ' button--primary' : ''}" href="${hrefFor('media', { kind: value })}">${label}</a>`).join('');
  const cards = items.map((media) => `<article class="media-card"><div class="poster" aria-hidden="true">${escapeHtml(media.title.slice(0, 1))}</div><h2><a href="${hrefFor('media-detail', { id: media.id })}">${escapeHtml(media.title)}</a></h2><p>${kindLabel[media.kind]}</p>${media.result === 'partial' ? '<span class="status status--partial">◐ 本地部分成功</span>' : statusBadge('success')}</article>`).join('');
  return `<section class="page"><header class="page-header"><h1>媒体</h1></header><nav class="nav-tabs" aria-label="媒体分类">${tabs}</nav><div class="media-grid">${cards}</div></section>`;
}

/**
 * 为一条已确认媒体记录渲染可追溯的本地结果详情。
 *
 * @param {object} state - 包含媒体记录的原型状态。
 * @param {string} mediaId - 由路由选定的媒体标识。
 * @returns {string} 详情 HTML；记录缺失或未确认时返回不抛错的未找到卡片。
 */
export function renderMediaDetail(state, mediaId) {
  const media = selectVisibleMedia(state).find((item) => item.id === mediaId);
  if (!media) return '<section class="card"><h1>媒体不存在</h1><a class="button" href="#/media">返回媒体</a></section>';
  const evidence = media.kind === 'generic' ? '明确规则归类的通用视频' : '文件名 + NFO + TMDB 或人工输入';
  const result = media.result === 'partial' ? '本地部分成功' : '本地整理成功';
  return `<section class="page"><header class="page-header"><h1>${escapeHtml(media.title)}</h1></header><div class="detail-hero"><div class="poster poster--large" aria-hidden="true">${escapeHtml(media.title.slice(0, 1))}</div><section class="card"><h2>媒体信息</h2><dl><dt>类型</dt><dd>${kindLabel[media.kind]}</dd><dt>本地整理结果</dt><dd>${result}</dd><dt>识别依据</dt><dd>${evidence}</dd></dl></section></div><section class="card"><h2>文件与结果</h2><p class="path">来源：/nas/inbox/example.mkv</p><p class="path">当前：/nas/media/example/example.mkv</p><p>NFO：保留已有字段并补齐允许字段</p><a class="button" href="${hrefFor('task', { id: media.taskId })}">相关任务</a></section></section>`;
}
