// 功能：声明各平台窗口能力、执行统一校验，并提供 Windows 原生策略与安全回退。
pub mod navigation;
#[cfg(windows)]
use crate::models::PlatformDragModifier;
use crate::{
    errors::AppMessage,
    models::{
        GestureModifier, PinConfig, PlatformCapabilities, PresetFields, ToolbarPosition, WindowMode,
    },
    state::AppState,
};
use tauri::{AppHandle, Manager};
#[cfg(windows)]
mod appearance;
pub mod autostart;
#[cfg(windows)]
mod caption;
#[cfg(windows)]
mod corners;
#[cfg(windows)]
mod desktop;
#[cfg(windows)]
mod gestures;
#[cfg(all(target_os = "macos", feature = "experimental-macos-drag"))]
mod macos_drag;
#[cfg(windows)]
pub(crate) use gestures::MenuRequest;
#[cfg(windows)]
mod shell;
pub mod locale;
#[cfg(windows)]
mod recovery;
#[cfg(windows)]
mod rules;
#[cfg(windows)]
mod window_policy;

/// 组内窗口共享的几何：x/y 为屏幕物理像素，width/height 为逻辑像素。
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: f64,
    pub height: f64,
}

/// 只同步尺寸与位置，不改动父窗口、边框、圆角或层级规则。
/// 切标签只对齐几何，避免重设桌面父窗口时与前台切换竞争。
#[cfg(windows)]
pub fn apply_geometry(window: &tauri::WebviewWindow, geo: &Geometry) -> Result<(), AppMessage> {
    window_for_geometry(window, geo)
}

#[cfg(windows)]
fn window_for_geometry(window: &tauri::WebviewWindow, geo: &Geometry) -> Result<(), AppMessage> {
    let window_for_ui = window.clone();
    let geo = *geo;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    window
        .run_on_main_thread(move || {
            let _ = sender.send(apply_geometry_on_ui(&window_for_ui, &geo));
        })
        .map_err(crate::errors::unknown)?;
    receiver
        .recv()
        .map_err(|_| AppMessage::new("errors.taskInterrupted"))?
}

#[cfg(windows)]
fn apply_geometry_on_ui(window: &tauri::WebviewWindow, geo: &Geometry) -> Result<(), AppMessage> {
    use tauri::{LogicalSize, PhysicalPosition};
    window
        .set_size(LogicalSize::new(geo.width, geo.height))
        .map_err(crate::errors::unknown)?;
    window
        .set_position(PhysicalPosition::new(geo.x, geo.y))
        .map_err(crate::errors::unknown)?;
    Ok(())
}

#[cfg(not(windows))]
pub fn apply_geometry(_window: &tauri::WebviewWindow, _geo: &Geometry) -> Result<(), AppMessage> {
    Ok(())
}

/// Windows 完整支持桌面嵌入、圆角、独立几何锁与可配置手势。
#[cfg(windows)]
pub fn capabilities() -> PlatformCapabilities {
    PlatformCapabilities {
        desktop_mode: true,
        rounded: true,
        transparent_background: true,
        position_lock: true,
        size_lock: true,
        window_gestures: true,
        drag_modifier: Some(PlatformDragModifier::Alt),
    }
}

/// macOS 默认只提供标准浮动窗口，显式实验构建才声明固定 Option 拖动。
#[cfg(target_os = "macos")]
pub fn capabilities() -> PlatformCapabilities {
    let mut capabilities = floating_capabilities();
    if cfg!(feature = "experimental-macos-drag") {
        capabilities.drag_modifier = Some(crate::models::PlatformDragModifier::Option);
    }
    capabilities
}

/// 其他平台保持最保守的浮动窗口组合，不继承 macOS 的实验能力。
#[cfg(not(any(windows, target_os = "macos")))]
pub fn capabilities() -> PlatformCapabilities {
    floating_capabilities()
}

