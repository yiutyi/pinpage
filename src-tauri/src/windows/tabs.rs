// 功能：管理一个磁贴的标签组：打开新标签、切换、关闭，并让工具条保持同步。
//
// 每个标签是一个独立的内容 WebView 窗口：非活动标签只隐藏、不销毁，页面状态与
// 后台任务（音乐、计时）继续运行；切回时复用同一 WebView，不重新加载。
// 标签不写入配置：重启只恢复磁贴首页，避免隐藏页面长期占用内存。
//
// 工具条属于整个标签组，不属于某个标签：标签操作只更新组的活动窗口、顺序与几何，
// 不创建 / 销毁 / 显隐工具条，也不复制图钉、收起或唤出状态。
use crate::{
    errors::AppMessage,
    models::{PageSource, PinConfig, WindowMode},
    platform,
    state::{self, AppState, TabEntry, TabGroup},
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use url::Url;

/// 单个磁贴最多保留的标签数（含首页）；每个标签都是一个 WebView，需要限额。
pub const MAX_TABS: usize = 6;
/// 工具条上每个标签占用的逻辑宽度，与 toolbar.css 的标签条保持一致。
pub const TAB_WIDTH: f64 = 88.;

/// 工具条在基础宽度之外为标签条额外增加的宽度；只有一个标签时不显示标签条。
pub fn tabs_width(tabs: usize) -> f64 {
    if tabs <= 1 {
        0.
    } else {
        TAB_WIDTH * tabs as f64
    }
}

/// 去掉系统标题后缀，得到网页自己的标题。后缀固定为 " · PinPage"。
fn strip_title_suffix(title: &str) -> &str {
    title.strip_suffix(" · PinPage").unwrap_or(title).trim_end()
}

/// 从 URL 取一个可读的站点名：域名取主域，IP 取路径首段，取不到返回 None。
/// 仅作为网页标题事件到达前的占位，避免标签条初始显示裸 IP。
fn site_label(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let ip = host.parse::<std::net::IpAddr>().is_ok();
    if ip {
        return url
            .path_segments()
            .and_then(|segments| segments.filter(|s| !s.is_empty()).last())
            .map(str::to_string);
    }
    host.split('.').rev().nth(1).map(str::to_string).or(Some(host.to_string()))
}

/// 关闭标签后切到哪个标签：优先前一个，其次后一个，最后回到首页。
pub fn neighbor(tabs: &[TabEntry], closing: &str) -> Option<String> {
    let index = tabs.iter().position(|tab| tab.window == closing)?;
    tabs.get(index.wrapping_sub(1))
        .or_else(|| tabs.get(index + 1))
        .map(|tab| tab.window.clone())
}

/// 任意窗口标签对应的磁贴 ID；首页窗口标签本身等于磁贴 ID。
pub fn group_of(app: &AppHandle, window: &str) -> Option<String> {
    let state = app.state::<AppState>();
    if state
        .settings
        .lock()
        .unwrap()
        .pins
        .iter()
        .any(|pin| pin.id == window)
    {
        return Some(window.to_string());
    }
    let groups = state.tabs.lock().unwrap();
    let found = groups
        .iter()
        .find(|(_, group)| group.tabs.iter().any(|tab| tab.window == window))
        .map(|(pin, _)| pin.clone());
    drop(groups);
    found
}

/// 当前显示在前台的窗口标签；没有标签组时就是磁贴首页。
pub fn active_window(app: &AppHandle, pin: &str) -> String {
    app.state::<AppState>()
        .tabs
        .lock()
        .unwrap()
        .get(pin)
        .map(|group| group.active.clone())
        .unwrap_or_else(|| pin.to_string())
}

/// 组内全部窗口标签，首页始终在第一位。
pub fn windows_of(app: &AppHandle, pin: &str) -> Vec<String> {
    app.state::<AppState>()
        .tabs
        .lock()
        .unwrap()
        .get(pin)
        .map(|group| group.tabs.iter().map(|tab| tab.window.clone()).collect())
        .unwrap_or_else(|| vec![pin.to_string()])
}

/// 工具条需要的标签视图；没有标签组时只返回磁贴首页。
///
/// 入参可以是磁贴 ID，也可以是组内任意标签的窗口标签：标签条按磁贴归组，
/// 额外标签单独查只会看到自己，标签条就不会出现。
pub fn tabs_of(app: &AppHandle, id: &str) -> Vec<TabInfo> {
    let group = group_of(app, id).unwrap_or_else(|| id.to_string());
    let pin = group.as_str();
    let state = app.state::<AppState>();
    let name = state
        .settings
        .lock()
        .unwrap()
        .pins
        .iter()
        .find(|item| item.id == pin)
        .map(|item| item.name.clone())
        .unwrap_or_else(|| pin.to_string());
    let groups = state.tabs.lock().unwrap();
    match groups.get(pin) {
        Some(group) => group
            .tabs
            .iter()
            .map(|tab| TabInfo {
                window: tab.window.clone(),
                title: if tab.window == pin {
                    name.clone()
                } else {
                    tab.title.clone()
                },
                active: tab.window == group.active,
                home: tab.window == pin,
            })
            .collect(),
        None => vec![TabInfo {
            window: pin.to_string(),
            title: name,
            active: true,
            home: true,
        }],
    }
}

/// 工具条标签条的数据；与 status 使用同一结构。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabInfo {
    pub window: String,
    pub title: String,
    pub active: bool,
    pub home: bool,
}

