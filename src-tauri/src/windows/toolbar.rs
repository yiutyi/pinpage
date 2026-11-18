// 功能：为每个磁贴（标签组）维护唯一工具条，负责常驻按钮、位置跟随与生命周期同步。
//
// 工具条是与内容 WebView 分离的透明窗口：网页导航不会移除按钮，网页脚本也拿不到
// 工具条所持有的命令权限。窗口尺寸等于按钮条本身，避免全屏透明层的鼠标穿透问题。
// 位置在「当前活动标签」客户区顶部居中；未固定时鼠标移开后自动收起，靠近顶部唤出。
//
// 一个标签组只有一个工具条窗口：切换标签只移动它，不重建 WebView、不复制状态、
// 不改动窗口层级。因此图钉、收起门闩、倒计时与标签数量变化互不影响。
use crate::{
    errors::AppMessage,
    models::PinConfig,
    state::{self, ToolbarState},
};
use std::time::{Duration, Instant};
use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

/// 横排工具条的逻辑尺寸；Vue 布局与原生窗口共用同一组数值。
pub const WIDTH: f64 = 360.;
pub const HEIGHT: f64 = 40.;
/// 竖排固定宽度：只容纳 30px 按钮与左右内边距，与 CSS 的窄条严格一致。
pub const VERTICAL_WIDTH: f64 = 48.;
/// 竖排固定高度：窗口尺寸恒定，不随标签数变化，避免尺寸抖动与滚动条。
/// 其中标签条固定占 96，其余留给按钮组；打开标签后不再挤压标签高度。
pub const VERTICAL_HEIGHT: f64 = 480.;

/// 轮询间隔：交互活跃时保持 25ms；光标静止且没有待收起的工具条时降频，
/// 完全没有工具条时进一步降频，避免常驻线程在空闲时空转一个 CPU 核。
pub fn poll_interval(bars: usize, cursor_moved: bool, pending: bool, dragging: bool) -> Duration {
    if dragging || cursor_moved || pending {
        Duration::from_millis(25)
    } else if bars == 0 {
        Duration::from_millis(150)
    } else {
        Duration::from_millis(50)
    }
}

/// 是否仍有工具条处于自动显示宽限期；该期间不降频，保证收起时机不变。
pub fn pending_visibility(deadlines: &[Instant]) -> bool {
    deadlines.iter().any(|deadline| *deadline > Instant::now())
}

/// 显隐决策的纯逻辑，四路输入彼此独立：
///
/// 页面不可见或用户刚点过「收起」一律隐藏；否则倒计时、前台唤出、图钉固定
/// 与光标宽限期任意一项成立就显示。`dismissed` 是临时门闩，不改写其它状态。
fn expanded(state: ToolbarState, page_visible: bool, grace: bool, lifted: bool) -> bool {
    if !page_visible || state.dismissed {
        return false;
    }
    state.hold || lifted || state.pinned || grace
}

/// 收起门闩是否继续生效：只有光标重新进入顶部热区才解除。
///
/// 点「收起」时鼠标仍在按钮条上，如果按「离开工具条就解除」处理，工具条会在
/// 下一帧立刻弹回；悬停与光标静止都不应解除门闩。
fn release_dismissed(dismissed: bool, hot: bool) -> bool {
    dismissed && !hot
}

/// 工具条显示期间始终压在页面上方：点击页面、切标签或唤出都会把页面升到置顶层
/// 最前，正好盖住顶部按钮条。`raise_above` 自带「已经在上方就跳过」的判断。
fn keep_above(show: bool) -> bool {
    show
}

/// 磁贴 ID 到工具条标签的映射，命令层据此校验调用方身份。
pub fn label(group: &str) -> String {
    format!("bar-{group}")
}

/// 把任意窗口标签归一成磁贴 ID；首页标签本身就是磁贴 ID。
fn group(app: &AppHandle, id: &str) -> String {
    super::tabs::group_of(app, id).unwrap_or_else(|| id.to_string())
}

/// 当前活动标签的内容窗口；工具条的位置、层级与热区都以它为准。
fn active_page(app: &AppHandle, group: &str) -> Option<WebviewWindow> {
    app.get_webview_window(&super::tabs::active_window(app, group))
}

/// 横排逻辑宽度：基础宽度加标签条，只有一个标签时与原来一致。
fn width_of(app: &AppHandle, group: &str) -> f64 {
    WIDTH + super::tabs::tabs_width(super::tabs::tabs_of(app, group).len())
}