/// 标准浮动窗口的共同基线；所有未实现的原生能力明确关闭。
#[cfg(not(windows))]
fn floating_capabilities() -> PlatformCapabilities {
    PlatformCapabilities {
        desktop_mode: false,
        rounded: false,
        transparent_background: false,
        position_lock: false,
        size_lock: false,
        window_gestures: false,
        drag_modifier: None,
    }
}

/// 页面是否只使用当前平台支持的能力。
fn supports_pin(capabilities: PlatformCapabilities, pin: &PinConfig) -> bool {
    (pin.mode == WindowMode::Floating || capabilities.desktop_mode)
        && (!pin.rounded || capabilities.rounded)
        && (!pin.transparent_background || capabilities.transparent_background)
        && (!pin.position_locked || capabilities.position_lock)
        && (!pin.size_locked || capabilities.size_lock)
}

/// 预设是否能在当前平台完整应用，避免通过菜单或 IPC 绕过前端裁剪。
fn supports_preset(capabilities: PlatformCapabilities, fields: &PresetFields) -> bool {
    (fields.mode == WindowMode::Floating || capabilities.desktop_mode)
        && (!fields.rounded || capabilities.rounded)
        && (!fields.transparent_background || capabilities.transparent_background)
        && (!fields.position_locked || capabilities.position_lock)
        && (!fields.size_locked || capabilities.size_lock)
}

/// 显式提交不支持的能力时拒绝，调用方保留原窗口与配置。
pub fn validate_pin(capabilities: PlatformCapabilities, pin: &PinConfig) -> Result<(), AppMessage> {
    if supports_pin(capabilities, pin) {
        Ok(())
    } else {
        Err(AppMessage::new("errors.platformUnsupported"))
    }
}

/// 预设写入与应用前使用同一套断言，内置项与自定义项不区分处理。
pub fn validate_preset(
    capabilities: PlatformCapabilities,
    fields: &PresetFields,
) -> Result<(), AppMessage> {
    if supports_preset(capabilities, fields) {
        Ok(())
    } else {
        Err(AppMessage::new("errors.platformUnsupported"))
    }
}

/// 把从其他平台带来的页面降级为标准浮动窗口，保证内容仍可访问和找回。
pub fn fallback_pin(capabilities: PlatformCapabilities, pin: &mut PinConfig) -> bool {
    let before = (
        pin.mode,
        pin.decorations,
        pin.rounded,
        pin.transparent_background,
        pin.position_locked,
        pin.size_locked,
    );
    if pin.mode == WindowMode::Desktop && !capabilities.desktop_mode {
        // 桌面模式本身依赖 Windows 专属宿主；回到浮动层时恢复系统边框作为可靠找回入口。
        pin.mode = WindowMode::Floating;
        pin.decorations = true;
        pin.rounded = false;
        pin.position_locked = false;
        pin.size_locked = false;
    }
    if !capabilities.transparent_background {
        pin.transparent_background = false;
    }
    if !capabilities.rounded {
        pin.rounded = false;
    }
    if !capabilities.position_lock {
        pin.position_locked = false;
    }
    if !capabilities.size_lock {
        pin.size_locked = false;
    }
    let changed = before
        != (
            pin.mode,
            pin.decorations,
            pin.rounded,
            pin.transparent_background,
            pin.position_locked,
            pin.size_locked,
        );
    if changed {
        // 回退后的组合通常不再匹配原预设，清除标记避免界面继续展示失效来源。
        pin.preset = None;
    }
    changed
}

/// 应用单个页面的原生配置；所有调用都在此做平台能力终检。
pub fn apply(window: &tauri::WebviewWindow, pin: &PinConfig) -> Result<(), AppMessage> {
    validate_pin(capabilities(), pin)?;
    apply_supported(window, pin)
}

/// Windows 使用完整窗口策略，包括桌面宿主、圆角与几何锁。
#[cfg(windows)]
fn apply_supported(window: &tauri::WebviewWindow, pin: &PinConfig) -> Result<(), AppMessage> {
    window_policy::apply(window, pin)?;
    appearance::apply(window, pin)
}

