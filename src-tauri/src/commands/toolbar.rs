// 功能：处理受信任工具条的动作，逐项校验调用方、目标页面与动作白名单。
//
// 内容 WebView 不持有这些命令的权限；即使网页伪造工具条标签，Rust 也会校验
// 窗口来源与目标页面记录。关机和重启还必须带明确的二次确认标记。
// 返回值是工具条需要同步的固定、唤出与标签状态，供图钉、桌面按钮和标签条渲染。
//
// 三条状态链路在这里保持独立：图钉只改 pinned，收起只改 dismissed，
// 倒计时只改 hold；唤出 / 收回、标签切换都只作用于窗口层级与标签组。
use crate::{errors::AppMessage, platform, state, windows};
use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewWindow};

/// 工具条按钮的状态：是否常驻、页面是否被唤出到前台、是否可以唤出、标签条内容。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolbarStatus {
    /// 用户自己设置的图钉，不受唤出、收起或切标签影响。
    pub pinned: bool,
    pub lifted: bool,
    /// 只有桌面模式的页面挂在桌面层，浮动页没有唤出入口。
    pub liftable: bool,
    /// 组内全部标签，首页始终在第一位。
    pub tabs: Vec<windows::tabs::TabInfo>,
}

/// 读取当前状态，所有动作执行后都回传最新值。
///
/// `group` 是磁贴 ID：一个组只有一个工具条，标签条、图钉与唤出状态都按组读取。
fn status(app: &AppHandle, group: &str) -> ToolbarStatus {
    ToolbarStatus {
        pinned: state::toolbar_state(app, group).pinned,
        lifted: windows::lift::is_lifted(app, group),
        liftable: windows::pin_config(app, group).is_ok_and(|pin| pin.mode.is_desktop()),
        tabs: windows::tabs::tabs_of(app, group),
    }
}

/// 工具条动作必须是 async：切换/关闭标签会建窗、改原生窗口样式，这些调用要等
/// 主线程处理消息；同步命令本身就在主线程上执行，等待会自锁。
#[tauri::command]
pub async fn toolbar_action(
    caller: WebviewWindow,
    app: AppHandle,
    id: String,
    action: String,
    confirm: bool,
    target: Option<String>,
) -> Result<ToolbarStatus, AppMessage> {
    let url = caller
        .url()
        .map_err(|error| AppMessage::new("errors.windowNotAuthorized").detail(error))?;
    if caller.label() != windows::toolbar::label(&id) || !windows::app_navigation(&url) {
        return Err(AppMessage::new("errors.toolbarNotAuthorized"));
    }
    // 工具条属于整个标签组，标签窗口不再是独立的调用方或状态单元。
    let group = windows::tabs::group_of(&app, &id).unwrap_or_else(|| id.clone());
    match action.as_str() {
        "state" => Ok(status(&app, &group)),
        // 图钉只影响本组的显隐策略，不改变网页、窗口层级或标签。
        "pin" => {
            let mut next = state::toolbar_state(&app, &group);
            next.pinned = !next.pinned;
            next.dismissed = false;
            state::set_toolbar_state(&app, &group, next);
            Ok(status(&app, &group))
        }
        // 收起只是工具条自己的临时门闩：不取消图钉，也不把唤出的页面送回桌面。
        // 想回到桌面请用「收回桌面」按钮或快捷键。
        "hide" => {
            let mut next = state::toolbar_state(&app, &group);
            next.dismissed = true;
            state::set_toolbar_state(&app, &group, next);
            windows::toolbar::collapse(&app, &group);
            Ok(status(&app, &group))
        }
        // 唤出与收回只改窗口层级；页面不重新加载，工具条状态也不被改写。
        "summon" | "recall" => {
            if action == "summon" {
                windows::lift::summon(&app, Some(group.as_str()))?;
            } else {
                windows::lift::recall(&app, Some(group.as_str()))?;
            }
            Ok(status(&app, &group))
        }
        // 确认/撤销倒计时期间强制显示；结束时由界面显式释放。
        "hold" | "release" => {
            let mut next = state::toolbar_state(&app, &group);
            next.hold = action == "hold";
            if next.hold {
                next.dismissed = false;
            }
            state::set_toolbar_state(&app, &group, next);
            Ok(status(&app, &group))
        }
        // 浏览历史作用于当前显示的标签，而不是隐藏的首页窗口。
        "back" | "forward" => {
            let active = windows::tabs::active_window(&app, &group);
            let page = app
                .get_webview_window(&active)
                .ok_or_else(|| AppMessage::new("errors.pageClosed"))?;
            let script = if action == "back" {
                "history.back()"
            } else {
                "history.forward()"
            };
            page.eval(script)
                .map_err(|error| AppMessage::new("errors.windowApplyFailed").detail(error))?;
            Ok(status(&app, &group))
        }
        // 首页标签回到配置里的来源地址；正在看额外标签时则切回首页标签。
        "home" => {
            if windows::tabs::active_window(&app, &group) == group {
                let pin = windows::pin_config(&app, &group)?;
                windows::navigate(&app, &group, windows::initial_url(&app, &pin)?)?;
            } else {
                windows::tabs::activate(&app, &group, &group)?;
            }
            Ok(status(&app, &group))
        }
        // 标签切换与关闭：目标必须属于同一标签组，不能跨磁贴操作。
        "tab-switch" | "tab-close" => {
            let target = target.ok_or_else(|| AppMessage::new("errors.tabNotFound"))?;
            if windows::tabs::group_of(&app, &target).as_deref() != Some(group.as_str()) {
                return Err(AppMessage::new("errors.tabNotAuthorized"));
            }
            if action == "tab-switch" {
                windows::tabs::activate(&app, &group, &target)?;
            } else {
                windows::tabs::close(&app, &target)?;
            }
            Ok(status(&app, &group))
        }
        "recycle" | "restart" | "shutdown" | "cancelShutdown" => {
            if matches!(action.as_str(), "restart" | "shutdown") && !confirm {
                return Err(AppMessage::new("errors.desktopActionConfirm"));
            }
            platform::desktop_action(&action)?;
            Ok(status(&app, &group))
        }
        _ => Err(AppMessage::new("errors.unknownAction")),
    }
}