/// 工具条的逻辑窗口尺寸。
///
/// 横排宽度随标签增长、高度固定；竖排宽高都固定，标签在固定高度内平分，
/// 标签数变化不引起尺寸抖动，也不需要滚动条。
fn logical_size(app: &AppHandle, group: &str) -> (f64, f64) {
    match crate::platform::toolbar_position_kind(app) {
        crate::models::ToolbarPosition::Top => (width_of(app, group), HEIGHT),
        crate::models::ToolbarPosition::Left | crate::models::ToolbarPosition::Right => {
            (VERTICAL_WIDTH, VERTICAL_HEIGHT)
        }
    }
}

/// 逻辑尺寸按窗口缩放换算成物理像素，供原生定位与命中判定使用。
///
/// 与 `apply_size` 共用 `scale_factor()`，保证尺寸与坐标来自同一来源。
fn physical_size(app: &AppHandle, page: &WebviewWindow, group: &str) -> (i32, i32) {
    let scale = crate::platform::window_scale(page);
    let (width, height) = logical_size(app, group);
    (
        (width * scale).round() as i32,
        (height * scale).round() as i32,
    )
}

/// 临时诊断：把尺寸换算的输入输出写到临时目录，定位后删除。
fn log_size(
    event: &str,
    id: &str,
    tauri_scale: f64,
    monitor_scale: f64,
    logical: (f64, f64),
    target: (u32, u32),
    before: Option<(u32, u32)>,
    outer: Option<(u32, u32)>,
) {
    use std::io::Write;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let line = format!(
        "{stamp} {event} id={id} tauri_scale={tauri_scale:.3} monitor_scale={monitor_scale:.3} logical={:.1}x{:.1} target={}x{} before={before:?} outer={outer:?}\n",
        logical.0, logical.1, target.0, target.1
    );
    let path = std::env::temp_dir().join("pinpage-toolbar-size.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 用物理像素显式校正工具条窗口尺寸。
///
/// 建窗时 `inner_size` 取逻辑尺寸，但那一刻窗口所属显示器的缩放未必已确定，
/// 会按 1.0 建成 48 物理宽的窗口；WebView2 随后按真实 DPI 排版，内容比窗口宽一倍，
/// 右半被裁掉，表现为「工具条一会宽一会窄」「图标只剩半截」。
/// 这里统一按物理像素设置，并与定位共用同一个 scale 来源（活动内容窗口）。
fn apply_size(app: &AppHandle, id: &str) -> bool {
    let id = group(app, id);
    let Some(bar) = app.get_webview_window(&label(&id)) else {
        return false;
    };
    let (width, height) = logical_size(app, &id);
    // 用显示器缩放换算物理像素，再用 PhysicalSize 下发；scale_factor 在 DPI
    // 虚拟化环境下会返回 96，不能用于这个换算。
    let scale = crate::platform::window_scale(&bar);
    let (target_width, target_height) = (
        (width * scale).round().max(1.) as u32,
        (height * scale).round().max(1.) as u32,
    );
    let before = bar.inner_size().map(|size| (size.width, size.height)).ok();
    let outer = crate::platform::window_outer_size(&bar);
    log_size(
        "apply",
        &id,
        bar.scale_factor().unwrap_or(1.),
        scale,
        (width, height),
        (target_width, target_height),
        before,
        outer,
    );
    // 系统最小跟踪宽度（175% 缩放下约 236 物理）会把窄条顶宽。显式给 1 逻辑像素
    // 下限，tao 的 WM_GETMINMAXINFO 才改用该值而不是 SM_CXMINTRACK。
    let _ = bar.set_min_size(Some(tauri::LogicalSize::new(1., 1.)));
    bar.set_size(PhysicalSize::new(target_width, target_height))
        .is_ok()
}

/// 异步创建工具条；一个磁贴只创建一次，失败只提示不阻断内容页。
pub fn open(app: &AppHandle, pin: &PinConfig) {
    let id = group(app, &pin.id);
    if app.get_webview_window(&label(&id)).is_some() {
        // 重新开启页面时沿用此前的图钉偏好，只刷新宽限期让入口再次可见。
        set_visible(app, &id, true);
        return;
    }
    let locale = match state::locale(app) {
        crate::models::AppLocale::EnUs => "en-US",
        _ => "zh-CN",
    };
    // 模式随查询参数下发，工具条首帧即可决定是否渲染「唤出」按钮。
    let mode = if pin.mode.is_desktop() {
        "desktop"
    } else {
        "floating"
    };
    let slot = match crate::platform::toolbar_position_kind(app) {
        crate::models::ToolbarPosition::Top => "top",
        crate::models::ToolbarPosition::Left => "left",
        crate::models::ToolbarPosition::Right => "right",
    };
    let entry = format!("toolbar.html?pin={id}&locale={locale}&mode={mode}&slot={slot}");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let (slot_width, slot_height) = logical_size(&app, &id);
        let builder =
            WebviewWindowBuilder::new(&app, label(&id), WebviewUrl::App(entry.into()));
        let result = builder
            .title("PinPage Toolbar")
            .inner_size(slot_width, slot_height)
            // 解除系统最小窗口宽度：175% 缩放时 SM_CXMINTRACK 会把 84 物理宽的窄条
            // 顶到 220 以上，表现为两侧大段空白。给定 1 像素下限后 WM_GETMINMAXINFO
            // 改用该值，窗口才能保持 48 逻辑宽。
            .min_inner_size(1., 1.)
            .resizable(false)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .skip_taskbar(true)
            .always_on_top(true)
            .visible(false)
            .on_navigation(super::app_navigation)
            .on_new_window(|_, _| NewWindowResponse::Deny)
            .build();
        match result {
            Ok(bar) => {
                // 工具条没有关闭按钮，误触发的系统关闭只保留窗口而不丢失入口。
                let guard = bar.clone();
                guard.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                    }
                });
                // 首次出现给两秒可见时间；轮询随后按状态决定是否收起。
                state::touch_toolbar_grace(&app, &id);
                let mut runtime = state::toolbar_runtime(&app, &id);
                runtime.state.dismissed = false;
                runtime.visible = true;
                state::set_toolbar_runtime(&app, &id, runtime);
                // 建窗尺寸按当时取到的缩放，可能与实际显示器不符，先按物理像素校正。
                let _ = apply_size(&app, &id);
                let _ = bar.show();
                // 显示后 Windows 会重新套用最小尺寸约束，这里按物理像素再确认一次。
                let _ = apply_size(&app, &id);
                let _ = sync(&app, &id);
                front(&app, &id);
            }
            Err(error) => state::report(
                &app,
                AppMessage::new("errors.toolbarOpenFailed").detail(error),
            ),
        }
    });
}

