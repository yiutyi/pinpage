// 功能：内容页导航桥，决定站内链接是在首页开新标签、替换当前页还是交给默认浏览器。
//
// WebView2 在 window.open(url, '_blank', 'noopener') 下，NewWindowRequested 事件
// 携带的 Uri 为空；tauri-runtime-wry 会在 URL 解析失败时直接 Deny，连 Rust 侧的
// on_new_window 都不会被调用。门户类站点大量使用这种写法，因此必须在文档创建时
// 注入脚本，在页面内把链接分成三类：
//   * 首页标签里的站内/本机/局域网链接 → open_tab，在应用内新开标签；
//   * 子页面里的普通链接 → location.href，替换当前标签内容；
//   * 其他 HTTP(S) 地址 → 请求应用用系统默认浏览器打开。
// 脚本只调用两个用途受限的命令，不授予窗口管理或文件系统权限；命令失败时回退
// 到不带 noopener 的原生 window.open，由 Rust 的 on_new_window 兜底。
// 内网判定必须与 content.rs::is_local_host 保持一致。
pub const SCRIPT: &str = r#"
(function () {
  if (window.__pinpageNavigationBridge) return;
  window.__pinpageNavigationBridge = true;
  var nativeOpen = window.open;
  var bridge = window.__pinpageBridge || {};
  var EXTERNAL_COMMAND = 'open_external';
  var TAB_COMMAND = 'open_tab';
  var lastExternal = { href: '', at: 0 };

  function isLocalHost(hostname) {
    if (!hostname) return false;
    var host = hostname.replace(/^\[|\]$/g, '').toLowerCase();
    if (!host) return false;
    if (host === 'localhost' || host.slice(-10) === '.localhost' || host === '::1') return true;
    if (host.indexOf(':') >= 0) {
      return /^(fc|fd|fe[89ab])/.test(host);
    }
    if (host.indexOf('.') < 0) return true;
    var parts = host.split('.');
    if (parts.length !== 4) return false;
    for (var i = 0; i < 4; i++) {
      if (!/^\d{1,3}$/.test(parts[i]) || Number(parts[i]) > 255) return false;
    }
    var first = Number(parts[0]);
    var second = Number(parts[1]);
    return first === 127 || first === 10 ||
      (first === 192 && second === 168) ||
      (first === 172 && second >= 16 && second <= 31) ||
      (first === 169 && second === 254);
  }

  function classify(raw) {
    if (raw == null || raw === '') return null;
    var url;
    try {
      url = new URL(String(raw), location.href);
    } catch (_) {
      return null;
    }
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return null;
    if (url.username || url.password) return null;
    var local = url.origin === location.origin ||
      url.hostname === location.hostname || isLocalHost(url.hostname);
    return { href: url.href, local: local };
  }

  function normalize(url) {
    return url.origin + url.pathname.replace(/\/+$/, '') + url.search;
  }

  // 只有磁贴首页标签、且仍停在配置的首页地址时才把普通链接开成新标签；
  // 一旦从首页跳到子页面，链接恢复为在当前标签内替换。
  function isHomePage() {
    if (bridge.isHome !== true || !bridge.home) return false;
    try {
      return normalize(new URL(location.href)) === normalize(new URL(bridge.home, location.href));
    } catch (_) {
      return false;
    }
  }

  function call(command, href, fallback) {
    var internals = window.__TAURI_INTERNALS__;
    var invoke = internals && internals.invoke;
    if (!invoke) {
      fallback();
      return;
    }
    try {
      var result = invoke(command, { url: href });
      if (result && typeof result.catch === 'function') result.catch(fallback);
    } catch (_) {
      fallback();
    }
  }

  function openExternal(href) {
    var now = Date.now();
    if (lastExternal.href === href && now - lastExternal.at < 1000) return;
    lastExternal = { href: href, at: now };
    var usedFallback = false;
    call(EXTERNAL_COMMAND, href, function () {
      if (usedFallback) return;
      usedFallback = true;
      try {
        nativeOpen.call(window, href, '_blank');
      } catch (_) {}
    });
  }

  function openTab(href) {
    var usedFallback = false;
    call(TAB_COMMAND, href, function () {
      if (usedFallback) return;
      usedFallback = true;
      location.href = href;
    });
  }

  function handle(raw, forceTab) {
    var target = classify(raw);
    if (!target) return false;
    if (target.local) {
      if (forceTab || isHomePage()) {
        openTab(target.href);
      } else {
        location.href = target.href;
      }
    } else {
      openExternal(target.href);
    }
    return true;
  }

  window.open = function (target, name, features) {
    if (handle(target, true)) return null;
    return nativeOpen.call(window, target, name, features);
  };

  document.addEventListener('click', function (event) {
    if (event.defaultPrevented || (event.button != null && event.button !== 0)) return;
    var node = event.target;
    var anchor = node && node.closest ? node.closest('a[target="_blank"]') : null;
    if (!anchor || anchor.hasAttribute('download')) return;
    var target = classify(anchor.getAttribute('href'));
    if (!target) return;
    // 先阻止默认跳转：opener 插件与网页自身的处理都会检查 defaultPrevented。
    event.preventDefault();
    handle(target.href, true);
  }, true);

  // 首页里的普通链接开新标签；放在冒泡阶段，页面自己的脚本先处理，
  // 已经 preventDefault 的链接（前端路由）保持原有行为。
  document.addEventListener('click', function (event) {
    if (event.defaultPrevented || (event.button != null && event.button !== 0)) return;
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    if (!isHomePage()) return;
    var node = event.target;
    var anchor = node && node.closest ? node.closest('a[href]') : null;
    if (!anchor) return;
    var target = (anchor.getAttribute('target') || '').toLowerCase();
    if ((target !== '' && target !== '_self') || anchor.hasAttribute('download')) return;
    var link = classify(anchor.getAttribute('href'));
    if (!link) return;
    event.preventDefault();
    // 首页当启动器用：内部地址开标签，外部地址交给系统浏览器。
    if (link.local) {
      openTab(link.href);
    } else {
      openExternal(link.href);
    }
  }, false);
})();
"#;

/// 生成注入脚本：首页地址与「当前是否为首页标签」随窗口下发，供链接分类使用。
pub fn script(home: &url::Url, is_home: bool) -> String {
    let home = serde_json::to_string(home.as_str()).unwrap_or_else(|_| "\"\"".into());
    format!("window.__pinpageBridge = {{ home: {home}, isHome: {is_home} }};\n{SCRIPT}")
}
