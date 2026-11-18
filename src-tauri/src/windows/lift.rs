// 功能：把桌面层页面临时抬到普通窗口层最前，并在收回时放回桌面层。
//
// 桌面模式的页面挂在 Explorer 图标宿主下，永远位于普通窗口之后。唤出时复用
// platform::apply 把父窗口切回顶层并置顶，页面本身不重新加载，当前浏览位置与
// 页面状态都保持不变；收回时按配置里的桌面模式重新挂回桌面，再把唤出前的
// 前台窗口调回前台，让用户回到原来的应用。
// 一个磁贴可能有多个标签，唤出与收回都以「当前活动的标签窗口」为单位。
// 本模块只管窗口层级：不备份、不改写工具条的图钉、收起或倒计时状态；
// 工具条把「已唤出」当作只读的常驻条件，唤出与固定因此互不影响。
use crate::{
    errors::AppMessage,
    models::{PinConfig, WindowMode},
    platform,
    state::{self, AppState},
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

/// 由当前几何派生抬升配置：切到浮动层、保留屏幕位置与尺寸、无边框置顶。
///
/// 尺寸按逻辑像素写回，避免 apply 内的 set_size 把物理像素再放大一次 DPI。
fn lifted(pin: &PinConfig, size: (u32, u32), scale: f64, position: (i32, i32)) -> PinConfig {
    let mut next = pin.clone();
    next.mode = WindowMode::Floating;
    next.always_on_top = true;
    next.decorations = false;
    if scale > 0. && size.0 > 0 && size.1 > 0 {
        next.width = size.0 as f64 / scale;
        next.height = size.1 as f64 / scale;
    }
    next.x = Some(position.0);
    next.y = Some(position.1);
    next
}

/// 读取窗口当前几何；查询失败时沿用配置里的位置与尺寸。
fn lifted_config(pin: &PinConfig, window: &WebviewWindow) -> PinConfig {
    let size = window
        .inner_size()
        .map(|size| (size.width, size.height))
        .unwrap_or((0, 0));
    let scale = window.scale_factor().unwrap_or(1.);
    let position = platform::position(window)
        .map(|value| (value.x, value.y))
        .unwrap_or((pin.x.unwrap_or(0), pin.y.unwrap_or(0)));
    lifted(pin, size, scale, position)
}

/// 可唤出的磁贴：已打开、当前活动标签可见且属于桌面模式。
///
/// 返回磁贴配置与该磁贴当前活动的窗口标签，唤出作用于可见的那个标签。
fn candidates(app: &AppHandle, only: Option<&str>) -> Vec<(PinConfig, String)> {
    let pins = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .pins
        .clone();
    pins.into_iter()
        .filter(|pin| only.is_none_or(|id| pin.id == id))
        .filter(|pin| pin.mode.is_desktop())
        .filter_map(|pin| {
            let window = super::tabs::active_window(app, &pin.id);
            app.get_webview_window(&window)
                .filter(|item| item.is_visible().unwrap_or(true))
                .map(|_| (pin, window))
        })
        .collect()
}

/// 页面当前是否处于前台唤出状态；标签窗口按所属磁贴判定。
pub fn is_lifted(app: &AppHandle, window: &str) -> bool {
    let Some(pin) = super::tabs::group_of(app, window) else {
        return false;
    };
    app.state::<AppState>()
        .lift
        .lock()
        .unwrap()
        .pages
        .contains_key(&pin)
}

/// 把页面抬到普通窗口层最前；页面不重新加载，保留当前导航位置与页面状态。
///
/// 没有可唤出的页面时直接返回，快捷键误触不应产生错误提示。重复唤出是幂等的，
/// 只会重新激活一次目标页面。
pub fn summon(app: &AppHandle, only: Option<&str>) -> Result<(), AppMessage> {
    let targets = candidates(app, only);
    if targets.is_empty() {
        return Ok(());
    }
    let foreground = platform::foreground_window();
    let mut pending = Vec::new();
    {
        let state = app.state::<AppState>();
        let mut lift = state.lift.lock().unwrap();
        // 只有首家唤出前的前台窗口才值得恢复；重复唤出不覆盖，避免记成自己的窗口。
        if lift.pages.is_empty() && foreground != 0 {
            lift.previous = foreground;
        }
        for (pin, window) in &targets {
            if lift.pages.insert(pin.id.clone(), window.clone()).is_none() {
                pending.push((pin.clone(), window.clone()));
            }
        }
    }
    let mut failure = None;
    let mut focused = false;
    for (pin, window) in &pending {
        let Some(active) = app.get_webview_window(window) else {
            forget(app, window);
            continue;
        };
        // 以活动标签的几何为准，把整组（含隐藏标签）一起切到浮动层。这样之后
        // 切标签只需显隐窗口，不必在其它应用持有前台时重设父窗口与窗口样式。
        let config = lifted_config(pin, &active);
        let labels = super::tabs::windows_of(app, &pin.id);
        let mut failed = None;
        for label in &labels {
            if let Some(item) = app.get_webview_window(label) {
                if let Err(error) = platform::apply(&item, &config) {
                    failed.get_or_insert(error);
                }
            }
        }
        if let Some(error) = failed {
            // 单页失败不影响其它页面：整组回到桌面层并放弃唤出记录。
            for label in &labels {
                if let Some(item) = app.get_webview_window(label) {
                    let _ = platform::apply(&item, pin);
                }
            }
            forget(app, window);
            failure.get_or_insert(error);
            continue;
        }
        // 唤出是只读常驻条件，工具条由轮询按它自行显示；只刷新一次宽限期，
        // 不解除用户主动收起的门闩，也不改图钉与倒计时。
        state::touch_toolbar_grace(app, &pin.id);
        if !focused {
            activate(&active);
            focused = true;
        }
        // 页面升到置顶层并取得焦点后会盖住工具条，重新把按钮压回页面上方。
        super::toolbar::front(app, &pin.id);
    }
    // 重复唤出时没有新页面，也要把已有页面重新激活到最前。
    if !focused {
        if let Some((pin, _)) = targets.first() {
            if let Some(target) = app.get_webview_window(&super::tabs::active_window(app, &pin.id))
            {
                activate(&target);
                super::toolbar::front(app, &pin.id);
            }
        }
    }
    notify(app);
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// 收回唤出的页面：按配置重新挂回桌面层、恢复工具条策略并还原先前的前台窗口。
///
/// only 用于单个磁贴的收回；最后一个离开前台的磁贴负责恢复前台窗口。
pub fn recall(app: &AppHandle, only: Option<&str>) -> Result<(), AppMessage> {
    let (pages, previous) = match only {
        Some(id) => {
            let state = app.state::<AppState>();
            let mut lift = state.lift.lock().unwrap();
            let Some(window) = lift.pages.remove(id) else {
                return Ok(());
            };
            let previous = if lift.pages.is_empty() {
                std::mem::replace(&mut lift.previous, 0)
            } else {
                0
            };
            (vec![(id.to_string(), window)], previous)
        }
        None => {
            let state = app.state::<AppState>();
            let mut lift = state.lift.lock().unwrap();
            let pages: Vec<(String, String)> = lift.pages.drain().collect();
            let previous = std::mem::replace(&mut lift.previous, 0);
            (pages, previous)
        }
    };
    let mut failure = None;
    for (pin, _window) in &pages {
        if let Ok(config) = crate::windows::pin_config(app, pin) {
            // 整组一起挂回桌面层，隐藏标签也保持同一层级，切标签无需再改样式。
            for label in super::tabs::windows_of(app, pin) {
                if let Some(target) = app.get_webview_window(&label) {
                    if let Err(error) = platform::apply(&target, &config) {
                        failure.get_or_insert(error);
                    }
                }
            }
            // 收回后结束自动显示宽限期：图钉仍常驻，未固定的页面恢复自动隐藏。
            // 这里不回写任何工具条状态，用户此前点的图钉或收起保持原样。
            super::toolbar::clear_grace(app, pin);
        }
    }
    platform::restore_foreground(previous);
    notify(app);
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// 唤出期间切换标签时更新记录，收回时对当前显示的标签生效。
pub fn retarget(app: &AppHandle, pin: &str, window: &str) {
    let state = app.state::<AppState>();
    let mut lift = state.lift.lock().unwrap();
    if let Some(entry) = lift.pages.get_mut(pin) {
        *entry = window.to_string();
    }
}

/// 页面离开唤出状态（关闭、隐藏或修改设置）时只清理层级记录。
///
/// 工具条状态由显隐决策自行读取「是否已唤出」，不需要在这里回写。
pub fn forget(app: &AppHandle, window: &str) {
    let Some(pin) = super::tabs::group_of(app, window) else {
        return;
    };
    let removed = {
        let state = app.state::<AppState>();
        let mut lift = state.lift.lock().unwrap();
        match lift.pages.get(&pin) {
            // 标签窗口被销毁时唤出记录随之结束；磁贴首页关闭也按同一规则处理。
            Some(lifted) if lifted == window || window == pin => lift.pages.remove(&pin),
            _ => None,
        }
    };
    if removed.is_some() {
        notify(app);
    }
}

/// 把窗口激活到最前；最小化或隐藏的页面先恢复可见再取焦点。
fn activate(window: &WebviewWindow) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// 工具条需要实时同步的层级状态；图钉由工具条动作自身维护，不在这里下发。
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct BarState {
    lifted: bool,
    /// 只有桌面模式的页面能唤出；设置页切换模式后按钮也要跟着出现或消失。
    liftable: bool,
}

/// 通知每个工具条自身的唤出与可唤出状态，快捷键触发时按钮也能同步。
pub(super) fn notify(app: &AppHandle) {
    for (label, _) in app.webview_windows() {
        let Some(group) = label.strip_prefix("bar-") else {
            continue;
        };
        let config = crate::windows::pin_config(app, group).ok();
        let payload = BarState {
            lifted: is_lifted(app, group),
            liftable: config.is_some_and(|config| config.mode.is_desktop()),
        };
        let _ = app.emit_to(label.as_str(), "lift-changed", payload);
    }
}

#[cfg(test)]
#[path = "lift_tests.rs"]
mod tests;
