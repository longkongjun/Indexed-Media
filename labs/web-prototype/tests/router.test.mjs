import test from 'node:test';
import assert from 'node:assert/strict';
import { hrefFor, parseHash } from '../src/router.mjs';

test('空 hash 默认进入概览', () => {
  assert.deepEqual(parseHash(''), { page: 'overview', id: null, params: {} });
});

test('任务详情保留筛选参数', () => {
  assert.deepEqual(parseHash('#/task/task-review?view=actionable'), {
    page: 'task', id: 'task-review', params: { view: 'actionable' },
  });
});

test('状态预览参数可被解析', () => {
  assert.deepEqual(parseHash('#/overview?state=offline'), {
    page: 'overview', id: null, params: { state: 'offline' },
  });
});

test('hrefFor 对参数执行 URL 编码', () => {
  assert.equal(hrefFor('tasks', { view: '待处理' }), '#/tasks?view=%E5%BE%85%E5%A4%84%E7%90%86');
});
