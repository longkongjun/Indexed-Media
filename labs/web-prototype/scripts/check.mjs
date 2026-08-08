import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { scenarioCoverage } from '../src/fixtures.mjs';
import { NAV_ITEMS } from '../src/ui.mjs';

const files = [
  'index.html', 'src/app.mjs', 'src/fixtures.mjs', 'src/state.mjs', 'src/router.mjs', 'src/ui.mjs',
  'src/views/overview.mjs', 'src/views/tasks.mjs', 'src/views/resources.mjs', 'src/views/media.mjs', 'src/views/states.mjs',
];
const root = new URL('../', import.meta.url);
const source = (await Promise.all(files.map((file) => readFile(new URL(file, root), 'utf8')))).join('\n');

assert.equal(scenarioCoverage.length, 10);
assert.deepEqual(NAV_ITEMS.map((item) => item.label), ['概览', '任务', '媒体', '资源库', '设置']);
for (const forbidden of ['fetch(', 'XMLHttpRequest', 'WebSocket', 'EventSource', 'apps/web', 'archive/']) {
  assert.equal(source.includes(forbidden), false, `forbidden prototype dependency: ${forbidden}`);
}
for (const external of ['https://', 'http://']) {
  assert.equal(source.includes(external), false, `external URL found: ${external}`);
}
console.log('PASS prototype-scope scenarios=10 navigation=5 network=none formal-dependencies=none');