/// 从首页或任意标签打开一个新标签；调用方由命令层校验过来源。
pub fn open(app: &AppHandle, caller: &str, url: Url) -> Result<(), AppMessage> {
    let pin = group_of(app, caller).ok_or_else(|| AppMessage::new("errors.tabNotAuthorized"))?;
    let home = crate::windows::pin_config(app, &pin)?;
    let current = active_window(app, &pin);
    let label = {
        let state = app.state::<AppState>();
        let mut groups = state.tabs.lock().unwrap();
        let group = groups.entry(pin.clone()).or_insert_with(|| TabGroup {
            tabs: vec![TabEntry {
                window: pin.clone(),
                title: home.name.clone(),
            }],
            active: pin.clone(),
            next: 1,
            geometry: None,
        });
        if group.tabs.len() >= MAX_TABS {
            return Err(AppMessage::new("errors.tabLimit").arg("max", MAX_TABS));
        }
        let label = format!("{}-t{}", pin, group.next);
        group.next += 1;
        label
    };
    let mut config = inherit(app, &home, &current);
    config.id = label.clone();
    // 标签标题用站点名，不用 host；host 是 IP 时标签条会退化成一串数字。
    // 精确标题在建窗后由网页 title 事件补写（见下方 window.title()）。
    config.name = site_label(&url).unwrap_or_else(|| home.name.clone());
    config.source = PageSource::Url {
        url: url.to_string(),
    };
    let home_url = crate::windows::initial_url(app, &home)?;
    let window = crate::windows::create_page(app, &config, &home_url, false)?;
    // 网页 title 事件在窗口创建期就可能触发，那时标签还没入表，事件被丢弃。
    // 入表时读一次窗口标题补写，之后的变化仍由 on_document_title_changed 更新。
    let initial_title = window
        .title()
        .ok()
        .map(|value| strip_title_suffix(&value).to_string())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.name.clone());
    {
        let state = app.state::<AppState>();
        let mut groups = state.tabs.lock().unwrap();
        if let Some(group) = groups.get_mut(&pin) {
            group.tabs.push(TabEntry {
                window: window.label().to_string(),
                title: initial_title,
            });
        }
    }
    // 标签只增加内容窗口；工具条仍只有一条，宽度随标签数变化。
    crate::windows::toolbar::resize(app, &pin);
    record_geometry(app, &label);
    activate(app, &pin, &label)
}

