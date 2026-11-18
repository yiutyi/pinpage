// 功能：验证内容页导航桥的链接分类：首页开新标签、子页面替换当前标签、外站交给浏览器。
// 脚本本身写在 Rust 源文件里，测试直接提取并在模拟 DOM 环境执行，确保实现与用例同步。
import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const rustSource = readFileSync(
  new URL('../src-tauri/src/windows/navigation.rs', import.meta.url),
  'utf8',
);
const bridge = rustSource.match(/r#"([\s\S]*?)"#/);
assert.ok(bridge, '未找到注入脚本');
const script = bridge[1];

const flush = () => new Promise((resolve) => setImmediate(resolve));

/// 构造一个最小页面环境；bridge 对应 Rust 注入的首页地址与「是否首页标签」。
function harness({ href = 'http://192.168.18.10/', isHome = true, home = href, ipc = 'ok' } = {}) {
  const calls = { nativeOpen: [], ipc: [] };
  const base = new URL(href);
  const location = { href, origin: base.origin, hostname: base.hostname };
  const listeners = {};
  const internals = {
    invoke(command, payload) {
      calls.ipc.push({ command, payload });
      return ipc === 'reject'
        ? Promise.reject(new Error('permission denied'))
        : Promise.resolve(null);
    },
  };
  const window = {
    open(...args) {
      calls.nativeOpen.push(args);
      return null;
    },
    __pinpageBridge: { home: new URL(home).href, isHome },
    __TAURI_INTERNALS__: ipc === 'missing' ? undefined : internals,
  };
  const document = {
    addEventListener(type, listener, capture) {
      listeners[`${type}:${capture ? 'capture' : 'bubble'}`] = listener;
    },
  };
  const context = vm.createContext({
    window,
    document,
    location,
    URL,
    Date,
    Promise,
    Number,
    String,
    RegExp,
    console,
  });
  vm.runInContext(script, context, { filename: 'navigation-bridge.js' });

  function dispatch(listener, linkHref, options) {
    if (!listener) return null;
    const { download = false, target = '', button = 0, omitButton = false } = options;
    const anchor = {
      hasAttribute: (name) => download && name === 'download',
      getAttribute: (name) => {
        if (name === 'href') return linkHref;
        if (name === 'target') return target;
        return null;
      },
    };
    const event = {
      defaultPrevented: options.defaultPrevented === true,
      target: {
        closest: (selector) =>
          selector === 'a[target="_blank"]'
            ? target === '_blank'
              ? anchor
              : null
            : selector === 'a[href]'
              ? anchor
              : null,
      },
      preventDefault() {
        this.defaultPrevented = true;
        calls.prevented = (calls.prevented ?? 0) + 1;
      },
    };
    if (!omitButton) event.button = button;
    listener(event);
    return event;
  }

  return {
    calls,
    location,
    open: window.open,
    /// 模拟 target=_blank 链接点击，走捕获阶段的拦截。
    clickBlank(linkHref, options = {}) {
      return dispatch(listeners['click:capture'], linkHref, { ...options, target: '_blank' });
    },
    /// 模拟首页里的普通链接点击，走冒泡阶段的分流。
    click(linkHref, options = {}) {
      return dispatch(listeners['click:bubble'], linkHref, options);
    },
  };
}

test('注入脚本语法可被页面直接执行', () => {
  assert.doesNotThrow(() => new Function(script));
});

test('首页里的普通站内链接打开新标签而不是替换页面', () => {
  for (const target of [
    'http://localhost:4560',
    'http://192.168.18.10:8080/poem',
    'http://192.168.18.11/',
    'http://127.0.0.1:8000',
    'http://10.1.2.3:9000',
    'http://172.16.5.4/',
    'http://nas:8080/',
    'http://[fe80::1]:8080/',
  ]) {
    const page = harness();
    const event = page.click(target);
    assert.equal(event.defaultPrevented, true, `应接管 ${target}`);
    assert.equal(page.calls.ipc.length, 1, `应请求打开标签 ${target}`);
    assert.equal(page.calls.ipc[0].command, 'open_tab');
    assert.equal(page.calls.ipc[0].payload.url, new URL(target).href);
    assert.equal(page.location.href, 'http://192.168.18.10/', '首页本身不应跳转');
    assert.equal(page.calls.nativeOpen.length, 0);
  }
});

test('相对路径与同主机不同端口在首页也开新标签', () => {
  const relative = harness();
  relative.click('/poem');
  assert.equal(relative.calls.ipc[0].payload.url, 'http://192.168.18.10/poem');

  const publicSite = harness({ href: 'https://portal.example.com/' });
  publicSite.click('https://portal.example.com:8443/console');
  assert.equal(publicSite.calls.ipc[0].command, 'open_tab');
  assert.equal(publicSite.calls.ipc[0].payload.url, 'https://portal.example.com:8443/console');
});

test('首页里的普通外站链接交给系统浏览器，不在应用内打开', () => {
  const page = harness();
  const event = page.click('https://example.com/docs');
  assert.equal(event.defaultPrevented, true);
  assert.equal(page.calls.ipc.length, 1);
  assert.equal(page.calls.ipc[0].command, 'open_external');
  assert.equal(page.calls.ipc[0].payload.url, 'https://example.com/docs');
  assert.equal(page.location.href, 'http://192.168.18.10/');
});

test('子页面里的普通站内链接不接管，交给浏览器在当前标签内跳转', () => {
  const page = harness({ isHome: false });
  const event = page.click('http://localhost:5000/app');
  assert.equal(event.defaultPrevented, false);
  assert.equal(page.calls.ipc.length, 0);
  assert.equal(page.location.href, 'http://192.168.18.10/');
});

test('首页标签跳转到子页面后，链接恢复为浏览器默认跳转', () => {
  const page = harness();
  page.location.href = 'http://192.168.18.10/poem';
  const event = page.click('http://192.168.18.10/music');
  assert.equal(event.defaultPrevented, false);
  assert.equal(page.calls.ipc.length, 0);
});

test('公网链接通过受控命令交给系统默认浏览器', async () => {
  const page = harness();
  page.open('https://example.com/docs', '_blank', 'noopener');
  assert.equal(page.location.href, 'http://192.168.18.10/');
  assert.equal(page.calls.ipc.length, 1);
  assert.equal(page.calls.ipc[0].command, 'open_external');
  assert.equal(page.calls.ipc[0].payload.url, 'https://example.com/docs');
  await flush();
  assert.equal(page.calls.nativeOpen.length, 0, '命令成功时不应再弹新窗口');
});

test('target=_blank 的站内链接始终开新标签，外站仍交给浏览器', () => {
  const inside = harness({ isHome: false });
  const insideEvent = inside.clickBlank('http://localhost:5000/app');
  assert.equal(insideEvent.defaultPrevented, true);
  assert.equal(inside.calls.ipc[0].command, 'open_tab');
  assert.equal(inside.location.href, 'http://192.168.18.10/');

  const outside = harness();
  const outsideEvent = outside.clickBlank('https://example.com/');
  assert.equal(outsideEvent.defaultPrevented, true);
  assert.equal(outside.calls.ipc[0].command, 'open_external');
  assert.equal(outside.calls.ipc[0].payload.url, 'https://example.com/');
  assert.equal(outside.location.href, 'http://192.168.18.10/');
});

test('打开标签失败时回退到当前标签内跳转', async () => {
  for (const ipc of ['reject', 'missing']) {
    const page = harness({ ipc });
    page.click('http://192.168.18.10/poem');
    await flush();
    assert.equal(page.location.href, 'http://192.168.18.10/poem', `应回退：${ipc}`);
  }
});

test('命令不可用时公网链接回退到不带 noopener 的原生新窗口请求', async () => {
  for (const ipc of ['reject', 'missing']) {
    const page = harness({ ipc });
    page.open('https://example.com/', '_blank', 'noopener');
    await flush();
    assert.deepEqual(
      page.calls.nativeOpen[0],
      ['https://example.com/', '_blank'],
      `回退链路应生效：${ipc}`,
    );
  }
});

test('下载属性、内嵌凭据与非 HTTP 协议不接管', () => {
  const download = harness();
  const downloadEvent = download.click('http://localhost:8000/file.zip', { download: true });
  assert.equal(downloadEvent.defaultPrevented, false);
  assert.equal(download.location.href, 'http://192.168.18.10/');

  const credential = harness();
  credential.click('https://user@192.168.18.10/poem');
  assert.equal(credential.location.href, 'http://192.168.18.10/');
  assert.equal(credential.calls.ipc.length, 0);

  const mailto = harness();
  mailto.open('mailto:hello@example.com', '_blank');
  assert.equal(mailto.calls.nativeOpen.length, 1);
  assert.equal(mailto.calls.nativeOpen[0][0], 'mailto:hello@example.com');
  assert.equal(mailto.calls.nativeOpen[0][1], '_blank');
});

test('页面自己已处理的链接保持原行为', () => {
  const page = harness();
  const event = page.click('http://192.168.18.10/poem', { defaultPrevented: true });
  assert.equal(event.defaultPrevented, true);
  assert.equal(page.calls.ipc.length, 0, '已 preventDefault 的链接不再开标签');
});

test('公网域名不会被 IPv6 内网前缀误判，重复点击也不会双开浏览器', () => {
  const page = harness();
  page.open('http://feast.example.com/', '_blank');
  assert.equal(page.calls.ipc.length, 1);
  assert.equal(page.calls.ipc[0].payload.url, 'http://feast.example.com/');

  page.open('http://feast.example.com/', '_blank');
  assert.equal(page.calls.ipc.length, 1, '同一地址 1 秒内只受理一次');
});

test('脚本派发的合成点击未带 button 字段时仍按链接规则分流', () => {
  const page = harness();
  const event = page.clickBlank('http://localhost:4560/', { omitButton: true });
  assert.equal(event.defaultPrevented, true);
  assert.equal(page.calls.ipc[0].command, 'open_tab');
  assert.equal(page.location.href, 'http://192.168.18.10/');
});
