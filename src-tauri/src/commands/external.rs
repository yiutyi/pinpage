// 功能：内容页请求用系统默认浏览器打开外部链接。
//
// 这是内容 WebView 唯一被授予的命令：不读取文件、不管理窗口、不修改配置。
// 调用方必须是已登记且仍存在的内容窗口，链接只允许安全的外部 HTTP(S) 地址，
// 并对同一窗口短时间内的重复请求去重，避免网页脚本反复唤起浏览器。
use crate::{content, errors::AppMessage};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Instant,
};
use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_plugin_opener::OpenerExt;
use url::Url;

/// 每个内容窗口最近一次成功受理的外部地址，用于拦截重复点击。
static RECENT: OnceLock<Mutex<HashMap<String, (String, Instant)>>> = OnceLock::new();

#[tauri::command]
pub fn open_external(caller: WebviewWindow, app: AppHandle, url: String) -> Result<(), AppMessage> {
    let id = caller.label().to_string();
    // 首页与标签窗口都算已登记内容页；标签没有配置记录，按所属磁贴校验。
    let known = crate::windows::tabs::group_of(&app, &id).is_some();
    if !known || app.get_webview_window(&id).is_none() {
        return Err(AppMessage::new("errors.windowNotAuthorized"));
    }
    let target =
        Url::parse(&url).map_err(|error| AppMessage::new("errors.urlInvalid").detail(error))?;
    if !content::external_url(&target) {
        return Err(AppMessage::new("errors.urlUnsupported"));
    }
    {
        let mut recent = RECENT.get_or_init(Default::default).lock().unwrap();
        let now = Instant::now();
        if let Some((last, at)) = recent.get(&id) {
            if *last == url && now.duration_since(*at).as_secs_f64() < 1. {
                return Ok(());
            }
        }
        recent.insert(id, (url.clone(), now));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| AppMessage::new("errors.openExternalFailed").detail(error))
}
