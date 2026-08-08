import test from 'node:test';
import assert from 'node:assert/strict';
import { prototypeData, scenarioCoverage } from '../src/fixtures.mjs';
import {
  createPrototypeState,
  selectActionableTasks,
  selectVisibleMedia,
  transitionTask,
} from '../src/state.mjs';

test('10 个 MVP 验收场景都有原型流程映射', () => {
  assert.deepEqual(
    scenarioCoverage.map((item) => item.id),
    Array.from({ length: 10 }, (_, index) => `MVP-AC-${String(index + 1).padStart(3, '0')}`),
  );
});

test('待确认、冲突、失败和部分成功进入待处理', () => {
  const state = createPrototypeState(prototypeData);
  assert.deepEqual(
    selectActionableTasks(state).map((task) => task.status).sort(),
    ['conflict', 'failed', 'partial', 'waiting'],
  );
});

test('未确认媒体不会进入媒体列表，本地部分成功媒体会进入', () => {
  const state = createPrototypeState(prototypeData);
  const visible = selectVisibleMedia(state);
  assert.equal(visible.some((media) => media.id === 'media-unconfirmed'), false);
  assert.equal(visible.some((media) => media.id === 'media-partial'), true);
});

test('本地部分成功只重试 NFO 阶段并保留文件结果', () => {
  const state = createPrototypeState(prototypeData);
  const next = transitionTask(state, 'task-partial', 'retry-nfo');
  const task = next.tasks.find((item) => item.id === 'task-partial');
  assert.equal(task.status, 'running');
  assert.equal(task.stage, 'nfo');
  assert.equal(task.localResultPreserved, true);
});

test('通用视频歧义确认会保存明确规则并重新计算计划', () => {
  const state = createPrototypeState(prototypeData);
  const next = transitionTask(state, 'task-review', 'confirm-generic-grouping');
  const task = next.tasks.find((item) => item.id === 'task-review');
  assert.equal(task.status, 'running');
  assert.equal(task.stage, 'planning');
  assert.equal(task.chosenKind, 'generic');
  assert.equal(task.savedExplicitRule, true);
  assert.equal(task.nextAction, '查看通用视频整理计划');
});