/// 把工具条贴到活动标签客户区顶部居中；返回是否定位成功。
pub fn sync(app: &AppHandle, id: &str) -> bool {
    let id = group(app, id);
    let (Some(page), Some(bar)) = (active_page(app, &id), app.get_webview_window(&label(&id)))
    else {
        return false;
    };
    // 临时诊断：记录窗口实际尺寸的变化，确认 set_size 是否落地。
    static LAST_SIZE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Ok(size) = bar.inner_size() {
        let packed = ((size.width as u64) << 32) | size.height as u64;
        if LAST_SIZE.swap(packed, std::sync::atomic::Ordering::Relaxed) != packed {
            log_size(
                "actual",
                &id,
                0.,
                0.,
                (0., 0.),
                (size.width, size.height),
                None,
                crate::platform::window_outer_size(&bar),
            );
        }
    }
    let (width, height) = physical_size(app, &page, &id);
    // 窗口被系统最小跟踪宽度顶宽时（外框明显大于目标），把最小尺寸与目标尺寸
    // 重新压回去。次数封顶，避免系统持续钳制导致每轮轮询都重设窗口。
    static RESIZE_FIXES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if let Some((outer_width, _)) = crate::platform::window_outer_size(&bar) {
        if outer_width as i32 > width + 2
            && RESIZE_FIXES.load(std::sync::atomic::Ordering::Relaxed) < 6
        {
            RESIZE_FIXES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = apply_size(app, &id);
        }
    }
    let Some((x, y)) = crate::platform::toolbar_position(app, &page, width, height) else {
        return false;
    };
    bar.set_position(PhysicalPosition::new(x, y)).is_ok()
}

/// 唤出的页面会升到置顶层最前并取得焦点，工具条必须重新压回它上方。
pub fn front(app: &AppHandle, id: &str) {
    let id = group(app, id);
    let (Some(page), Some(bar)) = (active_page(app, &id), app.get_webview_window(&label(&id)))
    else {
        return;
    };
    if !bar.is_visible().unwrap_or(false) {
        return;
    }
    let _ = sync(app, &id);
    crate::platform::raise_above(&page, &bar);
}

