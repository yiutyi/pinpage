// 功能：创建管理窗口与网页窗口，统一执行原生窗口操作。
mod actions;
pub(crate) mod editor;
mod events;
mod geometry;
pub(crate) mod limits;
pub(crate) mod navigation;
pub(crate) mod presets;
mod settings;
pub(crate) mod lift;
pub(crate) mod tabs;
pub(crate) mod toolbar;
pub use actions::action;
pub use settings::update;

use crate::{
    content,
    errors::AppMessage,
    i18n,
    models::{PageSource, PinConfig, PinRuntime},
    state::{self, AppState},
};
use std::{collections::BTreeMap, sync::atomic::Ordering};
use tauri::{
    webview::NewWindowResponse, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};
use tauri_plugin_opener::OpenerExt;

/// 只接受受信任管理窗口；同时验证当前来源，避免仅凭标签放行。
pub fn ensure_manager(window: &WebviewWindow) -> Result<(), AppMessage> {
    let url = window
        .url()
        .map_err(|error| AppMessage::new("errors.windowNotAuthorized").detail(error))?;
    if window.label() != "main" || !manager_navigation(&url) {
        return Err(AppMessage::new("errors.notManager"));
    }
    Ok(())
}

/// 应用内本地界面的统一导航校验：管理页与工具条共用。
pub(crate) fn app_navigation(url: &url::Url) -> bool {
    manager_navigation(url) || url.scheme() == "tauri"
}

/// 窗口标签是否对应一个真实的磁贴首页；额外标签不是配置记录。
pub(crate) fn is_pin(app: &AppHandle, id: &str) -> bool {
    app.state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .pins
        .iter()
        .any(|pin| pin.id == id)
}

/// 读取磁贴的持久化配置。
pub fn pin_config(app: &AppHandle, id: &str) -> Result<PinConfig, AppMessage> {
    app.state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .pins
        .iter()
        .find(|pin| pin.id == id)
        .cloned()
        .ok_or_else(|| AppMessage::new("errors.pinNotFound"))
}

/// 页面关闭时重置工具条临时状态，保留用户图钉偏好。
pub(crate) fn state_reset_for_close(app: &AppHandle, id: &str) {
    crate::state::reset_toolbar_runtime(app, id);
}

/// 建窗失败时把磁贴标记为未打开；标签没有配置记录，不需要回写运行态。
fn mark_closed(app: &AppHandle, pin: &PinConfig, home: bool) {
    if !home {
        return;
    }
    if let Some(runtime) = app.state::<AppState>().runtime.lock().unwrap().get_mut(&pin.id) {
        runtime.open = false;
    }
}

/// 内容页命令的身份校验：标签必须对应现存页面记录，并返回其配置。
/// 逐页能力以此为第一道闸门；capability 匹配只决定命令是否可达。
pub fn ensure_content(window: &WebviewWindow, app: &AppHandle) -> Result<PinConfig, AppMessage> {
    let label = window.label();
    app.state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .pins
        .iter()
        .find(|pin| pin.id == label)
        .cloned()
        .ok_or_else(|| AppMessage::new("errors.windowNotAuthorized"))
}

/// 内容窗口与管理窗口的系统标题：用户名称或应用标题按当前语言加后缀。
pub fn window_title(app: &AppHandle, title: &str) -> String {
    let args = BTreeMap::from([("title".to_string(), title.to_string())]);
    i18n::text(state::locale(app), "window.titleSuffix", &args)
}

/// 管理窗口的系统标题。
fn manager_title(app: &AppHandle) -> String {
    i18n::text(
        state::locale(app),
        "window.managerTitle",
        &Default::default(),
    )
}

/// 语言切换后更新已存在管理窗口的系统标题。
pub fn refresh_manager_title(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let title = manager_title(app);
        let _ = window.set_title(&title);
    }
}

/// 限制管理窗口为打包应用来源，开发时额外放行本项目开发服务器。
fn manager_navigation(url: &url::Url) -> bool {
    (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"))
        || (cfg!(debug_assertions)
            && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port() == Some(1420))
}

/// 已有管理窗口时激活，否则从异步任务创建，关闭后不保留隐藏 WebView。
pub fn show_manager(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let title = manager_title(&app);
        let result = WebviewWindowBuilder::new(&app, "main", WebviewUrl::App("index.html".into()))
            .title(&title)
            .inner_size(1080., 800.)
            .min_inner_size(760., 620.)
            .center()
            // 原生标题栏会与自绘顶栏叠成两层，管理窗口只保留界面内的一条。
            .decorations(false)
            .shadow(false)
            .on_navigation(manager_navigation)
            .on_new_window(|_, _| NewWindowResponse::Deny)
            .build();
        if let Err(error) = result {
            if app.get_webview_window("main").is_none() {
                state::report(
                    &app,
                    AppMessage::new("errors.openManagerFailed").detail(error),
                );
            }
        }
    });
}