/// 其他平台只应用 Tauri 通用的浮动窗口属性，不支持项已在上层校验。
#[cfg(not(windows))]
fn apply_supported(window: &tauri::WebviewWindow, pin: &PinConfig) -> Result<(), AppMessage> {
    use tauri::{LogicalSize, PhysicalPosition};
    let decorated = pin.effective_decorations();
    window
        .set_decorations(decorated)
        .and_then(|_| window.set_resizable(!pin.size_locked))
        .and_then(|_| {
            window.set_always_on_top(pin.mode == WindowMode::Floating && pin.always_on_top)
        })
        .and_then(|_| window.set_size(LogicalSize::new(pin.width, pin.height)))
        .and_then(|_| match (pin.x, pin.y) {
            (Some(x), Some(y)) => window.set_position(PhysicalPosition::new(x, y)),
            _ => Ok(()),
        })
        .map_err(|error| AppMessage::new("errors.windowApplyFailed").detail(error))
}

/// 内容窗口关闭浏览器默认右键菜单，避免放行右键时冒出浏览器菜单；其他平台保留系统行为。
pub fn suppress_context_menu(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    appearance::suppress_context_menu(window);
    #[cfg(not(windows))]
    let _ = window;
}

/// 原生读取屏幕物理坐标，避免桌面子窗口被保存成父窗口局部坐标。
#[cfg(windows)]
pub fn position(window: &tauri::WebviewWindow) -> Result<tauri::PhysicalPosition<i32>, AppMessage> {
    use windows_sys::Win32::{Foundation::RECT, UI::WindowsAndMessaging::GetWindowRect};
    let handle = window
        .hwnd()
        .map_err(|error| AppMessage::new("errors.readPosition").detail(error))?
        .0;
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(handle, &mut rect) } == 0 {
        return Err(AppMessage::new("errors.readPosition"));
    }
    Ok(tauri::PhysicalPosition::new(rect.left, rect.top))
}

/// 非 Windows 平台使用 Tauri 提供的位置查询。
#[cfg(not(windows))]
pub fn position(window: &tauri::WebviewWindow) -> Result<tauri::PhysicalPosition<i32>, AppMessage> {
    window
        .outer_position()
        .map_err(|error| AppMessage::new("errors.readPosition").detail(error))
}

/// 启动桌面恢复与唯一鼠标手势输入线程。
#[cfg(windows)]
pub fn start(app: &AppHandle) {
    recovery::start(app);
    gestures::start(app);
    toolbar_poll::start(app);
}

/// 工具条显隐轮询：光标位置只读一次，交互期间高频、空闲降频。
#[cfg(windows)]
mod toolbar_poll {
    use crate::{state::AppState, windows};
    use std::sync::atomic::Ordering;
    use tauri::{AppHandle, Manager};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    /// 启动独立轮询线程，退出标记置位后停止。
    pub fn start(app: &AppHandle) {
        let app = app.clone();
        std::thread::spawn(move || {
            let mut last: Option<(i32, i32)> = None;
            loop {
                if app.state::<AppState>().exiting.load(Ordering::SeqCst) {
                    break;
                }
                let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
                let ok = unsafe { GetCursorPos(&mut point) } != 0;
                let moved = ok && last != Some((point.x, point.y));
                last = ok.then_some((point.x, point.y));
                let (bars, pending) = if ok {
                    windows::toolbar::auto_visibility(&app, (point.x, point.y))
                } else {
                    (windows::toolbar::fallback_visible(&app), false)
                };
                std::thread::sleep(windows::toolbar::poll_interval(bars, moved, pending, false));
            }
        });
    }
}

/// 其他平台保留标准边框移动和托盘恢复入口。
#[cfg(not(any(windows, all(target_os = "macos", feature = "experimental-macos-drag"))))]
pub fn start(_app: &AppHandle) {}

