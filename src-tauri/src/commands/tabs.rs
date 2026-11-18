// 功能：接收内容页的「新标签」请求，校验来源与地址后交给标签模块打开。
//
// 内容页只持有这一个用途受限的命令：不能指定窗口、不能关标签、不能改配置，
// 命令自身再校验调用方是否为已登记的页面/标签，以及目标是否为内部地址。
use crate::{content, errors::AppMessage, windows};
use tauri::{AppHandle, WebviewWindow};
use url::Url;

/// 从首页或标签页打开内部页面：只接受本机/局域网或与来源同主机的 HTTP(S) 地址。
///
/// 命令必须是 async：同步命令在主线程执行，而建窗与原生配置要等主线程处理完消息，
/// 在主线程里等待会自锁。异步命令在运行时的线程上执行，等待链才成立。
#[tauri::command]
pub async fn open_tab(
    caller: WebviewWindow,
    app: AppHandle,
    url: String,
) -> Result<(), AppMessage> {
    let target = Url::parse(url.trim()).map_err(|_| AppMessage::new("errors.urlInvalid"))?;
    let caller_url = caller
        .url()
        .map_err(|error| AppMessage::new("errors.windowNotAuthorized").detail(error))?;
    if !content::is_local_url(&target) && !content::same_host(&caller_url, &target) {
        return Err(AppMessage::new("errors.tabNotAuthorized"));
    }
    let pin = windows::tabs::group_of(&app, caller.label())
        .ok_or_else(|| AppMessage::new("errors.tabNotAuthorized"))?;
    windows::tabs::open(&app, &pin, target)
}