/// 窗口菜单「页面设置」直达：先记录意图再激活或创建管理窗口。
/// 管理窗口可能尚未挂载，事件会丢失，前端挂载后另经 take 命令兜底取走意图。
pub fn open_pin_settings(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    if !state
        .settings
        .lock()
        .unwrap()
        .pins
        .iter()
        .any(|pin| pin.id == id)
    {
        return;
    }
    *state.pending_pin_settings.lock().unwrap() = Some(id.to_string());
    show_manager(app);
    let _ = app.emit_to("main", "open-pin-settings", ());
}

/// 按当前配置校验导航目标；来源可以随时被修改，因此不能沿用建窗时的快照。
/// 页面记录已不存在时一律拒绝，宁可不加载也不给已删除的窗口留白名单。
fn navigation_allowed_now(app: &AppHandle, id: &str, url: &url::Url) -> bool {
    app.state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .pins
        .iter()
        .find(|pin| pin.id == id)
        .is_some_and(|pin| content::navigation_allowed(id, &pin.source, url))
}

/// 计算内容窗口的首屏地址，导入页与随包数据的规则在此只保留一处实现。
/// 更新来源后重新调用，可复用同一入口而不必销毁重建窗口。
pub fn initial_url(app: &AppHandle, pin: &PinConfig) -> Result<url::Url, AppMessage> {
    let url = content::source_url(&pin.id, &pin.source)?;
    let PageSource::Bundled { entry } = &pin.source else {
        return Ok(url);
    };
    // 导入页内容在应用数据目录，缺失时给出比导航失败更明确的恢复提示。
    if !crate::import::site_root(app, &pin.id)?
        .join(entry)
        .is_file()
    {
        return Err(AppMessage::new("errors.importContentLost"));
    }
    // 存在随包数据时先加载引导页，回填完成后再跳转到真正的入口。
    let pending = crate::import::pages_root(app)?
        .join(&pin.id)
        .join(crate::import::PENDING_DATA);
    if !pending.is_file() {
        return Ok(url);
    }
    let mut target = url::Url::parse(&format!(
        "pinpage://{}{}",
        pin.id,
        crate::data::RESTORE_PAGE
    ))
    .map_err(|error| AppMessage::new("errors.localUrlInvalid").detail(error))?;
    target.query_pairs_mut().append_pair("entry", entry);
    Ok(target)
}

/// 把已打开窗口指向新的首屏地址，避免销毁重建带来的窗口抖动与几何丢失。
pub fn navigate(app: &AppHandle, id: &str, url: url::Url) -> Result<(), AppMessage> {
    let window = app
        .get_webview_window(id)
        .ok_or_else(|| AppMessage::new("errors.pageClosed"))?;
    crate::automation::begin_load(app, id);
    window
        .navigate(crate::platform::navigation::runtime_url(url)?)
        .map_err(|error| AppMessage::new("errors.windowApplyFailed").detail(error))?;
    events::watch_load(app, id);
    Ok(())
}

/// 创建一个内容 WebView；首页与标签共用同一套权限与导航桥。
pub(crate) fn create_page(
    app: &AppHandle,
    pin: &PinConfig,
    home: &url::Url,
    is_home: bool,
) -> Result<WebviewWindow, AppMessage> {
    let start = if is_home {
        home.clone()
    } else {
        content::source_url(&pin.id, &pin.source)?
    };
    let navigation_id = pin.id.clone();
    let navigation_app = app.clone();
    let navigation_home = is_home;
    let navigation_source = pin.source.clone();
    let popup_app = app.clone();
    let popup_id = pin.id.clone();
    let popup_source = pin.source.clone();
    let script = navigation::script(home, is_home);
    let (width, height) = limits::clamp_size(pin.width, pin.height);
    let builder = WebviewWindowBuilder::new(app, &pin.id, WebviewUrl::External(start))
        .title(window_title(app, &pin.name))
        .inner_size(width, height)
        .min_inner_size(limits::MIN_WIDTH, limits::MIN_HEIGHT)
        .resizable(!pin.size_locked)
        .skip_taskbar(true)
        .always_on_top(pin.mode == crate::models::WindowMode::Floating && pin.always_on_top)
        .decorations(pin.effective_decorations())
        .shadow(pin.effective_decorations())
        .transparent(cfg!(windows))
        .no_redirection_bitmap(cfg!(windows))
        .visible(false)
        .disable_drag_drop_handler()
        .initialization_script(script)
        .on_navigation(move |url| {
            let allowed = if navigation_home {
                navigation_allowed_now(&navigation_app, &navigation_id, url)
            } else {
                content::navigation_allowed(&navigation_id, &navigation_source, url)
            };
            if !allowed {
                events::set_error(
                    &navigation_app,
                    &navigation_id,
                    AppMessage::new("errors.navigationBlocked"),
                );
            }
            allowed
        })
        .on_new_window(move |url, _| {
            if content::is_local_url(&url) || content::same_host(&navigation_source_url(&popup_source), &url)
                || content::same_host(
                    &app_url(&popup_app, &popup_id).unwrap_or_else(|| navigation_source_url(&popup_source)),
                    &url,
                )
            {
                let app = popup_app.clone();
                let id = popup_id.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = tabs::open(&app, &id, url) {
                        state::report(&app, error);
                    }
                });
            } else if content::external_url(&url) {
                if let Err(error) = popup_app.opener().open_url(url.as_str(), None::<&str>) {
                    state::report(
                        &popup_app,
                        AppMessage::new("errors.openExternalFailed").detail(error),
                    );
                }
            }
            NewWindowResponse::Deny
        })
        .on_page_load(events::page_load)
        .on_document_title_changed(move |window, title| {
            let text: String = title.chars().take(100).collect();
            let title = window_title(window.app_handle(), &text);
            let _ = window.set_title(&title);
            tabs::set_title(window.app_handle(), window.label(), &text);
        });
    let window = match builder.build() {
        Ok(window) => window,
        Err(error) => {
            let message = AppMessage::new("errors.createWindowFailed").detail(error);
            events::set_error(app, &pin.id, message.clone());
            mark_closed(app, pin, is_home);
            return Err(message);
        }
    };
    #[cfg(windows)]
    crate::automation::lifecycle::attach(&window);
    let setup = (|| {
        geometry::restore(&window, pin)?;
        let mut positioned = pin.clone();
        let position = crate::platform::position(&window)?;
        positioned.x = Some(position.x);
        positioned.y = Some(position.y);
        crate::platform::apply(&window, &positioned)?;
        window
            .show()
            .map_err(|error| AppMessage::new("errors.windowApplyFailed").detail(error))
    })();
    if let Err(error) = setup {
        let _ = window.destroy();
        #[cfg(windows)]
        app.state::<AppState>().appearances.lock().unwrap().remove(&pin.id);
        events::set_error(app, &pin.id, error.clone());
        mark_closed(app, pin, is_home);
        return Err(error);
    }
    crate::platform::suppress_context_menu(&window);
    events::attach(&window);
    Ok(window)
}