/// 实验构建在应用主线程安装 macOS 本地鼠标事件监视器。
#[cfg(all(target_os = "macos", feature = "experimental-macos-drag"))]
pub fn start(app: &AppHandle) {
    macos_drag::start(app);
}

/// 桌面映射按钮只执行 Rust 内固定的系统动作，不接收网页给出的命令或路径。
#[cfg(windows)]
pub fn desktop_action(action: &str) -> Result<(), AppMessage> {
    shell::desktop_action(action)
}

#[cfg(not(windows))]
pub fn desktop_action(_action: &str) -> Result<(), AppMessage> {
    Err(AppMessage::new("errors.platformUnsupported"))
}

/// 读取工具条停靠位置；配置错误或未设置时回退顶部。
fn toolbar_slot(app: &AppHandle) -> ToolbarPosition {
    app.state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .toolbar_position
}

/// 窗口所在显示器的真实缩放（1.0 表示 96dpi）。
///
/// `scale_factor()` 与 `GetDpiForWindow` 在进程被 DPI 虚拟化时返回 96，
/// 据此换算逻辑尺寸会把窗口设小一半，而 WebView2 仍按真实缩放排版，内容被裁。
/// 显示器自身的缩放不受窗口虚拟化影响，优先用它。
pub fn window_scale(window: &tauri::WebviewWindow) -> f64 {
    window
        .current_monitor()
        .ok()
        .flatten()
        .map(|monitor| monitor.scale_factor())
        .or_else(|| window.scale_factor().ok())
        .unwrap_or(1.)
}

/// 窗口外框的物理尺寸（含阴影与不可见边框）。
///
/// 与 `inner_size()` 的客户区尺寸可能差一截：判断「窗口是否被系统顶宽」要看外框。
#[cfg(windows)]
pub fn window_outer_size(window: &tauri::WebviewWindow) -> Option<(u32, u32)> {
    use windows_sys::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::GetWindowRect,
    };
    let handle = window.hwnd().ok()?;
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(handle.0, &mut rect) } == 0 {
        return None;
    }
    Some((
        (rect.right - rect.left).max(0) as u32,
        (rect.bottom - rect.top).max(0) as u32,
    ))
}

#[cfg(not(windows))]
pub fn window_outer_size(_window: &tauri::WebviewWindow) -> Option<(u32, u32)> {
    None
}

/// 工具条锚点：由停靠位置决定，坐标为屏幕物理像素。
#[cfg(windows)]
pub fn toolbar_position(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
    width: i32,
    height: i32,
) -> Option<(i32, i32)> {
    let (left, top, right, bottom, work) = toolbar_area(window)?;
    let slot = toolbar_slot(app);
    let (x, y) = match slot {
        ToolbarPosition::Top => (
            left + (right - left - width) / 2,
            top + 8,
        ),
        ToolbarPosition::Left => (
            left + 8,
            top + (bottom - top - height) / 2,
        ),
        ToolbarPosition::Right => (
            right - width - 8,
            top + (bottom - top - height) / 2,
        ),
    };
    // 标签多时工具条可能超过页面宽度，按显示器工作区收边。
    let (minimum_x, maximum_x) = (work.0, work.2.saturating_sub(width));
    let (minimum_y, maximum_y) = (work.1, work.3.saturating_sub(height));
    Some((
        x.clamp(minimum_x.min(maximum_x), maximum_x.max(minimum_x)),
        y.clamp(minimum_y.min(maximum_y), maximum_y.max(minimum_y)),
    ))
}

#[cfg(not(windows))]
pub fn toolbar_position(
    _app: &AppHandle,
    _window: &tauri::WebviewWindow,
    _width: i32,
    _height: i32,
) -> Option<(i32, i32)> {
    None
}

