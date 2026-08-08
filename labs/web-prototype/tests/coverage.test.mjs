import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { scenarioCoverage } from '../src/fixtures.mjs';
import { NAV_ITEMS } from '../src/ui.mjs';
import { renderStateBoundary } from '../src/views/states.mjs';

test('验收场景恰好覆盖 001 到 010', () => {
  assert.deepEqual(
    scenarioCoverage.map((item) => item.id),
    Array.from({ length: 10 }, (_, index) => `MVP-AC-${String(index + 1).padStart(3, '0')}`),
  );
  assert.equal(scenarioCoverage.every((item) => item.flows.length > 0), true);
  const readme = readFileSync(new URL('../README.md', import.meta.url), 'utf8');
  for (const flow of ['P1', 'P2', 'P3', 'P4', 'P5', 'P6', 'P7']) assert.match(readme, new RegExp(`\\b${flow}\\b`));
});

test('一级导航只包含已确认的五个入口', () => {
  assert.deepEqual(NAV_ITEMS.map((item) => item.label), ['概览', '任务', '媒体', '资源库', '设置']);
  const responsiveCss = readFileSync(new URL('../styles/responsive.css', import.meta.url), 'utf8');
  assert.match(responsiveCss, /\.sidebar \.nav-link > span:nth-child\(2\), \.sidebar \.nav-badge/);
});

test('六种统一状态都有文字语义，可操作状态都有恢复入口', () => {
  for (const mode of ['loading', 'empty', 'error', 'offline', 'conflict', 'partial']) {
    const html = renderStateBoundary(mode, '<p>正常内容</p>');
    assert.match(html, /aria-live|aria-busy/);
    assert.doesNotMatch(html, /^\s*$/);
    if (mode !== 'loading') assert.match(html, /<a |<button /);
  }
});

test('部分成功状态只描述本地 NFO 恢复，不依赖 Jellyfin', () => {
  const html = renderStateBoundary('partial', '<p>正常内容</p>');
  assert.match(html, /NFO 写入失败/);
  assert.match(html, /只重试 NFO/);
  assert.doesNotMatch(html, /Jellyfin|下游|待同步/);
});
