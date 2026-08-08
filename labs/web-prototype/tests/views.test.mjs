import test from 'node:test';
import assert from 'node:assert/strict';
import { prototypeData } from '../src/fixtures.mjs';
import { createPrototypeState } from '../src/state.mjs';
import { NAV_ITEMS } from '../src/ui.mjs';
import { renderMedia, renderMediaDetail } from '../src/views/media.mjs';
import { renderOverview } from '../src/views/overview.mjs';
import { renderLibraries, renderSettings, renderSetup } from '../src/views/resources.mjs';
import { renderTaskDetail, renderTasks } from '../src/views/tasks.mjs';

const state = createPrototypeState(prototypeData);

test('一级导航不包含独立收件箱', () => {
  assert.deepEqual(NAV_ITEMS.map((item) => item.label), ['概览', '任务', '媒体', '资源库', '设置']);
});

test('概览显示阶段、待处理和服务状态', () => {
  const html = renderOverview(state);
  assert.match(html, /处理阶段/);
  assert.match(html, /需要处理/);
  assert.match(html, /NFO/);
  assert.match(html, /Jellyfin/);
  assert.match(html, /data-action="external-jellyfin"/);
  assert.match(html, /view=all/);
  assert.match(html, /stage=identification/);
  assert.doesNotMatch(html, /下游验证|待同步/);
});

test('任务中心包含四个稳定视图', () => {
  const html = renderTasks(state, { params: { view: 'actionable' } });
  for (const label of ['待处理', '运行中', '全部', '已完成']) assert.match(html, new RegExp(label));
});

test('任务中心按阶段筛选并保留对应任务', () => {
  const html = renderTasks(state, { params: { view: 'all', stage: 'nfo' } });
  assert.match(html, /NFO 写入失败/);
  assert.doesNotMatch(html, /低置信度识别/);
  assert.doesNotMatch(html, /下游验证|待同步/);
});

test('任务详情提供与状态匹配的恢复或结果动作', () => {
  const html = renderTaskDetail(state, 'task-partial');
  assert.match(html, /本地结果已保留/);
  assert.match(html, /data-event="retry-nfo"/);
  assert.match(html, /只重试 NFO 写入/);
  assert.doesNotMatch(html, /重新执行文件整理/);
  assert.doesNotMatch(html, /Jellyfin|下游验证|待同步/);

  const successHtml = renderTaskDetail(state, 'task-auto');
  assert.match(successHtml, /href="#\/media"[^>]*>查看媒体/);
});

test('通用视频歧义在任务中展示目标、规则与确认动作', () => {
  const html = renderTaskDetail(state, 'task-review');
  assert.match(html, /通用视频整理目标/);
  assert.match(html, /课程与家庭录像/);
  assert.match(html, /来源目录.*规范化名称.*连续编号/);
  assert.match(html, /确认分组并保存明确规则/);
  assert.match(html, /data-event="confirm-generic-grouping"/);
});

test('首次配置按元数据、目录、资源库、规则和摘要排序', () => {
  const html = renderSetup(state, { params: { step: '1' } });
  for (const label of ['元数据', '收件目录', '资源库', '整理规则', '配置摘要']) assert.match(html, new RegExp(label));
  assert.doesNotMatch(html, /Jellyfin|映射/);
});

test('资源库显示本地策略与可复用通用视频整理目标', () => {
  const html = renderLibraries(state);
  assert.match(html, /硬链接/);
  assert.match(html, /保留已有并补齐缺失/);
  assert.match(html, /通用视频整理目标/);
  assert.match(html, /名称格式/);
  assert.match(html, /相似内容聚合/);
  assert.match(html, /文件夹归类/);
  assert.doesNotMatch(html, /Jellyfin|媒体库映射/);
});

test('设置只展示 TMDB、Jellyfin 和系统', () => {
  const html = renderSettings(state);
  assert.match(html, /TMDB/);
  assert.match(html, /Jellyfin/);
  assert.match(html, /data-action="test-connection"/);
  assert.match(html, /data-action="external-jellyfin"/);
  assert.match(html, /不参与资源库和整理任务/);
  assert.doesNotMatch(html, /下载器|Emby|Plex|模型|Bot|插件/);
});

test('媒体列表排除未确认项目并只显示本地结果', () => {
  const html = renderMedia(state, { params: {} });
  assert.doesNotMatch(html, /未知文件/);
  assert.match(html, /沙丘 2/);
  assert.match(html, /本地部分成功/);
  assert.doesNotMatch(html, /Jellyfin|待同步/);
});

test('媒体详情可以追溯本地任务且不提供媒体级 Jellyfin 关联', () => {
  const html = renderMediaDetail(state, 'media-movie');
  assert.match(html, /相关任务/);
  assert.match(html, /本地整理结果/);
  assert.doesNotMatch(html, /Jellyfin|data-action="external-jellyfin"|<video|播放进度|转码/);

  const genericHtml = renderMediaDetail(state, 'media-generic');
  assert.match(genericHtml, /明确规则归类的通用视频/);
  assert.doesNotMatch(genericHtml, /TMDB/);
});
