// 功能：验证工具条首帧状态、部分事件合并，以及工具条页面持有订阅状态事件的权限。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import {
  canCloseTab,
  initialStatus,
  mergeStatus,
  tabStripVisible,
} from '../src/features/toolbar/status.ts';

test('桌面模式首帧就显示唤出按钮', () => {
  assert.deepEqual(initialStatus('desktop'), {
    pinned: false,
    lifted: false,
    liftable: true,
    tabs: [],
  });
});

test('浮动模式与缺少模式参数时隐藏唤出按钮', () => {
  assert.equal(initialStatus('floating').liftable, false);
  assert.equal(initialStatus(null).liftable, false);
});

test('缺字段的事件不会把唤出按钮清掉', () => {
  const current = { pinned: false, lifted: false, liftable: true, tabs: [] };
  assert.deepEqual(mergeStatus(current, { lifted: true }), {
    pinned: false,
    lifted: true,
    liftable: true,
    tabs: [],
  });
});

test('事件能更新固定、唤出与可唤出状态', () => {
  const current = { pinned: false, lifted: true, liftable: true, tabs: [] };
  assert.deepEqual(mergeStatus(current, { pinned: true, lifted: false, liftable: false }), {
    pinned: true,
    lifted: false,
    liftable: false,
    tabs: [],
  });
});

test('空事件保持当前状态不变', () => {
  const current = { pinned: true, lifted: false, liftable: true, tabs: [] };
  assert.equal(mergeStatus(current, null), current);
  assert.deepEqual(mergeStatus(current, {}), current);
});

test('工具条页面被允许订阅状态事件', () => {
  const capability = JSON.parse(
    readFileSync(new URL('../src-tauri/capabilities/toolbar.json', import.meta.url), 'utf8'),
  );
  assert.ok(capability.webviews.includes('bar-*'));
  for (const permission of [
    'core:event:allow-listen',
    'core:event:allow-unlisten',
    'allow-toolbar-action',
  ]) {
    assert.ok(capability.permissions.includes(permission), `缺少权限 ${permission}`);
  }
});

test('两个以上标签才显示标签条', () => {
  assert.equal(tabStripVisible([]), false);
  assert.equal(
    tabStripVisible([{ window: 'p-1', title: '首页', active: true, home: true }]),
    false,
  );
  assert.equal(
    tabStripVisible([
      { window: 'p-1', title: '首页', active: true, home: true },
      { window: 'p-1-t1', title: 'poem', active: false, home: false },
    ]),
    true,
  );
});

test('首页标签不可关闭，额外标签可以关闭', () => {
  assert.equal(canCloseTab({ window: 'p-1', title: '首页', active: true, home: true }), false);
  assert.equal(canCloseTab({ window: 'p-1-t1', title: 'poem', active: false, home: false }), true);
});

test('标签事件替换整张列表而不是逐项合并', () => {
  const current = {
    pinned: false,
    lifted: false,
    liftable: true,
    tabs: [{ window: 'p-1', title: '首页', active: true, home: true }],
  };
  const next = [
    { window: 'p-1', title: '首页', active: false, home: true },
    { window: 'p-1-t1', title: 'music', active: true, home: false },
  ];
  assert.deepEqual(mergeStatus(current, { tabs: next }).tabs, next);
  assert.equal(mergeStatus(current, { tabs: [] }).tabs.length, 0);
});

test('收起只作用于工具条，不再随唤出状态改变语义', () => {
  const source = readFileSync(
    new URL('../src/features/toolbar/ToolbarRoot.vue', import.meta.url),
    'utf8',
  );
  assert.match(source, /@click="run\('hide'\)"/);
  assert.doesNotMatch(source, /hideToDesktop/);
  for (const locale of ['zh-CN', 'en-US']) {
    const ui = JSON.parse(
      readFileSync(new URL(`../locales/${locale}/ui.json`, import.meta.url), 'utf8'),
    );
    assert.equal(ui.toolbar.hideToDesktop, undefined, `${locale} 仍保留旧文案`);
  }
});