/// 切换标签：把目标页带到前台，其余页隐藏但保留状态。
///
/// 桌面层页面会被其它应用完全遮住，点标签的意图就是「把页面叫到前台看」，
/// 因此桌面模式未唤出时先走 lift::summon。唤出只改窗口层级，不触碰工具条状态：
/// 图钉、收起门闩与倒计时保持原样，工具条跟着活动窗口移动。
/// 先切层再换标签很重要：直接在桌面层重设父窗口会与其它应用持有前台时的
/// 原生窗口状态竞争，是已复现的崩溃路径。
pub fn activate(app: &AppHandle, pin: &str, target: &str) -> Result<(), AppMessage> {
    let current = active_window(app, pin);
    if current == target {
        // 关掉前一个标签后组内可能只剩目标窗口且尚未显示，这里补一次显示与层级校准。
        let home = crate::windows::pin_config(app, pin)?;
        if home.mode.is_desktop() && !crate::windows::lift::is_lifted(app, pin) {
            crate::windows::lift::summon(app, Some(pin))?;
        }
        if let Some(window) = app.get_webview_window(target) {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
        crate::windows::toolbar::sync(app, pin);
        crate::windows::toolbar::front(app, pin);
        return Ok(());
    }
    if !windows_of(app, pin).iter().any(|window| window == target) {
        return Err(AppMessage::new("errors.tabNotFound"));
    }
    let home = crate::windows::pin_config(app, pin)?;
    let Some(window) = app.get_webview_window(target) else {
        return Err(AppMessage::new("errors.pageClosed"));
    };
    // 桌面层被其它应用遮住时先唤出，让接下来的标签切换对用户可见。
    if home.mode.is_desktop() && !crate::windows::lift::is_lifted(app, pin) {
        crate::windows::lift::summon(app, Some(pin))?;
    }
    for label in windows_of(app, pin) {
        if let Some(other) = app.get_webview_window(&label) {
            if label == target {
                let _ = other.show();
            } else {
                let _ = other.hide();
            }
        }
    }
    // 层级由 lift::summon / recall 对整组统一切换，切标签本身不再重设父窗口
    // 或窗口样式；只把目标窗口对齐到组内最新几何，避免原生窗口重入。
    if let Some(geo) = group_geometry(app, pin) {
        crate::platform::apply_geometry(&window, &geo)?;
    }
    // 恢复目标标签的显示并取焦点；层级已由 summon 处理，这里不再改窗口父子关系。
    let _ = window.unminimize();
    let _ = window.set_focus();
    // 登记活动窗口，工具条定位与唤出记录都以它为准。
    if let Some(group) = app.state::<AppState>().tabs.lock().unwrap().get_mut(pin) {
        group.active = target.to_string();
    }
    crate::windows::toolbar::sync(app, pin);
    crate::windows::toolbar::front(app, pin);
    // 唤出记录仍指向当前可见的标签，收回时对正确的窗口生效。
    crate::windows::lift::retarget(app, pin, target);
    notify(app, pin);
    Ok(())
}

/// 关闭一个额外标签；首页标签不可关闭，关掉最后一个标签会自动回到首页。
pub fn close(app: &AppHandle, target: &str) -> Result<(), AppMessage> {
    let pin = group_of(app, target).ok_or_else(|| AppMessage::new("errors.tabNotFound"))?;
    if target == pin {
        return Err(AppMessage::new("errors.tabHomeClose"));
    }
    let (was_active, next) = {
        let state = app.state::<AppState>();
        let mut groups = state.tabs.lock().unwrap();
        let group = groups
            .get_mut(&pin)
            .ok_or_else(|| AppMessage::new("errors.tabNotFound"))?;
        let next =
            neighbor(&group.tabs, target).ok_or_else(|| AppMessage::new("errors.tabNotFound"))?;
        let was_active = group.active == target;
        group.tabs.retain(|tab| tab.window != target);
        if was_active {
            group.active = next.clone();
        }
        (was_active, next)
    };
    if let Some(window) = app.get_webview_window(target) {
        let _ = window.destroy();
    }
    if was_active {
        activate(app, &pin, &next)?;
    }
    // 工具条属于整组：关标签只调整宽度，不销毁窗口、不清理图钉状态。
    crate::windows::toolbar::resize(app, &pin);
    notify(app, &pin);
    Ok(())
}

/// 更新某个标签的标题；页面标题变化时由事件层调用。
pub fn set_title(app: &AppHandle, window: &str, title: &str) {
    let trimmed: String = title.trim().chars().take(60).collect();
    if trimmed.is_empty() {
        return;
    }
    let Some(pin) = group_of(app, window) else {
        return;
    };
    // 首页标签显示磁贴名称，不被网页标题覆盖，便于和磁贴列表对应。
    if window == pin {
        return;
    }
    let changed = {
        let state = app.state::<AppState>();
        let mut groups = state.tabs.lock().unwrap();
        groups.get_mut(&pin).is_some_and(|group| {
            group
                .tabs
                .iter_mut()
                .find(|tab| tab.window == window)
                .is_some_and(|tab| {
                    let changed = tab.title != trimmed;
                    tab.title = trimmed.clone();
                    changed
                })
        })
    };
    if changed {
        notify(app, &pin);
    }
}

/// 窗口销毁后清理标签记录；首页销毁时整个组一起结束。
pub fn forget(app: &AppHandle, window: &str) {
    let Some(pin) = group_of(app, window) else {
        return;
    };
    if window == pin {
        let remaining = windows_of(app, &pin);
        app.state::<AppState>().tabs.lock().unwrap().remove(&pin);
        for label in remaining {
            if label == pin {
                continue;
            }
            if let Some(other) = app.get_webview_window(&label) {
                let _ = other.destroy();
            }
        }
        return;
    }
    {
        let state = app.state::<AppState>();
        let mut groups = state.tabs.lock().unwrap();
        if let Some(group) = groups.get_mut(&pin) {
            group.tabs.retain(|tab| tab.window != window);
            if group.active == window {
                group.active = pin.clone();
            }
        }
    }
    notify(app, &pin);
}

/// 关闭磁贴时销毁组内全部标签窗口与整组共用的工具条。
///
/// 关闭页面保留图钉偏好，删除页面才由调用方清理状态。
pub fn destroy_group(app: &AppHandle, pin: &str) {
    let labels = windows_of(app, pin);
    app.state::<AppState>().tabs.lock().unwrap().remove(pin);
    for label in labels {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.destroy();
        }
    }
    crate::windows::toolbar::close(app, pin);
}