/// 从来源配置得到用于同站判定的地址；非 URL 来源回退为本地回环。
fn navigation_source_url(source: &PageSource) -> url::Url {
    match source {
        PageSource::Url { url } => url::Url::parse(url).unwrap_or_else(|_| {
            url::Url::parse("http://localhost/").expect("固定地址可解析")
        }),
        PageSource::Bundled { .. } => {
            url::Url::parse("http://localhost/").expect("固定地址可解析")
        }
    }
}

/// 读取内容窗口当前地址，作为同站判定的补充来源。
fn app_url(app: &AppHandle, id: &str) -> Option<url::Url> {
    app.get_webview_window(id)
        .and_then(|window| window.url().ok())
        .and_then(|url| url::Url::parse(&url.to_string()).ok())
}

/// 创建一个内容 WebView，来源校验与资源授权在窗口创建前完成。
pub fn open_pin(app: &AppHandle, pin: &PinConfig) -> Result<(), AppMessage> {
    let state = app.state::<AppState>();
    state.ensure_mutable()?;
    crate::platform::validate_pin(crate::platform::capabilities(), pin)?;
    if let Some(window) = app.get_webview_window(&pin.id) {
        window
            .show()
            .map_err(|error| AppMessage::new("errors.windowApplyFailed").detail(error))?;
        if pin.mode == crate::models::WindowMode::Floating {
            window
                .set_focus()
                .map_err(|error| AppMessage::new("errors.windowApplyFailed").detail(error))?;
        }
        toolbar::open(app, pin);
        let _ = tabs::activate(app, &pin.id, &pin.id);
        return Ok(());
    }
    let url = initial_url(app, pin)?;
    crate::automation::begin_load(app, &pin.id);
    #[cfg(windows)]
    state.appearances.lock().unwrap().remove(&pin.id);
    state.runtime.lock().unwrap().insert(
        pin.id.clone(),
        PinRuntime {
            open: true,
            visible: true,
            loading: true,
            current_url: url.to_string(),
            error: None,
        },
    );
    create_page(app, pin, &url, true)?;
    if let Some(saved) = state
        .settings
        .lock()
        .unwrap()
        .pins
        .iter_mut()
        .find(|p| p.id == pin.id)
    {
        saved.enabled = true;
    }
    toolbar::open(app, pin);
    events::watch_load(app, &pin.id);
    state::notify(app);
    Ok(())
}

/// 恢复全部窗口的边框与可见位置，作为无边框模式的可靠找回入口。
pub fn recover_all(app: &AppHandle) {
    let ids: Vec<_> = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .pins
        .iter()
        .map(|p| p.id.clone())
        .collect();
    for id in ids {
        if app.get_webview_window(&id).is_some() {
            if let Err(error) = action(app, &id, "recover") {
                state::report(app, error);
            }
        }
    }
}

/// 退出前保存会话，并防止窗口销毁事件误删需要恢复的记录。
pub fn quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Err(error) = if state.read_only {
        Ok(())
    } else {
        state.save()
    } {
        state::report(app, error);
        show_manager(app);
        return;
    }
    app.state::<AppState>()
        .exiting
        .store(true, Ordering::SeqCst);
    app.exit(0);
}