/// 标签数量变化后调整工具条宽度并重新居中。
pub fn resize(app: &AppHandle, id: &str) {
    let id = group(app, id);
    if app.get_webview_window(&label(&id)).is_none() {
        return;
    }
    let _ = apply_size(app, &id);
    let _ = sync(app, &id);
}

/// 页面显示或隐藏时同步工具条：显隐写入与轮询共用同一份运行时状态。
///
/// 页面重新显示会解除临时收起门闩并刷新宽限期，但不改图钉与倒计时状态。
pub fn set_visible(app: &AppHandle, id: &str, visible: bool) {
    let id = group(app, id);
    let Some(bar) = app.get_webview_window(&label(&id)) else {
        return;
    };
    let mut runtime = state::toolbar_runtime(app, &id);
    if visible {
        runtime.state.dismissed = false;
        runtime.grace_until = Some(Instant::now() + state::TOOLBAR_GRACE);
        runtime.visible = true;
        state::set_toolbar_runtime(app, &id, runtime);
        let _ = sync(app, &id);
        let _ = bar.show();
        front(app, &id);
    } else {
        runtime.grace_until = None;
        runtime.visible = false;
        state::set_toolbar_runtime(app, &id, runtime);
        let _ = bar.hide();
    }
}

/// 主动收起：立即隐藏窗口并同步轮询状态，不依赖下一帧或光标位置是否可读。
///
/// 只结束自动显示宽限期、更新上一帧显隐；`pinned`、`hold` 与唤出状态都不改。
pub fn collapse(app: &AppHandle, id: &str) {
    let id = group(app, id);
    let mut runtime = state::toolbar_runtime(app, &id);
    runtime.grace_until = None;
    runtime.visible = false;
    state::set_toolbar_runtime(app, &id, runtime);
    if let Some(bar) = app.get_webview_window(&label(&id)) {
        let _ = bar.hide();
    }
}

/// 收回桌面时结束宽限期，让工具条回到图钉 / 自动隐藏策略。
pub fn clear_grace(app: &AppHandle, id: &str) {
    state::clear_toolbar_grace(app, &group(app, id));
}

/// 系统拒绝读取光标位置时的安全回退：仍能保证页面可见的工具条有操作入口。
///
/// 用户主动收起的门闩仍然生效；页面隐藏时不显示按钮。
pub fn fallback_visible(app: &AppHandle) -> usize {
    let mut bars = 0;
    for (label, bar) in app.webview_windows() {
        let Some(id) = label.strip_prefix("bar-") else {
            continue;
        };
        bars += 1;
        let Some(page) = active_page(app, id) else {
            continue;
        };
        if !page.is_visible().unwrap_or(false) || state::toolbar_state(app, id).dismissed {
            continue;
        }
        let mut runtime = state::toolbar_runtime(app, id);
        if runtime.visible && bar.is_visible().unwrap_or(false) {
            continue;
        }
        runtime.visible = true;
        state::set_toolbar_runtime(app, id, runtime);
        if !bar.is_visible().unwrap_or(false) {
            let _ = sync(app, id);
            let _ = bar.show();
        }
    }
    bars
}

/// 停靠位置改变后原地重排所有工具条：重新导航让前端按新 slot 重排，
/// 同时更新原生窗口尺寸与锚点。不销毁窗口，避免异步销毁/创建竞态丢失图钉状态。
pub fn relayout_all(app: &AppHandle) {
    let slot = match crate::platform::toolbar_position_kind(app) {
        crate::models::ToolbarPosition::Top => "top",
        crate::models::ToolbarPosition::Left => "left",
        crate::models::ToolbarPosition::Right => "right",
    };
    let groups: Vec<String> = app
        .webview_windows()
        .keys()
        .filter_map(|label| label.strip_prefix("bar-").map(str::to_string))
        .collect();
    for group in groups {
        let Some(bar) = app.get_webview_window(&label(&group)) else {
            continue;
        };
        let _ = apply_size(app, &group);
        // 用当前 URL 只替换 slot，保留 pin/locale/mode。导航会重载 Vue，
        // 前端首帧据此选择横排或竖排；失败时窗口尺寸仍已按新位置更新。
        if let Ok(mut url) = bar.url() {
            let mut pairs: Vec<(String, String)> = url
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            if let Some((_, value)) = pairs.iter_mut().find(|(key, _)| key == "slot") {
                *value = slot.to_string();
            } else {
                pairs.push(("slot".to_string(), slot.to_string()));
            }
            let query = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(pairs)
                .finish();
            url.set_query(Some(&query));
            let _ = bar.navigate(url);
        }
        let _ = sync(app, &group);
        front(app, &group);
    }
}