/// 设置变更后把新的模式与几何同步到组内所有窗口，切标签不会跳回旧尺寸。
pub fn apply_group(app: &AppHandle, pin: &PinConfig) {
    for label in windows_of(app, &pin.id) {
        let Some(window) = app.get_webview_window(&label) else {
            continue;
        };
        let mut config = pin.clone();
        config.id = label.clone();
        if let Err(error) = platform::apply(&window, &config) {
            state::report(app, error);
        }
    }
    // 工具条只有一条，跟随活动标签定位；宽度与标签数量一起校正。
    crate::windows::toolbar::resize(app, &pin.id);
}

/// 组内共享几何；没有标签组或尚未记录时为 None。
pub fn group_geometry(app: &AppHandle, pin: &str) -> Option<crate::platform::Geometry> {
    app.state::<AppState>()
        .tabs
        .lock()
        .unwrap()
        .get(pin)
        .and_then(|group| group.geometry)
}

/// 记录某窗口当前几何到所属标签组，供切换时对齐同组标签。
///
/// 仅作用于标签组：普通磁贴首页没有标签组，跳过。窗口隐藏或最小化时不记录，
/// 避免把错误尺寸写进共享几何。缩放、移动与 DPI 变化都会触发，保证切换到的标签
/// 始终与当前页尺寸位置一致。
pub fn record_geometry(app: &AppHandle, window: &str) {
    let Some(pin) = group_of(app, window) else {
        return;
    };
    let Some(handle) = app.get_webview_window(window) else {
        return;
    };
    let Ok(size) = handle.inner_size() else {
        return;
    };
    let Ok(position) = crate::platform::position(&handle) else {
        return;
    };
    if handle.is_minimized().unwrap_or(false) {
        return;
    }
    let scale = handle.scale_factor().unwrap_or(1.).max(0.1);
    let geo = crate::platform::Geometry {
        x: position.x,
        y: position.y,
        width: size.width as f64 / scale,
        height: size.height as f64 / scale,
    };
    if let Some(group) = app.state::<AppState>().tabs.lock().unwrap().get_mut(&pin) {
        group.geometry = Some(geo);
    }
}

/// 标签内容变化通知给组内唯一一条工具条；切换标签不会重建界面。
fn notify(app: &AppHandle, pin: &str) {
    let _ = app.emit_to(
        crate::windows::toolbar::label(pin),
        "tabs-changed",
        tabs_of(app, pin),
    );
}

/// 复制磁贴外观与窗口几何，作为新标签或切换目标的配置。
fn inherit(app: &AppHandle, home: &PinConfig, source: &str) -> PinConfig {
    let mut config = home.clone();
    if let Some(window) = app.get_webview_window(source) {
        if let Ok(size) = window.inner_size() {
            let scale = window.scale_factor().unwrap_or(1.).max(0.1);
            config.width = size.width as f64 / scale;
            config.height = size.height as f64 / scale;
            config.scale = scale;
        }
        if let Ok(position) = platform::position(&window) {
            config.x = Some(position.x);
            config.y = Some(position.y);
        }
    }
    // 唤出期间新建或切换的标签也要留在普通窗口层最前，否则会掉回桌面层。
    if crate::windows::lift::is_lifted(app, &home.id) {
        config.mode = WindowMode::Floating;
        config.always_on_top = true;
        config.decorations = false;
    }
    config
}

#[cfg(test)]
#[path = "tabs_tests.rs"]
mod tests;