/// 内容窗口客户区的屏幕物理矩形及所在显示器工作区，供定位与热区共用。
#[cfg(windows)]
pub fn toolbar_area(window: &tauri::WebviewWindow) -> Option<(i32, i32, i32, i32, (i32, i32, i32, i32))> {
    use windows_sys::Win32::{
        Foundation::{POINT, RECT},
        Graphics::Gdi::ClientToScreen,
        UI::WindowsAndMessaging::GetClientRect,
    };
    let hwnd = window.hwnd().ok()?.0;
    let mut area = RECT::default();
    unsafe {
        if GetClientRect(hwnd, &mut area) == 0 {
            return None;
        }
        let mut origin = POINT {
            x: area.left,
            y: area.top,
        };
        if ClientToScreen(hwnd, &mut origin) == 0 {
            return None;
        }
        let work = window
            .current_monitor()
            .ok()
            .flatten()
            .map(|monitor| {
                let value = monitor.work_area();
                (
                    value.position.x,
                    value.position.y,
                    value.position.x + value.size.width as i32,
                    value.position.y + value.size.height as i32,
                )
            })
            .unwrap_or((i32::MIN, i32::MIN, i32::MAX, i32::MAX));
        Some((
            origin.x,
            origin.y,
            origin.x + (area.right - area.left),
            origin.y + (area.bottom - area.top),
            work,
        ))
    }
}

#[cfg(not(windows))]
pub fn toolbar_area(_window: &tauri::WebviewWindow) -> Option<(i32, i32, i32, i32, (i32, i32, i32, i32))> {
    None
}

/// 工具条热区矩形：顶部为横条，左右侧为竖条，宽度 12 物理像素。
pub fn toolbar_hot_zone(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
) -> Option<(i32, i32, i32, i32)> {
    const HOT_ZONE: i32 = 12;
    let (left, top, right, bottom, _) = toolbar_area(window)?;
    Some(match toolbar_slot(app) {
        ToolbarPosition::Top => (left, top, right, top + HOT_ZONE),
        ToolbarPosition::Left => (left, top, left + HOT_ZONE, bottom),
        ToolbarPosition::Right => (right - HOT_ZONE, top, right, bottom),
    })
}

/// 当前停靠位置，供前端首帧决定竖排布局与唤出按钮。
pub fn toolbar_position_kind(app: &AppHandle) -> ToolbarPosition {
    toolbar_slot(app)
}

/// 把工具条压回唤出页面上方；两者同属置顶层，页面取焦点后会盖住按钮。
#[cfg(windows)]
pub fn raise_above(window: &tauri::WebviewWindow, bar: &tauri::WebviewWindow) {
    match (window.hwnd(), bar.hwnd()) {
        (Ok(page), Ok(bar)) if !desktop::above(page.0, bar.0) => desktop::raise(bar.0),
        _ => {}
    }
}

#[cfg(not(windows))]
pub fn raise_above(_window: &tauri::WebviewWindow, _bar: &tauri::WebviewWindow) {}

/// 当前系统前台窗口句柄；0 表示没有可用窗口。
#[cfg(windows)]
pub fn foreground_window() -> isize {
    desktop::foreground()
}

#[cfg(not(windows))]
pub fn foreground_window() -> isize {
    0
}

/// 收回桌面后把先前的前台窗口调回前台。
#[cfg(windows)]
pub fn restore_foreground(window: isize) {
    desktop::restore_foreground(window)
}

#[cfg(not(windows))]
pub fn restore_foreground(_window: isize) {}

/// 全局设置持久化成功后更新两种手势共用的修饰键。
pub fn set_gesture_modifier(modifier: GestureModifier) {
    #[cfg(windows)]
    gestures::set_modifier(modifier);
    #[cfg(not(windows))]
    let _ = modifier;
}

/// 应用退出时释放原生输入钩子。
pub fn stop() {
    #[cfg(windows)]
    gestures::stop();
    #[cfg(all(target_os = "macos", feature = "experimental-macos-drag"))]
    macos_drag::stop();
}

#[cfg(test)]
#[path = "platform/capabilities_tests.rs"]
mod capabilities_tests;