/// 页面关闭时销毁工具条，只重置临时状态，保留用户的图钉偏好。
pub fn close(app: &AppHandle, id: &str) {
    let id = group(app, id);
    if let Some(bar) = app.get_webview_window(&label(&id)) {
        let _ = bar.destroy();
    }
    state::reset_toolbar_runtime(app, &id);
}

/// 删除磁贴时连图钉偏好一起清理。
pub fn forget(app: &AppHandle, id: &str) {
    close(app, id);
    state::clear_toolbar_state(app, &group(app, id));
}

/// 按鼠标位置决定工具条显隐，返回工具条数量与是否仍处于宽限期。
///
/// 这是窗口显隐的唯一决策点：图钉、收起、唤出与标签切换都只改状态，
/// 不直接 show/hide 工具条。一个组只有一个窗口，也不存在多工具条抢 z 序。
pub fn auto_visibility(app: &AppHandle, cursor: (i32, i32)) -> (usize, bool) {
    let now = Instant::now();
    let mut bars = 0;
    let mut deadlines = Vec::new();
    for (bar_label, bar) in app.webview_windows() {
        let Some(id) = bar_label.strip_prefix("bar-").map(str::to_string) else {
            continue;
        };
        bars += 1;
        let Some(page) = active_page(app, &id) else {
            continue;
        };
        let (width, height) = physical_size(app, &page, &id);
        let (Some((bar_x, bar_y)), Some((hot_left, hot_top, hot_right, hot_bottom))) = (
            crate::platform::toolbar_position(app, &page, width, height),
            crate::platform::toolbar_hot_zone(app, &page),
        ) else {
            continue;
        };
        let hot = cursor.0 >= hot_left
            && cursor.0 < hot_right
            && cursor.1 >= hot_top
            && cursor.1 < hot_bottom;
        let before = state::toolbar_runtime(app, &id);
        let mut runtime = before;
        // 隐藏的窗口不把自己所在的矩形算作悬停，否则无法真正收起。
        let hover = runtime.visible
            && cursor.0 >= bar_x
            && cursor.0 < bar_x + width
            && cursor.1 >= bar_y
            && cursor.1 < bar_y + height;
        // 门闩只在光标重新进入顶部热区时解除：点「收起」时鼠标还在按钮条上，
        // 若按「离开就解除」处理，工具条会在下一帧立刻弹回来。
        runtime.state.dismissed = release_dismissed(runtime.state.dismissed, hot);
        if hot || hover {
            runtime.grace_until = Some(now + state::TOOLBAR_GRACE);
        }
        let grace = runtime.grace_until.is_some_and(|deadline| deadline > now);
        if let Some(deadline) = runtime.grace_until {
            deadlines.push(deadline);
        }
        let lifted = super::lift::is_lifted(app, &id);
        let show = expanded(
            runtime.state,
            page.is_visible().unwrap_or(false),
            grace,
            lifted,
        );
        let changed = runtime.visible != show;
        runtime.visible = show;
        // 空闲帧不重复写状态；只有门闩、宽限期或显隐真的变化才回写。
        if runtime != before {
            state::set_toolbar_runtime(app, &id, runtime);
        }
        if changed {
            // z 序只在显隐切换时校正一次；常态由 Focused 事件的 front() 维护。
            // 轮询每帧把工具条重设 TOPMOST 会与其它应用的前台切换竞争，
            // 在桌面子窗口场景下还会持续触发原生窗口消息重入。
            if keep_above(show) {
                crate::platform::raise_above(&page, &bar);
            }
            let target = bar.clone();
            let app = app.clone();
            let id = id.clone();
            // 窗口显示、隐藏与定位统一投递到所属 UI 线程。
            let _ = page.run_on_main_thread(move || {
                if show {
                    let _ = target.show();
                    let _ = sync(&app, &id);
                } else {
                    let _ = target.hide();
                }
            });
        }
    }
    (bars, pending_visibility(&deadlines))
}

#[cfg(test)]
#[path = "toolbar_tests.rs"]
mod tests;
